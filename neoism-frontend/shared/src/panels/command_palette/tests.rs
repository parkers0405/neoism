// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use super::actions::{
    command_visible_for_surface, shaders_modal_spec, theme_picker_modal_spec, HostKind,
    PaletteAction, PaletteServerEntry, PaletteShaderEntry, PaletteSurface,
    PaletteWorkspaceEntry, PaletteWorkspaceTarget, WorkspaceHostKind,
    WorkspaceVisibility,
};
use super::commands::COMMANDS;
use super::commands::EX_COMMANDS;
use super::fuzzy::fuzzy_score;
use super::modes::{PaletteMode, PaletteRow};
use super::state::{CommandPalette, WorkspaceMovePhase};
use super::MAX_VISIBLE_RESULTS;

/// Build a workspace entry under an explicit host, for the grouped-tree
/// tests below.
fn ws_entry(
    title: &str,
    workspace_id: &str,
    host_id: &str,
    host_label: &str,
    host_kind: HostKind,
    daemon_url: Option<&str>,
    host_online: bool,
) -> PaletteWorkspaceEntry {
    PaletteWorkspaceEntry {
        title: title.to_string(),
        detail: format!("/projects/{title}"),
        target: PaletteWorkspaceTarget {
            workspace_id: workspace_id.to_string(),
        },
        host_id: host_id.to_string(),
        host_label: host_label.to_string(),
        host_kind,
        workspace_host_kind: WorkspaceHostKind::Local,
        workspace_visibility: WorkspaceVisibility::Private,
        current: false,
        daemon_url: daemon_url.map(str::to_string),
        host_online,
    }
}

#[test]
fn test_set_enabled_resets_state() {
    let mut palette = CommandPalette::new();
    palette.set_query("test".to_string());
    palette.selected_index = 3;
    palette.scroll_offset = 2;

    palette.set_enabled(true);

    assert!(palette.query.is_empty());
    assert_eq!(palette.selected_index, 0);
    assert_eq!(palette.scroll_offset, 0);
}

#[test]
fn palette_close_commands_are_registered() {
    let close = COMMANDS
        .iter()
        .find(|cmd| cmd.action == PaletteAction::CloseCurrentSplitOrTab)
        .expect("close current command registered");
    assert!(close.shortcut.contains('q'));

    assert!(EX_COMMANDS.iter().any(|(name, _)| *name == "q"));
    assert!(EX_COMMANDS.iter().any(|(name, _)| *name == "qall"));
    assert!(EX_COMMANDS.iter().any(|(name, _)| *name == "qall!"));
    assert!(EX_COMMANDS.iter().any(|(name, _)| *name == "runall"));
    assert!(EX_COMMANDS.iter().any(|(name, _)| *name == "runbelow"));
    assert!(EX_COMMANDS
        .iter()
        .any(|(name, _)| *name == "interruptkernel"));
    assert!(EX_COMMANDS.iter().any(|(name, _)| *name == "clearoutput"));
    assert!(EX_COMMANDS.iter().any(|(name, _)| *name == "clearoutputs"));
    assert!(EX_COMMANDS.iter().any(|(name, _)| *name == "restartkernel"));
}

#[test]
fn theme_picker_modal_spec_lists_native_theme_choices() {
    let spec = theme_picker_modal_spec();
    assert_eq!(spec.title, "Theme Picker");
    assert!(spec.input.is_none());
    assert!(spec.blocking);
    assert_eq!(spec.buttons.len(), 5);
    assert_eq!(spec.buttons[0].label, "Pastel Dark");
    assert_eq!(spec.buttons[4].label, "Close");
    match &spec.buttons[0].action {
        crate::widgets::modal::ModalAction::ApplyTheme { name } => {
            assert_eq!(name, "pastel_dark");
        }
        action => panic!("unexpected first theme action: {action:?}"),
    }
}

#[test]
fn shaders_modal_spec_lists_configured_shaders() {
    let spec = shaders_modal_spec(["builtin:ctv_round", "/tmp/hypno_crt.glsl"]);
    assert_eq!(spec.title, "Shaders");
    assert!(spec.input.is_none());
    assert!(!spec.body.contains("/tmp/hypno_crt.glsl"));
    assert_eq!(spec.buttons.len(), 4);
    assert_eq!(spec.buttons[0].label, "None");
    assert_eq!(spec.buttons[1].label, "ctv_round");
    assert_eq!(spec.buttons[1].hint, "Apply shader overlay");
    assert_eq!(spec.buttons[2].label, "hypno_crt");
    assert_eq!(spec.buttons[3].label, "Close");
    match &spec.buttons[1].action {
        crate::widgets::modal::ModalAction::ApplyShaderOverlay { path } => {
            assert_eq!(path.as_deref(), Some("builtin:ctv_round"));
        }
        action => panic!("unexpected shader action: {action:?}"),
    }
}

#[test]
fn test_filtered_commands_empty_query() {
    let palette = CommandPalette::new();
    let filtered = palette.filtered_rows();
    // `StopSharingCurrentWorkspace` is now generally visible (its old
    // `!= Private` gate made it effectively unreachable), so it is no
    // longer excluded here. `ShareCurrentWorkspace` stays visible in the
    // default Private state, so it needs no special-casing either.
    let expected = COMMANDS
        .iter()
        .filter(|cmd| cmd.action != PaletteAction::ToggleAppearanceTheme)
        .filter(|cmd| command_visible_for_surface(&cmd.action, PaletteSurface::Terminal))
        .count();
    assert_eq!(filtered.len(), expected);
}

#[test]
fn servers_mode_filters_and_emits_connection_actions() {
    let mut palette = CommandPalette::new();
    palette.enter_servers_mode(vec![
        PaletteServerEntry {
            id: "local".into(),
            name: "Local Server".into(),
            address: "local daemon".into(),
            local: true,
            status: crate::panels::ServerIndicatorStatus::Online,
            active: true,
        },
        PaletteServerEntry {
            id: "home".into(),
            name: "Home Workstation".into(),
            address: "wss://home.example/session".into(),
            local: false,
            status: crate::panels::ServerIndicatorStatus::Unknown,
            active: false,
        },
    ]);

    assert_eq!(palette.filtered_rows().len(), 3);
    assert_eq!(
        palette.get_selected_action(),
        Some(PaletteAction::SelectServer { id: "local".into() })
    );

    palette.set_query("home".into());
    assert_eq!(palette.filtered_rows().len(), 1);
    assert_eq!(
        palette.get_selected_action(),
        Some(PaletteAction::SelectServer { id: "home".into() })
    );
}

