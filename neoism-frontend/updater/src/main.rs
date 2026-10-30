//! Standalone updater shared by the GPUIX CLI and the legacy migration payload.
use std::{path::{Path, PathBuf}, time::{Duration, Instant}};
mod download;
#[cfg(unix)]
mod macos_update;
#[cfg(windows)]
mod windows_update;
type Error = Box<dyn std::error::Error>;
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Default)]
struct Options { nightly: bool, force: bool, target: Option<String>, executable: Option<PathBuf>, parent: Option<u32>, relaunch: bool }
fn main() {
    if let Err(error) = run() { eprintln!("Neoism update failed: {}", download::error_chain(error.as_ref())); std::process::exit(1); }
}
fn run() -> Result<(), Error> {
    #[cfg(target_os="macos")]
    if macos_update::run_helper()? { return Ok(()); }
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|a| a=="--version" || a=="-V") {
        println!("{} {VERSION}", env!("CARGO_BIN_NAME")); return Ok(());
    }
    if args.iter().any(|a| a=="--help" || a=="-h") {
        println!("Usage: neoism update [--nightly] [--force] [--target-version TAG]\nUpdates the invoking installation, including its complete native runtime. Windows uses the MSI installer."); return Ok(());
    }
    if args.first().is_some_and(|a| a=="--manifest") {
        return Err("The legacy Lua plugin runner was retired; this companion is retained for installer compatibility.".into());
    }
    let mut options = Options::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "update" | "upgrade" | "self-update" | "--update" => {},
            "--nightly" | "nightly" => options.nightly=true,
            "--force" => options.force=true,
            "--relaunch" => options.relaunch=true,
            "--target-version" => options.target=Some(args.next().ok_or("missing release tag")?.to_owned()),
            "--invoking-exe" => options.executable=Some(PathBuf::from(args.next().ok_or("missing invoking executable")?)),
            "--parent-pid" => options.parent=Some(args.next().ok_or("missing process ID")?.parse()?),
            _ => return Err(format!("unknown update option: {arg}").into()),
        }
    }
    if options.nightly && options.target.is_some() { return Err("--nightly and --target-version cannot be combined".into()); }
    let executable = std::fs::canonicalize(options.executable.as_ref().ok_or("Invoke this updater through neoism update")?)?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let (tag, temporary, archive) = runtime.block_on(download(&options))?;
    let Some(archive) = archive else { return Ok(()); };
    #[cfg(windows)] {
        let receipt = windows_update::stage(&archive, &temporary, options.parent, options.relaunch, &executable, &tag)?;
        println!("Update staged. The MSI installer finishes after this command exits. Result: {}", receipt.display());
        return Ok(());
    }
    #[cfg(unix)] {
        let status=std::process::Command::new("tar").arg("-xzf").arg(&archive).arg("-C").arg(&temporary).status()?;
        if !status.success() { return Err("Cannot extract the verified release archive".into()); }
        let source=temporary.join(format!("neoism-{}-{}", platform(), architecture()?));
        #[cfg(target_os="macos")] {
            let handoff=match macos_update::installation(&executable)? {
                macos_update::Installation::Bundle(target) => macos_update::stage_update(&target,&source.join("Neoism.app"),&tag,options.parent,options.relaunch)?,
                macos_update::Installation::Loose(target) => macos_update::stage_loose_update(&target,&source,&tag,options.parent,options.relaunch)?,
            };
            println!("Update staged. Result: {}",handoff.result_path.display());
        }
        #[cfg(not(target_os="macos"))] {
            macos_update::install_unix_loose(&executable,&source)?;
            println!("Installed Neoism {tag}. Restart Neoism to use the updated application and services.");
        }
        std::fs::remove_dir_all(temporary)?;
        Ok(())
    }
}
fn platform() -> &'static str { if cfg!(windows) {"windows"} else if cfg!(target_os="macos") {"darwin"} else {"linux"} }
fn architecture() -> Result<&'static str, Error> { match std::env::consts::ARCH {"x86_64"=>Ok("x86_64"),"aarch64"=>Ok("aarch64"),_=>Err("Unsupported update architecture".into())} }
fn release_version(tag:&str) -> Result<semver::Version, Error> {
    if !tag.bytes().all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b)) { return Err("Invalid release tag".into()); }
    Ok(semver::Version::parse(tag.strip_prefix('v').unwrap_or(tag))?)
}
fn select_release(releases:&serde_json::Value, nightly:bool, target:Option<&str>) -> Result<serde_json::Value,Error> {
    if let Some(tag)=target { release_version(tag)?; }
    releases.as_array().ok_or("Invalid release list")?.iter()
        .filter(|r| r["draft"].as_bool()==Some(false))
        .filter_map(|r| { let tag=r["tag_name"].as_str()?; let version=release_version(tag).ok()?;
            let eligible=if let Some(target)=target { tag==target } else if nightly { r["prerelease"].as_bool()==Some(true)&&version.pre.as_str().starts_with("nightly.") } else {version.pre.is_empty()&&r["prerelease"].as_bool()==Some(false)};
            eligible.then_some((version,r)) })
        .max_by(|a,b| a.0.cmp(&b.0)).map(|(_,r)|r.clone()).ok_or_else(||"No matching published Neoism release is available".into())
}
async fn download(options:&Options)->Result<(String,PathBuf,Option<PathBuf>),Error> {
    let repo=std::env::var("NEOISM_REPO").unwrap_or_else(|_|"parkers0405/neoism".into());
    if repo.split('/').count()!=2 || !repo.bytes().all(|b|b.is_ascii_alphanumeric()||b"/_-.".contains(&b)) {return Err("Invalid release repository".into());}
    let client=reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).timeout(Duration::from_secs(900)).user_agent(format!("neoism/{VERSION}")).build()?;
    println!("Checking {} releases…",if options.nightly {"nightly"} else {"stable"});
    let releases=serde_json::from_str(&download::text(&client, &format!("https://api.github.com/repos/{repo}/releases?per_page=100")).await?)?;
    let release=select_release(&releases,options.nightly,options.target.as_deref())?;
    let tag=release["tag_name"].as_str().ok_or("Missing release tag")?.to_owned();
    if !options.force && release_version(&tag)? <= release_version(VERSION)? {
        println!("Neoism {VERSION} is already current; use --force to reinstall or downgrade.");return Ok((tag,PathBuf::new(),None));
    }
    let asset=if cfg!(windows) {format!("Neoism-{}.msi",architecture()?)} else {format!("neoism-{}-{}.tar.gz",platform(),architecture()?)};
    let names=release["assets"].as_array().ok_or("Missing release assets")?;
    for name in [&asset,&format!("{asset}.sha256")] {
        if !names.iter().any(|a|a["name"].as_str()==Some(name)) {return Err(format!("{tag} is published, but its {} download is not ready yet. Try again after that platform finishes.",platform()).into());}
    }
    let temporary=tempfile::Builder::new().prefix("neoism-update-").tempdir()?;
    let archive=temporary.path().join(&asset);
    let base=format!("https://github.com/{repo}/releases/download/{tag}");
    let expected=download::text(&client, &format!("{base}/{asset}.sha256")).await?;
    let expected=expected.split_whitespace().next().ok_or("Empty checksum")?;
    if expected.len()!=64||!expected.bytes().all(|b|b.is_ascii_hexdigit()){return Err("Invalid SHA-256 checksum".into());}
    println!("Downloading {tag} ({asset})…");
    download::verified(&client, &format!("{base}/{asset}"), &archive, expected).await?;
    Ok((tag,temporary.keep(),Some(archive)))
}
#[cfg(unix)]
fn process_is_alive(pid:u32)->bool {(unsafe{libc::kill(pid as i32,0)==0}) || std::io::Error::last_os_error().raw_os_error()==Some(libc::EPERM)}
#[cfg(unix)]
fn wait_for_process_exit(pid:u32,timeout:Duration)->Result<(),Error>{let end=Instant::now()+timeout;while process_is_alive(pid){if Instant::now()>=end{return Err(format!("Timed out waiting for process {pid}").into());}std::thread::sleep(Duration::from_millis(100));}Ok(())}

