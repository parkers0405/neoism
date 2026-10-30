//! OS-user SSH recipes, deliberately separate from servers and their credentials.
use crate::ssh_hosts::safe_connection_args;
use serde::{Deserialize, Serialize};
use std::{fs, io, io::Write, net::Ipv6Addr, path::PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SavedSystem {
    pub id: String,
    pub name: String,
    pub target: String,
    pub ssh_args: Vec<String>,
    #[serde(default = "legacy_enabled")]
    pub enabled: bool,
}

fn legacy_enabled() -> bool {
    true
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryData {
    systems: Vec<SavedSystem>,
}

pub(crate) struct SystemRegistry {
    directory: PathBuf,
    data: RegistryData,
}

impl SystemRegistry {
    pub(crate) fn load() -> io::Result<Self> {
        Self::load_directory(
            neoism_agent_service_api::server_registry::registry_directory(),
        )
    }

    fn load_directory(directory: PathBuf) -> io::Result<Self> {
        let mut registry = Self {
            directory,
            data: RegistryData::default(),
        };
        registry.reload()?;
        Ok(registry)
    }

    // Keep the lock file stable across atomic replacements of systems.json.
    // File::try_lock matches the shared server registry's bounded contention policy.
    fn lock_and_reload(&mut self) -> io::Result<fs::File> {
        fs::create_dir_all(&self.directory)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.directory.join("systems.lock"))?;
        lock.try_lock().map_err(|error| {
            io::Error::other(format!("system registry busy: {error}"))
        })?;
        let data: RegistryData = match fs::read(self.directory.join("systems.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                RegistryData::default()
            }
            Err(error) => return Err(error),
        };
        // Invalid recipes are corruption, not an invitation to silently repair and
        // overwrite a user's file. Keep the previous in-memory snapshot on error.
        let mut ids = std::collections::HashSet::new();
        for system in &data.systems {
            let valid = (|| {
                if system.id.is_empty() || !ids.insert(system.id.as_str()) {
                    return Err(invalid("empty or duplicate system id"));
                }
                normalized_name(&system.name, &system.target)?;
                normalize_target(&system.target)?;
                if normalize_args(&system.ssh_args)? != system.ssh_args {
                    return Err(invalid("noncanonical or unsafe saved SSH options"));
                }
                Ok(())
            })();
            valid.map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        }
        self.data = data;
        Ok(lock)
    }

    pub(crate) fn reload(&mut self) -> io::Result<()> {
        let _lock = self.lock_and_reload()?;
        Ok(())
    }

    pub(crate) fn systems(&self) -> &[SavedSystem] {
        &self.data.systems
    }

    pub(crate) fn system(&self, id: &str) -> Option<&SavedSystem> {
        self.data.systems.iter().find(|system| system.id == id)
    }

    /// Matching recipes retain their ID and get the new display name. Option
    /// order is significant (OpenSSH commonly uses the first supplied value).
    pub(crate) fn upsert(
        &mut self,
        name: &str,
        target: &str,
        ssh_args: Vec<String>,
    ) -> io::Result<SavedSystem> {
        let target = normalize_target(target)?;
        let name = normalized_name(name, &target)?;
        let ssh_args = normalize_args(&ssh_args)?;
        let _lock = self.lock_and_reload()?;
        let mut systems = self.data.systems.clone();
        let existing = systems.iter_mut().find(|system| {
            normalize_target(&system.target).ok().as_ref() == Some(&target)
                && normalize_args(&system.ssh_args).ok().as_ref() == Some(&ssh_args)
        });
        let saved = if let Some(system) = existing {
            system.name = name;
            system.target = target;
            system.ssh_args = ssh_args;
            system.clone()
        } else {
            let system = SavedSystem {
                id: uuid::Uuid::new_v4().to_string(),
                name,
                target,
                ssh_args,
                enabled: false,
            };
            systems.push(system.clone());
            system
        };
        self.persist(&RegistryData {
            systems: systems.clone(),
        })?;
        self.data.systems = systems;
        Ok(saved)
    }

    /// Change activation intent under the same lock as recipe updates. Failed
    /// persistence leaves the reloaded snapshot unchanged.
    pub(crate) fn set_enabled(
        &mut self,
        id: &str,
        enabled: bool,
    ) -> io::Result<SavedSystem> {
        let _lock = self.lock_and_reload()?;
        let mut systems = self.data.systems.clone();
        let system = systems
            .iter_mut()
            .find(|system| system.id == id)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "saved system not found")
            })?;
        system.enabled = enabled;
        let saved = system.clone();
        self.persist(&RegistryData {
            systems: systems.clone(),
        })?;
        self.data.systems = systems;
        Ok(saved)
    }

    /// Removing an unknown ID is a successful no-op.
    pub(crate) fn remove(&mut self, id: &str) -> io::Result<()> {
        let _lock = self.lock_and_reload()?;
        let systems: Vec<_> = self
            .data
            .systems
            .iter()
            .filter(|s| s.id != id)
            .cloned()
            .collect();
        if systems.len() != self.data.systems.len() {
            self.persist(&RegistryData {
                systems: systems.clone(),
            })?;
            self.data.systems = systems;
        }
        Ok(())
    }

    fn persist(&self, data: &RegistryData) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(data).map_err(io::Error::other)?;
        // Same-directory tempfile + persist replaces atomically, including on
        // Windows; RAII cleans up failed writes. Never truncate the live file.
        let mut temporary = tempfile::NamedTempFile::new_in(&self.directory)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(self.directory.join("systems.json"))
            .map_err(|error| error.error)?;
        Ok(())
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn normalized_name(name: &str, target: &str) -> io::Result<String> {
    if name.chars().any(char::is_control) {
        return Err(invalid("invalid system name"));
    }
    Ok(if name.trim().is_empty() {
        target
    } else {
        name.trim()
    }
    .to_owned())
}

