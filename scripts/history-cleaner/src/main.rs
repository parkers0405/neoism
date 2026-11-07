//! Offline saved-history cleaner. Never open this database through SQLite.
use anyhow::{bail, Context, Result};
use regex::Regex;
use serde_json::Value;
use std::{collections::BTreeMap, fs, io::Read, path::{Path, PathBuf}};

struct Filter {
    pattern: Regex,
    word_char: Regex,
    words: BTreeMap<String, String>,
}

impl Filter {
    fn new(words: BTreeMap<String, String>) -> Result<Self> {
        // ASCII-insensitive alternatives, but Unicode-aware word boundaries.
        // This avoids matching the ASCII dictionary inside accented words.
        if words.keys().any(|word| word.is_empty() || !word.is_ascii()) {
            bail!("history-cleaner dictionary keys must be nonempty ASCII words or phrases");
        }
        let mut keys: Vec<_> = words.keys().collect();
        keys.sort_by_key(|word| std::cmp::Reverse(word.len()));
        let pattern = if keys.is_empty() {
            // A valid pattern that cannot match a string.
            r"\b\B".to_owned()
        } else {
            format!("(?i-u:{})", keys.iter().map(|word| regex::escape(word)).collect::<Vec<_>>().join("|"))
        };
        Ok(Self { pattern: Regex::new(&pattern)?, word_char: Regex::new(r"^\w$")?, words })
    }

    fn clean(&self, text: &str) -> (String, usize) {
        let mut out = String::new();
        let mut last = 0;
        let mut hits = 0;
        for found in self.pattern.find_iter(text) {
            let is_word = |c: char| self.word_char.is_match(c.encode_utf8(&mut [0; 4]));
            if text[..found.start()].chars().next_back().is_some_and(is_word)
                || text[found.end()..].chars().next().is_some_and(is_word) {
                continue;
            }
            let word = found.as_str();
            let replacement = &self.words[&word.to_ascii_lowercase()];
            let letters: Vec<_> = word.chars().filter(|c| c.is_ascii_alphabetic()).collect();
            let mut word_start = true;
            let titlecase = !letters.is_empty() && word.chars().all(|c| {
                if c.is_ascii_alphabetic() {
                    let valid = if word_start { c.is_ascii_uppercase() } else { c.is_ascii_lowercase() };
                    word_start = false;
                    valid
                } else {
                    word_start = true;
                    true
                }
            });
            let replacement = if !letters.is_empty() && letters.iter().all(|c| c.is_ascii_uppercase()) {
                replacement.to_uppercase()
            } else if titlecase {
                let mut chars = replacement.chars();
                chars.next().map(|c| c.to_uppercase().to_string() + chars.as_str()).unwrap_or_default()
            } else {
                replacement.clone()
            };
            out.push_str(&text[last..found.start()]);
            out.push_str(&replacement);
            last = found.end();
            hits += 1;
        }
        out.push_str(&text[last..]);
        (out, hits)
    }

    fn field(&self, value: &mut Value) -> usize {
        let Some(text) = value.as_str() else { return 0 };
        let (replacement, hits) = self.clean(text);
        if hits > 0 { *value = Value::String(replacement); }
        hits
    }