#[test]
fn test_filtered_commands_by_title() {
    let mut palette = CommandPalette::new();
    palette.query = "split".to_string();
    let filtered = palette.filtered_rows();
    assert!(filtered.len() >= 2);
    for (_, row) in &filtered {
        assert!(row.title().to_lowercase().contains("split"));
    }
}

#[test]
fn test_filtered_commands_case_insensitive() {
    let mut palette = CommandPalette::new();
    palette.query = "QUIT".to_string();
    let filtered = palette.filtered_rows();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].1.title(), "Quit");
}

#[test]
fn test_fuzzy_matching() {
    let mut palette = CommandPalette::new();
    palette.query = "nt".to_string(); // Should match "New Tab", "Next Tab", etc.
    let filtered = palette.filtered_rows();
    assert!(!filtered.is_empty());
}

#[test]
fn search_keys_off_command_title_not_service_namespace() {
    // Typing a service namespace word ("neoism") must not surface every
    // command in that namespace — search matches the command title at the
    // top level, not the `neoism:`/`workspace:` group prefix. "New Tab" and
    // "Split Right" live under the Neoism namespace but their title/alias
    // don't contain "neoism", so they should not match.
    let mut palette = CommandPalette::new();
    palette.query = "neoism".to_string();
    let titles: Vec<&str> = palette
        .filtered_rows()
        .into_iter()
        .map(|(_, row)| row.title())
        .collect();
    assert!(!titles.contains(&"New Tab"));
    assert!(!titles.contains(&"Split Right"));
}

#[test]
fn command_aliases_are_searchable_without_duplicate_rows() {
    let mut palette = CommandPalette::new();
    palette.query = "terminal".to_string();
    let titles: Vec<&str> = palette
        .filtered_rows()
        .into_iter()
        .map(|(_, row)| row.title())
        .collect();
    assert!(titles.contains(&"New Tab"));
    assert!(!titles.contains(&"New Terminal Tab"));
}

#[test]
fn command_visibility_tracks_active_surface() {
    let mut palette = CommandPalette::new();
    palette.set_surface(PaletteSurface::Markdown);
    let markdown_titles: Vec<&str> = palette
        .filtered_rows()
        .into_iter()
        .map(|(_, row)| row.title())
        .collect();
    assert!(markdown_titles.contains(&"Write File"));
    assert!(!markdown_titles.contains(&"Hover Documentation"));
    assert!(!markdown_titles.contains(&"Clear History"));

    palette.set_surface(PaletteSurface::Editor);
    let editor_titles: Vec<&str> = palette
        .filtered_rows()
        .into_iter()
        .map(|(_, row)| row.title())
        .collect();
    assert!(editor_titles.contains(&"Hover Documentation"));
    assert!(editor_titles.contains(&"Write File"));
    assert!(!editor_titles.contains(&"Clear History"));
}

#[test]
fn code_buffer_commands_are_visible_only_on_editor_surface() {
    let mut palette = CommandPalette::new();

    palette.set_surface(PaletteSurface::Editor);
    let editor_titles: Vec<&str> = palette
        .filtered_rows()
        .into_iter()
        .map(|(_, row)| row.title())
        .collect();
    assert!(editor_titles.contains(&"Go to Line…"));
    assert!(editor_titles.contains(&"Toggle Minimap"));

    for surface in [
        PaletteSurface::Terminal,
        PaletteSurface::Markdown,
        PaletteSurface::Notebook,
    ] {
        palette.set_surface(surface);
        let titles: Vec<&str> = palette
            .filtered_rows()
            .into_iter()
            .map(|(_, row)| row.title())
            .collect();
        assert!(!titles.contains(&"Go to Line…"), "{surface:?}");
        assert!(!titles.contains(&"Toggle Minimap"), "{surface:?}");
    }
}

#[test]
fn ex_catalog_registers_native_intercepts() {
    let goto = COMMANDS
        .iter()
        .find(|cmd| cmd.title == "Go to Line…")
        .expect("Go to Line… registered");
    assert_eq!(goto.action, PaletteAction::GoToLine);

    assert!(EX_COMMANDS.iter().any(|(name, _)| *name == "w"));
    assert!(EX_COMMANDS.iter().any(|(name, _)| *name == "wq"));
    assert!(EX_COMMANDS.iter().any(|(name, _)| *name == "buffers"));
    assert!(EX_COMMANDS.iter().any(|(name, _)| *name == "tree"));
}

#[test]
fn search_mode_tracks_backward_direction() {
    let mut palette = CommandPalette::new();

    palette.enter_search_mode_backward();
    assert!(palette.is_search_mode());
    assert!(palette.search_is_backward());

    // Direction survives close so the commit dispatched right after
    // `set_enabled(false)` still sees it.
    palette.set_enabled(false);
    assert!(palette.search_is_backward());

    // A fresh `/` session resets to forward.
    palette.enter_search_mode();
    assert!(palette.is_search_mode());
    assert!(!palette.search_is_backward());
}

#[test]
fn test_set_query_resets_selection_and_scroll() {
    let mut palette = CommandPalette::new();
    palette.selected_index = 5;
    palette.scroll_offset = 3;
    palette.set_query("test".to_string());
    assert_eq!(palette.selected_index, 0);
    assert_eq!(palette.scroll_offset, 0);
}

#[test]
fn test_move_selection_down() {
    let mut palette = CommandPalette::new();
    palette.set_enabled(true);
    assert_eq!(palette.selected_index, 0);
    palette.move_selection_down();
    assert_eq!(palette.selected_index, 1);
    palette.move_selection_down();
    assert_eq!(palette.selected_index, 2);
}

#[test]
fn test_move_selection_down_boundary() {
    let mut palette = CommandPalette::new();
    palette.set_enabled(true);
    let count = palette.filtered_rows().len();
    palette.selected_index = count - 1;
    palette.move_selection_down();
    assert_eq!(palette.selected_index, count - 1);
}

