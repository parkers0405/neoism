//! Config + extensions read surface — web/desktop chrome -> daemon.
//!
//! Backs the web Settings page (generic get/set over the daemon host's
//! unified `config.json`, mirroring what the desktop's
//! `neoism_backend::config::write_setting` accepts) and the read-only
//! web Extensions catalog (a snapshot of what the daemon host knows
//! about MCP servers, language servers, kernels, and built-in
//! grammars). Pure wire shapes — no I/O here.

use serde::{Deserialize, Serialize};

/// Inbound config-plane messages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConfigClientMessage {
    /// Fetch the daemon host's full `config.json` as one raw JSON
    /// value (comments/trailing commas already stripped). The reply is
    /// [`ConfigServerMessage::Config`].
    GetConfig,
    /// Persist one setting at a golden dotted path
    /// (`appearance.fonts.family`, `editor.vim-mode`, ...). Exactly the
    /// contract of `neoism_backend::config::write_setting`; the daemon
    /// host's own fs-watcher hot-reloads the file so a desktop app
    /// running against the same config picks the change up live.
    SetSetting {
        key: String,
        value: serde_json::Value,
    },
    /// Upsert a `keybinds.keys` override for `action`; an empty `key`
    /// clears the override (mirrors
    /// `neoism_backend::config::write_keybind`).
    SetKeybind {
        action: String,
        key: String,
        with: String,
    },
    /// Read-only inventory of the daemon host's extensions: bundled
    /// MCP registry + installed index, engine language-server
    /// adapters with live status, kernels, and compiled-in grammars.
    ListExtensions,
}

/// Outbound config-plane replies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConfigServerMessage {
    /// The full config document.
    Config { value: serde_json::Value },
    /// A `SetSetting` landed on disk.
    SettingWritten { key: String },
    /// A `SetKeybind` landed on disk.
    KeybindWritten { action: String },
    /// Reply to `ListExtensions`.
    Extensions { entries: Vec<ExtensionSummary> },
    Error { message: String },
}

/// Lifecycle state of one extension row, as the daemon host sees it.
/// Mirrors the shared panel's `ExtensionStatus` minus the transient
/// `Installing`/`Uninstalling` states (the web surface is read-only).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionStatusSummary {
    NotInstalled,
    /// Ships with Neoism; no package lifecycle.
    BuiltIn,
    /// Binary found on the daemon host (`$PATH`/config) but not
    /// managed by Neoism's installer.
    Detected,
    /// No binary and no managed installer can supply one.
    Unavailable,
    Installed,
}

/// One extension catalog row. Field-for-field mirror of the shared
/// extensions panel's `ExtensionEntry` (which is not serializable and
/// lives in the UI crate) so hosts can map without loss.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtensionSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    #[serde(default)]
    pub downloads: Option<u64>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    pub status: ExtensionStatusSummary,
    /// Version string when `status == Installed`.
    #[serde(default)]
    pub installed_version: Option<String>,
    #[serde(default)]
    pub repository_url: Option<String>,
    /// Language-server rows: where the engine resolves the binary
    /// (`"connected"`, `"built-in/socket"`, `"extension"`, `"path"`,
    /// `"config"`, `"missing"`). `None` for non-LSP rows.
    #[serde(default)]
    pub lsp_source: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_messages_roundtrip() {
        let msgs = vec![
            ConfigClientMessage::GetConfig,
            ConfigClientMessage::SetSetting {
                key: "appearance.theme".into(),
                value: serde_json::json!("tokyo_night"),
            },
            ConfigClientMessage::SetKeybind {
                action: "opencommandpalette".into(),
                key: "p".into(),
                with: "alt".into(),
            },
            ConfigClientMessage::ListExtensions,
        ];
        for msg in msgs {
            let json = serde_json::to_string(&msg).unwrap();
            let back: ConfigClientMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(back, msg);
        }
    }

    #[test]
    fn extension_summary_roundtrips() {
        let reply = ConfigServerMessage::Extensions {
            entries: vec![ExtensionSummary {
                id: "rust-analyzer".into(),
                name: "Rust Language Server".into(),
                version: "1.0.0".into(),
                description: "d".into(),
                author: "Neoism".into(),
                downloads: None,
                categories: vec!["Language Server".into()],
                languages: vec!["Rust".into()],
                status: ExtensionStatusSummary::Installed,
                installed_version: Some("1.0.0".into()),
                repository_url: None,
                lsp_source: Some("connected".into()),
            }],
        };
        let json = serde_json::to_string(&reply).unwrap();
        let back: ConfigServerMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, reply);
    }
}
