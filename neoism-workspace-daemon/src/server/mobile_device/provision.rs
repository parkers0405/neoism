//! Native provisioning only. The caller owns legacy config, bind and reload.
//! Re-provision and load the returned pair at least monthly for continuous service.

use anyhow::{anyhow, ensure, Context, Result};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer,
    KeyPair, KeyUsagePurpose,
};
use serde::Deserialize;
use std::{
    fs::{self, File},
    io::{ErrorKind, Write},
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use time::OffsetDateTime;
use tokio::{io::AsyncReadExt, process::Command, time::timeout};
use url::Url;

pub struct Config {
    pub addr: SocketAddr,
    pub origin: Url,
    pub cert: PathBuf,
    pub key: PathBuf,
    pub ca: Option<PathBuf>,
}

const PORT: u16 = 8443;
// Use installation paths, never the project's working directory or its PATH.
fn installed_clis() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    return ["/Applications/Tailscale.app/Contents/MacOS/Tailscale", "/opt/homebrew/bin/tailscale", "/usr/local/bin/tailscale"].map(PathBuf::from).into();
    #[cfg(target_os = "linux")]
    return ["/usr/bin/tailscale", "/usr/local/bin/tailscale", "/run/current-system/sw/bin/tailscale"].map(PathBuf::from).into();
    #[cfg(windows)]
    return ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"].into_iter()
        .filter_map(std::env::var_os).map(PathBuf::from).filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("Tailscale").join("tailscale.exe")).collect();
}
const STATUS_LIMIT: u64 = 1024 * 1024;
const EPOCH: i64 = 30 * 86400;

pub async fn provision() -> anyhow::Result<Config> {
    let ip = discover(&installed_clis(), Duration::from_secs(3)).await?;
    tokio::task::spawn_blocking(move || {
        let base = dirs::data_local_dir()
            .filter(|p| p.is_absolute())
            .context("cannot locate the user's application data directory")?;
        fs::create_dir_all(&base)?;
        provision_at(&base.join("Neoism Mobile"), ip, OffsetDateTime::now_utc())
    })
    .await?
}

async fn discover(clis: &[impl AsRef<Path>], budget: Duration) -> Result<IpAddr> {
    let mut failure = anyhow!("install Tailscale and connect this computer to your tailnet");
    for cli in clis {
        let probe = async {
            let mut cmd = Command::new(cli.as_ref());
            cmd.env_clear();
            #[cfg(unix)]
            cmd.env("PATH", "/usr/bin:/bin");
            #[cfg(target_os = "macos")]
            cmd.env("TAILSCALE_BE_CLI", "1");
            #[cfg(windows)]
            {
                // GUI-triggered discovery must not flash a Windows console.
                cmd.creation_flags(0x08000000);
                for name in ["SystemRoot", "LOCALAPPDATA", "ProgramData", "USERPROFILE", "TEMP", "TMP"] {
                    if let Some(value) = std::env::var_os(name) { cmd.env(name, value); }
                }
            }
            if let Some(home) = dirs::home_dir() {
                cmd.env("HOME", home);
            }
            let mut child = cmd
                .args(["status", "--json", "--peers=false"])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()?;
            let mut bytes = Vec::new();
            child
                .stdout
                .take()
                .context("missing Tailscale status pipe")?
                .take(STATUS_LIMIT + 1)
                .read_to_end(&mut bytes)
                .await?;
            ensure!(
                bytes.len() as u64 <= STATUS_LIMIT,
                "Tailscale status too large"
            );
            ensure!(
                child.wait().await?.success(),
                "could not read Tailscale status"
            );
            self_ip(&bytes)
        };
        match timeout(budget, probe).await {
            Ok(Ok(ip)) => return Ok(ip),
            Ok(Err(e))
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == ErrorKind::NotFound) =>
            {
                continue
            }
            Ok(Err(e)) => failure = e,
            Err(_) => failure = anyhow!("Tailscale status timed out"),
        }
    }
    Err(failure)
}

fn tailnet(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => matches!(v.octets(), [100, 64..=127, _, _]),
        IpAddr::V6(v) => v.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}

