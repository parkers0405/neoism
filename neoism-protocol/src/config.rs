//! Config + extensions read surface — web/desktop chrome -> daemon.
//!
//! Backs the web Settings page (generic get/set over the daemon host's
//! unified `config.json`, mirroring what the desktop's
//! `neoism_backend::config::write_setting` accepts) and the read-only
//! web Extensions catalog (a snapshot of what the daemon host knows
//! about MCP servers, language servers, kernels, and built-in
//! grammars). Pure wire shapes — no I/O here.

use serde::{Deserialize, Serialize};

/// Machine-readable description of one supported config leaf.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigDescriptor {
    /// Canonical dotted JSON path (for example `appearance.fonts.family`).
    pub path: String,
    pub label: String,
    pub description: String,
    pub value_kind: ConfigValueKind,
    pub default: serde_json::Value,
    /// Stable suggestions shipped by Neoism.
    #[serde(default)]
    pub static_suggestions: Vec<String>,
    /// Suggestions discovered on the daemon host (fonts, shells, custom themes, ...).
    #[serde(default)]
    pub runtime_suggestions: Vec<String>,
    /// Typed choices used by both the Settings UI and JSONC completion.
    /// Unlike legacy suggestions, these preserve numbers, booleans and null.
    #[serde(default)]
    pub options: Vec<ConfigOption>,
    /// Host catalog responsible for dynamic choices. Hosts return this even
    /// after resolving it so clients can explain where choices came from.
    #[serde(default)]
    pub provider: Option<ConfigSuggestionProvider>,
    /// Validation and presentation hints for free-form values.
    #[serde(default)]
    pub constraints: ConfigConstraints,
    /// Additional JSON kinds accepted by union-shaped settings.
    #[serde(default)]
    pub accepted_kinds: Vec<ConfigValueKind>,
    /// Whether values outside the suggestions are valid.
    pub extensible: bool,
    pub category: ConfigCategory,
    pub control: ConfigControl,
    /// Whether this descriptor is a concrete control in the graphical
    /// Settings page. Completion-only templates remain in the schema.
    #[serde(default = "default_settings_visible")]
    pub settings_visible: bool,
}