#[test]
fn test_move_selection_up() {
    let mut palette = CommandPalette::new();
    palette.set_enabled(true);
    palette.selected_index = 3;
    palette.move_selection_up();
    assert_eq!(palette.selected_index, 2);
}

#[test]
fn test_move_selection_up_boundary() {
    let mut palette = CommandPalette::new();
    palette.set_enabled(true);
    palette.move_selection_up();
    assert_eq!(palette.selected_index, 0);
}

#[test]
fn test_get_selected_action() {
    let palette = CommandPalette::new();
    let action = palette.get_selected_action();
    assert!(action.is_some());
    // First command is "New Tab"
    assert_eq!(action.unwrap(), PaletteAction::TabCreate);
}

#[test]
fn test_get_selected_action_with_filter() {
    let mut palette = CommandPalette::new();
    palette.set_query("quit".to_string());
    let action = palette.get_selected_action();
    assert_eq!(action, Some(PaletteAction::Quit));
}

#[test]
fn test_scroll_offset_on_move_down() {
    let mut palette = CommandPalette::new();
    palette.set_enabled(true);
    for _ in 0..MAX_VISIBLE_RESULTS {
        palette.move_selection_down();
    }
    assert!(palette.scroll_offset > 0);
}

#[test]
fn test_hit_test_outside() {
    let palette = CommandPalette::new();
    assert!(palette.hit_test(0.0, 0.0, 1200.0, 1.0).is_err());
}

#[test]
fn test_fuzzy_score_basic() {
    assert!(fuzzy_score("nt", "New Tab").is_some());
    assert!(fuzzy_score("xyz", "New Tab").is_none());
    assert!(fuzzy_score("", "New Tab").is_some());
}

#[test]
fn test_fuzzy_score_ordering() {
    // "New Tab" should score higher than "Next Tab" for "net" because of word boundary
    let score_new = fuzzy_score("net", "New Tab").unwrap_or(-100);
    let score_next = fuzzy_score("net", "Next Tab").unwrap_or(-100);
    // Both should match
    assert!(score_new > -100);
    assert!(score_next > -100);
}

#[test]
fn enter_fonts_mode_switches_to_font_list() {
    let mut palette = CommandPalette::new();
    palette.set_enabled(true);
    palette.set_query("ab".to_string());
    palette.selected_index = 2;

    let fonts = vec![
        "JetBrains Mono".to_string(),
        "Fira Code".to_string(),
        "Cascadia Code".to_string(),
    ];
    palette.enter_fonts_mode(fonts);

    // Query cleared, selection reset, full list visible.
    assert!(palette.query.is_empty());
    assert_eq!(palette.selected_index, 0);
    assert_eq!(palette.filtered_rows().len(), 3);
    // Every row is a Font row, so no executable action.
    assert!(palette.get_selected_action().is_none());
}

#[test]
fn enter_workspaces_mode_selects_daemon_workspace_target() {
    let mut palette = CommandPalette::new();
    palette.set_enabled(true);
    palette.set_query("old query".to_string());

    // A flat single-host caller (the legacy "dropdown" shape) degrades
    // to one implicit Local group via `PaletteWorkspaceEntry::local`.
    palette.enter_workspaces_mode(vec![PaletteWorkspaceEntry::local(
        "Framework".to_string(),
        "/home/parkersettle/projects/neoism".to_string(),
        "workspace-framework".to_string(),
    )]);

    assert!(palette.query.is_empty());
    assert_eq!(palette.get_selected_action(), None);
    assert_eq!(palette.get_selected_buffer_target(), None);
    // Initial selection snaps past the leading host header onto the
    // first real workspace row.
    assert_eq!(
        palette.get_selected_workspace_target(),
        Some(PaletteWorkspaceTarget {
            workspace_id: "workspace-framework".to_string(),
        })
    );

    // Rows: [⌂ local header], [Framework workspace], then create-workspace.
    let rows = palette.filtered_rows();
    assert_eq!(rows.len(), 3);
    assert!(matches!(rows[0].1, PaletteRow::WorkspaceHost { .. }));
    assert!(!rows[0].1.is_selectable());
    assert!(matches!(rows[1].1, PaletteRow::Workspace { .. }));
    assert!(rows[1].1.is_selectable());
    assert!(matches!(rows[2].1, PaletteRow::WorkspaceCreate));
    assert!(rows[2].1.is_selectable());
}

#[test]
fn workspaces_group_under_their_host_headers() {
    let mut palette = CommandPalette::new();
    palette.enter_workspaces_mode(vec![
        ws_entry(
            "Alpha",
            "w1",
            "local",
            "framework",
            HostKind::Local,
            None,
            true,
        ),
        ws_entry(
            "Bravo",
            "w2",
            "mac",
            "mac",
            HostKind::Remote,
            Some("ws://100.64.0.2:7878/session"),
            true,
        ),
        ws_entry(
            "Charlie",
            "w3",
            "local",
            "framework",
            HostKind::Local,
            None,
            true,
        ),
    ]);

    let rows = palette.filtered_rows();
    // Two host groups, first-seen order: local (Alpha, Charlie) then mac (Bravo).
    let labels_kinds: Vec<(&str, bool)> = rows
        .iter()
        .map(|(_, row)| (row.title(), row.is_selectable()))
        .collect();
    assert_eq!(
        labels_kinds,
        vec![
            ("framework", false), // host header
            ("Alpha", true),
            ("Charlie", true),
            ("mac", false), // host header
            ("Bravo", true),
            ("+", true),
        ]
    );

    // Each workspace lands under the right header: Bravo's header is the
    // `mac` one (the row immediately preceding it that is a host).
    let bravo_idx = rows.iter().position(|(_, r)| r.title() == "Bravo").unwrap();
    let header_above = rows[..bravo_idx]
        .iter()
        .rev()
        .find(|(_, r)| matches!(r, PaletteRow::WorkspaceHost { .. }))
        .map(|(_, r)| r.title());
    assert_eq!(header_above, Some("mac"));
}