fn self_ip(bytes: &[u8]) -> Result<IpAddr> {
    #[derive(Deserialize)]
    struct Node {
        #[serde(rename = "TailscaleIPs")]
        ips: Vec<IpAddr>,
    }
    #[derive(Deserialize)]
    struct Status {
        #[serde(rename = "BackendState")]
        state: String,
        #[serde(rename = "Self")]
        node: Node,
        #[serde(rename = "TailscaleIPs")]
        ips: Vec<IpAddr>,
    }
    // Do not propagate JSON details: status contains private tailnet metadata.
    let s: Status =
        serde_json::from_slice(bytes).map_err(|_| anyhow!("invalid Tailscale status"))?;
    ensure!(s.state == "Running", "connect Tailscale before pairing");
    s.node
        .ips
        .into_iter()
        // Prefer IPv4, stably, independent of list order. Never use a peer or
        // subnet route, or a Self address not confirmed by the running backend.
        .filter(|ip| tailnet(*ip) && s.ips.contains(ip))
        .min()
        .context("no local Tailscale address")
}

// The CA's DN and key identifier algorithm are part of the on-disk v1 format.
// rcgen's long-lived root validity is intentional: changing the root breaks QR pins.
fn ca_params() -> CertificateParams {
    let mut p = CertificateParams::default();
    p.distinguished_name
        .push(DnType::CommonName, "Neoism Mobile CA");
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    p
}

