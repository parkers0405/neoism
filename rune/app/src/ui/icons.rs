//! File and folder icons (`shared/src/panels/file_tree/icons.rs`): Nerd glyph and colour by
//! file name, then extension. Shared by the tree, the finder and tool cards.

pub const FOLDER_CLOSED: &str = "\u{f07b}";
pub const FOLDER_OPEN: &str = "\u{f07c}";
const FILE_ICON_DEFAULT: u32 = 0xc8c8c8;

/// Glyph and `0xRRGGBB` colour for a file called `label`.
pub fn for_file(label: &str) -> (&'static str, u32) {
    let ext = label.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    builtin(label, &ext)
}

fn builtin(label: &str, ext: &str) -> (&'static str, u32) {
    let lower = label.to_ascii_lowercase();
    if lower.ends_with(".gd.uid") {
        return ("\u{e65f}", 0x478cbf);
    }
    if let Some(hit) = match lower.as_str() {
        "dockerfile" | ".dockerignore" => Some(("\u{f308}", 0x458ee6)),
        "project.godot" => Some(("\u{e65f}", 0x478cbf)),
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitconfig" => {
            Some(("\u{e702}", 0xe44d26))
        }
        "cargo.lock" | "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml"
        | "bun.lockb" | "poetry.lock" => Some(("\u{f023}", 0xb4b4b4)),
        "readme.md" | "readme" | "readme.txt" => Some(("\u{f15c}", 0xdddddd)),
        "license" | "license.md" | "license.txt" | "copying" => {
            Some(("\u{f718}", 0xcbcb41))
        }
        "makefile" | "gnumakefile" => Some(("\u{e673}", 0xcb4141)),
        "neoism graph.neodraw" => Some(("\u{f1e0}", 0x4287ff)),
        _ => None,
    } {
        return hit;
    }

    match ext {
        "rs" => ("\u{e7a8}", 0xdea584),
        "py" | "pyc" | "pyi" | "pyw" => ("\u{e73c}", 0xffe873),
        "js" | "mjs" | "cjs" => ("\u{e74e}", 0xcbcb41),
        "ts" => ("\u{e628}", 0x519aba),
        "tsx" | "jsx" => ("\u{e7ba}", 0x519aba),
        "html" | "htm" => ("\u{f13b}", 0xe44d26),
        "css" => ("\u{e749}", 0x42a5f5),
        "scss" | "sass" => ("\u{e603}", 0xcd6799),
        "json" | "jsonc" => ("\u{e60b}", 0xcbcb41),
        "yaml" | "yml" => ("\u{e6a8}", 0xcb171e),
        "toml" => ("\u{e6b2}", 0x9c413c),
        "md" | "markdown" => ("\u{f15c}", 0xdddddd),
        "ipynb" => ("\u{e606}", 0xf59242),
        "epub" => ("\u{f02d}", 0xd6ae5c),
        "neodraw" => ("\u{f040}", 0xf0c060),
        "sh" | "bash" | "zsh" | "fish" => ("\u{f489}", 0x88af5c),
        "lua" => ("\u{e620}", 0x51a0cf),
        "vim" => ("\u{e62b}", 0x7cb342),
        "go" => ("\u{e626}", 0x50b8cd),
        "c" => ("\u{e61e}", 0x5086bf),
        "cpp" | "cc" | "cxx" => ("\u{e61d}", 0x6699cc),
        "h" | "hpp" | "hh" => ("\u{f0fd}", 0xa569bd),
        "java" => ("\u{e738}", 0xcb4141),
        "rb" => ("\u{e739}", 0xcc342d),
        "php" => ("\u{e608}", 0xa17ac5),
        "swift" => ("\u{e755}", 0xe47733),
        "kt" | "kts" => ("\u{e634}", 0x9589e0),
        "dart" => ("\u{e798}", 0x50b8cd),
        "gd" | "gdshader" | "gdshaderinc" | "tscn" | "tres" | "godot" => {
            ("\u{e65f}", 0x478cbf)
        }
        "ex" | "exs" => ("\u{e62d}", 0xa17ac5),
        "elm" => ("\u{e62c}", 0x66a8ff),
        "haskell" | "hs" => ("\u{e61f}", 0xa17ac5),
        "clj" | "cljs" | "cljc" => ("\u{e768}", 0x88af5c),
        "ml" | "mli" => ("\u{e67a}", 0xe47733),
        "scala" => ("\u{e737}", 0xcb4141),
        "r" => ("\u{f25d}", 0x1682c0),
        "sql" => ("\u{e706}", 0xdea584),
        "xml" => ("\u{f72d}", 0xe44d26),
        "csv" | "tsv" => ("\u{f1c3}", 0x88af5c),
        "txt" | "log" => ("\u{f15c}", 0xc8c8c8),
        "lock" => ("\u{f023}", 0xb4b4b4),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "bmp" | "ico" => {
            ("\u{f1c5}", 0xa569bd)
        }
        "mp3" | "wav" | "flac" | "ogg" => ("\u{f1c7}", 0xa569bd),
        "mp4" | "mov" | "avi" | "mkv" | "webm" => ("\u{f1c8}", 0xe44d26),
        "pdf" => ("\u{f1c1}", 0xe44d26),
        "zip" | "tar" | "gz" | "xz" | "7z" | "bz2" | "rar" => {
            ("\u{f1c6}", 0xb4b4b4)
        }
        "env" => ("\u{f013}", 0xcbcb41),
        "nix" => ("\u{f313}", 0x7ebae4),
        _ => ("\u{f15b}", FILE_ICON_DEFAULT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_beat_extensions() {
        assert_eq!(for_file("main.rs"), ("\u{e7a8}", 0xdea584));
        assert_eq!(for_file("Cargo.lock").0, "\u{f023}");
        assert_eq!(for_file("README.md").0, "\u{f15c}");
        assert_eq!(for_file("weird.zzz"), ("\u{f15b}", FILE_ICON_DEFAULT));
    }
}
