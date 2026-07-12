use super::types::{NodeKind, TreeEntry};
use super::virtuals::NEOISM_FOLDER_ICON_COLOR;
use super::{FILE_ICON_DEFAULT, FOLDER_ICON_COLOR};
use crate::primitives::look::icon_override;

pub const FOLDER_CLOSED_ICON: &str = "\u{f07b}";
pub const FOLDER_OPEN_ICON: &str = "\u{f07c}";

/// Apply a mash-up pack / user icon override (`[icons]` in a pack's
/// `pack.toml` or the user config) on top of a built-in glyph+color
/// pair. Each field falls back independently, so a color-only
/// override keeps the built-in glyph and vice versa. With no override
/// registered this returns `default` untouched.
fn with_override(
    key: &str,
    default: (&'static str, [u8; 4]),
) -> (&'static str, [u8; 4]) {
    match icon_override(key) {
        Some(over) => (
            over.glyph.unwrap_or(default.0),
            over.color.unwrap_or(default.1),
        ),
        None => default,
    }
}

/// Pick the nerd-font icon glyph + color for a tree entry. Colors
/// follow the VSCode Material Icon Theme palette so the row reads the
/// same as Cursor / Zed once a Nerd Font is installed. Glyphs come
/// from the Devicons + Font Awesome ranges; fallback to a generic
/// page glyph in muted grey when no rule matches.
pub fn icon_for(entry: &TreeEntry) -> (&'static str, [u8; 4]) {
    if entry.is_neoism_workspace_virtual_root() {
        return match entry.kind {
            NodeKind::Dir { open: true } => {
                with_override("folder", (FOLDER_OPEN_ICON, NEOISM_FOLDER_ICON_COLOR))
            }
            NodeKind::Dir { open: false } => {
                with_override("folder", (FOLDER_CLOSED_ICON, NEOISM_FOLDER_ICON_COLOR))
            }
            NodeKind::File => {
                with_override("folder", (FOLDER_CLOSED_ICON, NEOISM_FOLDER_ICON_COLOR))
            }
        };
    }
    match entry.kind {
        NodeKind::Dir { open: true } => {
            with_override("folder", (FOLDER_OPEN_ICON, FOLDER_ICON_COLOR))
        }
        NodeKind::Dir { open: false } => {
            with_override("folder", (FOLDER_CLOSED_ICON, FOLDER_ICON_COLOR))
        }
        NodeKind::File => icon_for_file(&entry.label),
    }
}

pub fn workspace_root_icon() -> (&'static str, [u8; 4]) {
    with_override("workspace", (FOLDER_CLOSED_ICON, FOLDER_ICON_COLOR))
}

/// Glyph + color for a top-level workspace (Island) tab — the folder
/// glyph in the lighter folder blue the tree uses, so the tab reads as
/// "this is a workspace".
pub fn workspace_tab_icon() -> (&'static str, [u8; 4]) {
    with_override("workspace", (FOLDER_CLOSED_ICON, FOLDER_ICON_COLOR))
}

pub fn icon_for_file(label: &str) -> (&'static str, [u8; 4]) {
    let ext = label
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    // Per-extension mash-up override (`"file.rs"`, `"file.md"`, …)
    // wins over the built-in table; unset fields fall through to it.
    let over = if ext.is_empty() {
        None
    } else {
        icon_override(&format!("file.{ext}"))
    };
    let (glyph, color) = builtin_file_icon(label, &ext);
    match over {
        Some(over) => (over.glyph.unwrap_or(glyph), over.color.unwrap_or(color)),
        None => (glyph, color),
    }
}

fn builtin_file_icon(label: &str, ext: &str) -> (&'static str, [u8; 4]) {
    // Whole-name matches first — "Dockerfile" has no extension.
    let lower = label.to_ascii_lowercase();
    if let Some(hit) = match lower.as_str() {
        "dockerfile" | ".dockerignore" => Some(("\u{f308}", [69, 142, 230, 255])),
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitconfig" => {
            Some(("\u{e702}", [228, 77, 38, 255]))
        }
        "cargo.lock" | "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml"
        | "bun.lockb" | "poetry.lock" => Some(("\u{f023}", [180, 180, 180, 255])),
        "readme.md" | "readme" | "readme.txt" => Some(("\u{f48a}", [221, 221, 221, 255])),
        "license" | "license.md" | "license.txt" | "copying" => {
            Some(("\u{f718}", [203, 203, 65, 255]))
        }
        "makefile" | "gnumakefile" => Some(("\u{e673}", [203, 65, 65, 255])),
        // The virtual note-graph tab — show the blue "share" graph glyph
        // instead of the generic drawing pencil.
        "neoism graph.neodraw" => Some(("\u{f1e0}", [66, 135, 255, 255])),
        _ => None,
    } {
        return hit;
    }

    match ext {
        "rs" => ("\u{e7a8}", [222, 165, 132, 255]),
        "py" | "pyc" | "pyi" | "pyw" => ("\u{e73c}", [255, 232, 115, 255]),
        "js" | "mjs" | "cjs" => ("\u{e74e}", [203, 203, 65, 255]),
        "ts" => ("\u{e628}", [81, 154, 186, 255]),
        "tsx" | "jsx" => ("\u{e7ba}", [81, 154, 186, 255]),
        "html" | "htm" => ("\u{f13b}", [228, 77, 38, 255]),
        "css" => ("\u{e749}", [66, 165, 245, 255]),
        "scss" | "sass" => ("\u{e603}", [205, 103, 153, 255]),
        "json" | "jsonc" => ("\u{e60b}", [203, 203, 65, 255]),
        "yaml" | "yml" => ("\u{e6a8}", [203, 23, 30, 255]),
        "toml" => ("\u{e6b2}", [156, 65, 60, 255]),
        "md" | "markdown" => ("\u{f48a}", [221, 221, 221, 255]),
        "ipynb" => ("\u{e606}", [245, 146, 66, 255]),
        "neodraw" => ("\u{f040}", [240, 192, 96, 255]),
        "sh" | "bash" | "zsh" | "fish" => ("\u{f489}", [136, 175, 92, 255]),
        "lua" => ("\u{e620}", [81, 160, 207, 255]),
        "vim" => ("\u{e62b}", [124, 179, 66, 255]),
        "go" => ("\u{e626}", [80, 184, 205, 255]),
        "c" => ("\u{e61e}", [80, 134, 191, 255]),
        "cpp" | "cc" | "cxx" => ("\u{e61d}", [102, 153, 204, 255]),
        "h" | "hpp" | "hh" => ("\u{f0fd}", [165, 105, 189, 255]),
        "java" => ("\u{e738}", [203, 65, 65, 255]),
        "rb" => ("\u{e739}", [204, 52, 45, 255]),
        "php" => ("\u{e608}", [161, 122, 197, 255]),
        "swift" => ("\u{e755}", [228, 119, 51, 255]),
        "kt" | "kts" => ("\u{e634}", [149, 137, 224, 255]),
        "dart" => ("\u{e798}", [80, 184, 205, 255]),
        "ex" | "exs" => ("\u{e62d}", [161, 122, 197, 255]),
        "elm" => ("\u{e62c}", [102, 168, 255, 255]),
        "haskell" | "hs" => ("\u{e61f}", [161, 122, 197, 255]),
        "clj" | "cljs" | "cljc" => ("\u{e768}", [136, 175, 92, 255]),
        "ml" | "mli" => ("\u{e67a}", [228, 119, 51, 255]),
        "scala" => ("\u{e737}", [203, 65, 65, 255]),
        "r" => ("\u{f25d}", [22, 130, 192, 255]),
        "sql" => ("\u{e706}", [222, 165, 132, 255]),
        "xml" => ("\u{f72d}", [228, 77, 38, 255]),
        "csv" | "tsv" => ("\u{f1c3}", [136, 175, 92, 255]),
        "txt" | "log" => ("\u{f15c}", [200, 200, 200, 255]),
        "lock" => ("\u{f023}", [180, 180, 180, 255]),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "bmp" | "ico" => {
            ("\u{f1c5}", [165, 105, 189, 255])
        }
        "mp3" | "wav" | "flac" | "ogg" => ("\u{f1c7}", [165, 105, 189, 255]),
        "mp4" | "mov" | "avi" | "mkv" | "webm" => ("\u{f1c8}", [228, 77, 38, 255]),
        "pdf" => ("\u{f1c1}", [228, 77, 38, 255]),
        "zip" | "tar" | "gz" | "xz" | "7z" | "bz2" | "rar" => {
            ("\u{f1c6}", [180, 180, 180, 255])
        }
        "env" => ("\u{f013}", [203, 203, 65, 255]),
        "nix" => ("\u{f313}", [126, 186, 228, 255]),
        // Unknown file type — the generic page glyph. `"file"` is the
        // semantic override key for exactly this default; known types
        // above are overridden per extension via `"file.<ext>"`.
        _ => with_override("file", ("\u{f15b}", FILE_ICON_DEFAULT)),
    }
}
