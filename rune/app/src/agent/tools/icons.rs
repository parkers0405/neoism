//! Service artwork for tool rows: the agent logo a `task` row shows for its sub-agent, and the
//! brand marks of MCP servers that ship one (`agent_pane/icon.rs`, `mcp_icon.rs`).

use rune::{Image, ImageCache};

#[derive(Default)]
pub struct Icons {
    claude: Option<Image>,
    codex: Option<Image>,
    opencode: Option<Image>,
    neoism: Option<Image>,
    firecrawl: Option<Image>,
    webflow: Option<Image>,
}

impl Icons {
    pub fn load(&mut self, images: &ImageCache) {
        let load = |bytes: &[u8]| decode(bytes).map(|(w, h, rgba)| images.rgba(w, h, rgba));
        self.claude = load(include_bytes!("../../../assets/icons/claude.png"));
        self.codex = load(include_bytes!("../../../assets/icons/codex.png"));
        self.opencode = load(include_bytes!("../../../assets/icons/opencode.png"));
        self.neoism = load(include_bytes!("../../../assets/icons/neoism.png"));
        self.firecrawl = load(include_bytes!("../../../assets/icons/mcp-firecrawl.png"));
        self.webflow = load(include_bytes!("../../../assets/icons/mcp-webflow.png"));
    }

    /// The logo for a task's `agent_kind` (`neoism` when unknown).
    pub fn agent(&self, kind: Option<&str>) -> Option<&Image> {
        let kind = kind.unwrap_or("neoism").trim().to_ascii_lowercase();
        match kind.as_str() {
            "claude" | "claude-code" | "claude-agent" => self.claude.as_ref(),
            "codex" => self.codex.as_ref(),
            "opencode" => self.opencode.as_ref(),
            _ => self.neoism.as_ref(),
        }
        .or(self.neoism.as_ref())
    }

    pub fn mcp_brand(&self, name: &str) -> Option<&Image> {
        match name { "firecrawl" => self.firecrawl.as_ref(), "webflow" => self.webflow.as_ref(), _ => None }
    }

}

/// The file tree's icon and colour for a file name (`file_tree/icons.rs`).
pub fn file_icon(name: &str) -> (&'static str, rune::Color) {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    let (glyph, [r, g, b]) = match lower.as_str() {
        "dockerfile" | ".dockerignore" => ("\u{f308}", [69, 142, 230]),
        ".gitignore" | ".gitattributes" | ".gitmodules" | ".gitconfig" => ("\u{e702}", [228, 77, 38]),
        "cargo.lock" | "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml" | "bun.lockb" | "poetry.lock" => ("\u{f023}", [180, 180, 180]),
        "readme.md" | "readme" | "readme.txt" => ("\u{f15c}", [221, 221, 221]),
        "license" | "license.md" | "license.txt" | "copying" => ("\u{f718}", [203, 203, 65]),
        "makefile" | "gnumakefile" => ("\u{e673}", [203, 65, 65]),
        _ => match ext {
            "rs" => ("\u{e7a8}", [222, 165, 132]),
            "py" | "pyc" | "pyi" | "pyw" => ("\u{e73c}", [255, 232, 115]),
            "js" | "mjs" | "cjs" => ("\u{e74e}", [203, 203, 65]),
            "ts" => ("\u{e628}", [81, 154, 186]),
            "tsx" | "jsx" => ("\u{e7ba}", [81, 154, 186]),
            "html" | "htm" => ("\u{f13b}", [228, 77, 38]),
            "css" => ("\u{e749}", [66, 165, 245]),
            "scss" | "sass" => ("\u{e603}", [205, 103, 153]),
            "json" | "jsonc" => ("\u{e60b}", [203, 203, 65]),
            "yaml" | "yml" => ("\u{e6a8}", [203, 23, 30]),
            "toml" => ("\u{e6b2}", [156, 65, 60]),
            "md" | "markdown" => ("\u{f15c}", [221, 221, 221]),
            "sh" | "bash" | "zsh" | "fish" => ("\u{f489}", [136, 175, 92]),
            "lua" => ("\u{e620}", [81, 160, 207]),
            "go" => ("\u{e626}", [80, 184, 205]),
            "c" => ("\u{e61e}", [80, 134, 191]),
            "cpp" | "cc" | "cxx" => ("\u{e61d}", [102, 153, 204]),
            "h" | "hpp" | "hh" => ("\u{f0fd}", [165, 105, 189]),
            "java" => ("\u{e738}", [203, 65, 65]),
            "rb" => ("\u{e739}", [204, 52, 45]),
            "swift" => ("\u{e755}", [228, 119, 51]),
            "sql" => ("\u{e706}", [222, 165, 132]),
            "xml" => ("\u{f72d}", [228, 77, 38]),
            "txt" | "log" => ("\u{f15c}", [200, 200, 200]),
            "lock" => ("\u{f023}", [180, 180, 180]),
            "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "bmp" | "ico" => ("\u{f1c5}", [165, 105, 189]),
            _ => ("\u{f15b}", [200, 200, 200]),
        },
    };
    (glyph, crate::ui::theme::rgb((r << 16) | (g << 8) | b))
}

fn decode(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).ok()?;
    buffer.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => buffer.chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buffer.chunks_exact(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => buffer.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((info.width, info.height, rgba))
}
