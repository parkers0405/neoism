//! Chrome helper-page bridge — Settings / Extensions / NeoWorld /
//! About on web.
//!
//! JS-facing surface for the chrome-page hosts added in
//! `neoism-ui/src/chrome/pages.rs`. The TS side owns the buffer-tab
//! list (chrome-page tabs ride the normal `setBufferTabs` replay with
//! kinds `chrome-extensions` / `chrome-neoworld`); this module covers
//! everything else:
//!
//! - Settings: open/refresh the full-screen overlay with the daemon's
//!   `config.json`, drain the user's `SettingsAction`s (persisted by
//!   TS through the daemon `Config` envelope) and hot-apply the
//!   settings the web chrome can honor live (theme, cursor style,
//!   font size) — the web twin of desktop's config-watcher reload.
//! - Extensions: seed the read-only catalog from the daemon's
//!   `ListExtensions` reply; drain OpenRepository intents.
//! - NeoWorld: build the pane from a persisted `StoredPet`-shaped
//!   JSON blob (localStorage on web — the browser-profile analogue of
//!   desktop's per-device sqlite `NeoWorldStore`), and drain pet
//!   snapshots back out for TS to store.
//! - About: open the chrome-owned modal with version + commit.

use super::*;

use neoism_neoworld_core::{Emotions, PetId, PetMode, PetState, Vec2};
use neoism_protocol::config::{ExtensionStatusSummary, ExtensionSummary};
use neoism_ui::panels::extensions_page::{ExtensionEntry, ExtensionStatus, PaneAction};
use neoism_ui::panels::neoworld::NeoWorldPane;
use neoism_ui::panels::settings_page::SettingsAction;

/// JSON mirror of the daemon's `StoredPet` row (neoism-neoworld):
/// identity + the same persisted subset of `PetState` the sqlite
/// store keeps (mode, position, velocity, emotions, facing).
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredPetJs {
    /// 16-byte pet identity — drives temperament/behavior seeding.
    #[serde(default)]
    pet_id: Vec<u8>,
    #[serde(default)]
    name: String,
    /// 0 = Critter, 1 = Agent (same encoding as the sqlite store).
    #[serde(default)]
    mode: u8,
    position: [f32; 2],
    #[serde(default)]
    velocity: [f32; 2],
    #[serde(default)]
    happiness: u16,
    #[serde(default)]
    irritation: u16,
    #[serde(default)]
    excitement: u16,
    #[serde(default)]
    affection: u16,
    #[serde(default)]
    tiredness: u16,
    #[serde(default)]
    loneliness: u16,
    #[serde(default = "default_true")]
    facing_right: bool,
}

fn default_true() -> bool {
    true
}