#[test]
fn host_header_carries_kind_icon_online_and_daemon_url() {
    let mut palette = CommandPalette::new();
    palette.enter_workspaces_mode(vec![
        ws_entry(
            "Local1",
            "w1",
            "local",
            "framework",
            HostKind::Local,
            None,
            true,
        ),
        ws_entry(
            "Cloudy",
            "w2",
            "cloud-burst",
            "burst",
            HostKind::Cloud,
            Some("ws://10.0.0.9:7878/session"),
            false,
        ),
    ]);
    let rows = palette.filtered_rows();

    // Local header: ⌂ glyph, online, no daemon_url in the slot.
    let local_header = rows
        .iter()
        .find_map(|(_, r)| match r {
            PaletteRow::WorkspaceHost {
                kind,
                online,
                daemon_url,
                ..
            } if r.title() == "framework" => Some((*kind, *online, *daemon_url)),
            _ => None,
        })
        .unwrap();
    assert_eq!(local_header.0.icon(), "\u{2302}"); // ⌂
    assert!(local_header.1);
    assert_eq!(local_header.2, None);

    // Cloud header: ☁ glyph, offline, daemon_url surfaced in shortcut slot.
    let cloud_header = rows
        .iter()
        .find(|(_, r)| r.title() == "burst")
        .map(|(_, r)| r)
        .unwrap();
    match cloud_header {
        PaletteRow::WorkspaceHost {
            kind,
            online,
            daemon_url,
            ..
        } => {
            assert_eq!(kind.icon(), "\u{2601}"); // ☁
            assert!(!online);
            assert_eq!(*daemon_url, Some("ws://10.0.0.9:7878/session"));
        }
        _ => panic!("expected host header"),
    }
    assert_eq!(cloud_header.shortcut(), "ws://10.0.0.9:7878/session");
}

#[test]
fn workspaces_fuzzy_filters_across_hosts_and_titles() {
    let entries = vec![
        ws_entry(
            "Editor",
            "w1",
            "local",
            "framework",
            HostKind::Local,
            None,
            true,
        ),
        ws_entry(
            "Notes",
            "w2",
            "local",
            "framework",
            HostKind::Local,
            None,
            true,
        ),
        ws_entry(
            "Editor",
            "w3",
            "mac",
            "macbook",
            HostKind::Remote,
            Some("ws://x/session"),
            true,
        ),
    ];

    // Query matching a workspace title keeps only matching children,
    // dropping a group that has no match.
    let mut palette = CommandPalette::new();
    palette.enter_workspaces_mode(entries.clone());
    palette.set_query("notes".to_string());
    let titles: Vec<&str> = palette
        .filtered_rows()
        .iter()
        .map(|(_, r)| r.title())
        .collect();
    // framework header kept (has Notes), mac header dropped (no match).
    assert_eq!(titles, vec!["framework", "Notes", "+"]);

    // Query matching a HOST label keeps the whole group (all its
    // workspaces), even when the workspace titles don't match.
    let mut palette = CommandPalette::new();
    palette.enter_workspaces_mode(entries);
    palette.set_query("macbook".to_string());
    let titles: Vec<&str> = palette
        .filtered_rows()
        .iter()
        .map(|(_, r)| r.title())
        .collect();
    assert_eq!(titles, vec!["macbook", "Editor", "+"]);
}

#[test]
fn workspaces_selection_skips_host_headers() {
    let mut palette = CommandPalette::new();
    palette.set_enabled(true);
    palette.enter_workspaces_mode(vec![
        ws_entry(
            "Alpha",
            "w1",
            "local",
            "framework",
            HostKind::Local,
            None,
            true,
        ),
        ws_entry(
            "Bravo",
            "w2",
            "mac",
            "mac",
            HostKind::Remote,
            Some("ws://x/session"),
            true,
        ),
    ]);

    // Initial selection is on Alpha (index 1), not the local header.
    assert_eq!(
        palette
            .get_selected_workspace_target()
            .map(|t| t.workspace_id),
        Some("w1".to_string())
    );

    // Moving down jumps past the `mac` header straight onto Bravo.
    palette.move_selection_down();
    assert_eq!(
        palette
            .get_selected_workspace_target()
            .map(|t| t.workspace_id),
        Some("w2".to_string())
    );
    let rows = palette.filtered_rows();
    assert!(rows[palette.selected_index].1.is_selectable());

    // Moving back up lands on Alpha again, never on a header.
    palette.move_selection_up();
    assert_eq!(
        palette
            .get_selected_workspace_target()
            .map(|t| t.workspace_id),
        Some("w1".to_string())
    );
}

#[test]
fn fonts_mode_filters_by_fuzzy_score() {
    let mut palette = CommandPalette::new();
    palette.enter_fonts_mode(vec![
        "JetBrains Mono".to_string(),
        "Fira Code".to_string(),
        "Cascadia Code".to_string(),
    ]);
    palette.set_query("cas".to_string());
    let filtered = palette.filtered_rows();
    assert!(filtered.iter().any(|(_, r)| r.title() == "Cascadia Code"));
    assert!(filtered.iter().all(|(_, r)| {
        r.title().to_lowercase().contains('c')
            && r.title().to_lowercase().contains('a')
            && r.title().to_lowercase().contains('s')
    }));
}

#[test]
fn fonts_mode_row_has_no_shortcut_column() {
    let mut palette = CommandPalette::new();
    palette.enter_fonts_mode(vec!["Fira Code".to_string()]);
    let filtered = palette.filtered_rows();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].1.shortcut(), "");
}

#[test]
fn set_enabled_resets_fonts_mode_to_commands() {
    // Re-opening the palette with the keyboard must drop any stale
    // font list — reopening otherwise would land the user on fonts
    // they saw yesterday, which is surprising.
    let mut palette = CommandPalette::new();
    palette.enter_fonts_mode(vec!["Fira Code".to_string()]);
    palette.enabled = true;
    palette.set_enabled(false);
    palette.set_enabled(true);
    assert!(matches!(palette.mode, PaletteMode::Commands));
    // Commands list is back (non-empty modulo adaptive-theme filter).
    assert!(!palette.filtered_rows().is_empty());
}

