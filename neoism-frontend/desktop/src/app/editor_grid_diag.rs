//! Always-on, low-volume diagnostic for the embedded-nvim editor grid
//! *selection*.
//!
//! Symptom this exists to catch: the editor freezes on the nvim view —
//! every code file opens to the same stale row, the cursor is stuck, but
//! markdown panes and the rest of the chrome work fine, and it only
//! happens while writing code. That is NOT a deadlock (the app keeps
//! responding); it is a wrong/sticky `target_editor_grid` in the nvim
//! pump (`context/tab.rs`). neoism multiplexes several nvim grids — the
//! main editor plus a separate grid for every LSP popup (hover, the
//! inline diagnostic float, completion). The pump picks ONE as the
//! editor and DROPS every redraw for the others (`*grid !=
//! target_editor_grid { continue }` + `apply_redraw_events`). If the
//! selection latches onto the wrong grid (an LSP float can satisfy the
//! width-only "has surface" test) it sticks, and from then on the real
//! editor grid's redraws are silently discarded.
//!
//! Why this module is needed: the app normally logs to `/dev/null` and
//! the freeze watchdog is env-gated, so the wedge left no trace and
//! couldn't be diagnosed from a live process. This records the two
//! things that pinpoint the cause — grid-id flips, and "shadow editor"
//! batches where a *different* editor-height grid drew a screenful of
//! content while the chosen target drew ~nothing — to a per-pid file.
//!
//! Inspect a live wedged process with:
//!   `cat ~/.config/neoism/log/editor-grid-<pid>.log`
//! A `SHADOW_EDITOR_RECOVERED` line names the stale grid we were about
//! to use (`recovered_from=`) and the real editor grid we switched to
//! (`to=`).
//!
//! Cost: writes only on a state *change* (deduped per route), so a
//! sustained wedge does not spam; the file self-truncates past a cap.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_FILE_BYTES: u64 = 256 * 1024;

pub struct Record<'a> {
    pub route_id: usize,
    pub from_grid: Option<u64>,
    pub to_grid: u64,
    pub selected_viewport_grid: Option<u64>,
    pub active_grid_by_lines: Option<u64>,
    pub viewport_has_surface: bool,
    pub previous_has_activity: bool,
    pub active_has_surface: bool,
    pub editor_cols: u64,
    pub editor_rows: u64,
    pub target_cells: u32,
    /// `Some(grid)` when an editor-height grid OTHER than the target drew
    /// a screenful of content this batch — the smoking gun for the
    /// dropped-editor wedge.
    pub shadow_editor: Option<u64>,
    /// True when `shadow_editor` is set AND the chosen target drew almost
    /// nothing this batch (starved) — i.e. we are dropping the real
    /// editor's redraws right now.
    pub wedge_suspected: bool,
    /// The blank/stale grid we recovered away from, if this record is an
    /// automatic self-heal rather than detection-only evidence.
    pub recovered_from_grid: Option<u64>,
    pub grid_activity_summary: &'a str,
}

fn last_signatures() -> &'static Mutex<HashMap<usize, String>> {
    static MAP: OnceLock<Mutex<HashMap<usize, String>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn log_path() -> PathBuf {
    neoism_backend::config::config_dir_path()
        .join("log")
        .join(format!("editor-grid-{}.log", std::process::id()))
}

/// File logging is OPT-IN (`NEOISM_EDITOR_GRID_LOG=1`) — the dropped-
/// editor wedge this diagnosed is fixed, and always-on left a per-pid
/// file behind on every launch. The self-heal itself lives in the
/// context pump and keeps working regardless; this module only records
/// evidence.
pub(crate) fn enabled() -> bool {
    std::env::var_os("NEOISM_EDITOR_GRID_LOG")
        .map(|raw| neoism_ui::lifecycle_policy::env_flag_truthy(&raw.to_string_lossy()))
        .unwrap_or(false)
}

pub fn record(rec: Record<'_>) {
    if !enabled() {
        return;
    }
    // Dedupe per route on the meaningful signature so a sustained wedge
    // doesn't append a line every frame — the last line always reflects
    // the current state.
    let signature = format!(
        "{}|{:?}|{}|{:?}",
        rec.to_grid, rec.shadow_editor, rec.wedge_suspected, rec.recovered_from_grid
    );
    {
        let Ok(mut map) = last_signatures().lock() else {
            return;
        };
        if map
            .get(&rec.route_id)
            .map(|s| s == &signature)
            .unwrap_or(false)
        {
            return;
        }
        map.insert(rec.route_id, signature);
    }

    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::metadata(&path)
        .map(|m| m.len() > MAX_FILE_BYTES)
        .unwrap_or(false)
    {
        let _ = fs::write(&path, b"");
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let kind = if rec.recovered_from_grid.is_some() {
        "SHADOW_EDITOR_RECOVERED"
    } else if rec.wedge_suspected {
        "SHADOW_EDITOR_DROPPED"
    } else {
        "grid_change"
    };
    let _ = writeln!(
        file,
        "{now} pid={} route={} {kind} from={:?} to={} target_cells={} \
         viewport={:?} active_by_lines={:?} viewport_has_surface={} \
         prev_has_activity={} active_has_surface={} editor={}x{} shadow={:?} \
         recovered_from={:?} activity=[{}]",
        std::process::id(),
        rec.route_id,
        rec.from_grid,
        rec.to_grid,
        rec.target_cells,
        rec.selected_viewport_grid,
        rec.active_grid_by_lines,
        rec.viewport_has_surface,
        rec.previous_has_activity,
        rec.active_has_surface,
        rec.editor_cols,
        rec.editor_rows,
        rec.shadow_editor,
        rec.recovered_from_grid,
        rec.grid_activity_summary,
    );
    let _ = file.flush();
}
