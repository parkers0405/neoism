//! Supervised loader for approved native extension and Tree-sitter modules.
//!
//! Dynamic libraries are loaded only in a short-lived child Neoism process.
//! Retiring a generation terminates that process; Neoism never attempts unsafe
//! in-process library unloading.

use std::collections::BTreeSet;
use std::ffi::{c_char, c_void, CStr};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

use neoism_extensions::trust::{
    ApprovalScope, ApprovalState, ExtensionTrustStore, NATIVE_ABI_VERSION,
    TREE_SITTER_ABI_MAX, TREE_SITTER_ABI_MIN,
};
use neoism_lua::{OpaqueArtifact, PluginOwner};

#[repr(C)]
struct NativeHostV1 {
    abi_version: u32,
    log: Option<unsafe extern "C" fn(level: u32, message: *const c_char)>,
}
#[repr(C)]
struct NativeInstanceV1 {
    context: *mut c_void,
    shutdown: Option<unsafe extern "C" fn(*mut c_void)>,
}

pub(crate) struct NativeGeneration {
    pub owner: PluginOwner,
    stdin: Option<ChildStdin>,
    child: Option<Child>,
}
impl Drop for NativeGeneration {
    fn drop(&mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = writeln!(stdin, "close");
        }
        let Some(mut child) = self.child.take() else {
            return;
        };
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                if child.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let _ = child.kill();
            let _ = child.wait();
        });
    }
}

pub(crate) fn start_approved(
    owner: PluginOwner,
    package_root: &Path,
    package_digest: &str,
    artifact: &OpaqueArtifact,
    capabilities: &BTreeSet<String>,
    scope: ApprovalScope,
) -> Result<NativeGeneration, String> {
    validate_platform(artifact)?;
    if artifact.abi != NATIVE_ABI_VERSION {
        return Err(format!(
            "native extension ABI {} is incompatible with host ABI {NATIVE_ABI_VERSION}",
            artifact.abi
        ));
    }
    let path = contained_artifact(package_root, &artifact.resource)?;
    neoism_extensions::trust::verify_artifact(&path, &artifact.sha256)
        .map_err(|e| e.to_string())?;
    let store = ExtensionTrustStore::managed();
    let approval = store
        .exact(
            &owner.plugin_id,
            &owner.revision.0,
            package_digest,
            &artifact.sha256,
            artifact.abi,
            capabilities,
            &scope,
        )
        .map_err(|e| e.to_string())?
        .ok_or("native extension requires an exact host-owned approval")?;
    if approval.state != ApprovalState::Approved {
        return Err(format!("native extension approval is {:?}", approval.state));
    }
    let generation = spawn_host(owner, &path, "native", None, None)?;
    store
        .audit_event(
            "native_activated",
            &approval,
            Some(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            ),
        )
        .map_err(|e| e.to_string())?;
    Ok(generation)
}

pub(crate) fn validate_tree_sitter_approved(
    owner: PluginOwner,
    package_root: &Path,
    package_digest: &str,
    artifact: &OpaqueArtifact,
    language: &str,
    capabilities: &BTreeSet<String>,
    scope: ApprovalScope,
) -> Result<NativeGeneration, String> {
    validate_platform(artifact)?;
    if !(TREE_SITTER_ABI_MIN..=TREE_SITTER_ABI_MAX).contains(&artifact.abi) {
        return Err(format!("Tree-sitter ABI {} is outside supported range {TREE_SITTER_ABI_MIN}..={TREE_SITTER_ABI_MAX}", artifact.abi));
    }
    let path = contained_artifact(package_root, &artifact.resource)?;
    neoism_extensions::trust::verify_artifact(&path, &artifact.sha256)
        .map_err(|e| e.to_string())?;
    let store = ExtensionTrustStore::managed();
    let approval = store
        .exact(
            &owner.plugin_id,
            &owner.revision.0,
            package_digest,
            &artifact.sha256,
            artifact.abi,
            capabilities,
            &scope,
        )
        .map_err(|e| e.to_string())?
        .ok_or("Tree-sitter parser requires an exact host-owned approval")?;
    if approval.state != ApprovalState::Approved {
        return Err(format!(
            "Tree-sitter parser approval is {:?}",
            approval.state
        ));
    }
    let generation = spawn_host(
        owner,
        &path,
        "tree-sitter",
        Some(language),
        Some(artifact.abi),
    )?;
    store
        .audit_event("tree_sitter_activated", &approval, Some(language.into()))
        .map_err(|e| e.to_string())?;
    Ok(generation)
}