#[test]
fn get_selected_font_returns_family_in_fonts_mode() {
    let mut palette = CommandPalette::new();
    palette.enter_fonts_mode(vec!["JetBrains Mono".to_string(), "Fira Code".to_string()]);
    // First row (sorted alphabetically by fuzzy_score tie-break:
    // both score 0 with empty query, so first-inserted wins).
    let selected = palette.get_selected_font();
    assert!(selected.is_some());
    // The returned name must be one of the inputs, irrespective
    // of fuzzy-sort ordering.
    let s = selected.unwrap();
    assert!(s == "JetBrains Mono" || s == "Fira Code");
}

#[test]
fn get_selected_font_none_in_commands_mode() {
    let palette = CommandPalette::new();
    // Default mode is Commands; no font to copy.
    assert!(palette.get_selected_font().is_none());
}

#[test]
fn enter_themes_mode_filters_and_returns_selected_theme() {
    let mut palette = CommandPalette::new();
    palette.enter_themes_mode(vec![
        "pastel_dark".to_string(),
        "nvchad_one".to_string(),
        "tokyo_night".to_string(),
        "catppuccin_mocha".to_string(),
    ]);
    palette.set_query("tokyo".to_string());
    let filtered = palette.filtered_rows();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].1.title(), "tokyo_night");
    assert_eq!(filtered[0].1.shortcut(), "theme");
    assert_eq!(
        palette.get_selected_theme(),
        Some("tokyo_night".to_string())
    );
    assert!(palette.get_selected_action().is_none());
}

#[test]
fn enter_shaders_mode_filters_and_returns_selected_shader() {
    let mut palette = CommandPalette::new();
    palette.enter_shaders_mode(vec![
        PaletteShaderEntry {
            title: "None".to_string(),
            detail: "Disable shaders".to_string(),
            filter: None,
        },
        PaletteShaderEntry {
            title: "Classic CRT TV".to_string(),
            detail: "Browser CRT approximation".to_string(),
            filter: Some("crt_curve".to_string()),
        },
    ]);
    palette.set_query("crt".to_string());
    let filtered = palette.filtered_rows();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].1.title(), "Classic CRT TV");
    assert_eq!(filtered[0].1.shortcut(), "Browser CRT approximation");
    assert_eq!(
        palette.get_selected_shader().map(|shader| shader.filter),
        Some(Some("crt_curve".to_string()))
    );
    assert!(palette.get_selected_action().is_none());
}

#[test]
fn get_selected_font_none_when_empty_filter() {
    let mut palette = CommandPalette::new();
    palette.enter_fonts_mode(vec!["Fira Code".to_string()]);
    palette.set_query("zzzz".to_string());
    // Query doesn't match anything → no selected font.
    assert!(palette.get_selected_font().is_none());
}

// Scrollbar geometry + fade math live in `renderer::scrollbar` and
// are tested there. The tests below cover the palette's own contract:
// the scrollbar only surfaces after the user actually scrolls, and
// resets when the list reshapes.

#[test]
fn scrollbar_hidden_until_first_scroll() {
    // Long list, palette just opened — no scroll event has happened,
    // so the fade timer is `None` and the scrollbar stays invisible
    // despite the list being taller than the visible window.
    let mut palette = CommandPalette::new();
    palette.enter_fonts_mode((0..50).map(|i| format!("Family {i:02}")).collect());
    assert!(palette.last_scroll_time.is_none());
}

#[test]
fn scrollbar_triggered_only_when_offset_actually_changes() {
    // The first few `move_selection_down` calls don't change
    // `scroll_offset` (selection walks within the visible window).
    // Only when selection crosses the window boundary does
    // `scroll_offset` bump, and only then does the scrollbar wake.
    let mut palette = CommandPalette::new();
    palette.enter_fonts_mode((0..50).map(|i| format!("Family {i:02}")).collect());
    for _ in 0..MAX_VISIBLE_RESULTS {
        palette.move_selection_down();
    }
    // At this point selection has just crossed into scroll territory.
    assert!(palette.last_scroll_time.is_some());
}

#[test]
fn scrollbar_timer_reset_on_query_change() {
    // Typing re-filters the list, which can shrink it below the
    // visible window. Any stale scrollbar timer must clear so a
    // leftover thumb doesn't linger over the new short list.
    let mut palette = CommandPalette::new();
    palette.enter_fonts_mode((0..50).map(|i| format!("Family {i:02}")).collect());
    for _ in 0..MAX_VISIBLE_RESULTS {
        palette.move_selection_down();
    }
    assert!(palette.last_scroll_time.is_some());
    palette.set_query("Family 00".to_string());
    assert!(palette.last_scroll_time.is_none());
}

#[test]
fn scrollbar_timer_reset_on_palette_reopen() {
    // Closing and re-opening the palette must drop any lingering
    // scrollbar state so the user doesn't see a fading thumb on a
    // fresh palette.
    let mut palette = CommandPalette::new();
    palette.enter_fonts_mode((0..50).map(|i| format!("Family {i:02}")).collect());
    for _ in 0..MAX_VISIBLE_RESULTS {
        palette.move_selection_down();
    }
    palette.set_enabled(false);
    palette.set_enabled(true);
    assert!(palette.last_scroll_time.is_none());
}

#[test]
fn list_fonts_command_is_present_and_actionable() {
    // Confirms `List Fonts` shows up in the command list and
    // reports the correct action when selected.
    let mut palette = CommandPalette::new();
    palette.set_query("list fonts".to_string());
    let filtered = palette.filtered_rows();
    assert!(!filtered.is_empty());
    assert_eq!(filtered[0].1.title(), "List Fonts");
    palette.selected_index = 0;
    assert_eq!(
        palette.get_selected_action(),
        Some(PaletteAction::ListFonts)
    );
}