fn default_settings_visible() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigOption {
    pub value: serde_json::Value,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ConfigConstraints {
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub step: Option<f64>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub nullable: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSuggestionProvider {
    SystemFonts,
    IdeThemes,
    TerminalPalettes,
    MashupPacks,
    Shells,
    Executables,
    AgentNames,
    ProviderIds,
    Models,
    LspAdapters,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConfigValueKind {
    Boolean,
    Integer,
    Number,
    String,
    Array,
    Object,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConfigCategory {
    General,
    Appearance,
    Editor,
    Terminal,
    Ui,
    Presence,
    Keybinds,
    Agent,
    Platform,
    Renderer,
    Developer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ConfigControl {
    Toggle,
    Select,
    Text,
    Number,
    FontFamily,
    Color,
    Keybinding,
    StringList,
    Object,
}

/// Raw JSONC document plus the metadata needed by a remote editor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigDocument {
    pub content: String,
    /// User-facing host path; not intended as a remotely addressable path.
    pub display_path: String,
    /// Opaque content revision. Clients must return it when saving.
    pub revision: String,
    pub writable: bool,
}

/// Inbound config-plane messages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConfigClientMessage {
    /// Fetch the daemon host's full `config.json` as one raw JSON
    /// value (comments/trailing commas already stripped). The reply is
    /// [`ConfigServerMessage::Config`].
    GetConfig,
    /// Fetch canonical config descriptors, including host-derived suggestions.
    GetConfigSchema,
    /// Fetch the raw JSONC file without creating it.
    GetConfigDocument,
    /// Create the config file if absent, then fetch its raw document.
    EnsureConfigDocument,
    /// Validate and atomically save a complete raw JSONC document.
    SaveConfigDocument {
        content: String,
        expected_revision: String,
    },
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
    /// List Mash Up Packs installed on the daemon host.
    ListMashupPacks,
    /// Apply an installed pack, or deactivate it with `None`.
    ApplyMashupPack { id: Option<String> },
}

/// Outbound config-plane replies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ConfigServerMessage {
    /// The full config document.
    Config {
        value: serde_json::Value,
    },
    ConfigSchema {
        descriptors: Vec<ConfigDescriptor>,
    },
    ConfigDocument {
        document: ConfigDocument,
    },
    ConfigDocumentSaved {
        document: ConfigDocument,
    },
    /// A `SetSetting` landed on disk.
    SettingWritten {
        key: String,
    },
    /// A `SetKeybind` landed on disk.
    KeybindWritten {
        action: String,
    },
    /// Reply to `ListExtensions`.
    Extensions {
        entries: Vec<ExtensionSummary>,
    },
    MashupPacks {
        entries: Vec<MashupPackSummary>,
    },
    MashupPackApplied {
        id: Option<String>,
        config: serde_json::Value,
    },
    Error {
        message: String,
    },
}

/// Host-resolved Mash Up Pack row. Asset paths are deliberately opaque to
/// clients; web only uses the shader id/path for its existing visual filter.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MashupPackSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub shader_overlay: Option<String>,
    #[serde(default)]
    pub font_family: Option<String>,
    #[serde(default)]
    pub slots: Vec<String>,
    #[serde(default)]
    pub theme_extends: Option<String>,
    #[serde(default)]
    pub theme_colors: std::collections::BTreeMap<String, String>,
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
            ConfigClientMessage::GetConfigSchema,
            ConfigClientMessage::GetConfigDocument,
            ConfigClientMessage::EnsureConfigDocument,
            ConfigClientMessage::SaveConfigDocument {
                content: "{}\n".into(),
                expected_revision: "rev".into(),
            },
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
            ConfigClientMessage::ListMashupPacks,
            ConfigClientMessage::ApplyMashupPack { id: Some("phosphor".into()) },
        ];
        for msg in msgs {
            let json = serde_json::to_string(&msg).unwrap();
            let back: ConfigClientMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(back, msg);
        }
    }

    #[test]
    fn descriptor_and_document_roundtrip() {
        let reply = ConfigServerMessage::ConfigSchema {
            descriptors: vec![ConfigDescriptor {
                path: "editor.vim-mode".into(),
                label: "Vim mode".into(),
                description: "Use Vim keybindings.".into(),
                value_kind: ConfigValueKind::Boolean,
                default: serde_json::json!(true),
                static_suggestions: vec![],
                runtime_suggestions: vec![],
                options: vec![ConfigOption {
                    value: serde_json::json!(true),
                    label: Some("Enabled".into()),
                    description: Some("Use Vim keybindings.".into()),
                }],
                provider: None,
                constraints: ConfigConstraints::default(),
                accepted_kinds: vec![],
                extensible: false,
                category: ConfigCategory::Editor,
                control: ConfigControl::Toggle,
                settings_visible: true,
            }],
        };
        let encoded = serde_json::to_string(&reply).unwrap();
        assert_eq!(
            serde_json::from_str::<ConfigServerMessage>(&encoded).unwrap(),
            reply
        );

        let document = ConfigDocument {
            content: "// config\n{}\n".into(),
            display_path: "~/.config/neoism/config.json".into(),
            revision: "abc".into(),
            writable: true,
        };
        let reply = ConfigServerMessage::ConfigDocumentSaved { document };
        let encoded = serde_json::to_string(&reply).unwrap();
        assert_eq!(
            serde_json::from_str::<ConfigServerMessage>(&encoded).unwrap(),
            reply
        );
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

    #[test]
    fn mashup_messages_roundtrip() {
        let reply = ConfigServerMessage::MashupPacks {
            entries: vec![MashupPackSummary {
                id: "phosphor".into(),
                name: "Phosphor".into(),
                description: "CRT green".into(),
                theme: Some("phosphor".into()),
                shader_overlay: Some("builtin:crt".into()),
                font_family: None,
                slots: vec!["theme".into(), "shader".into()],
                theme_extends: Some("pastel_dark".into()),
                theme_colors: [("fg".into(), "#33ff66".into())].into(),
            }],
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert_eq!(serde_json::from_str::<ConfigServerMessage>(&json).unwrap(), reply);
    }
}
