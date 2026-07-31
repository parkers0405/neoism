#!/usr/bin/env bash
# check_no_ui_in_desktop_fork.sh
#
# Guard against the desktop fork (`neoism-frontend/desktop/src/`) regrowing the
# chrome/UI code that has been migrated into the shared `neoism-frontend/shared` crate.
#
# Rule of thumb: any new `.rs` file under the paths listed in `BANNED_*`
# below must instead live under `neoism-frontend/shared/src/`. The desktop fork keeps
# only the host-IO shims (file-system walkers, pty plumbing, libc-only
# helpers, etc.) that the shared crate hands off to via the service
# traits — everything that renders or composes UI belongs in the shared
# crate so the native and web frontends produce identical chrome.
#
# How the guard works:
#   1. Collects every `.rs` path the working tree knows about
#      (tracked + untracked, but ignoring `target/`).
#   2. Splits the desktop fork into two buckets:
#        - Fully banned dirs/files: ANY presence is a failure
#          (e.g. `neoism-frontend/desktop/src/chrome/**`).
#        - Mixed dirs: an allowlist snapshot pins which files may
#          remain; any other `.rs` file under that dir is a failure.
#   3. Prints a single message per violation pointing at the shared crate
#      and exits non-zero so `make`, CI, or a pre-commit hook can act.
#
# How to bypass (only when you know what you're doing — e.g. you have
# just landed a legit new desktop-IO shim and need to update the
# allowlist in this file at the same time):
#
#   ALLOW_DESKTOP_UI=1 ./scripts/check_no_ui_in_desktop_fork.sh
#
# When you bypass, please update the ALLOWLIST_* arrays below in the
# same commit so the guard stays useful for the next contributor.
#
# Rule of thumb: shared UI lives in neoism-frontend/shared/; the desktop
# fork only wires it up.

set -u

if [[ "${ALLOW_DESKTOP_UI:-0}" == "1" ]]; then
    echo "[check_no_ui_in_desktop_fork] ALLOW_DESKTOP_UI=1 set; skipping guard." >&2
    exit 0
fi

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"
if [[ -z "${REPO_ROOT}" ]]; then
    echo "error: must be run inside the neoism git repo" >&2
    exit 2
fi
cd "${REPO_ROOT}"

DESKTOP_ROOT="neoism-frontend/desktop/src"

# ---------------------------------------------------------------------------
# Fully banned: any .rs file under these prefixes is a violation.
# ---------------------------------------------------------------------------
BANNED_PREFIXES=(
    "${DESKTOP_ROOT}/chrome/"
    "${DESKTOP_ROOT}/editor/markdown/render/"
    "${DESKTOP_ROOT}/neoism/agent/view/"
)

# Individual files that have been deleted and must not return.
BANNED_FILES=(
    "${DESKTOP_ROOT}/screen/hint.rs"
    "${DESKTOP_ROOT}/workspace/tags_view.rs"
)

# ---------------------------------------------------------------------------
# Mixed dirs: only the snapshotted allowlist may live under the prefix.
# Everything else under that prefix is a violation.
# ---------------------------------------------------------------------------

# editor/git_diff_panel/: only host-IO + module root may stay.
MIXED_PREFIX_GIT_DIFF="${DESKTOP_ROOT}/editor/git_diff_panel/"
ALLOWLIST_GIT_DIFF=(
    "${DESKTOP_ROOT}/editor/git_diff_panel/io.rs"
    "${DESKTOP_ROOT}/editor/git_diff_panel/mod.rs"
)

# terminal/blocks/: only the desktop-IO shims and the module root may stay.
# The chrome/command/echo/history/completion/shell.rs files here are
# already thin re-export shims that point at neoism_ui::terminal_blocks;
# `input.rs`, `shell.rs`, and `shell_detect.rs` carry the host-only
# plumbing (pty, libc::tcgetpgrp, dirs::data_local_dir, etc.).
MIXED_PREFIX_TERM_BLOCKS="${DESKTOP_ROOT}/terminal/blocks/"
ALLOWLIST_TERM_BLOCKS=(
    "${DESKTOP_ROOT}/terminal/blocks/mod.rs"
    "${DESKTOP_ROOT}/terminal/blocks/chrome.rs"
    "${DESKTOP_ROOT}/terminal/blocks/command.rs"
    "${DESKTOP_ROOT}/terminal/blocks/completion.rs"
    "${DESKTOP_ROOT}/terminal/blocks/echo.rs"
    "${DESKTOP_ROOT}/terminal/blocks/history.rs"
    "${DESKTOP_ROOT}/terminal/blocks/input.rs"
    "${DESKTOP_ROOT}/terminal/blocks/shell.rs"
    "${DESKTOP_ROOT}/terminal/blocks/shell_detect.rs"
)

