use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::install_runner::InstallError;
use crate::installed::write_atomic;
use crate::manifest::ExtensionManifest;

/// `$XDG_CONFIG_HOME/neoism/config.json`, fallback `$HOME/.config/...` —
/// the unified config the terminal AND the agent server both read.
/// Mirrors `neoism-agent-server::server_util::default_config_dir`.
pub fn agent_config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .or_else(|_| std::env::var("HOME").map(|home| format!("{home}/.config")))
        .unwrap_or_else(|_| ".neoism/config".to_string());
    PathBuf::from(base).join("neoism/config.json")
}

/// Merge an MCP entry into the user's agent config under `mcp.<id>`.
/// `$XDG_CONFIG_HOME/neoism/mcp.json` — the standalone MCP catalog the
/// extensions page writes to (wrapped form: `{ "mcp": { id: {...} } }`).
/// The agent server merges it AFTER config.json, so entries here win.
pub fn mcp_config_path() -> PathBuf {
    agent_config_path().with_file_name("mcp.json")
}

pub fn install_mcp_entry(
    id: &str,
    manifest: &ExtensionManifest,
    bin_path: &Path,
) -> Result<(), InstallError> {
    // New installs land in the dedicated mcp.json; drop any same-id
    // entry still living in config.json so the loader's deep merge
    // can't blend a stale record into the fresh one.
    install_mcp_entry_at(&mcp_config_path(), id, manifest, bin_path)?;
    uninstall_mcp_entry_at(&agent_config_path(), id)
}

/// Remove `mcp.<id>` from the user's agent config — the entry may live
/// in mcp.json (new home) or config.json (legacy/unified), so clean both.
pub fn uninstall_mcp_entry(id: &str) -> Result<(), InstallError> {
    uninstall_mcp_entry_at(&mcp_config_path(), id)?;
    uninstall_mcp_entry_at(&agent_config_path(), id)
}

/// Disable a built-in MCP entry without deleting its config key. The agent
/// server normalizes missing built-ins back to enabled, so built-in uninstall
/// needs an explicit `enabled: false` record to persist user intent. Written
/// to mcp.json (which merges last and wins); any config.json copy is dropped.
pub fn disable_builtin_mcp_entry(id: &str) -> Result<(), InstallError> {
    disable_builtin_mcp_entry_at(&mcp_config_path(), id)?;
    uninstall_mcp_entry_at(&agent_config_path(), id)
}

pub fn install_mcp_entry_at(
    path: &Path,
    id: &str,
    manifest: &ExtensionManifest,
    bin_path: &Path,
) -> Result<(), InstallError> {
    let mut root = read_root_object(path)?;

    // Build new entry from manifest.run + bin_path.
    let mut entry = Map::new();
    entry.insert("type".to_string(), Value::String("local".to_string()));

    let mut command: Vec<Value> = manifest
        .run
        .as_ref()
        .map(|r| r.command.clone())
        .unwrap_or_default()
        .into_iter()
        .map(Value::String)
        .collect();
    let bin_str = bin_path.display().to_string();
    if command.is_empty() {
        command.push(Value::String(bin_str));
    } else {
        command[0] = Value::String(bin_str);
    }
    entry.insert("command".to_string(), Value::Array(command));

    let env_map = manifest
        .run
        .as_ref()
        .map(|r| r.env.clone())
        .unwrap_or_default();
    let mut env_value = Map::new();
    for (k, v) in env_map {
        env_value.insert(k, Value::String(v));
    }
    entry.insert("environment".to_string(), Value::Object(env_value));
    entry.insert("enabled".to_string(), Value::Bool(true));

    // Get-or-create the "mcp" object and insert.
    let mcp = root
        .entry("mcp".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let mcp_obj = mcp.as_object_mut().ok_or_else(|| {
        InstallError::ParseManifest(
            "agent config `mcp` field must be an object".to_string(),
        )
    })?;
    mcp_obj.insert(id.to_string(), Value::Object(entry));

    let serialised = serde_json::to_vec_pretty(&Value::Object(root))
        .map_err(|e| InstallError::ParseManifest(e.to_string()))?;
    write_atomic(path, &serialised)
}

pub fn uninstall_mcp_entry_at(path: &Path, id: &str) -> Result<(), InstallError> {
    if !path.exists() {
        return Ok(());
    }
    let mut root = read_root_object(path)?;
    if let Some(mcp) = root.get_mut("mcp").and_then(|v| v.as_object_mut()) {
        mcp.remove(id);
    }
    let serialised = serde_json::to_vec_pretty(&Value::Object(root))
        .map_err(|e| InstallError::ParseManifest(e.to_string()))?;
    write_atomic(path, &serialised)
}

pub fn disable_builtin_mcp_entry_at(path: &Path, id: &str) -> Result<(), InstallError> {
    let mut root = read_root_object(path)?;
    let mut entry = Map::new();
    entry.insert("type".to_string(), Value::String("local".to_string()));
    entry.insert(
        "command".to_string(),
        Value::Array(vec![
            Value::String("builtin".to_string()),
            Value::String(id.to_string()),
        ]),
    );
    entry.insert("enabled".to_string(), Value::Bool(false));

    let mcp = root
        .entry("mcp".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let mcp_obj = mcp.as_object_mut().ok_or_else(|| {
        InstallError::ParseManifest(
            "agent config `mcp` field must be an object".to_string(),
        )
    })?;
    mcp_obj.insert(id.to_string(), Value::Object(entry));

    let serialised = serde_json::to_vec_pretty(&Value::Object(root))
        .map_err(|e| InstallError::ParseManifest(e.to_string()))?;
    write_atomic(path, &serialised)
}

fn read_root_object(path: &Path) -> Result<Map<String, Value>, InstallError> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(InstallError::Io(e)),
    };
    if bytes.is_empty() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        return Ok(Map::new());
    }
    // The unified config is JSONC (comments + trailing commas legal) —
    // strip before the strict parse so a hand-commented file doesn't
    // fail MCP installs. Programmatic writes below emit plain pretty
    // JSON, so comments are lost on the first write; same trade the
    // in-app preference writers make.
    let text = String::from_utf8_lossy(&bytes);
    let cleaned = strip_trailing_commas(&strip_json_comments(&text));
    let value: Value = serde_json::from_str(&cleaned)
        .map_err(|e| InstallError::ParseManifest(e.to_string()))?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(InstallError::ParseManifest(
            "agent config root must be a JSON object".to_string(),
        )),
    }
}