    fn payload(&self, value: &mut Value, session: bool) -> usize {
        if session {
            return value.get_mut("title").map(|v| self.field(v)).unwrap_or(0);
        }
        let Some(parts) = value.get_mut("parts").and_then(Value::as_array_mut) else { return 0 };
        parts.iter_mut().map(|part| {
            if matches!(part.get("type").and_then(Value::as_str), Some("text" | "reasoning")) {
                part.get_mut("text").map(|v| self.field(v)).unwrap_or(0)
            } else { 0 }
        }).sum()
    }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

// Separate native Turso processes must not write the same database concurrently.
fn ensure_offline(db: &Path, history: &Path) -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    bail!("this initial cleaner supports Linux only: it requires /proc to check database ownership");
    #[cfg(target_os = "linux")]
    {
        let mut inaccessible = 0;
        let targets: Vec<_> = [db.to_path_buf(), sidecar(db, "-wal"), sidecar(db, "-shm"), history.to_path_buf()]
            .into_iter().filter_map(|p| fs::canonicalize(p).ok()).collect();
        for entry in fs::read_dir("/proc")? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(pid) = name.to_str().filter(|s| s.bytes().all(|b| b.is_ascii_digit())) else { continue };
            if pid == std::process::id().to_string() { continue; }
            let fds = match fs::read_dir(entry.path().join("fd")) {
                Ok(fds) => fds,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    // Some same-user system services (e.g. sd-pam) hide their
                    // fds too. The ownership check is best-effort, not a lock.
                    use std::os::unix::fs::MetadataExt;
                    if entry.metadata()?.uid() == fs::metadata(db)?.uid() {
                        let name = fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
                        if name.to_ascii_lowercase().contains("neoism") {
                            bail!("cannot inspect Neoism process {pid}; quit Neoism and its agent server first");
                        }
                        inaccessible += 1;
                    }
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            for fd in fds.flatten() {
                if fs::read_link(fd.path()).is_ok_and(|p| targets.contains(&p)) {
                    bail!("history is open by process {pid}; quit Neoism and its agent server before cleaning; nothing changed");
                }
            }
        }
        if inaccessible > 0 {
            eprintln!("Warning: {inaccessible} same-user processes hide their file descriptors; ownership checks are best-effort. Neoism and its agent server must remain closed.");
        }
    }
    Ok(())
}

fn backup(db: &Path, history: &Path) -> Result<PathBuf> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let directory = sidecar(db, &format!(".history-backup-{stamp}"));
    fs::create_dir(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    }
    for (source, name) in [(db.to_path_buf(), "agent.turso.db"), (sidecar(db, "-wal"), "agent.turso.db-wal"), (sidecar(db, "-shm"), "agent.turso.db-shm"), (history.to_path_buf(), "agent_prompt_history")] {
        if source.exists() { fs::copy(&source, directory.join(name)).with_context(|| format!("backing up {}", source.display()))?; }
    }
    Ok(directory)
}

async fn open(db: &Path) -> Result<turso::Connection> {
    let database = turso::Builder::new_local(db.to_str().context("database path must be UTF-8")?)
        .experimental_index_method(true).build().await?;
    Ok(database.connect()?)
}

async fn has_table(con: &turso::Connection, name: &str) -> Result<bool> {
    let mut rows = con.query("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?", [name]).await?;
    Ok(rows.next().await?.is_some())
}

async fn clean_table(con: &turso::Connection, filter: &Filter, session: bool, apply: bool) -> Result<(usize, usize, usize)> {
    let (table, column) = if session { ("sessions", "info_json") } else { ("messages", "message_json") };
    let search = has_table(con, "conversation_search_docs").await?;
    let queue = has_table(con, "conversation_search_dirty").await?;
    let embeddings = !session && has_table(con, "message_embeddings").await?;
    let mut cursor = String::new();
    let (mut scanned, mut changed, mut hits) = (0, 0, 0);
    loop {
        let mut rows = con.query(&format!("SELECT id, {column} FROM {table} WHERE id > ? ORDER BY id LIMIT 128"), [cursor.as_str()]).await?;
        let mut page = Vec::new();
        while let Some(row) = rows.next().await? {
            page.push((row.get::<String>(0)?, row.get::<String>(1)?));
        }
        drop(rows);
        if page.is_empty() { break; }
        for (id, raw) in page {
            cursor.clone_from(&id);
            scanned += 1;
            let mut payload: Value = serde_json::from_str(&raw).with_context(|| format!("invalid JSON in {table} row {id}; refusing partial cleanup"))?;
            let count = filter.payload(&mut payload, session);
            if count == 0 { continue; }
            hits += count;
            changed += 1;
            if !apply { continue; }
            con.execute(&format!("UPDATE {table} SET {column}=? WHERE id=?"), [payload.to_string(), id.clone()]).await?;
            let key = format!("{}:{id}", if session { "session" } else { "message" });
            if search { con.execute("DELETE FROM conversation_search_docs WHERE key=?", [key.as_str()]).await?; }
            if queue {
                let (session_id, message_id) = if session { ("id", "''") } else { ("session_id", "id") };
                con.execute(&format!("INSERT INTO conversation_search_dirty(key, session_id, message_id, revision, ready_at) SELECT ?, {session_id}, {message_id}, lower(hex(randomblob(16))), 0 FROM {table} WHERE id=? ON CONFLICT(key) DO UPDATE SET revision=excluded.revision, ready_at=0"), [key, id.clone()]).await?;
            }
            if embeddings { con.execute("DELETE FROM message_embeddings WHERE message_id=?", [id]).await?; }
        }
        eprintln!("{table}: scanned {scanned} rows, {hits} matches");
    }
    Ok((scanned, changed, hits))
}