#[test]
fn notebook_commands_are_visible_only_on_notebook_surface() {
    let mut palette = CommandPalette::new();
    palette.set_surface(PaletteSurface::Notebook);
    palette.set_query("run all cells".to_string());
    let filtered = palette.filtered_rows();
    assert!(!filtered.is_empty());
    assert_eq!(filtered[0].1.title(), "Run All Cells");
    palette.selected_index = 0;
    assert_eq!(
        palette.get_selected_action(),
        Some(PaletteAction::RunAllNotebookCells)
    );

    palette.set_query("interrupt kernel".to_string());
    let filtered = palette.filtered_rows();
    assert!(!filtered.is_empty());
    assert_eq!(filtered[0].1.title(), "Interrupt Kernel");
    palette.selected_index = 0;
    assert_eq!(
        palette.get_selected_action(),
        Some(PaletteAction::InterruptNotebookKernel)
    );

    palette.set_query("insert code cell below".to_string());
    let filtered = palette.filtered_rows();
    assert!(!filtered.is_empty());
    assert_eq!(filtered[0].1.title(), "Insert Code Cell Below");
    palette.selected_index = 0;
    assert_eq!(
        palette.get_selected_action(),
        Some(PaletteAction::InsertNotebookCodeCellBelow)
    );

    palette.set_query("delete current cell".to_string());
    let filtered = palette.filtered_rows();
    assert!(!filtered.is_empty());
    assert_eq!(filtered[0].1.title(), "Delete Current Cell");
    palette.selected_index = 0;
    assert_eq!(
        palette.get_selected_action(),
        Some(PaletteAction::DeleteNotebookCell)
    );

    palette.set_surface(PaletteSurface::Markdown);
    palette.set_query("insert code cell below".to_string());
    assert!(palette.filtered_rows().is_empty());
}

#[test]
fn shaders_command_is_present_and_actionable() {
    let mut palette = CommandPalette::new();
    palette.set_query("shaders".to_string());
    let filtered = palette.filtered_rows();
    assert!(!filtered.is_empty());
    assert_eq!(filtered[0].1.title(), "Shaders");
    palette.selected_index = 0;
    assert_eq!(
        palette.get_selected_action(),
        Some(PaletteAction::OpenShaders)
    );
}

#[test]
fn search_finder_commands_are_present_and_actionable() {
    let mut palette = CommandPalette::new();
    palette.set_query("search files".to_string());
    let filtered = palette.filtered_rows();
    assert!(!filtered.is_empty());
    assert_eq!(filtered[0].1.title(), "Search Files");
    palette.selected_index = 0;
    assert_eq!(
        palette.get_selected_action(),
        Some(PaletteAction::SearchFiles)
    );

    palette.set_query("search words".to_string());
    let filtered = palette.filtered_rows();
    assert!(!filtered.is_empty());
    assert_eq!(filtered[0].1.title(), "Search Words");
    palette.selected_index = 0;
    assert_eq!(
        palette.get_selected_action(),
        Some(PaletteAction::SearchWords)
    );

    palette.set_query("git changes".to_string());
    let filtered = palette.filtered_rows();
    assert!(!filtered.is_empty());
    assert_eq!(filtered[0].1.title(), "Search Git Changes");
    palette.selected_index = 0;
    assert_eq!(
        palette.get_selected_action(),
        Some(PaletteAction::SearchGitChanges)
    );
}

#[test]
fn ex_mode_recommends_search_finder_commands() {
    let mut palette = CommandPalette::new();
    palette.enter_ex_mode();
    palette.set_query("search".to_string());
    let names: Vec<&str> = palette
        .filtered_rows()
        .into_iter()
        .map(|(_, row)| row.title())
        .collect();
    assert!(names.contains(&"Search Files"));
    assert!(names.contains(&"Search Words"));
    assert!(names.contains(&"Search Git Changes"));
}

// ---------------------------------------------------------------------
// 5D-drag: drag a workspace row onto a host header to move it.
// ---------------------------------------------------------------------

const WIN_W: f32 = 1200.0;
const SCALE: f32 = 1.0;

/// Two-host Workspaces tree used by the drag tests. Filtered rows:
///   0: `framework` header (Local)
///   1: Alpha  (w1, local)
///   2: Charlie (w3, local)
///   3: `mac` header (Remote, daemon_url)
///   4: Bravo  (w2, mac)
fn drag_palette() -> CommandPalette {
    let mut palette = CommandPalette::new();
    palette.enter_workspaces_mode(vec![
        ws_entry(
            "Alpha",
            "w1",
            "local",
            "framework",
            HostKind::Local,
            None,
            true,
        ),
        ws_entry(
            "Charlie",
            "w3",
            "local",
            "framework",
            HostKind::Local,
            None,
            true,
        ),
        ws_entry(
            "Bravo",
            "w2",
            "mac",
            "mac",
            HostKind::Remote,
            Some("ws://100.64.0.2:7878/session"),
            true,
        ),
    ]);
    palette
}

#[test]
fn drag_workspace_onto_other_host_emits_move_intent() {
    let mut palette = drag_palette();

    // Press on Alpha (a local workspace, filtered row 1).
    let (px, py) = palette.row_center_coords(1, WIN_W, SCALE);
    assert!(palette.workspace_drag_press(px, py, WIN_W, SCALE));
    // Armed but not yet active — a press alone isn't a drag.
    assert!(!palette.is_dragging_workspace());

    // Move down onto the `mac` host header (filtered row 3), well past
    // the activation threshold.
    let (hx, hy) = palette.row_center_coords(3, WIN_W, SCALE);
    assert!(palette.workspace_drag_move(hx, hy, WIN_W, SCALE));
    assert!(palette.is_dragging_workspace());
    // The hovered host header is the live drop target.
    assert_eq!(palette.workspace_drag_drop_host_id(), Some("mac"));

    // Release over the `mac` header → MoveWorkspaceToHost(w1 → mac).
    let (was_active, action) = palette.workspace_drag_release(hx, hy, WIN_W, SCALE);
    assert!(was_active);
    assert_eq!(
        action,
        Some(PaletteAction::MoveWorkspaceToHost {
            workspace_id: "w1".to_string(),
            target_host_id: "mac".to_string(),
            target_daemon_url: Some("ws://100.64.0.2:7878/session".to_string()),
            target_is_local: false,
        })
    );
    // Drag state cleared after release.
    assert!(!palette.is_dragging_workspace());
}