/// Validate a draft without changing it or touching the registry. Callers should
/// dial using the target/options returned by `upsert`, not the unnormalized draft:
/// the SSH engine retains and hashes those exact argv strings for identity.
pub(crate) fn validate_system_target(target: &str) -> io::Result<()> {
    normalize_target(target).map(|_| ())
}

/// Compare runtime SSH recipes with saved recipes using the same validation and
/// canonicalization as persistence. Invalid routes never match, even themselves;
/// ignored transport flags follow `safe_connection_args`, just as in the engine.
pub(crate) fn same_system_route(
    a_target: &str,
    a_args: &[String],
    b_target: &str,
    b_args: &[String],
) -> bool {
    let canonical =
        |target: &str, args: &[String]| -> io::Result<(String, Vec<String>)> {
            Ok((normalize_target(target)?, normalize_args(args)?))
        };
    match (canonical(a_target, a_args), canonical(b_target, b_args)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn normalize_target(target: &str) -> io::Result<String> {
    if target.is_empty()
        || target.starts_with('-')
        || target.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(invalid(
            "expected an SSH alias or [user@]host, without commands",
        ));
    }
    let (user, host) = match target.split_once('@') {
        Some((user, host)) => {
            if user.is_empty()
                || user.starts_with('-')
                || !user
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-+".contains(&c))
            {
                return Err(invalid("invalid SSH user"));
            }
            (Some(user), host)
        }
        None => (None, target),
    };
    let host = if host.contains(':') {
        let address = host
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(host);
        address
            .parse::<Ipv6Addr>()
            .map_err(|_| invalid("invalid IPv6 host (ports belong in -p)"))?
            .to_string()
    } else {
        if host.is_empty()
            || host.starts_with('-')
            || !host
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        {
            return Err(invalid("invalid SSH host"));
        }
        host.to_ascii_lowercase()
    };
    Ok(match user {
        Some(user) => format!("{user}@{host}"),
        None => host,
    })
}

fn normalize_args(args: &[String]) -> io::Result<Vec<String>> {
    if args.len() % 2 != 0 {
        return Err(invalid("SSH options must be flag/value pairs"));
    }
    if args.iter().any(|arg| arg.chars().any(char::is_control)) {
        return Err(invalid("control characters in SSH options"));
    }
    let safe = safe_connection_args(args);
    let mut result = Vec::new();
    for pair in safe.chunks_exact(2) {
        let flag = pair[0];
        let value = pair[1];
        if value.is_empty() {
            return Err(invalid("empty SSH option value"));
        }
        let value = match flag {
            "-p" => value
                .parse::<u16>()
                .ok()
                .filter(|p| *p != 0)
                .map(|p| p.to_string())
                .ok_or_else(|| invalid("invalid SSH port"))?,
            "-l" => {
                normalize_target(&format!("{value}@host"))?;
                value.to_owned()
            }
            "-J" => value
                .split(',')
                .map(normalize_target)
                .collect::<io::Result<Vec<_>>>()?
                .join(","),
            // Paths, not key contents or passphrases. A config file path is a
            // recipe reference just as an alias using ~/.ssh/config is.
            "-i" | "-F" => value.to_owned(),
            "-b" => normalize_target(value)?,
            "-c" if value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"@.,_+-".contains(&c)) =>
            {
                value.to_owned()
            }
            "-o" => normalize_option(value)?,
            _ => return Err(invalid("SSH option cannot be saved")),
        };
        result.push(flag.to_owned());
        result.push(value);
    }
    Ok(result)
}