# editor/file_tree/: native chrome lives here during the staged port to
# neoism_ui::panels::FileTree, but no NEW files may be added.
MIXED_PREFIX_FILE_TREE="${DESKTOP_ROOT}/editor/file_tree/"
ALLOWLIST_FILE_TREE=(
    "${DESKTOP_ROOT}/editor/file_tree/git.rs"
    "${DESKTOP_ROOT}/editor/file_tree/icons.rs"
    "${DESKTOP_ROOT}/editor/file_tree/mod.rs"
    "${DESKTOP_ROOT}/editor/file_tree/render.rs"
    "${DESKTOP_ROOT}/editor/file_tree/scan.rs"
    "${DESKTOP_ROOT}/editor/file_tree/state.rs"
    "${DESKTOP_ROOT}/editor/file_tree/tests.rs"
    "${DESKTOP_ROOT}/editor/file_tree/types.rs"
    "${DESKTOP_ROOT}/editor/file_tree/update.rs"
    "${DESKTOP_ROOT}/editor/file_tree/virtuals.rs"
)

# Newly ported desktop adapter layers. These files are allowed to stay
# only while they remain small bindings around shared neoism-frontend/shared code.
# If one grows past the line caps below, it is almost certainly a
# desktop-only UI fork regrowing where a shared helper/macro belongs.
MAX_AGENT_VIEW_LINES=400
MAX_FILE_TREE_ADAPTER_LINES=550

BANNED_AGENT_VIEW_FILES=(
    "${DESKTOP_ROOT}/neoism/view/assistant.rs"
    "${DESKTOP_ROOT}/neoism/view/tool_message.rs"
    "${DESKTOP_ROOT}/neoism/view/wordmark.rs"
)

SHIM_LINE_LIMITS=(
    "${DESKTOP_ROOT}/neoism/agent/picker.rs:35"
    "${DESKTOP_ROOT}/neoism/agent/side_panel.rs:35"
    "${DESKTOP_ROOT}/neoism/view/chat.rs:15"
    "${DESKTOP_ROOT}/neoism/view/code_block.rs:30"
    "${DESKTOP_ROOT}/neoism/view/draw.rs:10"
    "${DESKTOP_ROOT}/neoism/view/home.rs:15"
    "${DESKTOP_ROOT}/neoism/view/layout.rs:20"
    "${DESKTOP_ROOT}/neoism/view/markdown.rs:60"
    "${DESKTOP_ROOT}/neoism/view/message_card.rs:120"
    "${DESKTOP_ROOT}/neoism/view/mod.rs:90"
    "${DESKTOP_ROOT}/neoism/view/picker.rs:30"
    "${DESKTOP_ROOT}/neoism/view/side_panel.rs:30"
    "${DESKTOP_ROOT}/neoism/view/timeline.rs:50"
    "${DESKTOP_ROOT}/neoism/view/user_input.rs:30"
    "${DESKTOP_ROOT}/editor/file_tree/git.rs:110"
    "${DESKTOP_ROOT}/editor/file_tree/icons.rs:20"
    "${DESKTOP_ROOT}/editor/file_tree/mod.rs:280"
    "${DESKTOP_ROOT}/editor/file_tree/render.rs:60"
    "${DESKTOP_ROOT}/editor/file_tree/scan.rs:90"
    "${DESKTOP_ROOT}/editor/file_tree/state.rs:60"
    "${DESKTOP_ROOT}/editor/file_tree/types.rs:20"
    "${DESKTOP_ROOT}/editor/file_tree/update.rs:80"
    "${DESKTOP_ROOT}/editor/file_tree/virtuals.rs:20"
)

