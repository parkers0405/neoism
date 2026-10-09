//! Local development Docker backend. This is container isolation, NOT a VM or a
//! disk-quota boundary. Only trusted operator image aliases and CLI paths are accepted.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use neoism_cloud_runtime::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

type PResult<T> = std::result::Result<T, ProviderError>;
fn err(code: FailureCode) -> ProviderError {
    ProviderError::new(
        code,
        matches!(
            code,
            FailureCode::Transport | FailureCode::Timeout | FailureCode::Unavailable
        ),
    )
}
fn io<T>(r: std::io::Result<T>) -> PResult<T> {
    r.map_err(|_| err(FailureCode::Unavailable))
}
fn hash<T: Serialize>(v: &T) -> PResult<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(v).map_err(|_| err(FailureCode::Protocol))?)
    ))
}
#[derive(Clone, Debug)]
pub struct DockerConfig {
    pub id: String,
    pub controller_directory: PathBuf,
    pub image_catalog: BTreeMap<String, String>,
    /// Required: disk_gib is only a capacity hint, never an enforced quota.
    pub allow_unenforced_disk: bool,
}
pub struct DockerDevelopmentProvider {
    config: DockerConfig,
    cli: PathBuf,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    allocation: Allocation,
    /// Immutable resolved image reference, retained for inspection and cleanup
    /// after the operator changes or removes this catalog alias.
    image: String,
    digest: String,
    name: String,
    destroying: bool,
    destroyed: bool,
    created: bool,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ledger {
    version: u32,
    records: BTreeMap<u64, Record>,
}
struct Locked {
    _file: File,
    path: PathBuf,
    ledger: Ledger,
}
impl Drop for Locked {
    fn drop(&mut self) {
        // Explicit unlock also releases the lock if a concurrently spawning
        // process briefly inherited this open-file description before exec.
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe {
                libc::flock(self._file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}
impl Locked {
    fn save(&self) -> PResult<()> {
        let tmp = self
            .path
            .with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let mut f = io(OpenOptions::new().create_new(true).write(true).open(&tmp))?;
        private_file(&f)?;
        io(f.write_all(
            &serde_json::to_vec(&self.ledger).map_err(|_| err(FailureCode::Protocol))?,
        ))?;
        io(f.sync_all())?;
        io(fs::rename(&tmp, &self.path))?;
        io(File::open(self.path.parent().unwrap()).and_then(|f| f.sync_all()))
    }
}
#[cfg(unix)]
fn private_file(f: &File) -> PResult<()> {
    use std::os::unix::fs::PermissionsExt;
    io(f.set_permissions(fs::Permissions::from_mode(0o600)))
}
#[cfg(not(unix))]
fn private_file(_: &File) -> PResult<()> {
    Err(err(FailureCode::Rejected))
}
impl DockerDevelopmentProvider {
    pub fn new(config: DockerConfig) -> PResult<Self> {
        Self::with_cli(config, PathBuf::from("/usr/bin/docker"))
    }
    /// `cli` is an absolute trusted operator executable, never derived from a model.
    /// This local reference backend currently requires Linux.
    pub fn with_cli(config: DockerConfig, cli: PathBuf) -> PResult<Self> {
        if !cfg!(target_os = "linux")
            || !config.allow_unenforced_disk
            || !cli.is_absolute()
            || !config.controller_directory.is_absolute()
            || config.id.is_empty()
            || config.id.len() > 128
            || !config
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || config.image_catalog.values().any(|s| {
                s.is_empty()
                    || s.starts_with('-')
                    || s.chars().any(|c| c.is_whitespace() || c.is_control())
            })
        {
            return Err(err(FailureCode::Rejected));
        }
        let p = &config.controller_directory;
        if !p.exists() {
            io(fs::create_dir_all(p))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                io(fs::set_permissions(p, fs::Permissions::from_mode(0o700)))?;
            }
        }
        let meta = io(fs::symlink_metadata(p))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if !meta.is_dir()
                || meta.mode() & 0o077 != 0
                || meta.uid() != unsafe { libc::geteuid() }
            {
                return Err(err(FailureCode::Rejected));
            }
        }
        let canonical = io(fs::canonicalize(p))?;
        if canonical != *p || p.to_string_lossy().contains(',') {
            return Err(err(FailureCode::Rejected));
        }
        Ok(Self { config, cli })
    }
    fn owner(&self, a: &Allocation) -> PResult<String> {
        hash(&(&self.config.id, &a.owner))
    }
    fn name(&self, a: &Allocation) -> PResult<String> {
        Ok(format!("neoism-{}-g{}", self.owner(a)?, a.generation))
    }
    fn image(&self, a: &Allocation) -> PResult<&str> {
        self.config
            .image_catalog
            .get(&a.spec.image)
            .map(String::as_str)
            .ok_or_else(|| err(FailureCode::Rejected))
    }
    fn digest(&self, a: &Allocation) -> PResult<String> {
        // Unrelated catalog aliases are mutable operator configuration, not
        // part of this generation's immutable launch identity.
        hash(&(a, self.image(a)?))
    }
    fn validate(&self, a: &Allocation, active: bool) -> PResult<()> {
        if a.owner.validate().is_err()
            || a.spec.validate().is_err()
            || a.provider != self.id()
            || a.generation == 0
        {
            return Err(err(FailureCode::Rejected));
        }
        let l = a
            .launch
            .as_ref()
            .ok_or_else(|| err(FailureCode::Rejected))?;
        if l.validate().is_err()
            || (active && l.validate_active().is_err())
            || !safe_target(l.root())
            || !safe_target(l.state_root())
        {
            return Err(err(FailureCode::Rejected));
        }
        Ok(())
    }
    fn lock(&self, a: &Allocation) -> PResult<Locked> {
        self.validate(a, false)?;
        let stem = self.config.controller_directory.join(self.owner(a)?);
        let file = io(OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(stem.with_extension("lock")))?;
        private_file(&file)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }
                != 0
            {
                return Err(err(FailureCode::Unavailable));
            }
        }
        let path = stem.with_extension("json");
        let ledger = if path.exists() {
            serde_json::from_slice::<Ledger>(&io(fs::read(&path))?)
                .map_err(|_| err(FailureCode::Protocol))?
        } else {
            Ledger {
                // Version 2 records bind the selected image, not the full catalog.
                version: 2,
                ..Ledger::default()
            }
        };
        if ledger.version != 2 {
            return Err(err(FailureCode::Protocol));
        }
        Ok(Locked {
            _file: file,
            path,
            ledger,
        })
    }
    fn guard(
        &self,
        a: &Allocation,
        h: Option<&MachineHandle>,
        lock: &Locked,
    ) -> PResult<()> {
        let r = lock
            .ledger
            .records
            .get(&a.generation)
            .ok_or_else(|| err(FailureCode::NotFound))?;
        if r.allocation != *a
            || r.digest != hash(&(a, r.image.as_str()))?
            || r.name != self.name(a)?
            || h.is_some_and(|h| *h != self.handle(a, &r.name))
        {
            return Err(err(FailureCode::Identity));
        }
        Ok(())
    }
    fn handle(&self, a: &Allocation, name: &str) -> MachineHandle {
        MachineHandle {
            owner: a.owner.clone(),
            generation: a.generation,
            provider: self.id().into(),
            machine_id: name.into(),
        }
    }
    fn dead(&self, a: &Allocation) -> PResult<MachineStatus> {
        Ok(MachineStatus {
            handle: self.handle(a, &self.name(a)?),
            state: MachineState::Destroyed,
            ready: false,
            connection: None,
            failure: None,
        })
    }
    async fn cli(&self, args: &[String]) -> PResult<Vec<u8>> {
        use std::process::Stdio;
        let mut child = Command::new(&self.cli)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| err(FailureCode::Transport))?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let result = tokio::time::timeout(Duration::from_secs(30), async {
            let (out, noise, status) =
                tokio::join!(bounded(stdout), bounded(stderr), child.wait());
            let out = out?;
            noise?;
            if !io(status)?.success() {
                return Err(err(FailureCode::Unavailable));
            }
            Ok(out)
        })
        .await;
        match result {
            Ok(v) => v,
            Err(_) => {
                let _ = child.kill().await;
                Err(err(FailureCode::Timeout))
            }
        }
    }
    async fn exists(&self, kind: &str, name: &str) -> PResult<bool> {
        let args = if kind == "container" {
            vec![
                kind.into(),
                "ls".into(),
                "--all".into(),
                "--filter".into(),
                format!("name=^/{name}$"),
                "--format".into(),
                "{{.Names}}".into(),
            ]
        } else {
            vec![
                kind.into(),
                "ls".into(),
                "--filter".into(),
                format!("name=^{name}$"),
                "--format".into(),
                "{{.Name}}".into(),
            ]
        };
        let bytes = self.cli(&args).await?;
        let text = std::str::from_utf8(&bytes).map_err(|_| err(FailureCode::Protocol))?;
        Ok(text.lines().any(|line| line == name))
    }
    fn labels(&self, a: &Allocation) -> PResult<BTreeMap<String, String>> {
        Ok(BTreeMap::from([
            ("dev.neoism.owner".into(), self.owner(a)?),
            ("dev.neoism.generation".into(), a.generation.to_string()),
            ("dev.neoism.digest".into(), self.digest(a)?),
            ("dev.neoism.provider".into(), self.id().into()),
        ]))
    }
    async fn dto(&self, a: &Allocation) -> PResult<Option<serde_json::Value>> {
        let name = self.name(a)?;
        if !self.exists("container", &name).await? {
            return Ok(None);
        }
        let bytes = self
            .cli(&["container".into(), "inspect".into(), name.clone()])
            .await?;
        let values: Vec<serde_json::Value> =
            serde_json::from_slice(&bytes).map_err(|_| err(FailureCode::Protocol))?;
        let v = values
            .into_iter()
            .next()
            .ok_or_else(|| err(FailureCode::Protocol))?;
        let labels: BTreeMap<String, String> =
            serde_json::from_value(v["Config"]["Labels"].clone())
                .map_err(|_| err(FailureCode::Identity))?;
        if self
            .labels(a)?
            .iter()
            .any(|(k, val)| labels.get(k) != Some(val))
            || v["Name"].as_str() != Some(&format!("/{name}"))
        {
            return Err(err(FailureCode::Identity));
        }
        Ok(Some(v))
    }
    async fn status(&self, a: &Allocation) -> PResult<MachineStatus> {
        let v = self
            .dto(a)
            .await?
            .ok_or_else(|| err(FailureCode::NotFound))?;
        let running = v["State"]["Running"]
            .as_bool()
            .ok_or_else(|| err(FailureCode::Protocol))?;
        let state = if running {
            MachineState::Running
        } else if v["State"]["Status"] == "created" {
            MachineState::Provisioning
        } else {
            MachineState::Stopped
        };
        let handle = self.handle(a, &self.name(a)?);
        let connection = if running {
            let ports = v["NetworkSettings"]["Ports"]["4096/tcp"]
                .as_array()
                .ok_or_else(|| err(FailureCode::Protocol))?;
            if ports.len() != 1 || ports[0]["HostIp"] != "127.0.0.1" {
                return Err(err(FailureCode::Identity));
            }
            let port: u16 = ports[0]["HostPort"]
                .as_str()
                .ok_or_else(|| err(FailureCode::Protocol))?
                .parse()
                .map_err(|_| err(FailureCode::Protocol))?;
            if port == 0 {
                return Err(err(FailureCode::Protocol));
            }
            Some(
                WorkerConnection::new_development_loopback(
                    handle.clone(),
                    &format!("http://127.0.0.1:{port}/"),
                )
                .map_err(|_| err(FailureCode::Protocol))?,
            )
        } else {
            None
        };
        let status = MachineStatus {
            handle,
            state,
            ready: running && v["State"]["Health"]["Status"] == "healthy",
            connection,
            failure: None,
        };
        status.validate(a, None)?;
        Ok(status)
    }
    async fn volume(&self, a: &Allocation, role: &str) -> PResult<String> {
        let owner = self.owner(a)?;
        let name = format!("neoism-{owner}-{role}");
        let labels = BTreeMap::from([
            ("dev.neoism.owner", owner.as_str()),
            ("dev.neoism.role", role),
            ("dev.neoism.provider", self.id()),
        ]);
        if !self.exists("volume", &name).await? {
            let mut args = vec!["volume".into(), "create".into()];
            for (k, v) in &labels {
                args.extend(["--label".into(), format!("{k}={v}")]);
            }
            args.push(name.clone());
            self.cli(&args).await?;
        }
        let bytes = self
            .cli(&["volume".into(), "inspect".into(), name.clone()])
            .await?;
        let v: Vec<serde_json::Value> =
            serde_json::from_slice(&bytes).map_err(|_| err(FailureCode::Protocol))?;
        let v = v.first().ok_or_else(|| err(FailureCode::Protocol))?;
        if v["Name"] != name
            || v["Driver"] != "local"
            || v["Options"].as_object().is_some_and(|o| !o.is_empty())
            || labels
                .iter()
                .any(|(k, val)| v["Labels"][*k].as_str() != Some(val))
        {
            return Err(err(FailureCode::Identity));
        }
        Ok(name)
    }
    fn bootstrap(&self, a: &Allocation) -> PResult<PathBuf> {
        let l = a.launch.as_ref().unwrap();
        let dir = self.config.controller_directory.join(self.name(a)?);
        if !dir.exists() {
            io(fs::create_dir(&dir))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                io(fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)))?;
            }
        }
        let bootstrap = serde_json::to_vec(&serde_json::json!({"version":1,"tenantId":a.owner.tenant,"workspaceId":a.owner.workspace,"runtimeId":l.runtime_id(),"runtimeGeneration":a.generation,"root":l.root(),"expiresAt":l.expires_at()})).map_err(|_| err(FailureCode::Protocol))?;
        let key = URL_SAFE_NO_PAD
            .decode(l.verification_key())
            .map_err(|_| err(FailureCode::Rejected))?;
        public_file(&dir.join("bootstrap.json"), &bootstrap)?;
        public_file(&dir.join("verification.key"), &key)?;
        // Worker uid must traverse this directory inside the read-only mount. The
        // enclosing controller directory remains 0700 on the host.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            io(fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)))?;
        }
        Ok(dir)
    }
    async fn provision(&self, a: &Allocation) -> PResult<()> {
        if self.dto(a).await?.is_some() {
            return Ok(());
        }
        let workspace = self.volume(a, "workspace").await?;
        let state = self.volume(a, "state").await?;
        let dir = self.bootstrap(a)?;
        let l = a.launch.as_ref().unwrap();
        let mut args: Vec<String> = ["container", "create", "--name"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        args.push(self.name(a)?);
        for (k, v) in self.labels(a)? {
            args.extend(["--label".into(), format!("{k}={v}")]);
        }
        args.extend([
            "--cpus".into(),
            a.spec.vcpus.to_string(),
            "--memory".into(),
            format!("{}m", a.spec.memory_mib),
            "--memory-swap".into(),
            format!("{}m", a.spec.memory_mib),
        ]);
        args.extend(
            [
                "--init",
                "--pids-limit",
                "512",
                "--read-only",
                "--user",
                "10001:10001",
                "--cap-drop",
                "ALL",
                "--security-opt",
                "no-new-privileges:true",
                "--network",
                "bridge",
                "--publish",
                "127.0.0.1::4096",
                "--tmpfs",
                "/tmp:rw,nosuid,nodev,size=64m,mode=1777",
                "--workdir",
                l.root(),
            ]
            .into_iter()
            .map(str::to_owned),
        );
        for mount in [
            format!("type=volume,src={workspace},dst={}", l.root()),
            format!("type=volume,src={state},dst={}", l.state_root()),
            format!(
                "type=bind,src={},dst=/run/neoism-worker,readonly",
                dir.display()
            ),
        ] {
            args.extend(["--mount".into(), mount]);
        }
        for (k, v) in [
            ("HOME", format!("{}/home", l.state_root())),
            ("XDG_CONFIG_HOME", format!("{}/config", l.state_root())),
            ("XDG_STATE_HOME", format!("{}/state", l.state_root())),
            ("XDG_CACHE_HOME", format!("{}/cache", l.state_root())),
            ("XDG_DATA_HOME", format!("{}/data", l.state_root())),
            (
                "NEOISM_AGENT_STATE_DIR",
                format!("{}/state/neoism-agent", l.state_root()),
            ),
            (
                "NEOISM_AGENT_AUTH_PATH",
                format!("{}/state/neoism-agent/auth.json", l.state_root()),
            ),
            (
                "NEOISM_AGENT_WORKER_BOOTSTRAP",
                "/run/neoism-worker/bootstrap.json".into(),
            ),
            (
                "NEOISM_AGENT_WORKER_VERIFICATION_KEY_FILE",
                "/run/neoism-worker/verification.key".into(),
            ),
        ] {
            args.extend(["--env".into(), format!("{k}={v}")]);
        }
        args.push(self.config.image_catalog[&a.spec.image].clone());
        // A failed/cancelled create may have succeeded. The durable name and labels
        // make the next call recover it; never fall back to an alternate name.
        self.cli(&args).await?;
        self.dto(a)
            .await?
            .ok_or_else(|| err(FailureCode::NotFound))?;
        Ok(())
    }
    async fn stop_container(&self, a: &Allocation) -> PResult<()> {
        let result = self
            .cli(&[
                "container".into(),
                "stop".into(),
                "--time".into(),
                "30".into(),
                self.name(a)?,
            ])
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(e) if e.code == FailureCode::Timeout => {
                // A 30-second Docker grace period can complete just after the
                // 30-second CLI deadline. Never infer power state from that
                // lost acknowledgement; inspect the owned container instead.
                for _ in 0..10 {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    if let Some(v) = self.dto(a).await? {
                        if v["State"]["Running"] == false {
                            return Ok(());
                        }
                    } else {
                        return Err(err(FailureCode::NotFound));
                    }
                }
                Err(e)
            }
            Err(e) => Err(e),
        }
    }
    async fn limited(
        &self,
        a: &Allocation,
        h: Option<&MachineHandle>,
        op: &str,
    ) -> PResult<MachineStatus> {
        tokio::time::timeout(Duration::from_secs(180), self.operation(a, h, op))
            .await
            .map_err(|_| err(FailureCode::Timeout))?
    }
    async fn operation(
        &self,
        a: &Allocation,
        h: Option<&MachineHandle>,
        op: &str,
    ) -> PResult<MachineStatus> {
        let mut lock = self.lock(a)?;
        if op == "ensure" && !lock.ledger.records.contains_key(&a.generation) {
            self.validate(a, true)?;
            match lock.ledger.records.last_key_value() {
                None if a.generation == 1 => {}
                Some((g, r)) if a.generation > *g && r.destroyed => {}
                _ => return Err(err(FailureCode::Conflict)),
            }
            lock.ledger.records.insert(
                a.generation,
                Record {
                    allocation: a.clone(),
                    image: self.image(a)?.to_owned(),
                    digest: self.digest(a)?,
                    name: self.name(a)?,
                    destroying: false,
                    destroyed: false,
                    created: false,
                },
            );
            lock.save()?;
        }
        self.guard(a, h, &lock)?;
        let r = &lock.ledger.records[&a.generation];
        if r.destroyed {
            return if op == "destroy" || op == "inspect" {
                self.dead(a)
            } else {
                Err(err(FailureCode::Conflict))
            };
        }
        if r.destroying && op != "destroy" {
            return Err(err(FailureCode::Conflict));
        }
        // Use the ledger's original selected reference for Docker identity checks
        // and cleanup, even after an alias is changed or removed. Launching an
        // existing generation still requires the current alias to match exactly.
        if matches!(op, "ensure" | "start") && self.image(a)? != r.image {
            return Err(err(FailureCode::Conflict));
        }
        let mut bound_config = self.config.clone();
        bound_config.image_catalog =
            BTreeMap::from([(a.spec.image.clone(), r.image.clone())]);
        let bound = Self {
            config: bound_config,
            cli: self.cli.clone(),
        };
        match op {
            "ensure" | "start" => {
                self.validate(a, true)?;
                if op == "ensure" {
                    if lock.ledger.records[&a.generation].created
                        && bound.dto(a).await?.is_none()
                    {
                        return Err(err(FailureCode::NotFound));
                    }
                    bound.provision(a).await?;
                    lock.ledger.records.get_mut(&a.generation).unwrap().created = true;
                    lock.save()?;
                }
                let status = bound.status(a).await?;
                if status.state != MachineState::Running {
                    bound
                        .cli(&["container".into(), "start".into(), bound.name(a)?])
                        .await?;
                }
                bound.status(a).await
            }
            "inspect" => bound.status(a).await,
            "stop" => {
                let status = bound.status(a).await?;
                if status.state == MachineState::Running {
                    bound.stop_container(a).await?;
                }
                bound.status(a).await
            }
            "destroy" => {
                lock.ledger
                    .records
                    .get_mut(&a.generation)
                    .unwrap()
                    .destroying = true;
                lock.save()?;
                if let Some(v) = bound.dto(a).await? {
                    if v["State"]["Running"] == true {
                        bound.stop_container(a).await?;
                    }
                    bound
                        .cli(&["container".into(), "rm".into(), bound.name(a)?])
                        .await?;
                }
                if bound.dto(a).await?.is_some() {
                    return Err(err(FailureCode::Conflict));
                }
                lock.ledger
                    .records
                    .get_mut(&a.generation)
                    .unwrap()
                    .destroyed = true;
                lock.save()?;
                self.dead(a)
            }
            _ => Err(err(FailureCode::Rejected)),
        }
    }
}
fn safe_target(p: &str) -> bool {
    p.starts_with('/')
        && p.len() < 1024
        && p.split('/').skip(1).all(|s| {
            !s.is_empty()
                && s != "."
                && s != ".."
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        })
        && ![
            "/run", "/tmp", "/proc", "/sys", "/dev", "/etc", "/bin", "/sbin", "/usr",
            "/lib", "/lib64",
        ]
        .iter()
        .any(|r| p == *r || p.starts_with(&format!("{r}/")))
        && p != "/var"
        && p != "/var/lib"
}
fn public_file(path: &Path, bytes: &[u8]) -> PResult<()> {
    if path.exists() {
        if io(fs::read(path))? != bytes {
            return Err(err(FailureCode::Identity));
        }
        return Ok(());
    }
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut f = io(OpenOptions::new().create_new(true).write(true).open(&tmp))?;
    io(f.write_all(bytes))?;
    io(f.sync_all())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        io(f.set_permissions(fs::Permissions::from_mode(0o444)))?;
    }
    io(fs::rename(tmp, path))?;
    io(File::open(path.parent().unwrap()).and_then(|f| f.sync_all()))
}
async fn bounded<R: tokio::io::AsyncRead + Unpin>(mut reader: R) -> PResult<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    let mut overflow = false;
    loop {
        let n = io(reader.read(&mut buf).await)?;
        if n == 0 {
            break;
        }
        if out.len() + n <= 1024 * 1024 {
            out.extend_from_slice(&buf[..n]);
        } else {
            overflow = true;
        }
    }
    if overflow {
        Err(err(FailureCode::Protocol))
    } else {
        Ok(out)
    }
}
impl RuntimeProvider for DockerDevelopmentProvider {
    fn id(&self) -> &str {
        &self.config.id
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            isolation: IsolationKind::Container,
            cpu_limit: true,
            memory_limit: true,
            disk_limit: false,
            durable_workspace: true,
            stop_start: true,
        }
    }
    fn ensure<'a>(&'a self, a: &'a Allocation) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(self.limited(a, None, "ensure"))
    }
    fn inspect<'a>(
        &'a self,
        a: &'a Allocation,
        h: Option<&'a MachineHandle>,
    ) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(self.limited(a, h, "inspect"))
    }
    fn start<'a>(
        &'a self,
        a: &'a Allocation,
        h: &'a MachineHandle,
    ) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(self.limited(a, Some(h), "start"))
    }
    fn stop<'a>(
        &'a self,
        a: &'a Allocation,
        h: &'a MachineHandle,
    ) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(self.limited(a, Some(h), "stop"))
    }
    fn destroy<'a>(
        &'a self,
        a: &'a Allocation,
        h: &'a MachineHandle,
    ) -> ProviderFuture<'a, MachineStatus> {
        Box::pin(self.limited(a, Some(h), "destroy"))
    }
}
