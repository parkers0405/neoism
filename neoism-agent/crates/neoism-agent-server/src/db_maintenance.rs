//! Offline agent-database maintenance: prune the event journal to the
//! retention window, then rebuild the file with `VACUUM INTO` so freed pages
//! are returned to the filesystem. Deleting rows alone only makes pages
//! reusable; it never shrinks the file.
//!
//! Must run while no other process has the database open.
use anyhow::{bail, Context};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::state::SessionStore;

#[derive(Debug)]
pub struct CompactReport {
    pub pruned_events: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
    /// Pre-compaction copy, kept unless the caller asked to discard it.
    pub backup: Option<PathBuf>,
}

/// The agent database used by `neoism-agent serve` and the desktop.
pub fn default_database_path() -> PathBuf {
    PathBuf::from(crate::default_state_dir()).join("agent.turso.db")
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn size_with_wal(path: &Path) -> u64 {
    [path.to_path_buf(), sidecar(path, "-wal")]
        .iter()
        .filter_map(|file| std::fs::metadata(file).ok())
        .map(|meta| meta.len())
        .sum()
}

/// Refuse to touch a database another process has open (Linux: /proc fds).
fn ensure_not_in_use(path: &Path) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let targets = [path.to_path_buf(), sidecar(path, "-wal")]
            .map(|file| std::fs::canonicalize(&file).unwrap_or(file));
        let own = std::process::id().to_string();
        for proc_entry in std::fs::read_dir("/proc")?.flatten() {
            let pid = proc_entry.file_name();
            let Some(pid) = pid.to_str().filter(|pid| pid.bytes().all(|b| b.is_ascii_digit())) else {
                continue;
            };
            if pid == own {
                continue;
            }
            let Ok(fds) = std::fs::read_dir(proc_entry.path().join("fd")) else {
                continue;
            };
            for fd in fds.flatten() {
                if std::fs::read_link(fd.path()).is_ok_and(|link| targets.contains(&link)) {
                    bail!(
                        "{} is open by process {pid}; quit Neoism (and any neoism-agent server) first",
                        path.display()
                    );
                }
            }
        }
    }
    Ok(())
}

async fn open_raw(path: &Path) -> anyhow::Result<turso::Connection> {
    let path = path.to_str().context("database path is not valid UTF-8")?;
    let database = turso::Builder::new_local(path)
        .experimental_index_method(true)
        .build()
        .await
        .with_context(|| format!("failed to open {path}"))?;
    Ok(database.connect()?)
}

async fn scalar(connection: &turso::Connection, sql: &str) -> anyhow::Result<i64> {
    let mut rows = connection.query(sql, ()).await?;
    let row = rows.next().await?.context("query returned no rows")?;
    match row.get_value(0)? {
        turso::Value::Integer(value) => Ok(value),
        other => bail!("unexpected scalar {other:?} for {sql}"),
    }
}

/// Canonical tables a compacted copy must reproduce exactly.
const VERIFIED_TABLES: &[&str] = &["sessions", "messages", "events", "artifacts"];

async fn table_counts(connection: &turso::Connection) -> anyhow::Result<Vec<i64>> {
    let mut counts = Vec::with_capacity(VERIFIED_TABLES.len());
    for table in VERIFIED_TABLES {
        counts.push(scalar(connection, &format!("SELECT count(*) FROM {table}")).await?);
    }
    Ok(counts)
}