#[test]
fn drag_remote_workspace_onto_local_host_marks_target_local() {
    let mut palette = drag_palette();

    // Press on Bravo (the remote `mac` workspace, filtered row 4).
    let (px, py) = palette.row_center_coords(4, WIN_W, SCALE);
    assert!(palette.workspace_drag_press(px, py, WIN_W, SCALE));

    // Drag up onto the local `framework` header (filtered row 0).
    let (hx, hy) = palette.row_center_coords(0, WIN_W, SCALE);
    assert!(palette.workspace_drag_move(hx, hy, WIN_W, SCALE));
    assert_eq!(palette.workspace_drag_drop_host_id(), Some("local"));

    let (was_active, action) = palette.workspace_drag_release(hx, hy, WIN_W, SCALE);
    assert!(was_active);
    // Local target → 5D-wire reads this as a demote.
    assert_eq!(
        action,
        Some(PaletteAction::MoveWorkspaceToHost {
            workspace_id: "w2".to_string(),
            target_host_id: "local".to_string(),
            target_daemon_url: None,
            target_is_local: true,
        })
    );
}

#[test]
fn drag_release_on_source_host_is_a_noop() {
    let mut palette = drag_palette();

    // Press + drag Alpha (local) and drop it back on its OWN `framework`
    // (local) header (row 0) — it didn't move anywhere.
    let (px, py) = palette.row_center_coords(1, WIN_W, SCALE);
    assert!(palette.workspace_drag_press(px, py, WIN_W, SCALE));
    let (hx, hy) = palette.row_center_coords(0, WIN_W, SCALE);
    assert!(palette.workspace_drag_move(hx, hy, WIN_W, SCALE));
    assert!(palette.is_dragging_workspace());

    let (was_active, action) = palette.workspace_drag_release(hx, hy, WIN_W, SCALE);
    // The gesture was a real drag (so the host suppresses the click)…
    assert!(was_active);
    // …but dropping on the source host emits nothing.
    assert_eq!(action, None);
}

#[test]
fn drag_release_off_any_header_is_a_noop() {
    let mut palette = drag_palette();

    // Press + drag Alpha, then release over another *workspace* row
    // (Bravo, row 4) — not a host header → cancel.
    let (px, py) = palette.row_center_coords(1, WIN_W, SCALE);
    assert!(palette.workspace_drag_press(px, py, WIN_W, SCALE));
    let (wx, wy) = palette.row_center_coords(4, WIN_W, SCALE);
    assert!(palette.workspace_drag_move(wx, wy, WIN_W, SCALE));
    assert!(palette.is_dragging_workspace());
    // No host under the cursor → no drop target highlighted.
    assert_eq!(palette.workspace_drag_drop_host_id(), None);

    let (was_active, action) = palette.workspace_drag_release(wx, wy, WIN_W, SCALE);
    assert!(was_active);
    assert_eq!(action, None);
}

#[test]
fn plain_click_on_workspace_is_not_a_drag() {
    let mut palette = drag_palette();

    // Press on Alpha and release at (almost) the same point — never
    // crosses the activation threshold.
    let (px, py) = palette.row_center_coords(1, WIN_W, SCALE);
    assert!(palette.workspace_drag_press(px, py, WIN_W, SCALE));
    // A sub-threshold move keeps the drag dormant.
    assert!(!palette.workspace_drag_move(px + 1.0, py + 1.0, WIN_W, SCALE));
    assert!(!palette.is_dragging_workspace());

    // Release: reported as NOT an active drag, so the host runs its
    // normal click-to-switch path. No move intent is produced.
    let (was_active, action) =
        palette.workspace_drag_release(px + 1.0, py + 1.0, WIN_W, SCALE);
    assert!(!was_active);
    assert_eq!(action, None);
    // The pressed workspace is still the selected switch target.
    assert_eq!(
        palette
            .get_selected_workspace_target()
            .map(|t| t.workspace_id),
        Some("w1".to_string())
    );
}

#[test]
fn press_on_host_header_does_not_arm_a_drag() {
    let mut palette = drag_palette();
    // The `framework` host header is filtered row 0 — pressing it must
    // not start a drag (only workspace child rows are draggable).
    let (hx, hy) = palette.row_center_coords(0, WIN_W, SCALE);
    assert!(!palette.workspace_drag_press(hx, hy, WIN_W, SCALE));
    assert!(!palette.is_dragging_workspace());
}

#[test]
fn drag_press_outside_workspaces_mode_is_inert() {
    // In Commands mode there are no workspace rows to drag.
    let mut palette = CommandPalette::new();
    palette.set_enabled(true);
    let (x, y) = palette.row_center_coords(0, WIN_W, SCALE);
    assert!(!palette.workspace_drag_press(x, y, WIN_W, SCALE));
}

#[test]
fn closing_palette_clears_in_flight_drag() {
    let mut palette = drag_palette();
    let (px, py) = palette.row_center_coords(1, WIN_W, SCALE);
    assert!(palette.workspace_drag_press(px, py, WIN_W, SCALE));
    let (hx, hy) = palette.row_center_coords(3, WIN_W, SCALE);
    palette.workspace_drag_move(hx, hy, WIN_W, SCALE);
    assert!(palette.is_dragging_workspace());

    palette.set_enabled(false);
    assert!(!palette.is_dragging_workspace());
    // A release after close is a clean no-op.
    let (was_active, action) = palette.workspace_drag_release(hx, hy, WIN_W, SCALE);
    assert!(!was_active);
    assert_eq!(action, None);
}

// ---------------------------------------------------------------------
// Wave 6A: tailnet peers as header-only drop targets in the tree.
// ---------------------------------------------------------------------

use super::actions::PaletteHostEntry;

fn peer_host(host_id: &str, label: &str, url: &str, online: bool) -> PaletteHostEntry {
    PaletteHostEntry {
        host_id: host_id.to_string(),
        label: label.to_string(),
        kind: HostKind::Remote,
        daemon_url: Some(url.to_string()),
        online,
    }
}

