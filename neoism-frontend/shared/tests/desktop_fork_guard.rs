use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<repo>/neoism-frontend/shared`; the repo root
    // is two levels up.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("neoism-frontend/shared should live two levels below the repo root")
        .to_path_buf()
}

fn read_repo_file(path: &str) -> String {
    fs::read_to_string(repo_root().join(path))
        .unwrap_or_else(|err| panic!("failed to read {path}: {err}"))
}

fn line_count(path: &str) -> usize {
    read_repo_file(path).lines().count()
}

fn assert_contains(path: &str, needle: &str) {
    let text = read_repo_file(path);
    assert!(
        text.contains(needle),
        "{path} should remain shared-backed and contain marker {needle:?}"
    );
}

#[test]
fn desktop_agent_view_stays_a_thin_shared_adapter() {
    let root = repo_root();
    for deleted in [
        "neoism-frontend/desktop/src/neoism/view/assistant.rs",
        "neoism-frontend/desktop/src/neoism/view/tool_message.rs",
        "neoism-frontend/desktop/src/neoism/view/wordmark.rs",
    ] {
        assert!(
            !root.join(deleted).exists(),
            "{deleted} was folded into shared adapter macros and must not return"
        );
    }

    let view_dir = root.join("neoism-frontend/desktop/src/neoism/view");
    let total_lines: usize = fs::read_dir(&view_dir)
        .expect("desktop agent view dir should exist")
        .map(|entry| entry.expect("valid dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .map(|path| fs::read_to_string(path).expect("view file should be readable"))
        .map(|text| text.lines().count())
        .sum();
    assert!(
        total_lines <= 400,
        "desktop agent view adapter layer regrew to {total_lines} lines"
    );

    for (path, max_lines, marker) in [
        (
            "neoism-frontend/desktop/src/neoism/view/chat.rs",
            15,
            "neoism_ui_impl_agent_chat_pane",
        ),
        (
            "neoism-frontend/desktop/src/neoism/view/code_block.rs",
            30,
            "agent_pane::view::code_block",
        ),
        (
            "neoism-frontend/desktop/src/neoism/view/draw.rs",
            10,
            "agent_pane::view::draw",
        ),
        (
            "neoism-frontend/desktop/src/neoism/view/home.rs",
            15,
            "neoism_ui_impl_agent_home_pane",
        ),
        (
            "neoism-frontend/desktop/src/neoism/view/markdown.rs",
            60,
            "AgentMarkdownPane",
        ),
        (
            "neoism-frontend/desktop/src/neoism/view/message_card.rs",
            120,
            "render_message_card_with",
        ),
        (
            "neoism-frontend/desktop/src/neoism/view/mod.rs",
            130,
            "render_agent_pane_with",
        ),
        (
            "neoism-frontend/desktop/src/neoism/view/timeline.rs",
            50,
            "neoism_ui_impl_agent_timeline_pane",
        ),
    ] {
        assert!(
            line_count(path) <= max_lines,
            "{path} exceeded adapter cap {max_lines}"
        );
        assert_contains(path, marker);
    }
}

#[test]
fn desktop_agent_picker_and_side_panel_are_reexport_shims() {
    for (path, marker) in [
        (
            "neoism-frontend/desktop/src/neoism/agent/picker.rs",
            "neoism_ui::panels::agent_pane::state::picker",
        ),
        (
            "neoism-frontend/desktop/src/neoism/agent/side_panel.rs",
            "neoism_ui::panels::agent_pane::state::side_panel",
        ),
    ] {
        assert!(
            line_count(path) <= 35,
            "{path} should stay a small shared re-export shim"
        );
        assert_contains(path, marker);
    }
}

#[test]
fn desktop_file_tree_stays_a_native_io_adapter_over_shared_state() {
    let root = repo_root();
    let file_tree_dir = root.join("neoism-frontend/desktop/src/editor/file_tree");
    let adapter_lines: usize = fs::read_dir(&file_tree_dir)
        .expect("desktop file tree dir should exist")
        .map(|entry| entry.expect("valid dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .filter(|path| path.file_name().is_none_or(|name| name != "tests.rs"))
        .map(|path| fs::read_to_string(path).expect("file_tree file should be readable"))
        .map(|text| text.lines().count())
        .sum();
    assert!(
        adapter_lines <= 550,
        "desktop file_tree adapter layer regrew to {adapter_lines} lines"
    );

    for (path, max_lines, marker) in [
        (
            "neoism-frontend/desktop/src/editor/file_tree/git.rs",
            110,
            "neoism_ui::panels::file_tree",
        ),
        (
            "neoism-frontend/desktop/src/editor/file_tree/icons.rs",
            20,
            "neoism_ui::panels::file_tree::icons",
        ),
        (
            "neoism-frontend/desktop/src/editor/file_tree/render.rs",
            60,
            "self.inner.render",
        ),
        (
            "neoism-frontend/desktop/src/editor/file_tree/scan.rs",
            90,
            "neoism_ui::panels::file_tree",
        ),
        (
            "neoism-frontend/desktop/src/editor/file_tree/state.rs",
            60,
            "shared_file_tree::FileTree",
        ),
        (
            "neoism-frontend/desktop/src/editor/file_tree/update.rs",
            80,
            "with_native_panel_context",
        ),
    ] {
        assert!(
            line_count(path) <= max_lines,
            "{path} exceeded adapter cap {max_lines}"
        );
        assert_contains(path, marker);
    }
}