fn normalize_option(value: &str) -> io::Result<String> {
    let (key, value) = value
        .split_once('=')
        .or_else(|| value.split_once(char::is_whitespace))
        .ok_or_else(|| invalid("expected SSH option key=value"))?;
    let key = key.to_ascii_lowercase();
    let value = value.trim();
    // Positive allowlist: no ProxyCommand/LocalCommand/RemoteCommand, forwarding,
    // environment injection, arbitrary provider plugins, or credential values.
    let value = match key.as_str() {
        "port" => value
            .parse::<u16>()
            .ok()
            .filter(|p| *p != 0)
            .map(|p| p.to_string())
            .ok_or_else(|| invalid("invalid SSH port"))?,
        "hostname" => normalize_target(value).and_then(|host| {
            if host.contains('@') {
                Err(invalid("invalid HostName"))
            } else {
                Ok(host)
            }
        })?,
        "user" => {
            normalize_target(&format!("{value}@host"))?;
            value.to_owned()
        }
        "identityfile" | "userknownhostsfile" | "globalknownhostsfile" => {
            value.to_owned()
        }
        "identitiesonly"
        | "batchmode"
        | "compression"
        | "stricthostkeychecking"
        | "addressfamily" => {
            let value = value.to_ascii_lowercase();
            if !matches!(
                value.as_str(),
                "yes" | "no" | "ask" | "accept-new" | "any" | "inet" | "inet6"
            ) {
                return Err(invalid("invalid SSH option value"));
            }
            value
        }
        "connecttimeout"
        | "connectionattempts"
        | "serveraliveinterval"
        | "serveralivecountmax" => value
            .parse::<u32>()
            .map_err(|_| invalid("invalid SSH numeric option"))?
            .to_string(),
        _ => return Err(invalid(
            "SSH option cannot be saved (command or credential options are forbidden)",
        )),
    };
    if value.is_empty() {
        return Err(invalid("empty SSH option value"));
    }
    Ok(format!("{key}={value}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn legacy_recipes_default_enabled() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("systems.json"), br#"{"systems":[{"id":"legacy","name":"Old","target":"host","ssh_args":[]}]}"#).unwrap();
        let mut registry = SystemRegistry::load_directory(dir.path().into()).unwrap();
        assert!(registry.system("legacy").unwrap().enabled);
        let duplicate = registry.upsert("Renamed", "host", vec![]).unwrap();
        assert_eq!(duplicate.id, "legacy");
        assert!(duplicate.enabled);
    }

    #[test]
    fn activation_roundtrip_and_duplicate_preserve_intent() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = SystemRegistry::load_directory(dir.path().into()).unwrap();
        let saved = registry.upsert("Host", "host", vec![]).unwrap();
        assert!(!saved.enabled);
        assert!(
            !registry
                .upsert("Duplicate", "HOST", vec![])
                .unwrap()
                .enabled
        );
        registry.reload().unwrap();
        assert!(!registry.system(&saved.id).unwrap().enabled);
        assert!(registry.set_enabled(&saved.id, true).unwrap().enabled);
        assert!(registry.upsert("Renamed", "host", vec![]).unwrap().enabled);
        registry.reload().unwrap();
        assert!(registry.system(&saved.id).unwrap().enabled);
        registry.set_enabled(&saved.id, false).unwrap();
        registry.reload().unwrap();
        assert!(!registry.system(&saved.id).unwrap().enabled);
        assert!(registry.set_enabled("unknown", true).is_err());
    }

    #[test]
    fn stale_activation_writers_preserve_other_records_and_intent() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = SystemRegistry::load_directory(dir.path().into()).unwrap();
        let first = a.upsert("First", "first", vec![]).unwrap();
        let mut b = SystemRegistry::load_directory(dir.path().into()).unwrap();
        a.set_enabled(&first.id, true).unwrap();
        b.upsert("Renamed", "first", vec![]).unwrap();
        let second = a.upsert("Second", "second", vec![]).unwrap();
        b.set_enabled(&first.id, false).unwrap();
        a.reload().unwrap();
        assert_eq!(a.system(&first.id).unwrap().name, "Renamed");
        assert!(!a.system(&first.id).unwrap().enabled);
        assert_eq!(a.system(&second.id), Some(&second));
    }

    #[test]
    fn activation_write_failure_preserves_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = SystemRegistry::load_directory(dir.path().into()).unwrap();
        let saved = registry.upsert("Host", "host", vec![]).unwrap();
        let saved = registry.set_enabled(&saved.id, true).unwrap();
        // Hold the valid reloaded snapshot while forcing atomic replacement to
        // fail, independent of platform permissions (including root test runs).
        let lock = registry.lock_and_reload().unwrap();
        let mut stale = SystemRegistry {
            directory: dir.path().into(),
            data: RegistryData {
                systems: vec![saved.clone()],
            },
        };
        assert!(stale.set_enabled(&saved.id, false).is_err());
        assert_eq!(stale.system(&saved.id), Some(&saved));
        fs::remove_file(dir.path().join("systems.json")).unwrap();
        fs::create_dir(dir.path().join("systems.json")).unwrap();
        let mut data = RegistryData {
            systems: vec![saved.clone()],
        };
        data.systems[0].enabled = false;
        assert!(registry.persist(&data).is_err());
        assert_eq!(registry.system(&saved.id), Some(&saved));
        drop(lock);
    }

    #[test]
    fn runtime_routes_match_canonical_saved_recipes() {
        assert!(same_system_route("me@HOST", &[], "me@host", &[]));
        assert!(same_system_route(
            "me@HOST",
            &args(&["-p", "022", "-o", "IdentitiesOnly yes", "-i", "/tmp/key"]),
            "me@host",
            &args(&["-p", "22", "-o", "identitiesonly=yes", "-i", "/tmp/key"]),
        ));
        assert!(same_system_route(
            "me@[2001:0db8::1]",
            &[],
            "me@2001:db8::1",
            &[]
        ));
    }

    #[test]
    fn runtime_routes_preserve_user_port_key_and_option_order() {
        assert!(!same_system_route("Alice@host", &[], "alice@host", &[]));
        assert!(!same_system_route("me@host", &[], "host", &[]));
        assert!(!same_system_route(
            "host",
            &args(&["-p", "22"]),
            "host",
            &args(&["-p", "2222"])
        ));
        assert!(!same_system_route(
            "host",
            &args(&["-i", "/tmp/a"]),
            "host",
            &args(&["-i", "/tmp/b"])
        ));
        assert!(!same_system_route(
            "host",
            &args(&["-p", "22", "-p", "2222"]),
            "host",
            &args(&["-p", "2222", "-p", "22"])
        ));
    }

    #[test]
    fn invalid_runtime_routes_never_match_even_exactly() {
        for target in ["", "-host", "host command", "host;id"] {
            assert!(!same_system_route(target, &[], target, &[]));
        }
        for values in [
            args(&["-p", "0"]),
            args(&["-i"]),
            args(&["-o", "ProxyCommand=echo secret"]),
            args(&["-o", "Password=secret"]),
            args(&["-i", "key\nfile"]),
        ] {
            assert!(!same_system_route("host", &values, "host", &values));
            assert!(!same_system_route("host", &values, "host", &[]));
            assert!(!same_system_route("host", &[], "host", &values));
        }
    }

    #[test]
    fn draft_validation_and_saved_argv_are_stable() {
        let draft = "user@[2001:0db8::1]";
        validate_system_target(draft).unwrap();
        assert_eq!(draft, "user@[2001:0db8::1]");
        assert!(validate_system_target("host command").is_err());
        let dir = tempfile::tempdir().unwrap();
        let mut registry = SystemRegistry::load_directory(dir.path().into()).unwrap();
        let saved = registry
            .upsert(
                "IPv6",
                draft,
                args(&["-p", "022", "-o", "IdentitiesOnly yes"]),
            )
            .unwrap();
        assert_eq!(normalize_target(&saved.target).unwrap(), saved.target);
        assert_eq!(normalize_args(&saved.ssh_args).unwrap(), saved.ssh_args);
        // The engine passes these exact pairs to SSH and hashes them in order.
        assert_eq!(
            safe_connection_args(&saved.ssh_args),
            saved
                .ssh_args
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        registry.reload().unwrap();
        assert_eq!(registry.system(&saved.id), Some(&saved));
    }

    #[test]
    fn persistence_dedup_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = SystemRegistry::load_directory(dir.path().into()).unwrap();
        assert!(registry.systems().is_empty());
        let first = registry
            .upsert(
                " Dev ",
                "me@HOST",
                args(&["-p", "022", "-i", "/tmp/my key", "-o", "IdentitiesOnly=yes"]),
            )
            .unwrap();
        let second = registry
            .upsert(
                "Renamed",
                "me@host",
                args(&["-p", "22", "-i", "/tmp/my key", "-o", "identitiesonly yes"]),
            )
            .unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(registry.systems().len(), 1);
        assert_eq!(registry.system(&first.id).unwrap().name, "Renamed");
        let mut loaded = SystemRegistry::load_directory(dir.path().into()).unwrap();
        assert_eq!(loaded.systems(), registry.systems());
        loaded.remove(&first.id).unwrap();
        loaded.remove("unknown").unwrap();
        registry.reload().unwrap();
        assert!(registry.systems().is_empty());
    }

    #[test]
    fn stale_writers_reload_and_locks_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = SystemRegistry::load_directory(dir.path().into()).unwrap();
        let mut b = SystemRegistry::load_directory(dir.path().into()).unwrap();
        a.upsert("a", "a", vec![]).unwrap();
        b.upsert("b", "b", vec![]).unwrap();
        a.reload().unwrap();
        assert_eq!(a.systems().len(), 2);
        let lock = a.lock_and_reload().unwrap();
        assert!(b.remove("anything").is_err());
        drop(lock);
        b.reload().unwrap();
    }

    #[test]
    fn rejects_commands_credentials_and_invalid_targets() {
        for target in [
            "",
            "-host",
            " host",
            "host\n",
            "host command",
            "host;id",
            "ssh://host",
            "u:p@host",
            "u@@host",
            "host:22",
            "[nope]",
        ] {
            assert!(normalize_target(target).is_err(), "{target:?}");
        }
        for target in ["alias", "user@host.example", "user@[2001:db8::1]", "::1"] {
            assert!(normalize_target(target).is_ok(), "{target:?}");
        }
        for option in [
            "ProxyCommand=echo secret",
            "RemoteCommand=id",
            "LocalCommand=id",
            "SetEnv=TOKEN=secret",
            "Password=secret",
            "IdentityAgent=/tmp/socket",
        ] {
            assert!(normalize_args(&args(&["-o", option])).is_err());
        }
        assert!(normalize_args(&args(&["-p", "0"])).is_err());
        assert!(normalize_args(&args(&["-i"])).is_err());
        assert_eq!(
            normalize_args(&args(&["-L", "1234:host:22", "-p", "22"])).unwrap(),
            args(&["-p", "22"])
        );
    }

    #[test]
    fn corruption_is_not_overwritten_and_snapshot_survives() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = SystemRegistry::load_directory(dir.path().into()).unwrap();
        let saved = registry.upsert("ok", "host", vec![]).unwrap();
        let path = dir.path().join("systems.json");
        for bytes in [
            b"not json".as_slice(),
            b"{}",
            br#"{"systems":[],"password":"secret"}"#,
        ] {
            fs::write(&path, bytes).unwrap();
            assert!(SystemRegistry::load_directory(dir.path().into()).is_err());
            assert_eq!(
                registry.reload().unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            assert!(registry.upsert("no", "other", vec![]).is_err());
            assert!(registry.remove(&saved.id).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert_eq!(registry.system(&saved.id), Some(&saved));
        }
    }
}
