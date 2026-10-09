use crate::*;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

const MAX_RECORD: u64 = 64 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    binding: Binding,
}

/// Registry directory must be private, durable local storage (not an untrusted/NFS directory).
/// A nonblocking OS lock is held across provider calls, so competing processes get Busy.
/// No process-local mutex is required. Dropping/cancelling a future releases its lock,
/// leaving the previously committed intent available to the next reconciler.
pub struct Runtime {
    directory: PathBuf,
    provider: Arc<dyn RuntimeProvider>,
}
struct Locked {
    path: PathBuf,
    _lock: File,
}
impl Locked {
    fn load(&self, key: &WorkspaceKey) -> Result<Option<Binding>> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut bytes = Vec::new();
        file.take(MAX_RECORD + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_RECORD {
            return Err(Error::Corrupt);
        }
        let record: Record =
            serde_json::from_slice(&bytes).map_err(|_| Error::Corrupt)?;
        let b = record.binding;
        if record.version != 1
            || &b.allocation.owner != key
            || b.allocation.generation == 0
            || b.revision == 0
            || b.allocation.spec.validate().is_err()
            || !valid_id(&b.allocation.provider)
            || b.allocation
                .launch
                .as_ref()
                .is_some_and(|l| l.validate().is_err())
            || b.status
                .as_ref()
                .is_some_and(|s| s.validate(&b.allocation, None).is_err())
            || b.retired
                != b.status
                    .as_ref()
                    .is_some_and(|s| s.state == MachineState::Destroyed)
            || b.retired && b.pending.is_some()
            || b.status.is_none() && b.pending != Some(Intent::Ensure)
        {
            return Err(Error::Corrupt);
        }
        Ok(Some(b))
    }
    fn save_if_changed(&self, b: &mut Binding, previous: &Binding) -> Result<()> {
        // Revision identifies durable state changes, not the number of observations.
        // Full equality includes allocation, status, pending intent and uncertainty.
        if b == previous {
            return Ok(());
        }
        self.save(b)
    }
    fn save(&self, b: &mut Binding) -> Result<()> {
        b.revision = b.revision.checked_add(1).ok_or(Error::Corrupt)?;
        let bytes = serde_json::to_vec(&Record {
            version: 1,
            binding: b.clone(),
        })
        .map_err(|_| Error::Corrupt)?;
        if bytes.len() as u64 > MAX_RECORD {
            return Err(Error::Corrupt);
        }
        let temp = self
            .path
            .with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            // Close the temporary handle before replacement (important on Windows).
            // std::fs::rename replaces an existing file on Unix and Windows. Never
            // emulate replacement by deleting the destination: that loses atomicity.
            drop(file);
            fs::rename(&temp, &self.path)?;
            #[cfg(unix)]
            File::open(self.path.parent().ok_or(Error::Invalid)?)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }
}
impl Runtime {
    pub fn new(
        directory: impl AsRef<Path>,
        provider: Arc<dyn RuntimeProvider>,
    ) -> Result<Self> {
        Self::build(directory, provider, false)
    }
    /// Explicit container opt-in. CPU/memory limits and workspace durability remain mandatory.
    pub fn new_development(
        directory: impl AsRef<Path>,
        provider: Arc<dyn RuntimeProvider>,
    ) -> Result<Self> {
        Self::build(directory, provider, true)
    }
    fn build(
        directory: impl AsRef<Path>,
        provider: Arc<dyn RuntimeProvider>,
        development: bool,
    ) -> Result<Self> {
        let caps = provider.capabilities();
        if !valid_id(provider.id())
            || !caps.cpu_limit
            || !caps.memory_limit
            || !caps.durable_workspace
            || (!development
                && (caps.isolation != IsolationKind::VirtualMachine || !caps.disk_limit))
        {
            return Err(Error::Invalid);
        }
        fs::create_dir_all(directory.as_ref())?;
        Ok(Self {
            directory: fs::canonicalize(directory)?,
            provider,
        })
    }
    fn lock(&self, key: &WorkspaceKey) -> Result<Locked> {
        key.validate()?;
        // Length separators prevent concatenation collisions; hashing avoids path traversal.
        let mut hash = Sha256::new();
        hash.update((key.tenant.len() as u64).to_be_bytes());
        hash.update(key.tenant.as_bytes());
        hash.update(key.workspace.as_bytes());
        let name = format!("{:x}", hash.finalize());
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options.open(self.directory.join(format!("{name}.lock")))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(Error::Busy),
            Err(std::fs::TryLockError::Error(e)) => return Err(Error::Io(e)),
        }
        Ok(Locked {
            path: self.directory.join(format!("{name}.json")),
            _lock: lock,
        })
    }
    fn current(&self, lock: &Locked, key: &WorkspaceKey) -> Result<Binding> {
        let b = lock.load(key)?.ok_or(Error::Fenced)?;
        if b.allocation.provider != self.provider.id() {
            return Err(Error::Conflict);
        }
        Ok(b)
    }
    pub fn binding(&self, key: &WorkspaceKey) -> Result<Option<Binding>> {
        let lock = self.lock(key)?;
        let binding = lock.load(key)?;
        if binding
            .as_ref()
            .is_some_and(|b| b.allocation.provider != self.provider.id())
        {
            return Err(Error::Conflict);
        }
        Ok(binding)
    }
    /// Ensure allocation only; it does not implicitly power on a stopped machine.
    pub async fn ensure(
        &self,
        key: &WorkspaceKey,
        spec: &WorkspaceSpec,
    ) -> Result<Binding> {
        self.ensure_inner(key, spec, None).await
    }
    /// Requires an immutable worker launch contract; never repairs an existing machine-only record.
    pub async fn ensure_with_initializer(
        &self,
        key: &WorkspaceKey,
        spec: &WorkspaceSpec,
        initializer: &dyn AllocationInitializer,
    ) -> Result<Binding> {
        self.ensure_inner(key, spec, Some(initializer)).await
    }
    async fn ensure_inner(
        &self,
        key: &WorkspaceKey,
        spec: &WorkspaceSpec,
        initializer: Option<&dyn AllocationInitializer>,
    ) -> Result<Binding> {
        spec.validate()?;
        let lock = self.lock(key)?;
        let old = lock.load(key)?;
        let mut b = if let Some(b) = old.as_ref().filter(|b| !b.retired) {
            if b.allocation.provider != self.provider.id() || &b.allocation.spec != spec {
                return Err(Error::Conflict);
            }
            if let Some(launch) = &b.allocation.launch {
                launch.validate_active()?;
            }
            if initializer.is_some() {
                b.allocation
                    .launch
                    .as_ref()
                    .ok_or(Error::MissingLaunch)?
                    .validate_active()?;
            }
            b.clone()
        } else {
            let generation = old
                .as_ref()
                .map_or(Some(1), |b| b.allocation.generation.checked_add(1))
                .ok_or(Error::Corrupt)?;
            let launch = match initializer {
                Some(i) => {
                    let seed = AllocationSeed {
                        owner: key.clone(),
                        provider: self.provider.id().into(),
                        generation,
                        spec: spec.clone(),
                    };
                    let launch = i.initialize(&seed).await?;
                    launch.validate_active()?;
                    Some(launch)
                }
                None => None,
            };
            let mut b = Binding {
                allocation: Allocation {
                    owner: key.clone(),
                    generation,
                    provider: self.provider.id().into(),
                    spec: spec.clone(),
                    launch,
                },
                revision: old.as_ref().map_or(0, |b| b.revision),
                status: None,
                pending: Some(Intent::Ensure),
                last_error: None,
                retired: false,
            };
            lock.save(&mut b)?;
            b
        };
        self.drive(&lock, &mut b).await?;
        Ok(b)
    }
    /// Replay any durable intent, otherwise inspect the known machine. No implicit replacement.
    pub async fn reconcile(&self, key: &WorkspaceKey) -> Result<Binding> {
        let lock = self.lock(key)?;
        let mut b = self.current(&lock, key)?;
        if !b.retired {
            let expired = b.allocation.launch.as_ref().is_some_and(|launch| {
                matches!(launch.validate_active(), Err(Error::ExpiredLaunch))
            });
            if expired && matches!(b.pending, Some(Intent::Ensure | Intent::Start)) {
                self.inspect_for_recovery(&lock, &mut b).await?;
            } else {
                self.drive(&lock, &mut b).await?;
            }
        }
        Ok(b)
    }
    /// Inspect only, including an expired allocation with no saved handle after a lost
    /// ensure reply. Never creates compute, renews authority, replays an intent, or
    /// infers retirement from absence. Pending intent and uncertainty remain intact.
    /// A validated discovered handle lets the caller explicitly destroy this generation.
    /// Failed/foreign inspection leaves the durable registry byte-for-byte unchanged.
    pub async fn recover_handle(&self, key: &WorkspaceKey) -> Result<Binding> {
        let lock = self.lock(key)?;
        let mut b = self.current(&lock, key)?;
        if !b.retired {
            self.inspect_for_recovery(&lock, &mut b).await?;
        }
        Ok(b)
    }
    async fn inspect_for_recovery(&self, lock: &Locked, b: &mut Binding) -> Result<()> {
        let expected = b.status.as_ref().map(|s| &s.handle);
        let status = self.provider.inspect(&b.allocation, expected).await?;
        status.validate(&b.allocation, expected)?;
        // Inspection is not confirmation of our durable destroy intent. Require an
        // explicit destroy response before retiring, even for an owned tombstone.
        if status.state == MachineState::Destroyed {
            return Err(ProviderError::new(FailureCode::NotFound, false).into());
        }
        let previous = b.clone();
        b.status = Some(status);
        lock.save_if_changed(b, &previous)
    }
    pub async fn start(&self, handle: &MachineHandle) -> Result<Binding> {
        self.command(handle, Intent::Start).await
    }
    pub async fn stop(&self, handle: &MachineHandle) -> Result<Binding> {
        self.command(handle, Intent::Stop).await
    }
    pub async fn destroy(&self, handle: &MachineHandle) -> Result<Binding> {
        self.command(handle, Intent::Destroy).await
    }
    async fn command(&self, handle: &MachineHandle, intent: Intent) -> Result<Binding> {
        let lock = self.lock(&handle.owner)?;
        let mut b = self.current(&lock, &handle.owner)?;
        if b.status.as_ref().map(|s| &s.handle) != Some(handle) {
            return Err(Error::Fenced);
        }
        if b.retired {
            return if intent == Intent::Destroy {
                Ok(b)
            } else {
                Err(Error::Fenced)
            };
        }
        // Reject expired start before changing the durable intent or last error.
        if intent == Intent::Start {
            if let Some(launch) = &b.allocation.launch {
                launch.validate_active()?;
            }
        }
        if matches!(intent, Intent::Start | Intent::Stop)
            && !self.provider.capabilities().stop_start
        {
            return Err(Error::Invalid);
        }
        if intent != Intent::Destroy && b.pending.is_some_and(|p| p != intent) {
            return Err(Error::Conflict);
        }
        b.pending = Some(intent);
        b.last_error = None;
        lock.save(&mut b)?;
        self.drive(&lock, &mut b).await?;
        Ok(b)
    }
    async fn drive(&self, lock: &Locked, b: &mut Binding) -> Result<()> {
        let previous = b.clone();
        let a = &b.allocation;
        if matches!(b.pending, Some(Intent::Ensure | Intent::Start)) {
            if let Some(launch) = &a.launch {
                launch.validate_active()?;
            }
        }
        let handle = b.status.as_ref().map(|s| &s.handle);
        let outcome = match (b.pending, handle) {
            (Some(Intent::Ensure), _) => self.provider.ensure(a).await,
            (Some(Intent::Start), Some(h)) => self.provider.start(a, h).await,
            (Some(Intent::Stop), Some(h)) => self.provider.stop(a, h).await,
            (Some(Intent::Destroy), Some(h)) => self.provider.destroy(a, h).await,
            (None, h) => self.provider.inspect(a, h).await,
            _ => return Err(Error::Corrupt),
        }
        .and_then(|s| {
            s.validate(a, handle)?;
            // Destruction must never be inferred from a missing/failed inspection.
            if s.state == MachineState::Destroyed && b.pending != Some(Intent::Destroy) {
                return Err(ProviderError::new(FailureCode::NotFound, false));
            }
            Ok(s)
        });
        match outcome {
            Ok(status) => {
                let complete = match b.pending {
                    Some(Intent::Ensure) => status.state != MachineState::Provisioning,
                    Some(Intent::Start) => matches!(
                        status.state,
                        MachineState::Running | MachineState::Failed
                    ),
                    Some(Intent::Stop) => matches!(
                        status.state,
                        MachineState::Stopped | MachineState::Failed
                    ),
                    Some(Intent::Destroy) => status.state == MachineState::Destroyed,
                    None => true,
                };
                b.retired = status.state == MachineState::Destroyed;
                b.status = Some(status);
                if complete {
                    b.pending = None;
                }
                b.last_error = None;
                lock.save_if_changed(b, &previous)
            }
            Err(error) => {
                b.last_error = Some(error.clone());
                lock.save_if_changed(b, &previous)?;
                Err(Error::Provider(error))
            }
        }
    }
}