#[cfg(windows)]
mod win_store {
    use super::*;
    use std::{
        ffi::c_void,
        io,
        mem::{size_of, size_of_val},
        os::windows::{
            ffi::OsStrExt,
            fs::MetadataExt,
            io::{FromRawHandle, OwnedHandle},
        },
        ptr::null_mut,
    };
    use windows_sys::Win32::{
        Foundation::{LocalFree, ERROR_SUCCESS},
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW,
                SDDL_REVISION_1, SE_FILE_OBJECT,
            },
            EqualSid, GetAce, GetSecurityDescriptorControl, GetTokenInformation, IsWellKnownSid,
            TokenUser, WinBuiltinAdministratorsSid, WinCreatorOwnerRightsSid, WinLocalSystemSid,
            ACCESS_ALLOWED_ACE, ACE_HEADER, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION,
            NO_PROPAGATE_INHERIT_ACE, OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION,
            PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, SECURITY_MAX_SID_SIZE,
            SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
        },
        Storage::FileSystem::{CreateDirectoryW, MoveFileExW, FILE_ATTRIBUTE_REPARSE_POINT, MOVEFILE_WRITE_THROUGH},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    struct Descriptor(PSECURITY_DESCRIPTOR);
    impl Drop for Descriptor {
        fn drop(&mut self) {
            unsafe { LocalFree(self.0) };
        }
    }

    pub(super) fn reject_reparse(m: &fs::Metadata) -> io::Result<()> {
        if m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(ErrorKind::InvalidData, "reparse point in mobile TLS state path"));
        }
        Ok(())
    }

    fn wide(path: &Path) -> io::Result<Vec<u16>> {
        let mut w: Vec<_> = path.as_os_str().encode_wide().collect();
        if w.contains(&0) {
            return Err(io::Error::new(ErrorKind::InvalidInput, "NUL in TLS state path"));
        }
        w.push(0);
        Ok(w)
    }

    // Install the root ACL at creation, not after another initializer could
    // open it. Children (including staging files) start private too. OW is
    // OWNER RIGHTS; OI/CI propagate these grants to both files and directories.
    pub(super) fn create_root(path: &Path) -> io::Result<()> {
        // symlink_metadata checks only the final component. Do not reach the
        // store through a junction (or other redirection) higher in the path.
        for ancestor in path.ancestors().skip(1) {
            reject_reparse(&fs::symlink_metadata(ancestor)?)?;
        }
        let path = wide(path)?;
        unsafe {
            let mut sd = null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                windows_sys::w!("D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)"),
                SDDL_REVISION_1, &mut sd, null_mut(),
            ) == 0 {
                return Err(io::Error::last_os_error());
            }
            let sd = Descriptor(sd);
            let sa = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: 0,
            };
            if CreateDirectoryW(path.as_ptr(), &sa) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    // check_owner_only checks *read* grants. A TLS store must reject foreign
    // owners, write/delete grants and unrecognized ACE types as well.
    pub(super) fn closed_acl(path: &Path, kind: StateKind) -> Result<bool> {
        let path = wide(path)?;
        unsafe {
            let (mut sd, mut owner, mut acl) = (null_mut(), null_mut(), null_mut());
            let status = GetNamedSecurityInfoW(path.as_ptr(), SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner, null_mut(), &mut acl, null_mut(), &mut sd);
            if status != ERROR_SUCCESS {
                return Err(io::Error::from_raw_os_error(status as i32).into());
            }
            let _sd = Descriptor(sd);
            if owner.is_null() || acl.is_null() {
                return Ok(false);
            }
            // The root and PEMs must not start inheriting new grants if their
            // parent changes. Only the generation dirs inherit our root ACL.
            if !matches!(kind, StateKind::Generation) {
                let (mut control, mut revision) = (0, 0);
                if GetSecurityDescriptorControl(sd, &mut control, &mut revision) == 0 {
                    return Err(io::Error::last_os_error().into());
                }
                if control & SE_DACL_PROTECTED == 0 {
                    return Ok(false);
                }
            }
            let mut token = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error().into());
            }
            let _token = OwnedHandle::from_raw_handle(token);
            // TOKEN_USER followed by the SID; usize supplies pointer alignment.
            const TOKEN_BYTES: usize = size_of::<TOKEN_USER>() + SECURITY_MAX_SID_SIZE as usize;
            let mut buf = [0usize; TOKEN_BYTES.div_ceil(size_of::<usize>())];
            let mut len = 0;
            if GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(),
                size_of_val(&buf) as u32, &mut len) == 0 {
                return Err(io::Error::last_os_error().into());
            }
            let user = (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid;
            let trusted = |s| EqualSid(s, user) != 0 || IsWellKnownSid(s, WinLocalSystemSid) != 0
                || IsWellKnownSid(s, WinBuiltinAdministratorsSid) != 0;
            if !trusted(owner) {
                return Ok(false);
            }
            let mut inherits = false;
            for i in 0..u32::from((*acl).AceCount) {
                let mut ace: *mut c_void = null_mut();
                if GetAce(acl, i, &mut ace) == 0 {
                    return Err(io::Error::last_os_error().into());
                }
                // ACCESS_ALLOWED_ACE_TYPE (winnt.h).
                if (*ace.cast::<ACE_HEADER>()).AceType != 0 {
                    return Ok(false);
                }
                let ace = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                let sid = (&raw const ace.SidStart) as PSID;
                if !trusted(sid) && IsWellKnownSid(sid, WinCreatorOwnerRightsSid) == 0 {
                    return Ok(false);
                }
                let flags = u32::from(ace.Header.AceFlags);
                // Without an inheritable ACE a child can get the token's
                // default DACL instead. Require propagation to grandchildren.
                let both = OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE;
                inherits |= flags & (both | NO_PROPAGATE_INHERIT_ACE) == both;
            }
            Ok(matches!(kind, StateKind::Pem) || inherits)
        }
    }

    pub(super) fn publish(src: &Path, dst: &Path) -> io::Result<()> {
        let (src, dst) = (wide(src)?, wide(dst)?);
        // Same-volume, no replacement. Windows does not support Unix-style
        // fsync of these directory handles; request write-through on the move.
        if unsafe { MoveFileExW(src.as_ptr(), dst.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum StateKind {
    Root,
    Generation,
    Pem,
}

fn private(path: &Path, kind: StateKind) -> Result<()> {
    let m = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    win_store::reject_reparse(&m)?;
    let dir = !matches!(kind, StateKind::Pem);
    ensure!(
        if dir { m.is_dir() } else { m.is_file() },
        "unexpected mobile TLS state file type"
    );
    ensure!(dir || m.len() <= 8192, "oversized mobile TLS state file");
    #[cfg(unix)]
    ensure!(
        m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0,
        "insecure mobile TLS state permissions"
    );
    #[cfg(windows)]
    ensure!(win_store::closed_acl(path, kind)?, "insecure mobile TLS state ACL");
    Ok(())
}

fn write_private(path: &Path, pem: &str) -> Result<()> {
    let mut opts = File::options();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    opts.mode(0o600);
    let mut f = opts.open(path)?;
    #[cfg(windows)]
    {
        crate::windows_acl::harden_owner_only(path)?;
        private(path, StateKind::Pem)?;
    }
    f.write_all(pem.as_bytes())?;
    f.sync_all()?;
    Ok(())
}

// Publish an entire immutable pair or nothing. Concurrent initializers cannot
// replace a populated directory, and all callers use the winning pair.
fn pair(
    root: &Path,
    name: &str,
    make: impl FnOnce() -> Result<(String, String)>,
) -> Result<(PathBuf, PathBuf)> {
    let dir = root.join(name);
    match fs::symlink_metadata(&dir) {
        Ok(_) => (),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            #[cfg(unix)]
            let tmp = tempfile::Builder::new()
                .permissions(fs::Permissions::from_mode(0o700))
                .tempdir_in(root)?;
            #[cfg(windows)]
            let tmp = tempfile::tempdir_in(root)?;
            private(tmp.path(), StateKind::Generation)?;
            let (cert, key) = make()?;
            write_private(&tmp.path().join("cert.pem"), &cert)?;
            write_private(&tmp.path().join("key.pem"), &key)?;
            #[cfg(unix)]
            let published = {
                File::open(tmp.path())?.sync_all()?;
                fs::rename(tmp.path(), &dir)
            };
            #[cfg(windows)]
            let published = win_store::publish(tmp.path(), &dir);
            match published {
                Ok(()) => {
                    #[cfg(unix)]
                    File::open(root)?.sync_all()?;
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        ErrorKind::DirectoryNotEmpty | ErrorKind::AlreadyExists
                    ) => {}
                // Windows may report ACCESS_DENIED for an existing directory.
                // Only adopt it if it passes all the normal validation below.
                #[cfg(windows)]
                Err(e) if e.kind() == ErrorKind::PermissionDenied && fs::symlink_metadata(&dir).is_ok() => {}
                Err(e) => return Err(e.into()),
            }
        }
        Err(e) => return Err(e.into()),
    }
    private(&dir, StateKind::Generation)?;
    let cert = dir.join("cert.pem");
    let key = dir.join("key.pem");
    private(&cert, StateKind::Pem)?;
    private(&key, StateKind::Pem)?;
    Ok((cert, key))
}

fn provision_at(root: &Path, ip: IpAddr, now: OffsetDateTime) -> Result<Config> {
    ensure!(tailnet(ip), "an explicit Tailscale IP is required");
    #[cfg(unix)]
    let created = fs::DirBuilder::new().mode(0o700).create(root);
    #[cfg(windows)]
    let created = win_store::create_root(root);
    match created {
        Ok(()) => (),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.into()),
    }
    private(root, StateKind::Root)?;
    let (ca, signer) = pair(root, "ca-v1", || {
        let key = KeyPair::generate()?; // ECDSA P-256
        let cert = ca_params().self_signed(&key)?;
        Ok((cert.pem(), key.serialize_pem()))
    })?;
    let epoch = now.unix_timestamp().div_euclid(EPOCH);
    // Keep existing Unix generation paths. ':' is not a Windows filename character.
    let ip_name = ip.to_string();
    #[cfg(windows)]
    let ip_name = ip_name.replace(':', "_");
    let (cert, key) = pair(root, &format!("leaf-{epoch}-{ip_name}"), || {
        use rustls::{
            pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer},
            sign::CertifiedKey,
        };
        let secret = fs::read_to_string(&signer)?;
        // A restored or damaged identity must not poison a new generation.
        CertifiedKey::from_der(
            vec![CertificateDer::from_pem_file(&ca)?],
            PrivateKeyDer::from_pem_slice(secret.as_bytes())?,
            &rustls::crypto::ring::default_provider(),
        )?;
        let issuer = Issuer::new(ca_params(), KeyPair::from_pem(&secret)?);
        let key = KeyPair::generate()?;
        let mut p = CertificateParams::new(vec![ip.to_string()])?;
        p.distinguished_name
            .push(DnType::CommonName, "Neoism Mobile");
        p.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        p.use_authority_key_identifier_extension = true;
        p.not_before = OffsetDateTime::from_unix_timestamp(epoch * EPOCH - 86400)?;
        p.not_after = OffsetDateTime::from_unix_timestamp((epoch + 3) * EPOCH)?;
        Ok((p.signed_by(&key, &issuer)?.pem(), key.serialize_pem()))
    })?;
    let addr = SocketAddr::new(ip, PORT);
    let cfg = Config {
        addr,
        origin: Url::parse(&format!("https://{addr}"))?,
        cert,
        key,
        ca: Some(ca),
    };
    verify(&cfg, now)?;
    // Other listeners may still be loading a previous generation. Remove only
    // expired generations, never the CA or unrelated entries.
    for entry in fs::read_dir(root)?.flatten() {
        let name = entry.file_name();
        let Some((n, ip)) = name
            .to_str()
            .and_then(|s| s.strip_prefix("leaf-"))
            .and_then(|s| s.split_once('-'))
        else {
            continue;
        };
        #[cfg(windows)]
        let ip = ip.replace('_', ":");
        if n.parse::<i64>().is_ok_and(|n| n <= epoch - 3)
            && ip.parse::<IpAddr>().is_ok_and(tailnet)
            && private(&entry.path(), StateKind::Generation).is_ok()
        {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    Ok(cfg)
}

// Verify a cached or newly generated pair with the same provider as the server.
// Never return a broken pin, wrong SAN, mismatched key, or expired leaf.
fn verify(cfg: &Config, now: OffsetDateTime) -> Result<()> {
    use rustls::{
        client::{verify_server_cert_signed_by_trust_anchor, verify_server_name},
        crypto::ring,
        pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer, UnixTime},
        server::ParsedCertificate,
        sign::CertifiedKey,
        RootCertStore,
    };
    let provider = ring::default_provider();
    let ca =
        CertificateDer::from_pem_file(cfg.ca.as_ref().context("missing mobile TLS CA")?)?;
    ensure!(ca.len() <= 1050, "mobile CA exceeds pairing QR budget");
    let mut roots = RootCertStore::empty();
    roots.add(ca)?;
    let ck = CertifiedKey::from_der(
        vec![CertificateDer::from_pem_file(&cfg.cert)?],
        PrivateKeyDer::from_pem_file(&cfg.key)?,
        &provider,
    )?;
    let leaf = ParsedCertificate::try_from(ck.end_entity_cert()?)?;
    let time =
        UnixTime::since_unix_epoch(Duration::from_secs(now.unix_timestamp().try_into()?));
    verify_server_cert_signed_by_trust_anchor(
        &leaf,
        &roots,
        &[],
        time,
        provider.signature_verification_algorithms.all,
    )?;
    verify_server_name(&leaf, &cfg.addr.ip().into())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(700 * EPOCH + 100).unwrap()
    }

    #[test]
    fn selects_only_running_self_tailnet_ip() {
        let status = |state, ips| {
            format!(
                r#"{{"BackendState":"{state}","TailscaleIPs":{ips},"Peer":{{"other":{{"TailscaleIPs":["100.64.0.99"]}}}},"Self":{{"TailscaleIPs":{ips}}}}}"#
            )
        };
        let pick = |state, ips| self_ip(status(state, ips).as_bytes());
        let both = r#"["fd7a:115c:a1e0::4","100.127.255.254"]"#;
        assert_eq!(
            pick("Running", both).unwrap().to_string(),
            "100.127.255.254"
        );
        assert!(pick("NeedsLogin", both).is_err());
        assert!(pick("Running", r#"["0.0.0.0","127.0.0.1","192.168.1.2","100.63.255.255","100.128.0.0","fd7a:115c:a1e1::1","::"]"#).is_err());
        assert!(pick("Running", r#"["fd7a:115c:a1e0::7"]"#)
            .unwrap()
            .is_ipv6());
        assert!(pick("Running", "[]").is_err());
        assert!(self_ip(br#"{"BackendState":"Running","TailscaleIPs":["100.64.0.99"],"Self":{"TailscaleIPs":["100.64.0.1"]}}"#).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bounded_cli_fallback_and_private_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let script = |name, body: &str| {
            let p = tmp.path().join(name);
            fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
            p
        };
        let slow = script("slow", "exec /bin/sleep 30");
        let big = script("big", "exec /usr/bin/yes PRIVATE");
        let invalid = script("bad", "echo 'PRIVATE' >&2; echo 'PRIVATE'");
        let ok = script(
            "CLI with spaces",
            r#"[ "$#" = 3 ] && [ "$1" = status ] && [ "$2" = --json ] && [ "$3" = --peers=false ] || exit 2
echo '{"BackendState":"Running","TailscaleIPs":["100.64.1.2"],"Self":{"TailscaleIPs":["100.64.1.2"]}}'"#,
        );
        let budget = Duration::from_millis(250);
        for (p, expected) in [
            (&slow, "timed out"),
            (&big, "too large"),
            (&invalid, "invalid Tailscale status"),
        ] {
            let err = discover(&[p.to_str().unwrap()], budget)
                .await
                .unwrap_err()
                .to_string();
            assert!(err.contains(expected) && !err.contains("PRIVATE"), "{err}");
        }
        assert_eq!(
            discover(&[invalid.to_str().unwrap(), ok.to_str().unwrap()], budget)
                .await
                .unwrap()
                .to_string(),
            "100.64.1.2"
        );
    }

    #[test]
    fn tls_rotation_preserves_pin_and_previous_generation() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path().join("mobile");
        let ip = "fd7a:115c:a1e0::123".parse()?;
        let mut cfg = provision_at(&root, ip, now())?;
        assert_eq!(cfg.origin.as_str(), "https://[fd7a:115c:a1e0::123]:8443/");
        let ca = fs::read(cfg.ca.as_ref().unwrap())?;
        assert_eq!(cfg.cert, provision_at(&root, ip, now())?.cert);
        for file in [&cfg.key, &cfg.cert, cfg.ca.as_ref().unwrap(), &root.join("ca-v1/key.pem")] {
            #[cfg(unix)]
            assert_eq!(fs::metadata(file)?.mode() & 0o777, 0o600);
            #[cfg(windows)]
            assert!(crate::windows_acl::check_owner_only(file)?);
        }
        let later = now() + time::Duration::days(30);
        let next = provision_at(&root, ip, later)?;
        assert_eq!(ca, fs::read(next.ca.unwrap())?);
        assert_ne!(fs::read(&cfg.cert)?, fs::read(next.cert)?);
        assert_ne!(fs::read(&cfg.key)?, fs::read(next.key)?);
        verify(&cfg, later)?;
        assert!(verify(&cfg, now() + time::Duration::days(91)).is_err());
        cfg.addr.set_ip("100.64.1.99".parse()?);
        assert!(verify(&cfg, now()).is_err());
        provision_at(&root, ip, now() + time::Duration::days(91))?;
        assert!(!cfg.key.exists());
        Ok(())
    }

    #[test]
    fn concurrent_first_use_converges_and_insecure_state_fails_closed() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path().join("mobile");
        let create = || provision_at(&root, "100.64.1.2".parse()?, now());
        let start = std::sync::Barrier::new(2);
        let cfg = std::thread::scope(|s| -> Result<_> {
            let other = s.spawn(|| -> Result<_> {
                start.wait();
                let cfg = create()?;
                Ok((fs::read(cfg.ca.unwrap())?, fs::read(cfg.key)?))
            });
            start.wait();
            let cfg = create()?;
            let observed = (fs::read(cfg.ca.as_ref().unwrap())?, fs::read(&cfg.key)?);
            assert!(observed == other.join().unwrap()?, "initializers used different identities");
            verify(&cfg, now())?;
            Ok(cfg)
        })?;
        let signer = root.join("ca-v1/key.pem");
        let ca = cfg.ca.unwrap();
        let pin = fs::read(&ca)?;
        #[cfg(unix)]
        fs::set_permissions(&signer, fs::Permissions::from_mode(0o644))?;
        #[cfg(windows)]
        {
            // Even a write-only grant could replace a pinned identity.
            let status = std::process::Command::new("icacls.exe")
                .arg(&signer).args(["/grant", "*S-1-1-0:(WD)"]).status()?;
            assert!(status.success());
        }
        assert!(create().is_err());
        fs::remove_file(&signer)?;
        assert!(create().is_err());
        #[cfg(unix)]
        {
            symlink(&cfg.key, &signer)?;
            assert!(create().is_err());
        }
        assert_eq!(pin, fs::read(&ca)?);
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn rejects_junction_even_to_a_private_store() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let ip = "100.64.1.2".parse()?;
        let valid = tmp.path().join("valid");
        provision_at(&valid, ip, now())?;
        // Directory junctions don't need SeCreateSymbolicLinkPrivilege.
        let status = std::process::Command::new("cmd.exe").current_dir(tmp.path())
            .args(["/d", "/c", "mklink", "/j", "redirect", "valid"]).status()?;
        assert!(status.success());
        let redirect = tmp.path().join("redirect");
        assert!(provision_at(&redirect, ip, now()).is_err());
        assert!(provision_at(&redirect.join("nested"), ip, now()).is_err());
        assert!(!valid.join("nested").exists());
        fs::remove_dir(redirect)?;
        Ok(())
    }
}