fn clean_prompts(filter: &Filter, original: &str) -> (String, usize) {
    let mut output = String::new();
    let mut hits = 0;
    for line in original.split_inclusive('\n') {
        if let Ok(Value::String(text)) = serde_json::from_str::<Value>(line) {
            let (replacement, count) = filter.clean(&text);
            hits += count;
            if count > 0 {
                output.push_str(&serde_json::to_string(&replacement).expect("string serialization"));
                if line.ends_with("\r\n") { output.push_str("\r\n"); }
                else if line.ends_with('\n') { output.push('\n'); }
            } else { output.push_str(line); }
        } else {
            let (replacement, count) = filter.clean(line);
            output.push_str(&replacement);
            hits += count;
        }
    }
    (output, hits)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() < 2 || args.len() > 3 || (args.len() == 3 && args[2] != "--apply") {
        bail!("usage: neoism-history-cleaner DATABASE PROMPT_HISTORY [--apply]; dictionary JSON on stdin");
    }
    let (db, history) = (Path::new(&args[0]), Path::new(&args[1]));
    let apply = args.len() == 3;
    if !db.is_file() { bail!("no agent database at {}", db.display()); }
    ensure_offline(db, history)?;
    let mut dictionary = String::new();
    std::io::stdin().read_to_string(&mut dictionary)?;
    let filter = Filter::new(serde_json::from_str(&dictionary)?)?;
    let original = if history.exists() { fs::read_to_string(history)? } else { String::new() };
    let (prompts, prompt_hits) = clean_prompts(&filter, &original);
    if apply {
        let directory = backup(db, history)?;
        eprintln!("Backup kept at {} (contains original, unfiltered history)", directory.display());
        ensure_offline(db, history)?;
    }
    let con = open(db).await?;
    con.execute(if apply { "BEGIN IMMEDIATE" } else { "BEGIN" }, ()).await?;
    let result: Result<_> = async {
        let sessions = clean_table(&con, &filter, true, apply).await?;
        let messages = clean_table(&con, &filter, false, apply).await?;
        Ok((sessions, messages))
    }.await;
    let (sessions, messages) = match result {
        Ok(report) => report,
        Err(error) => {
            con.execute("ROLLBACK", ()).await?;
            return Err(error.context("database cleanup rolled back; prompt history unchanged"));
        }
    };
    con.execute(if apply { "COMMIT" } else { "ROLLBACK" }, ()).await?;
    // Do not claim filesystem + database atomicity. Both originals are backed up.
    if apply && prompt_hits > 0 {
        if fs::read_to_string(history)? != original {
            bail!("database cleaned, but prompt history changed during cleanup; prompt history left untouched; backup retained");
        }
        let temp = sidecar(history, &format!(".clean-{}", std::process::id()));
        use std::io::Write;
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
        file.set_permissions(fs::metadata(history)?.permissions())?;
        file.write_all(prompts.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, history).context("database cleaned; could not replace prompt history; originals are backed up")?;
    }
    println!("{}", serde_json::json!({
        "mode": if apply { "applied" } else { "dry-run" },
        "dictionary_entries": filter.words.len(),
        "sessions_scanned": sessions.0, "session_titles_matched": sessions.1,
        "messages_scanned": messages.0, "messages_matched": messages.1,
        "title_matches": sessions.2, "message_matches": messages.2,
        "prompt_matches": prompt_hits,
    }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter() -> Filter {
        Filter::new(BTreeMap::from([("damn".into(), "heck".into()), ("shit".into(), "beans".into()), ("ass".into(), "waffle".into()), ("f*ck".into(), "fudge".into())])).unwrap()
    }

    #[test]
    fn case_boundaries_and_disguises() {
        assert_eq!(filter().clean("DAMN Damn damn class shité shit_count f*ck"), ("HECK Heck heck class shité shit_count fudge".into(), 4));
    }

    #[test]
    fn only_display_text_changes_and_json_history_stays_valid() {
        let mut payload = serde_json::json!({"id":"shit", "parts":[
            {"type":"text", "text":"DAMN"}, {"type":"reasoning", "text":"shit"},
            {"type":"tool", "text":"damn", "input":{"command":"shit"}}
        ]});
        assert_eq!(filter().payload(&mut payload, false), 2);
        assert_eq!(payload["id"], "shit");
        assert_eq!(payload["parts"][2]["text"], "damn");
        assert_eq!(payload["parts"][0]["text"], "HECK");
        let (prompts, hits) = clean_prompts(&filter(), "\"DAMN\\nshit\"\r\nplain damn\n");
        assert_eq!(hits, 3);
        assert_eq!(prompts, "\"HECK\\nbeans\"\r\nplain heck\n");
    }

    #[tokio::test]
    async fn native_turso_scan_rewrite_and_search_invalidation() {
        let db = turso::Builder::new_local(":memory:").experimental_index_method(true).build().await.unwrap();
        let con = db.connect().unwrap();
        for sql in [
            "CREATE TABLE sessions(id TEXT PRIMARY KEY, info_json TEXT NOT NULL)",
            "CREATE TABLE messages(id TEXT PRIMARY KEY, session_id TEXT NOT NULL, message_json TEXT NOT NULL)",
            "CREATE TABLE conversation_search_docs(key TEXT PRIMARY KEY, title TEXT, body TEXT)",
            "CREATE INDEX conversation_search_fts ON conversation_search_docs USING fts (title, body)",
            "CREATE TABLE conversation_search_dirty(key TEXT PRIMARY KEY, session_id TEXT, message_id TEXT, revision TEXT, ready_at INTEGER)",
            "CREATE TABLE message_embeddings(message_id TEXT PRIMARY KEY, embedding BLOB)",
        ] { con.execute(sql, ()).await.unwrap(); }
        con.execute("INSERT INTO sessions VALUES (?,?)", ["s1", r#"{"title":"DAMN","directory":"/shit"}"#]).await.unwrap();
        let original = r#"{"parts":[{"type":"text","text":"shit"},{"type":"tool","text":"damn"}]}"#;
        con.execute("INSERT INTO messages VALUES (?,?,?)", ["m1", "s1", original]).await.unwrap();
        con.execute("INSERT INTO conversation_search_docs VALUES ('message:m1','','shit')", ()).await.unwrap();
        con.execute("INSERT INTO message_embeddings VALUES ('m1',NULL)", ()).await.unwrap();
        con.execute("BEGIN", ()).await.unwrap();
        assert_eq!(clean_table(&con, &filter(), false, false).await.unwrap(), (1, 1, 1));
        con.execute("ROLLBACK", ()).await.unwrap();
        let mut rows = con.query("SELECT message_json FROM messages", ()).await.unwrap();
        assert_eq!(rows.next().await.unwrap().unwrap().get::<String>(0).unwrap(), original);
        drop(rows);
        con.execute("BEGIN IMMEDIATE", ()).await.unwrap();
        assert_eq!(clean_table(&con, &filter(), true, true).await.unwrap(), (1, 1, 1));
        assert_eq!(clean_table(&con, &filter(), false, true).await.unwrap(), (1, 1, 1));
        con.execute("COMMIT", ()).await.unwrap();
        let mut rows = con.query("SELECT message_json FROM messages", ()).await.unwrap();
        let message: Value = serde_json::from_str(&rows.next().await.unwrap().unwrap().get::<String>(0).unwrap()).unwrap();
        assert_eq!(message["parts"][0]["text"], "beans");
        assert_eq!(message["parts"][1]["text"], "damn");
        drop(rows);
        for (table, expected) in [("conversation_search_docs", 0), ("conversation_search_dirty", 2), ("message_embeddings", 0), ("messages", 1)] {
            let mut rows = con.query(&format!("SELECT count(*) FROM {table}"), ()).await.unwrap();
            assert_eq!(rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap(), expected);
        }
        assert_eq!(clean_table(&con, &filter(), false, false).await.unwrap(), (1, 0, 0));
    }
}