thread_local! {
    /// The live pet's display name. `NeoWorldPane` keeps its name
    /// private (that file is off-limits to grow accessors), so the
    /// bridge remembers what it seeded here for snapshot round-trips.
    /// Wasm is single-threaded, so a thread_local is a plain cell.
    static NEOWORLD_PET_NAME: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

#[wasm_bindgen]
impl ChromeBridge {
    // ── Settings ───────────────────────────────────────────────────

    /// Open the full-screen Settings overlay. `config_json` is the
    /// daemon host's `config.json` as one JSON document; pass `None`
    /// to open immediately with the last-known values while the fetch
    /// is in flight (follow up with `set_settings_values`).
    pub fn open_settings_page(&mut self, config_json: Option<String>) {
        let values = config_json
            .as_deref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
        match values {
            Some(values) => self.chrome.open_settings_page(values, Vec::new()),
            None => {
                // Open immediately with whatever values are already
                // seeded; the daemon fetch follows up through
                // `set_settings_values`.
                self.hide_modals();
                self.chrome.settings_page.open();
            }
        }
        self.relayout_chrome();
    }

    /// Refresh the (open) settings overlay with a newer config
    /// snapshot from the daemon.
    pub fn set_settings_values(&mut self, config_json: &str) -> Result<(), JsValue> {
        let values: serde_json::Value = serde_json::from_str(config_json)
            .map_err(|e| JsValue::from_str(&format!("config parse: {e}")))?;
        self.chrome.set_settings_values(values);
        Ok(())
    }

    pub fn settings_page_active(&self) -> bool {
        self.chrome.settings_page.is_active()
    }

    /// Drain queued settings actions as a JSON array for the TS host
    /// to persist through the daemon `Config` envelope:
    /// `[{kind:"set", key, value} | {kind:"set_keybind", action, key,
    /// with} | {kind:"open_config_file"} | {kind:"run_action",
    /// action}]`.
    ///
    /// Side effects the web chrome can honor live are applied here
    /// before the actions surface (theme swap, cursor style, font
    /// size) — desktop gets the same effect from its config
    /// fs-watcher; web applies eagerly and persists in parallel.
    pub fn drain_settings_actions(&mut self) -> JsValue {
        let actions = self.chrome.drain_settings_actions();
        if actions.is_empty() {
            return JsValue::NULL;
        }
        let mut out: Vec<serde_json::Value> = Vec::with_capacity(actions.len());
        for action in actions {
            match &action {
                SettingsAction::Set { key, value } => {
                    self.hot_apply_setting(key, value);
                    out.push(serde_json::json!({
                        "kind": "set",
                        "key": *key,
                        "value": value,
                    }));
                }
                SettingsAction::SetKeybind { action, key, with } => {
                    out.push(serde_json::json!({
                        "kind": "set_keybind",
                        "action": *action,
                        "key": key,
                        "with": with,
                    }));
                }
                SettingsAction::OpenConfigFile => {
                    // The raw config.json lives on the daemon host's
                    // disk — the web editor can't open it in place.
                    self.chrome.notifications.push(
                        "config.json lives on the daemon host — open it from the Neoism desktop app.",
                        neoism_ui::panels::notifications::NotificationLevel::Info,
                    );
                    out.push(serde_json::json!({ "kind": "open_config_file" }));
                }
                SettingsAction::RunAction(name) => {
                    if *name == "open-model" {
                        // Desktop: close settings, open the agent's
                        // model/provider picker. Same here.
                        self.chrome.close_settings_page();
                        self.queue_agent_tab_open();
                        if let Some(agent) = self.chrome.agent_pane_mut() {
                            agent.open_model_picker();
                        }
                    }
                    out.push(serde_json::json!({
                        "kind": "run_action",
                        "action": *name,
                    }));
                }
            }
        }
        JsValue::from_str(
            &serde_json::to_string(&out).unwrap_or_else(|_| "[]".to_string()),
        )
    }

    /// Live-apply the settings the web chrome can honor without a
    /// restart. Mirrors the desktop config-watcher's hot-reload
    /// side effects for the same keys.
    fn hot_apply_setting(&mut self, key: &str, value: &serde_json::Value) {
        match key {
            "appearance.theme" => {
                if let Some(name) = value.as_str() {
                    let name = name.to_string();
                    self.set_ide_theme(&name);
                }
            }
            "presence.cursor-style" => {
                if let Some(style) = value.as_str() {
                    let style = style.to_string();
                    self.chrome.set_cursor_style_config(None, &style);
                }
            }
            "appearance.fonts.size" => {
                let size = value
                    .as_f64()
                    .or_else(|| value.as_str().and_then(|s| s.parse::<f64>().ok()));
                if let Some(size) = size {
                    // 14px is the chrome's 1.0-scale baseline.
                    self.set_font_scale((size as f32 / 14.0).clamp(0.5, 3.0));
                }
            }
            _ => {}
        }
    }

    // ── About ──────────────────────────────────────────────────────

    /// Open the About modal (desktop `Screen::open_about` twin).
    pub fn open_about_modal(&mut self) {
        let version = env!("CARGO_PKG_VERSION");
        let commit = option_env!("GIT_HASH").unwrap_or("dev");
        self.chrome
            .open_about_modal(version, &format!("{commit} (web)"));
        self.relayout_chrome();
    }

    // ── Extensions (read-only) ─────────────────────────────────────

    /// Seed the Extensions page from the daemon's `ListExtensions`
    /// reply (`Vec<ExtensionSummary>` as JSON). Statuses reflect the
    /// DAEMON host's disk + live LSP engine; the page is read-only on
    /// web (install clicks surface an honest "manage from desktop"
    /// toast inside the shared chrome).
    pub fn set_extensions_entries(&mut self, entries_json: &str) -> Result<(), JsValue> {
        let summaries: Vec<ExtensionSummary> = serde_json::from_str(entries_json)
            .map_err(|e| JsValue::from_str(&format!("extensions parse: {e}")))?;
        let entries: Vec<ExtensionEntry> = summaries
            .into_iter()
            .map(|summary| ExtensionEntry {
                id: summary.id,
                name: summary.name,
                version: summary.version,
                description: summary.description,
                author: summary.author,
                downloads: summary.downloads,
                categories: summary.categories,
                languages: summary.languages,
                status: match summary.status {
                    ExtensionStatusSummary::NotInstalled => {
                        ExtensionStatus::NotInstalled
                    }
                    ExtensionStatusSummary::BuiltIn => ExtensionStatus::BuiltIn,
                    ExtensionStatusSummary::Detected => ExtensionStatus::Detected,
                    ExtensionStatusSummary::Unavailable => ExtensionStatus::Unavailable,
                    ExtensionStatusSummary::Installed => ExtensionStatus::Installed {
                        version: summary
                            .installed_version
                            .unwrap_or_else(|| "managed".to_string()),
                    },
                },
                repository_url: summary.repository_url,
                lsp_source: summary.lsp_source,
            })
            .collect();
        self.chrome.extensions_page.set_entries(entries);
        Ok(())
    }

    /// Auto-focus the Extensions search box (desktop parity on page
    /// open).
    pub fn extensions_focus_search(&mut self) {
        self.chrome.extensions_page.focus_search();
    }

    /// Drain Extensions page intents needing a browser host:
    /// `[{kind:"open_repository", url}]`.
    pub fn drain_extensions_actions(&mut self) -> JsValue {
        let actions = self.chrome.drain_extensions_actions();
        if actions.is_empty() {
            return JsValue::NULL;
        }
        let out: Vec<serde_json::Value> = actions
            .into_iter()
            .filter_map(|action| match action {
                PaneAction::OpenRepository(url) => Some(serde_json::json!({
                    "kind": "open_repository",
                    "url": url,
                })),
                PaneAction::InstallToggleRequested { .. } => None,
            })
            .collect();
        JsValue::from_str(
            &serde_json::to_string(&out).unwrap_or_else(|_| "[]".to_string()),
        )
    }

    // ── NeoWorld ───────────────────────────────────────────────────

    /// Ensure the NeoWorld pane exists, seeding from a persisted
    /// `StoredPetJs` JSON blob when given (TS reads it from
    /// localStorage). Safe to call repeatedly — an installed pane is
    /// kept so the live sim never resets mid-session.
    pub fn neoworld_ensure(&mut self, stored_json: Option<String>) {
        if self.chrome.neoworld_pane().is_some() {
            return;
        }
        let stored = stored_json
            .as_deref()
            .and_then(|raw| serde_json::from_str::<StoredPetJs>(raw).ok());
        let pane = match stored {
            Some(stored) => {
                let mut id_bytes = [0u8; 16];
                for (slot, byte) in id_bytes.iter_mut().zip(stored.pet_id.iter()) {
                    *slot = *byte;
                }
                if stored.pet_id.is_empty() {
                    id_bytes = fresh_pet_id_bytes();
                }
                let mode = if stored.mode == 1 {
                    PetMode::Agent
                } else {
                    PetMode::Critter
                };
                let state = PetState::restored(
                    PetId(id_bytes),
                    mode,
                    Vec2::new(stored.position[0], stored.position[1]),
                    Vec2::new(stored.velocity[0], stored.velocity[1]),
                    Emotions {
                        happiness: stored.happiness,
                        irritation: stored.irritation,
                        excitement: stored.excitement,
                        affection: stored.affection,
                        tiredness: stored.tiredness,
                        loneliness: stored.loneliness,
                    },
                    stored.facing_right,
                );
                let name = if stored.name.trim().is_empty() {
                    "Pip".to_string()
                } else {
                    stored.name
                };
                NEOWORLD_PET_NAME.with(|cell| {
                    *cell.borrow_mut() = Some(name.clone());
                });
                NeoWorldPane::new(state, name)
            }
            None => {
                NEOWORLD_PET_NAME.with(|cell| {
                    *cell.borrow_mut() = Some("Pip".to_string());
                });
                NeoWorldPane::new(
                    PetState::new(PetId(fresh_pet_id_bytes()), Vec2::new(120.0, 150.0)),
                    "Pip",
                )
            }
        };
        self.chrome.install_neoworld_pane(pane);
    }

    /// Drain the newest queued pet snapshot as `StoredPetJs` JSON for
    /// TS to persist (localStorage key). `None` when nothing changed
    /// since the last drain.
    pub fn drain_neoworld_snapshot(&mut self) -> Option<String> {
        let snapshots = self.chrome.drain_neoworld_snapshots();
        let state = *snapshots.last()?;
        let name = NEOWORLD_PET_NAME
            .with(|cell| cell.borrow().clone())
            .unwrap_or_else(|| "Pip".to_string());
        let stored = StoredPetJs {
            pet_id: state.id.0.to_vec(),
            name,
            mode: match state.mode {
                PetMode::Critter => 0,
                PetMode::Agent => 1,
            },
            position: [state.position.x, state.position.y],
            velocity: [state.velocity.x, state.velocity.y],
            happiness: state.emotions.happiness,
            irritation: state.emotions.irritation,
            excitement: state.emotions.excitement,
            affection: state.emotions.affection,
            tiredness: state.emotions.tiredness,
            loneliness: state.emotions.loneliness,
            facing_right: state.facing_right,
        };
        serde_json::to_string(&stored).ok()
    }
}

/// 16 identity bytes for a freshly-minted web pet — seeded from the
/// wall clock + `Math.random` (no `getrandom` on this target). The
/// id drives temperament/behavior variety, not security.
fn fresh_pet_id_bytes() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    let now = js_sys::Date::now() as u64;
    bytes[..8].copy_from_slice(&now.to_le_bytes());
    let rand_a = (js_sys::Math::random() * f64::from(u32::MAX)) as u32;
    let rand_b = (js_sys::Math::random() * f64::from(u32::MAX)) as u32;
    bytes[8..12].copy_from_slice(&rand_a.to_le_bytes());
    bytes[12..16].copy_from_slice(&rand_b.to_le_bytes());
    bytes
}