SHIM_REQUIRED_PATTERNS=(
    "${DESKTOP_ROOT}/neoism/agent/picker.rs|neoism_ui::panels::agent_pane::state::picker"
    "${DESKTOP_ROOT}/neoism/agent/side_panel.rs|neoism_ui::panels::agent_pane::state::side_panel"
    "${DESKTOP_ROOT}/neoism/view/chat.rs|neoism_ui::neoism_ui_impl_agent_chat_pane"
    "${DESKTOP_ROOT}/neoism/view/code_block.rs|neoism_ui::panels::agent_pane::view::code_block"
    "${DESKTOP_ROOT}/neoism/view/draw.rs|neoism_ui::panels::agent_pane::view::draw"
    "${DESKTOP_ROOT}/neoism/view/home.rs|neoism_ui::neoism_ui_impl_agent_home_pane"
    "${DESKTOP_ROOT}/neoism/view/layout.rs|neoism_ui::panels::agent_pane::view::layout"
    "${DESKTOP_ROOT}/neoism/view/markdown.rs|AgentMarkdownPane"
    "${DESKTOP_ROOT}/neoism/view/message_card.rs|render_message_card_with"
    "${DESKTOP_ROOT}/neoism/view/mod.rs|render_agent_pane_with"
    "${DESKTOP_ROOT}/neoism/view/picker.rs|neoism_ui::panels::agent_pane::view::picker"
    "${DESKTOP_ROOT}/neoism/view/side_panel.rs|neoism_ui::neoism_ui_impl_agent_side_panel"
    "${DESKTOP_ROOT}/neoism/view/timeline.rs|neoism_ui::neoism_ui_impl_agent_timeline_pane"
    "${DESKTOP_ROOT}/neoism/view/user_input.rs|neoism_ui::neoism_ui_impl_agent_user_input"
    "${DESKTOP_ROOT}/editor/file_tree/git.rs|neoism_ui::panels::file_tree"
    "${DESKTOP_ROOT}/editor/file_tree/icons.rs|neoism_ui::panels::file_tree::icons"
    "${DESKTOP_ROOT}/editor/file_tree/mod.rs|neoism_ui::panels::PanelContext"
    "${DESKTOP_ROOT}/editor/file_tree/render.rs|self.inner.render"
    "${DESKTOP_ROOT}/editor/file_tree/scan.rs|neoism_ui::panels::file_tree"
    "${DESKTOP_ROOT}/editor/file_tree/state.rs|shared_file_tree::FileTree"
    "${DESKTOP_ROOT}/editor/file_tree/types.rs|neoism_ui::panels::file_tree"
    "${DESKTOP_ROOT}/editor/file_tree/update.rs|with_native_panel_context"
    "${DESKTOP_ROOT}/editor/file_tree/virtuals.rs|neoism_ui::panels::file_tree"
)

# ---------------------------------------------------------------------------
# Collect candidate files: tracked + untracked, exclude ignored.
# ---------------------------------------------------------------------------
mapfile -t ALL_RS_FILES < <(
    git ls-files --cached --others --exclude-standard -- "${DESKTOP_ROOT}/*.rs" \
        | sort -u
)

VIOLATIONS=()

report() {
    local path="$1"
    VIOLATIONS+=("${path}")
    echo "Forbidden file: ${path}. UI code must live in neoism-frontend/shared/." >&2
}

report_detail() {
    local path="$1"
    local detail="$2"
    VIOLATIONS+=("${path}")
    echo "Forbidden desktop UI regression: ${path}: ${detail}. Move portable behavior to neoism-frontend/shared/." >&2
}

# Returns 0 if $1 is contained in the array named by $2.
in_array() {
    local needle="$1"
    local -n haystack="$2"
    local item
    for item in "${haystack[@]}"; do
        if [[ "${item}" == "${needle}" ]]; then
            return 0
        fi
    done
    return 1
}