#[cfg(test)] mod tests {
    use super::*;
    #[test]
    #[cfg(all(unix, not(target_os="macos")))]
    fn legacy_installer_keeps_gpuix_runtime() {
        let Some(payload)=std::env::var_os("NEOISM_TEST_UPDATE_PAYLOAD") else { return; };
        let target=tempfile::tempdir().unwrap();
        let executable=target.path().join("neoism");
        std::fs::write(&executable,"old installation").unwrap();
        std::fs::write(target.path().join("unrelated-note"),"keep me").unwrap();
        macos_update::install_unix_loose(&executable,Path::new(&payload)).unwrap();
        // This is the deployed old installer's four-executables-plus-web
        // transaction. It deliberately does not copy root-level GPUIX libs.
        assert!(!target.path().join("libneoism_gpuix_core.so").exists());
        assert!(target.path().join("web/native/libneoism_gpuix_core.so").is_file());
        for args in [vec!["--neoism-verify-runtime"],vec!["update","--help"],vec!["--update","nightly","--help"]] {
            let output=std::process::Command::new(&executable).args(args).output().unwrap();
            assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
        }
        assert_eq!(std::fs::read_to_string(target.path().join("unrelated-note")).unwrap(),"keep me");
    }
    #[test] fn selects_nightly_by_version_and_excludes_drafts_and_stable(){
        let releases=serde_json::json!([
            {"tag_name":"v0.7.143-nightly.20261030.2","draft":false,"prerelease":true},
            {"tag_name":"v0.7.143-nightly.20261030.10","draft":false,"prerelease":true},
            {"tag_name":"v0.7.144-nightly.1","draft":true,"prerelease":true},
            {"tag_name":"v0.7.143","draft":false,"prerelease":false}]);
        assert_eq!(select_release(&releases,true,None).unwrap()["tag_name"],"v0.7.143-nightly.20261030.10");
        assert_eq!(select_release(&releases,false,None).unwrap()["tag_name"],"v0.7.143");
        assert!(select_release(&releases,true,Some("../bad")).is_err());
    }
}