fn spawn_host(
    owner: PluginOwner,
    path: &Path,
    kind: &str,
    language: Option<&str>,
    abi: Option<u32>,
) -> Result<NativeGeneration, String> {
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command
        .arg("--neoism-native-extension-host")
        .arg(kind)
        .arg(path);
    if let Some(language) = language {
        command.arg(language);
    }
    if let Some(abi) = abi {
        command.arg(abi.to_string());
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear()
        .spawn()
        .map_err(|e| e.to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or("native extension host has no stdout")?;
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout)
            .take(1024)
            .read_line(&mut line)
            .map(|_| line);
        let _ = ready_tx.send(result);
    });
    let line = match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok(line)) => line,
        Ok(Err(error)) => {
            let _ = child.kill();
            return Err(error.to_string());
        }
        Err(_) => {
            let _ = child.kill();
            return Err("native extension host initialization timed out".into());
        }
    };
    if line.trim() != "ready" {
        let _ = child.kill();
        let stderr = child
            .stderr
            .take()
            .map(|value| {
                let mut output = String::new();
                let _ = value.take(16 * 1024).read_to_string(&mut output);
                output
            })
            .unwrap_or_default();
        return Err(format!(
            "native extension host rejected module: {} {stderr}",
            line.trim()
        ));
    }
    let stdin = child.stdin.take();
    Ok(NativeGeneration {
        owner,
        stdin,
        child: Some(child),
    })
}

fn validate_platform(artifact: &OpaqueArtifact) -> Result<(), String> {
    if !artifact.platforms.is_empty()
        && !artifact
            .platforms
            .iter()
            .any(|item| item == std::env::consts::OS)
    {
        Err(format!(
            "artifact is not compatible with {}",
            std::env::consts::OS
        ))
    } else {
        Ok(())
    }
}
fn contained_artifact(root: &Path, resource: &str) -> Result<PathBuf, String> {
    let relative = Path::new(resource);
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err("native artifact must be a contained package-relative path".into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let path = root
        .join(relative)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err("native artifact resolves outside the immutable package".into());
    }
    Ok(path)
}

pub(crate) fn maybe_run_host() -> Result<bool, Box<dyn std::error::Error>> {
    let args = std::env::args_os().collect::<Vec<_>>();
    if args.get(1).and_then(|value| value.to_str())
        != Some("--neoism-native-extension-host")
    {
        return Ok(false);
    }
    let kind = args
        .get(2)
        .and_then(|value| value.to_str())
        .ok_or("native host kind missing")?;
    let path = args
        .get(3)
        .map(PathBuf::from)
        .ok_or("native host path missing")?;
    // SAFETY: this is the process isolation boundary. Every symbol is checked
    // before invocation and a crash terminates only this supervised child.
    unsafe {
        let library = libloading::Library::new(&path)?;
        if kind == "tree-sitter" {
            let language = args
                .get(4)
                .and_then(|value| value.to_str())
                .ok_or("Tree-sitter language missing")?;
            let declared_abi = args
                .get(5)
                .and_then(|value| value.to_str())
                .ok_or("Tree-sitter ABI missing")?
                .parse::<usize>()?;
            if language.is_empty()
                || !language
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                return Err("invalid Tree-sitter language symbol".into());
            }
            let symbol = format!("tree_sitter_{language}");
            let builder: libloading::Symbol<'_, unsafe extern "C" fn() -> *const ()> =
                library.get(symbol.as_bytes())?;
            let language = tree_sitter::Language::new(
                tree_sitter_language::LanguageFn::from_raw(*builder),
            );
            let actual_abi = language.abi_version();
            if actual_abi != declared_abi
                || !(TREE_SITTER_ABI_MIN as usize..=TREE_SITTER_ABI_MAX as usize)
                    .contains(&actual_abi)
            {
                return Err(format!("Tree-sitter module ABI {actual_abi} does not match approved ABI {declared_abi}").into());
            }
            let mut parser = tree_sitter::Parser::new();
            parser.set_language(&language)?;
            println!("ready");
            std::io::stdout().flush()?;
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            drop(library);
            return Ok(true);
        }
        let abi: libloading::Symbol<'_, unsafe extern "C" fn() -> u32> =
            library.get(b"neoism_extension_abi_version")?;
        if abi() != NATIVE_ABI_VERSION {
            return Err(
                "native module ABI symbol returned an incompatible version".into()
            );
        }
        let init: libloading::Symbol<
            '_,
            unsafe extern "C" fn(*const NativeHostV1, *mut NativeInstanceV1) -> i32,
        > = library.get(b"neoism_extension_init_v1")?;
        unsafe extern "C" fn log(_level: u32, message: *const c_char) {
            if !message.is_null() {
                let text = unsafe { CStr::from_ptr(message) }.to_string_lossy();
                eprintln!("native extension: {text}");
            }
        }
        let host = NativeHostV1 {
            abi_version: NATIVE_ABI_VERSION,
            log: Some(log),
        };
        let mut instance = NativeInstanceV1 {
            context: std::ptr::null_mut(),
            shutdown: None,
        };
        let status = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            init(&host, &mut instance)
        }))
        .map_err(|_| "native extension initialization panicked")?;
        if status != 0 {
            return Err(format!(
                "native extension initialization failed with status {status}"
            )
            .into());
        }
        println!("ready");
        std::io::stdout().flush()?;
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        if let Some(shutdown) = instance.shutdown {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                shutdown(instance.context)
            }));
        }
        drop(library);
    }
    Ok(true)
}