/// JSONC comment stripper — mirrors `neoism-agent-server::config_parse`
/// and `neoism-backend::config` so every reader of the unified
/// `config.json` accepts the same dialect.
fn strip_json_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            escaped = ch == '\\' && !escaped;
            if ch == '"' && !escaped {
                in_string = false;
            }
            if ch != '\\' {
                escaped = false;
            }
            out.push(ch);
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'/') {
            let _ = chars.next();
            for next in chars.by_ref() {
                if next == '\n' {
                    out.push('\n');
                    break;
                }
            }
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'*') {
            let _ = chars.next();
            let mut previous = '\0';
            for next in chars.by_ref() {
                if previous == '*' && next == '/' {
                    break;
                }
                previous = next;
            }
            continue;
        }
        out.push(ch);
    }
    out
}

fn strip_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            escaped = ch == '\\' && !escaped;
            if ch == '"' && !escaped {
                in_string = false;
            }
            if ch != '\\' {
                escaped = false;
            }
            out.push(ch);
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            continue;
        }
        if ch == ',' {
            let closes_next = chars
                .clone()
                .find(|next| !next.is_whitespace())
                .is_some_and(|next| matches!(next, '}' | ']'));
            if closes_next {
                continue;
            }
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{ExtensionManifest, InstallKind, RunSpec};
    use std::collections::BTreeMap;

    fn fixture() -> &'static str {
        r#"{
  "model": "openai/gpt-5.5",
  "variant": "xhigh",
  "mcp": {
    "supabase": {
      "type": "local",
      "command": ["npx", "-y", "@supabase/mcp-server-supabase@latest", "--project-ref", "zfbcxnsjbjssmsrlydgf"],
      "enabled": false,
      "environment": {
        "SUPABASE_ACCESS_TOKEN": "{env:SUPABASE_ACCESS_TOKEN}"
      }
    },
    "webflow": {
      "type": "remote",
      "url": "https://mcp.webflow.com/mcp",
      "enabled": false
    }
  },
  "custom_top_level": {
    "weird_thing": [1, 2, 3]
  }
}"#
    }

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "neoism-extensions-agentcfg-{}-{}",
            std::process::id(),
            ts()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn ts() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{:x}", n)
    }

    fn sample_manifest() -> ExtensionManifest {
        let mut env = BTreeMap::new();
        env.insert("FOO_TOKEN".to_string(), "{env:FOO_TOKEN}".to_string());
        ExtensionManifest {
            id: "server-filesystem".to_string(),
            name: "Filesystem".to_string(),
            version: "1.2.3".to_string(),
            description: "fs server".to_string(),
            author: "test".to_string(),
            downloads: None,
            categories: vec!["mcp".to_string()],
            languages: vec![],
            repository_url: None,
            homepage: None,
            install: InstallKind::Npm {
                package: "@modelcontextprotocol/server-filesystem".to_string(),
                version: "1.2.3".to_string(),
            },
            run: Some(RunSpec {
                command: vec![
                    "server-filesystem".to_string(),
                    "--root".to_string(),
                    "/".to_string(),
                ],
                env,
            }),
            env_keys: vec![],
        }
    }

    #[test]
    fn install_preserves_unknown_fields() {
        let dir = tempdir();
        let path = dir.join("config.json");
        std::fs::write(&path, fixture()).unwrap();

        let manifest = sample_manifest();
        let bin = PathBuf::from("/abs/path/bin/server-filesystem");
        install_mcp_entry_at(&path, "server-filesystem", &manifest, &bin).unwrap();

        let after: Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();

        // Top-level untouched fields.
        assert_eq!(
            after.get("model").and_then(|v| v.as_str()),
            Some("openai/gpt-5.5")
        );
        assert_eq!(after.get("variant").and_then(|v| v.as_str()), Some("xhigh"));
        assert!(after.get("custom_top_level").is_some());
        assert_eq!(
            after
                .pointer("/custom_top_level/weird_thing/2")
                .and_then(|v| v.as_i64()),
            Some(3)
        );

        // Existing mcp entries preserved verbatim.
        let supabase = after.pointer("/mcp/supabase").unwrap();
        assert_eq!(supabase.get("type").and_then(|v| v.as_str()), Some("local"));
        assert_eq!(
            supabase
                .pointer("/environment/SUPABASE_ACCESS_TOKEN")
                .and_then(|v| v.as_str()),
            Some("{env:SUPABASE_ACCESS_TOKEN}")
        );
        let webflow = after.pointer("/mcp/webflow").unwrap();
        assert_eq!(webflow.get("type").and_then(|v| v.as_str()), Some("remote"));
        assert_eq!(
            webflow.get("url").and_then(|v| v.as_str()),
            Some("https://mcp.webflow.com/mcp")
        );

        // New entry shape.
        let new_entry = after.pointer("/mcp/server-filesystem").unwrap();
        assert_eq!(
            new_entry.get("type").and_then(|v| v.as_str()),
            Some("local")
        );
        assert_eq!(
            new_entry.get("enabled").and_then(|v| v.as_bool()),
            Some(true)
        );
        let cmd = new_entry.get("command").and_then(|v| v.as_array()).unwrap();
        assert_eq!(cmd[0].as_str(), Some("/abs/path/bin/server-filesystem"));
        assert_eq!(cmd[1].as_str(), Some("--root"));
        assert_eq!(cmd[2].as_str(), Some("/"));
        assert_eq!(
            new_entry
                .pointer("/environment/FOO_TOKEN")
                .and_then(|v| v.as_str()),
            Some("{env:FOO_TOKEN}")
        );
    }

    #[test]
    fn install_overwrites_existing_id() {
        let dir = tempdir();
        let path = dir.join("config.json");
        std::fs::write(&path, fixture()).unwrap();

        let mut m = sample_manifest();
        m.id = "supabase".to_string();
        let bin = PathBuf::from("/new/bin/supabase");
        install_mcp_entry_at(&path, "supabase", &m, &bin).unwrap();

        let after: Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let cmd = after
            .pointer("/mcp/supabase/command")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(cmd[0].as_str(), Some("/new/bin/supabase"));
    }

    #[test]
    fn uninstall_removes_only_target() {
        let dir = tempdir();
        let path = dir.join("config.json");
        std::fs::write(&path, fixture()).unwrap();

        uninstall_mcp_entry_at(&path, "webflow").unwrap();

        let after: Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(after.pointer("/mcp/webflow").is_none());
        assert!(after.pointer("/mcp/supabase").is_some());
        // Unknown top-level keys still present.
        assert!(after.get("custom_top_level").is_some());
    }

    #[test]
    fn install_creates_missing_file() {
        let dir = tempdir();
        let path = dir.join("nested/path/config.json");
        let manifest = sample_manifest();
        let bin = PathBuf::from("/abs/bin/server-filesystem");
        install_mcp_entry_at(&path, "server-filesystem", &manifest, &bin).unwrap();

        let after: Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(after.pointer("/mcp/server-filesystem").is_some());
    }

    #[test]
    fn uninstall_missing_file_is_noop() {
        let dir = tempdir();
        let path = dir.join("nope.json");
        uninstall_mcp_entry_at(&path, "anything").unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn disable_builtin_mcp_entry_writes_explicit_disabled_local_entry() {
        let dir = tempdir();
        let path = dir.join("config.json");

        disable_builtin_mcp_entry_at(&path, "neoism-memory").unwrap();

        let after: Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let entry = after.pointer("/mcp/neoism-memory").unwrap();
        assert_eq!(entry.get("type").and_then(|v| v.as_str()), Some("local"));
        assert_eq!(
            entry.pointer("/command/0").and_then(|v| v.as_str()),
            Some("builtin")
        );
        assert_eq!(
            entry.pointer("/command/1").and_then(|v| v.as_str()),
            Some("neoism-memory")
        );
        assert_eq!(entry.get("enabled").and_then(|v| v.as_bool()), Some(false));
    }
}