pub async fn compact_database(
    path: &Path,
    retention: Option<Duration>,
    keep_backup: bool,
) -> anyhow::Result<CompactReport> {
    if !path.exists() {
        bail!("no agent database at {}", path.display());
    }
    ensure_not_in_use(path)?;
    let bytes_before = size_with_wal(path);

    // Open through the normal store so schema migrations (including the
    // journal-state table) are current before pruning.
    let pruned_events = {
        let store = SessionStore::open(path.to_path_buf()).await?;
        store.stop_background_workers().await;
        match retention {
            Some(window) => store.prune_event_journal(window).await?,
            None => 0,
        }
    };

    let compacted = sidecar(path, ".compact-tmp");
    for stale in [compacted.clone(), sidecar(&compacted, "-wal"), sidecar(&compacted, "-shm")] {
        let _ = std::fs::remove_file(stale);
    }
    let source = open_raw(path).await?;
    let expected = table_counts(&source).await?;
    let target = compacted.to_str().context("database path is not valid UTF-8")?;
    source
        .execute(&format!("VACUUM INTO '{}'", target.replace('\'', "''")), ())
        .await
        .context("VACUUM INTO failed; the pruned database is intact and its free pages will be reused")?;
    drop(source);

    let copy = open_raw(&compacted).await?;
    let actual = table_counts(&copy).await?;
    let integrity = {
        let mut rows = copy.query("PRAGMA quick_check", ()).await?;
        match rows.next().await?.map(|row| row.get_value(0)) {
            Some(Ok(turso::Value::Text(text))) => text,
            other => format!("{other:?}"),
        }
    };
    drop(copy);
    if actual != expected || integrity != "ok" {
        let _ = std::fs::remove_file(&compacted);
        bail!(
            "compacted copy failed verification (counts {actual:?} vs {expected:?}, quick_check {integrity}); original left untouched"
        );
    }

    let backup = sidecar(path, ".pre-compact");
    std::fs::rename(path, &backup).context("failed to move original aside")?;
    for suffix in ["-wal", "-shm"] {
        let old = sidecar(path, suffix);
        if old.exists() {
            std::fs::rename(&old, sidecar(&backup, suffix))?;
        }
    }
    std::fs::rename(&compacted, path).context("failed to install compacted database")?;
    let compacted_wal = sidecar(&compacted, "-wal");
    if compacted_wal.exists() {
        std::fs::rename(&compacted_wal, sidecar(path, "-wal"))?;
    }
    let _ = std::fs::remove_file(sidecar(&compacted, "-shm"));

    let backup = if keep_backup {
        Some(backup)
    } else {
        for file in [backup.clone(), sidecar(&backup, "-wal"), sidecar(&backup, "-shm")] {
            let _ = std::fs::remove_file(file);
        }
        None
    };
    Ok(CompactReport {
        pruned_events,
        bytes_before,
        bytes_after: size_with_wal(path),
        backup,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::TenantQueryScope;

    #[tokio::test]
    async fn compaction_prunes_shrinks_verifies_and_keeps_a_backup() {
        let dir = std::env::temp_dir().join(format!(
            "neoism-db-compact-{}",
            neoism_agent_core::new_session_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent.turso.db");
        {
            let store = SessionStore::open(path.clone()).await.unwrap();
            store.stop_background_workers().await;
            // 3,000 expired ~32 KiB rows, then 5 recent rows that must survive.
            store.insert_test_events(1..=3_000, 10, 32 * 1024).await.unwrap();
            let recent = crate::state::store_i64_for_test(crate::now_millis());
            store.insert_test_events(3_001..=3_005, recent, 256).await.unwrap();
        }
        let report = compact_database(&path, Some(Duration::from_secs(60)), true)
            .await
            .unwrap();
        assert_eq!(report.pruned_events, 3_000);
        assert!(
            report.bytes_after * 4 < report.bytes_before,
            "{} -> {}",
            report.bytes_before,
            report.bytes_after
        );
        assert!(report.backup.as_ref().unwrap().exists());
        // The compacted file reopens through the normal store, data intact.
        let store = SessionStore::open(path.clone()).await.unwrap();
        store.stop_background_workers().await;
        assert_eq!(store.event_replay_floor().await.unwrap(), 3_000);
        assert_eq!(store.latest_event_sequence().await.unwrap(), 3_005);
        let kept = store
            .list_events_after(TenantQueryScope::LocalAll, 0, 128, None)
            .await
            .unwrap();
        assert_eq!(kept.iter().map(|event| event.seq).collect::<Vec<_>>(), vec![3_001, 3_002, 3_003, 3_004, 3_005]);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
