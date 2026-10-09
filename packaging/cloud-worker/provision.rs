//! Controller-side provisioning example; never include this program or its seed in the worker image.
use neoism_agent_service_api::{WorkspaceWorkerBootstrap, WorkspaceWorkerSigningKey};
use std::{fs::OpenOptions, io::Write, path::PathBuf, time::{SystemTime, UNIX_EPOCH}};

fn write_new(path: PathBuf, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o400);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 6 {
        return Err("usage: worker-provision PRIVATE_SEED_FILE OUTPUT_DIR TENANT_ID WORKSPACE_ID RUNTIME_ID GENERATION".into());
    }
    let seed = std::fs::read(&args[0])?;
    let signing = WorkspaceWorkerSigningKey::new(&seed)?;
    let generation: u64 = args[5].parse()?;
    if generation == 0 || args[2..5].iter().any(|id| id.trim().is_empty()) {
        return Err("nonblank identities and a positive generation are required".into());
    }
    let now = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
    let bootstrap = WorkspaceWorkerBootstrap {
        version: 1,
        tenant_id: args[2].clone(),
        workspace_id: args[3].clone(),
        runtime_id: args[4].clone(),
        runtime_generation: generation,
        root: PathBuf::from("/workspace"),
        expires_at: now.checked_add(3600).ok_or("expiry overflow")?,
    };
    let output = PathBuf::from(&args[1]);
    // Only public outputs are written here. Existing files are never overwritten.
    write_new(output.join("verification.key"), signing.verification_key().as_bytes())?;
    write_new(output.join("bootstrap.json"), &serde_json::to_vec_pretty(&bootstrap)?)?;
    Ok(())
}
