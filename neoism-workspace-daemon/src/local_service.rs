//! Per-user discovery for automatically managed loopback services.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
};
use tokio::net::TcpListener;

fn path(name: &str) -> PathBuf {
    crate::daemon_token::daemon_token_path()
        .with_file_name(format!("{name}-endpoint.json"))
}

#[derive(Serialize, Deserialize)]
struct Record {
    protocol: u8,
    pid: u32,
    address: SocketAddr,
    nonce: String,
}

pub(crate) fn read(name: &str) -> Option<SocketAddr> {
    read_at(&path(name))
}

fn read_at(path: &Path) -> Option<SocketAddr> {
    if fs::metadata(path).ok()?.len() > 4096 {
        return None;
    }
    let record: Record = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    (record.protocol == 1
        && record.pid > 0
        && !record.nonce.is_empty()
        && record.address.ip().is_loopback()
        && record.address.port() != 0)
        .then_some(record.address)
}

/// Keep the file open for the managed daemon's lifetime. The OS releases the
/// lock on crash/update; never unlink it and split concurrent owners' locks.
pub(crate) fn lock(name: &str) -> io::Result<File> {
    let path = path(name).with_extension("lock");
    lock_at(&path)
}

fn lock_at(path: &Path) -> io::Result<File> {
    fs::create_dir_all(path.parent().unwrap())?;
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    file.try_lock().map_err(|error| match error {
        fs::TryLockError::WouldBlock => io::Error::new(
            io::ErrorKind::WouldBlock,
            "managed local service already owned",
        ),
        fs::TryLockError::Error(error) => error,
    })?;
    Ok(file)
}

/// Reserve the listener, not merely a port number: no drop/rebind race.
pub(crate) async fn bind(
    preferred: SocketAddr,
    allow_fallback: bool,
) -> io::Result<TcpListener> {
    match TcpListener::bind(preferred).await {
        // Windows exclusive listeners/excluded port ranges can report access
        // denied rather than AddrInUse. Only implicit loopback ports may move.
        Err(error)
            if allow_fallback
                && preferred.ip().is_loopback()
                && (error.kind() == io::ErrorKind::AddrInUse
                    || cfg!(windows)
                        && error.kind() == io::ErrorKind::PermissionDenied) =>
        {
            TcpListener::bind(SocketAddr::new(preferred.ip(), 0)).await
        }
        result => result,
    }
}

pub(crate) struct Publication {
    path: PathBuf,
    contents: Vec<u8>,
}

impl Publication {
    pub(crate) fn publish(name: &str, address: SocketAddr) -> io::Result<Self> {
        Self::publish_at(path(name), address)
    }

    fn publish_at(path: PathBuf, address: SocketAddr) -> io::Result<Self> {
        if !address.ip().is_loopback() || address.port() == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "discovery requires a bound loopback address",
            ));
        }
        fs::create_dir_all(path.parent().unwrap())?;
        let nonce = uuid::Uuid::new_v4().to_string();
        let contents = serde_json::to_vec(&Record {
            protocol: 1,
            pid: std::process::id(),
            address,
            nonce: nonce.clone(),
        })?;
        let temporary = path.with_extension(format!("tmp-{nonce}"));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&contents)?;
            drop(file);
            fs::rename(&temporary, &path)
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(Self { path, contents })
    }
}

impl Drop for Publication {
    fn drop(&mut self) {
        if fs::read(&self.path).ok().as_deref() == Some(self.contents.as_slice()) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_lock_is_exclusive_and_released_without_stale_pid_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.lock");
        let owner = lock_at(&path).unwrap();
        assert!(
            matches!(lock_at(&path), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
        );
        drop(owner);
        assert!(lock_at(&path).is_ok());
    }

    #[tokio::test]
    async fn fallback_reserves_a_distinct_loopback_listener_and_explicit_bind_fails() {
        let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let preferred = occupied.local_addr().unwrap();
        assert!(bind(preferred, false).await.is_err());
        let fallback = bind(preferred, true).await.unwrap();
        let actual = fallback.local_addr().unwrap();
        assert_ne!(actual, preferred);
        assert!(actual.ip().is_loopback());
        assert!(TcpListener::bind(actual).await.is_err());
    }

    #[test]
    fn discovery_replacement_survives_previous_owner_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon-endpoint.json");
        let old =
            Publication::publish_at(path.clone(), "127.0.0.1:12345".parse().unwrap())
                .unwrap();
        let new =
            Publication::publish_at(path.clone(), "127.0.0.1:12346".parse().unwrap())
                .unwrap();
        drop(old);
        assert_eq!(read_at(&path), Some("127.0.0.1:12346".parse().unwrap()));
        drop(new);
        assert!(!path.exists());
        assert!(Publication::publish_at(path, "0.0.0.0:12345".parse().unwrap()).is_err());
    }
}