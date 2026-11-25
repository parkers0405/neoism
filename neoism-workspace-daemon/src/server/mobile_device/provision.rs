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
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
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
const CLIS: &[&str] = &[
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
    "/opt/homebrew/bin/tailscale",
    "/usr/local/bin/tailscale",
];
const STATUS_LIMIT: u64 = 1024 * 1024;
const EPOCH: i64 = 30 * 86400;

pub async fn provision() -> anyhow::Result<Config> {
    let ip = discover(CLIS, Duration::from_secs(3)).await?;
    tokio::task::spawn_blocking(move || {
        let base = dirs::data_local_dir()
            .filter(|p| p.is_absolute())
            .context("cannot locate user Application Support directory")?;
        fs::create_dir_all(&base)?;
        provision_at(&base.join("Neoism Mobile"), ip, OffsetDateTime::now_utc())
    })
    .await?
}

async fn discover(clis: &[&str], budget: Duration) -> Result<IpAddr> {
    let mut failure = anyhow!("install Tailscale and connect this Mac to your tailnet");
    for cli in clis {
        let probe = async {
            let mut cmd = Command::new(cli);
            cmd.env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("TAILSCALE_BE_CLI", "1");
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

fn private(path: &Path, dir: bool) -> Result<()> {
    let m = fs::symlink_metadata(path)?;
    ensure!(
        if dir { m.is_dir() } else { m.is_file() },
        "unexpected mobile TLS state file type"
    );
    ensure!(dir || m.len() <= 8192, "oversized mobile TLS state file");
    ensure!(
        m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0,
        "insecure mobile TLS state permissions"
    );
    Ok(())
}

fn write_private(path: &Path, pem: &str) -> Result<()> {
    let mut f = File::options()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
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
            let tmp = tempfile::Builder::new()
                .permissions(fs::Permissions::from_mode(0o700))
                .tempdir_in(root)?;
            let (cert, key) = make()?;
            write_private(&tmp.path().join("cert.pem"), &cert)?;
            write_private(&tmp.path().join("key.pem"), &key)?;
            File::open(tmp.path())?.sync_all()?;
            match fs::rename(tmp.path(), &dir) {
                Ok(()) => File::open(root)?.sync_all()?,
                Err(e)
                    if matches!(
                        e.kind(),
                        ErrorKind::DirectoryNotEmpty | ErrorKind::AlreadyExists
                    ) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Err(e) => return Err(e.into()),
    }
    private(&dir, true)?;
    let cert = dir.join("cert.pem");
    let key = dir.join("key.pem");
    private(&cert, false)?;
    private(&key, false)?;
    Ok((cert, key))
}

fn provision_at(root: &Path, ip: IpAddr, now: OffsetDateTime) -> Result<Config> {
    ensure!(tailnet(ip), "an explicit Tailscale IP is required");
    match fs::DirBuilder::new().mode(0o700).create(root) {
        Ok(()) => (),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.into()),
    }
    private(root, true)?;
    let (ca, signer) = pair(root, "ca-v1", || {
        let key = KeyPair::generate()?; // ECDSA P-256
        let cert = ca_params().self_signed(&key)?;
        Ok((cert.pem(), key.serialize_pem()))
    })?;
    let epoch = now.unix_timestamp().div_euclid(EPOCH);
    let (cert, key) = pair(root, &format!("leaf-{epoch}-{ip}"), || {
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
        if n.parse::<i64>().is_ok_and(|n| n <= epoch - 3)
            && ip.parse::<IpAddr>().is_ok_and(tailnet)
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
        for file in [&cfg.key, &root.join("ca-v1/key.pem")] {
            assert_eq!(fs::metadata(file)?.mode() & 0o777, 0o600);
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
        let cfg = std::thread::scope(|s| -> Result<_> {
            let other = s.spawn(create);
            let cfg = create()?;
            assert_eq!(cfg.cert, other.join().unwrap()?.cert);
            verify(&cfg, now())?;
            Ok(cfg)
        })?;
        let signer = root.join("ca-v1/key.pem");
        let ca = cfg.ca.unwrap();
        let pin = fs::read(&ca)?;
        fs::set_permissions(&signer, fs::Permissions::from_mode(0o644))?;
        assert!(create().is_err());
        fs::remove_file(&signer)?;
        assert!(create().is_err());
        symlink(&cfg.key, &signer)?;
        assert!(create().is_err());
        assert_eq!(pin, fs::read(&ca)?);
        Ok(())
    }
}