for path in "${ALL_RS_FILES[@]}"; do
    # Skip any path that is no longer present on disk (e.g. tracked but
    # already deleted in the working tree — we only police live files).
    [[ -f "${path}" ]] || continue

    # 1) Fully-banned prefixes.
    matched_banned_prefix=0
    for prefix in "${BANNED_PREFIXES[@]}"; do
        if [[ "${path}" == ${prefix}* ]]; then
            report "${path}"
            matched_banned_prefix=1
            break
        fi
    done
    if (( matched_banned_prefix )); then
        continue
    fi

    # 2) Fully-banned individual files.
    if in_array "${path}" BANNED_FILES; then
        report "${path}"
        continue
    fi

    # 3) Mixed-dir allowlists.
    if [[ "${path}" == ${MIXED_PREFIX_GIT_DIFF}* ]]; then
        if ! in_array "${path}" ALLOWLIST_GIT_DIFF; then
            report "${path}"
        fi
        continue
    fi
    if [[ "${path}" == ${MIXED_PREFIX_TERM_BLOCKS}* ]]; then
        if ! in_array "${path}" ALLOWLIST_TERM_BLOCKS; then
            report "${path}"
        fi
        continue
    fi
    if [[ "${path}" == ${MIXED_PREFIX_FILE_TREE}* ]]; then
        if ! in_array "${path}" ALLOWLIST_FILE_TREE; then
            report "${path}"
        fi
        continue
    fi
done

# 4) Adapter-regrowth assertions for areas that were recently ported.
for path in "${BANNED_AGENT_VIEW_FILES[@]}"; do
    if [[ -f "${path}" ]]; then
        report_detail "${path}" "this agent-view module was folded into shared adapter macros and must not return"
    fi
done

agent_view_lines=0
while IFS= read -r path; do
    [[ -f "${path}" ]] || continue
    lines=$(wc -l < "${path}" | tr -d ' ')
    agent_view_lines=$((agent_view_lines + lines))
done < <(find "${DESKTOP_ROOT}/neoism/view" -maxdepth 1 -type f -name '*.rs' | sort)
if (( agent_view_lines > MAX_AGENT_VIEW_LINES )); then
    report_detail "${DESKTOP_ROOT}/neoism/view" "agent view adapter layer is ${agent_view_lines} lines, max ${MAX_AGENT_VIEW_LINES}"
fi

file_tree_adapter_lines=0
while IFS= read -r path; do
    [[ -f "${path}" ]] || continue
    [[ "${path}" == "${DESKTOP_ROOT}/editor/file_tree/tests.rs" ]] && continue
    lines=$(wc -l < "${path}" | tr -d ' ')
    file_tree_adapter_lines=$((file_tree_adapter_lines + lines))
done < <(find "${DESKTOP_ROOT}/editor/file_tree" -maxdepth 1 -type f -name '*.rs' | sort)
if (( file_tree_adapter_lines > MAX_FILE_TREE_ADAPTER_LINES )); then
    report_detail "${DESKTOP_ROOT}/editor/file_tree" "file_tree adapter layer is ${file_tree_adapter_lines} lines excluding tests, max ${MAX_FILE_TREE_ADAPTER_LINES}"
fi

for entry in "${SHIM_LINE_LIMITS[@]}"; do
    path="${entry%:*}"
    max_lines="${entry##*:}"
    [[ -f "${path}" ]] || continue
    lines=$(wc -l < "${path}" | tr -d ' ')
    if (( lines > max_lines )); then
        report_detail "${path}" "${lines} lines exceeds adapter cap ${max_lines}"
    fi
done

for entry in "${SHIM_REQUIRED_PATTERNS[@]}"; do
    path="${entry%%|*}"
    pattern="${entry#*|}"
    [[ -f "${path}" ]] || continue
    if ! grep -Eq "${pattern}" "${path}"; then
        report_detail "${path}" "missing shared-backend marker '${pattern}'"
    fi
done

if (( ${#VIOLATIONS[@]} > 0 )); then
    echo "" >&2
    echo "${#VIOLATIONS[@]} file(s) in the desktop fork are UI code that should live in neoism-frontend/shared/." >&2
    echo "Move them under neoism-frontend/shared/src/ and import from there, or set ALLOW_DESKTOP_UI=1 to bypass" >&2
    echo "(and update the allowlist in scripts/check_no_ui_in_desktop_fork.sh in the same commit)." >&2
    exit 1
fi

echo "[check_no_ui_in_desktop_fork] OK — no banned desktop UI files."
exit 0