/// `drag_palette()` plus two tailnet peers without any workspaces:
///   5: `pi` header   (Remote, online)
///   6: `nas` header  (Remote, offline)
fn drag_palette_with_peers() -> CommandPalette {
    let mut palette = CommandPalette::new();
    palette.enter_workspaces_mode_with_hosts(
        vec![
            ws_entry(
                "Alpha",
                "w1",
                "local",
                "framework",
                HostKind::Local,
                None,
                true,
            ),
            ws_entry(
                "Charlie",
                "w3",
                "local",
                "framework",
                HostKind::Local,
                None,
                true,
            ),
            ws_entry(
                "Bravo",
                "w2",
                "mac",
                "mac",
                HostKind::Remote,
                Some("ws://100.64.0.2:7878/session"),
                true,
            ),
        ],
        vec![
            peer_host("tailnet:pi", "pi", "ws://100.64.0.7:7878/session", true),
            peer_host("tailnet:nas", "nas", "ws://100.64.0.9:7878/session", false),
        ],
    );
    palette
}

#[test]
fn peer_hosts_render_as_trailing_header_only_rows() {
    let palette = drag_palette_with_peers();
    let rows = palette.filtered_rows();
    assert_eq!(rows.len(), 8);
    let peer_hosts = rows
        .iter()
        .filter_map(|(_, row)| match row {
            PaletteRow::WorkspaceHost {
                host_id,
                label,
                kind,
                daemon_url,
                online,
            } if host_id.starts_with("tailnet:") => {
                Some((*host_id, *label, *kind, *daemon_url, *online, row))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(peer_hosts.len(), 2);
    let pi = peer_hosts
        .iter()
        .find(|(host_id, _, _, _, _, _)| *host_id == "tailnet:pi")
        .expect("pi peer host row");
    assert_eq!(pi.1, "pi");
    assert_eq!(pi.2, HostKind::Remote);
    assert_eq!(pi.3, Some("ws://100.64.0.7:7878/session"));
    assert!(pi.4);
    let nas = peer_hosts
        .iter()
        .find(|(host_id, _, _, _, _, _)| *host_id == "tailnet:nas")
        .expect("nas peer host row");
    assert!(!nas.4);
    // Header-only peers are separators, never the selection cursor.
    assert!(peer_hosts
        .iter()
        .all(|(_, _, _, _, _, row)| !row.is_selectable()));
}

#[test]
fn peer_hosts_fuzzy_filter_by_label() {
    let mut palette = drag_palette_with_peers();
    palette.set_query("pi".to_string());
    let rows = palette.filtered_rows();
    let labels: Vec<&str> = rows.iter().map(|(_, row)| row.title()).collect();
    assert!(labels.contains(&"pi"));
    assert!(!labels.contains(&"nas"));
}

#[test]
fn peer_host_with_id_already_in_tree_is_deduped() {
    let mut palette = CommandPalette::new();
    palette.enter_workspaces_mode_with_hosts(
        vec![ws_entry(
            "Bravo",
            "w2",
            "mac",
            "mac",
            HostKind::Remote,
            Some("ws://100.64.0.2:7878/session"),
            true,
        )],
        vec![peer_host(
            "mac",
            "mac",
            "ws://100.64.0.2:7878/session",
            true,
        )],
    );
    let rows = palette.filtered_rows();
    // One `mac` header + Bravo + create-workspace — no duplicate trailing header.
    assert_eq!(rows.len(), 3);
}

#[test]
fn drag_workspace_onto_peer_host_emits_promote_intent() {
    let mut palette = drag_palette_with_peers();

    // Press on Alpha (row 1), drag onto the `pi` peer header (row 5).
    let (px, py) = palette.row_center_coords(1, WIN_W, SCALE);
    assert!(palette.workspace_drag_press(px, py, WIN_W, SCALE));
    let (hx, hy) = palette.row_center_coords(5, WIN_W, SCALE);
    assert!(palette.workspace_drag_move(hx, hy, WIN_W, SCALE));
    assert_eq!(palette.workspace_drag_drop_host_id(), Some("tailnet:pi"));

    let (was_active, action) = palette.workspace_drag_release(hx, hy, WIN_W, SCALE);
    assert!(was_active);
    assert_eq!(
        action,
        Some(PaletteAction::MoveWorkspaceToHost {
            workspace_id: "w1".to_string(),
            target_host_id: "tailnet:pi".to_string(),
            target_daemon_url: Some("ws://100.64.0.7:7878/session".to_string()),
            target_is_local: false,
        })
    );
}

#[test]
fn offline_peer_host_is_not_a_drop_target() {
    let mut palette = drag_palette_with_peers();

    // Press on Alpha (row 1), drag onto the OFFLINE `nas` header (row 6).
    let (px, py) = palette.row_center_coords(1, WIN_W, SCALE);
    assert!(palette.workspace_drag_press(px, py, WIN_W, SCALE));
    let (hx, hy) = palette.row_center_coords(6, WIN_W, SCALE);
    assert!(palette.workspace_drag_move(hx, hy, WIN_W, SCALE));
    assert!(palette.is_dragging_workspace());
    // Unreachable host → never highlighted as a drop target…
    assert_eq!(palette.workspace_drag_drop_host_id(), None);

    // …and releasing on it is a cancel, not a doomed promote.
    let (was_active, action) = palette.workspace_drag_release(hx, hy, WIN_W, SCALE);
    assert!(was_active);
    assert_eq!(action, None);
}

#[test]
fn workspace_move_feedback_lifecycle() {
    let mut palette = CommandPalette::default();
    assert!(!palette.tick_workspace_move());

    palette.begin_workspace_move("w1".to_string(), "tailnet:pi".to_string());
    let status = palette.workspace_move_status().expect("in-flight status");
    assert_eq!(status.target_host_id, "tailnet:pi");
    assert_eq!(status.phase, WorkspaceMovePhase::InFlight);
    // Spinner keeps the host redrawing.
    assert!(palette.tick_workspace_move());

    palette.finish_workspace_move(false, "promote requires a git remote");
    let status = palette.workspace_move_status().expect("done status");
    assert_eq!(
        status.phase,
        WorkspaceMovePhase::Done {
            ok: false,
            message: "promote requires a git remote".to_string(),
        }
    );
    // Result still on screen → still redrawing; it ages out after the
    // TTL (not simulated here — expiry math is `since.elapsed()`).
    assert!(palette.tick_workspace_move());
    assert!(palette.workspace_move_status().is_some());

    // Closing and reopening the palette keeps the visible result.
    palette.set_enabled(false);
    assert!(palette.workspace_move_status().is_some());
}
