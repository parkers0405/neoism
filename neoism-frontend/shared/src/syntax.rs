//! Shared syntax highlighter for previews, markdown code blocks, diff
//! cards, finder cards, and agent chat code blocks.
//!
//! Native builds use tree-sitter where we have parser crates. Wasm and
//! unsupported languages keep the lightweight scanner fallback so UI code
//! can call one stable `highlight_line` entrypoint everywhere.
//!
//! Multi-line constructs (block comments, raw strings) are NOT carried
//! across lines — for a preview pane this trades correctness for
//! simplicity and the few cases where it misclassifies a line are
//! invisible in practice (the surrounding lines look right).

use crate::primitives::IdeTheme;

#[cfg(not(target_arch = "wasm32"))]
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
};

#[cfg(not(target_arch = "wasm32"))]
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

#[cfg(not(target_arch = "wasm32"))]
const TREE_SITTER_LINE_CACHE_LIMIT: usize = 4096;

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static TREE_SITTER_CACHE: RefCell<TreeSitterCache> = RefCell::new(TreeSitterCache::new());
}

#[cfg(not(target_arch = "wasm32"))]
const TREE_SITTER_HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "boolean",
    "character",
    "character.special",
    "comment",
    "comment.documentation",
    "comment.error",
    "comment.note",
    "comment.todo",
    "comment.warning",
    "conditional",
    "constant",
    "constant.builtin",
    "constant.macro",
    "constructor",
    "define",
    "delimiter",
    "directive",
    "embedded",
    "escape",
    "exception",
    "field",
    "function",
    "function.builtin",
    "function.call",
    "function.method",
    "function.method.call",
    "keyword",
    "keyword.conditional",
    "keyword.conditional.ternary",
    "keyword.coroutine",
    "keyword.directive",
    "keyword.exception",
    "keyword.export",
    "keyword.function",
    "keyword.import",
    "keyword.modifier",
    "keyword.operator",
    "keyword.repeat",
    "keyword.return",
    "keyword.type",
    "include",
    "label",
    "macro",
    "module",
    "module.builtin",
    "namespace",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "repeat",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.escape",
    "string.regexp",
    "string.special",
    "symbol",
    "tag",
    "tag.attribute",
    "tag.delimiter",
    "type",
    "type.builtin",
    "type.definition",
    "annotation",
    "variable",
    "variable.builtin",
    "variable.member",
    "variable.parameter",
    "variable.super",
];

/// One coloured span of a preview line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SynTok {
    Plain,
    Keyword,
    Type,
    String,
    Number,
    Comment,
    Function,
    Punct,
}

/// The tree-sitter grammars compiled into Neoism, as `(grammar id,
/// display language)` pairs. This is presentation data for the
/// Extensions page's Syntax Parsers tab; keep it in sync with
/// [`ParserLang`] below (the actual compiled-in parser set).
pub fn built_in_grammars() -> &'static [(&'static str, &'static str)] {
    &[
        ("rust", "Rust"),
        ("javascript", "JavaScript"),
        ("jsx", "JSX"),
        ("typescript", "TypeScript"),
        ("tsx", "TSX"),
        ("python", "Python"),
        ("go", "Go"),
        ("lua", "Lua"),
        ("toml", "TOML"),
        ("json", "JSON"),
        ("nix", "Nix"),
        ("make", "Makefile"),
        ("bash", "Bash"),
        ("c", "C"),
        ("cpp", "C++"),
        ("yaml", "YAML"),
        ("css", "CSS"),
        ("html", "HTML"),
    ]
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ParserLang {
    Rust,
    Javascript,
    Jsx,
    Typescript,
    Tsx,
    Python,
    Go,
    Lua,
    Toml,
    Json,
    Nix,
    Make,
    Bash,
    C,
    Cpp,
    Yaml,
    Css,
    Html,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct HighlightCacheKey {
    lang: ParserLang,
    line: String,
}

#[cfg(not(target_arch = "wasm32"))]
struct TreeSitterCache {
    configs: HashMap<ParserLang, HighlightConfiguration>,
    lines: HashMap<HighlightCacheKey, Vec<(SynTok, usize, usize)>>,
    order: VecDeque<HighlightCacheKey>,
}

#[cfg(not(target_arch = "wasm32"))]
impl TreeSitterCache {
    fn new() -> Self {
        Self {
            configs: HashMap::new(),
            lines: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn config(&mut self, lang: ParserLang) -> Option<&HighlightConfiguration> {
        if !self.configs.contains_key(&lang) {
            let mut config = tree_sitter_config(lang)?;
            config.configure(TREE_SITTER_HIGHLIGHT_NAMES);
            self.configs.insert(lang, config);
        }
        self.configs.get(&lang)
    }

    fn line(&self, key: &HighlightCacheKey) -> Option<Vec<(SynTok, usize, usize)>> {
        self.lines.get(key).cloned()
    }

    fn insert_line(
        &mut self,
        key: HighlightCacheKey,
        spans: Vec<(SynTok, usize, usize)>,
    ) {
        if self.lines.contains_key(&key) {
            self.lines.insert(key, spans);
            return;
        }
        self.order.push_back(key.clone());
        self.lines.insert(key, spans);
        while self.order.len() > TREE_SITTER_LINE_CACHE_LIMIT {
            if let Some(old) = self.order.pop_front() {
                self.lines.remove(&old);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Javascript,
    Jsx,
    Typescript,
    Tsx,
    Python,
    Go,
    Lua,
    Toml,
    Json,
    Nix,
    Make,
    Bash,
    C,
    Cpp,
    Yaml,
    Css,
    Html,
    /// Markdown source. The file-viewer in chrome.rs branches on this
    /// to render block-aware markdown (headings, lists, code blocks,
    /// quotes) via the lifted `editor::markdown::MarkdownPane` parser
    /// instead of the plain per-line syntax highlighter.
    Markdown,
    Other,
}

impl Lang {
    pub fn from_path(path: &str) -> Self {
        let ext = path
            .rsplit('.')
            .next()
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match ext.as_str() {
            "rs" => Lang::Rust,
            "js" | "mjs" | "cjs" => Lang::Javascript,
            "jsx" => Lang::Jsx,
            "ts" => Lang::Typescript,
            "tsx" => Lang::Tsx,
            "py" => Lang::Python,
            "go" => Lang::Go,
            "lua" => Lang::Lua,
            "toml" => Lang::Toml,
            "json" | "jsonc" => Lang::Json,
            "nix" => Lang::Nix,
            "mk" | "makefile" | "gnumakefile" => Lang::Make,
            "sh" | "bash" | "zsh" => Lang::Bash,
            // `.h` headers are overwhelmingly C in the wild; C++ headers
            // use the unambiguous `.hpp`/`.hh`/`.hxx` spellings.
            "c" | "h" => Lang::C,
            "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Lang::Cpp,
            "yaml" | "yml" => Lang::Yaml,
            "css" => Lang::Css,
            "html" | "htm" => Lang::Html,
            "md" | "markdown" => Lang::Markdown,
            _ => Lang::Other,
        }
    }

    fn keywords(self) -> &'static [&'static str] {
        match self {
            Lang::Rust => &[
                "as", "async", "await", "break", "const", "continue", "crate", "dyn",
                "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in",
                "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
                "self", "Self", "static", "struct", "super", "trait", "true", "type",
                "unsafe", "use", "where", "while", "yield",
            ],
            Lang::Javascript | Lang::Jsx | Lang::Typescript | Lang::Tsx => &[
                "async",
                "await",
                "break",
                "case",
                "catch",
                "class",
                "const",
                "continue",
                "debugger",
                "default",
                "delete",
                "do",
                "else",
                "enum",
                "export",
                "extends",
                "false",
                "finally",
                "for",
                "from",
                "function",
                "if",
                "import",
                "in",
                "instanceof",
                "let",
                "new",
                "null",
                "of",
                "return",
                "super",
                "switch",
                "this",
                "throw",
                "true",
                "try",
                "typeof",
                "undefined",
                "var",
                "void",
                "while",
                "with",
                "yield",
                "interface",
                "type",
                "implements",
                "readonly",
                "abstract",
                "as",
                "namespace",
            ],
            Lang::Python => &[
                "False", "None", "True", "and", "as", "assert", "async", "await",
                "break", "class", "continue", "def", "del", "elif", "else", "except",
                "finally", "for", "from", "global", "if", "import", "in", "is", "lambda",
                "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
                "with", "yield",
            ],
            Lang::Go => &[
                "break",
                "case",
                "chan",
                "const",
                "continue",
                "default",
                "defer",
                "else",
                "fallthrough",
                "for",
                "func",
                "go",
                "goto",
                "if",
                "import",
                "interface",
                "map",
                "package",
                "range",
                "return",
                "select",
                "struct",
                "switch",
                "type",
                "var",
                "true",
                "false",
                "nil",
            ],
            Lang::Lua => &[
                "and", "break", "do", "else", "elseif", "end", "false", "for",
                "function", "goto", "if", "in", "local", "nil", "not", "or", "repeat",
                "return", "then", "true", "until", "while",
            ],
            Lang::C => &[
                "auto", "break", "case", "char", "const", "continue", "default",
                "do", "double", "else", "enum", "extern", "float", "for", "goto",
                "if", "inline", "int", "long", "register", "restrict", "return",
                "short", "signed", "sizeof", "static", "struct", "switch",
                "typedef", "union", "unsigned", "void", "volatile", "while",
                "NULL", "true", "false",
            ],
            Lang::Cpp => &[
                "auto", "bool", "break", "case", "catch", "char", "class",
                "const", "constexpr", "continue", "default", "delete", "do",
                "double", "else", "enum", "explicit", "extern", "false", "float",
                "for", "friend", "goto", "if", "inline", "int", "long",
                "mutable", "namespace", "new", "noexcept", "nullptr", "operator",
                "override", "private", "protected", "public", "return", "short",
                "signed", "sizeof", "static", "struct", "switch", "template",
                "this", "throw", "true", "try", "typedef", "typename", "union",
                "unsigned", "using", "virtual", "void", "volatile", "while",
            ],
            Lang::Bash => &[
                "if", "then", "elif", "else", "fi", "for", "while", "until",
                "do", "done", "case", "esac", "in", "function", "select",
                "time", "return", "break", "continue", "local", "export",
                "readonly", "declare", "unset", "shift", "exit", "trap",
                "source", "alias", "eval", "exec",
            ],
            Lang::Make => &[
                "ifeq", "ifneq", "ifdef", "ifndef", "else", "endif", "include",
                "define", "endef", "export", "unexport", "override",
            ],
            Lang::Nix => &[
                "let", "in", "if", "then", "else", "with", "inherit", "rec",
                "assert", "or", "true", "false", "null",
            ],
            Lang::Yaml => &["true", "false", "null", "yes", "no"],
            Lang::Css
            | Lang::Html
            | Lang::Toml
            | Lang::Json
            | Lang::Markdown
            | Lang::Other => &[],
        }
    }

    fn type_starts_uppercase(self) -> bool {
        matches!(
            self,
            Lang::Rust
                | Lang::Javascript
                | Lang::Jsx
                | Lang::Typescript
                | Lang::Tsx
                | Lang::Go
                | Lang::C
                | Lang::Cpp
        )
    }

    fn line_comment(self) -> Option<&'static str> {
        match self {
            Lang::Rust
            | Lang::Javascript
            | Lang::Jsx
            | Lang::Typescript
            | Lang::Tsx
            | Lang::Go
            | Lang::C
            | Lang::Cpp => Some("//"),
            Lang::Python | Lang::Toml | Lang::Bash | Lang::Nix | Lang::Yaml
            | Lang::Make => {
                Some("#")
            }
            Lang::Lua => Some("--"),
            // CSS/HTML only have block comments, which the per-line
            // fallback can't carry across lines anyway.
            Lang::Css | Lang::Html | Lang::Json | Lang::Markdown | Lang::Other => {
                None
            }
        }
    }
}

pub fn highlight_line<'a>(line: &'a str, lang: Lang) -> Vec<(SynTok, &'a str)> {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(spans) = tree_sitter_highlight_line(line, lang) {
        return spans;
    }

    fallback_highlight_line(line, lang)
}

fn fallback_highlight_line<'a>(line: &'a str, lang: Lang) -> Vec<(SynTok, &'a str)> {
    let bytes = line.as_bytes();
    let mut out: Vec<(SynTok, &'a str)> = Vec::new();
    let mut i = 0;

    let push = |out: &mut Vec<(SynTok, &'a str)>, kind: SynTok, slice: &'a str| {
        if !slice.is_empty() {
            out.push((kind, slice));
        }
    };

    let comment_marker = lang.line_comment();
    let kws = lang.keywords();

    while i < bytes.len() {
        let c = bytes[i];

        if let Some(marker) = comment_marker {
            let mb = marker.as_bytes();
            if bytes[i..].starts_with(mb) {
                push(&mut out, SynTok::Comment, &line[i..]);
                return out;
            }
        }

        if c == b'"' || c == b'\'' || c == b'`' {
            let start = i;
            let quote = c;
            i += 1;
            while i < bytes.len() {
                let bc = bytes[i];
                if bc == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if bc == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            push(&mut out, SynTok::String, &line[start..i]);
            continue;
        }

        if c.is_ascii_digit()
            || (c == b'.' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit())
        {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric()
                    || bytes[i] == b'.'
                    || bytes[i] == b'_')
            {
                i += 1;
            }
            push(&mut out, SynTok::Number, &line[start..i]);
            continue;
        }

        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
            {
                i += 1;
            }
            let slice = &line[start..i];
            let kind = if kws.contains(&slice) {
                SynTok::Keyword
            } else if lang.type_starts_uppercase()
                && slice
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_uppercase())
                    .unwrap_or(false)
            {
                SynTok::Type
            } else if i < bytes.len() && bytes[i] == b'(' {
                SynTok::Function
            } else {
                SynTok::Plain
            };
            push(&mut out, kind, slice);
            continue;
        }

        if !c.is_ascii_alphanumeric() && c != b' ' && c != b'\t' {
            let start = i;
            while i < bytes.len() {
                let bc = bytes[i];
                if bc.is_ascii_alphanumeric()
                    || bc == b'_'
                    || bc == b' '
                    || bc == b'\t'
                    || bc == b'"'
                    || bc == b'\''
                    || bc == b'`'
                {
                    break;
                }
                if let Some(m) = comment_marker {
                    if bytes[i..].starts_with(m.as_bytes()) {
                        break;
                    }
                }
                i += 1;
            }
            push(&mut out, SynTok::Punct, &line[start..i]);
            continue;
        }

        let start = i;
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        if i == start {
            i += 1;
        }
        push(&mut out, SynTok::Plain, &line[start..i]);
    }
    out
}

#[cfg(not(target_arch = "wasm32"))]
fn tree_sitter_highlight_line<'a>(
    line: &'a str,
    lang: Lang,
) -> Option<Vec<(SynTok, &'a str)>> {
    let parser_lang = ParserLang::from_lang(lang)?;
    let key = HighlightCacheKey {
        lang: parser_lang,
        line: line.to_owned(),
    };
    let cached = TREE_SITTER_CACHE.with(|cache| cache.borrow().line(&key));
    if let Some(spans) = cached {
        return Some(spans_to_slices(line, &spans));
    }

    let spans = TREE_SITTER_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let config = cache.config(parser_lang)?;
        tree_sitter_highlight_ranges(line, lang, config)
    })?;

    TREE_SITTER_CACHE.with(|cache| cache.borrow_mut().insert_line(key, spans.clone()));
    Some(spans_to_slices(line, &spans))
}

#[cfg(not(target_arch = "wasm32"))]
impl ParserLang {
    fn from_lang(lang: Lang) -> Option<Self> {
        match lang {
            Lang::Rust => Some(ParserLang::Rust),
            Lang::Javascript => Some(ParserLang::Javascript),
            Lang::Jsx => Some(ParserLang::Jsx),
            Lang::Typescript => Some(ParserLang::Typescript),
            Lang::Tsx => Some(ParserLang::Tsx),
            Lang::Python => Some(ParserLang::Python),
            Lang::Go => Some(ParserLang::Go),
            Lang::Lua => Some(ParserLang::Lua),
            Lang::Toml => Some(ParserLang::Toml),
            Lang::Json => Some(ParserLang::Json),
            Lang::Nix => Some(ParserLang::Nix),
            Lang::Make => Some(ParserLang::Make),
            Lang::Bash => Some(ParserLang::Bash),
            Lang::C => Some(ParserLang::C),
            Lang::Cpp => Some(ParserLang::Cpp),
            Lang::Yaml => Some(ParserLang::Yaml),
            Lang::Css => Some(ParserLang::Css),
            Lang::Html => Some(ParserLang::Html),
            _ => None,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn spans_to_slices<'a>(
    line: &'a str,
    spans: &[(SynTok, usize, usize)],
) -> Vec<(SynTok, &'a str)> {
    spans
        .iter()
        .filter_map(|(kind, start, end)| {
            line.get(*start..*end).map(|slice| (*kind, slice))
        })
        .collect()
}

#[cfg(not(target_arch = "wasm32"))]
fn tree_sitter_highlight_ranges(
    line: &str,
    lang: Lang,
    config: &HighlightConfiguration,
) -> Option<Vec<(SynTok, usize, usize)>> {
    let mut highlighter = Highlighter::new();
    let events = highlighter
        .highlight(&config, line.as_bytes(), None, |_| None)
        .ok()?;

    let mut out: Vec<(SynTok, usize, usize)> = Vec::new();
    let mut active: Vec<SynTok> = Vec::new();
    let mut had_highlight = false;

    for event in events {
        match event.ok()? {
            HighlightEvent::Source { start, end } => {
                if start >= end {
                    continue;
                }
                if let Some(kind) = active.last().copied() {
                    push_range(&mut out, kind, start, end);
                } else {
                    let fallback = fallback_highlight_line(&line[start..end], lang);
                    had_highlight |=
                        fallback.iter().any(|(tok, _)| *tok != SynTok::Plain);
                    out.extend(fallback.into_iter().map(|(kind, slice)| {
                        let slice_start =
                            slice.as_ptr() as usize - line.as_ptr() as usize;
                        (kind, slice_start, slice_start + slice.len())
                    }));
                }
            }
            HighlightEvent::HighlightStart(highlight) => {
                had_highlight = true;
                let kind = TREE_SITTER_HIGHLIGHT_NAMES
                    .get(highlight.0)
                    .copied()
                    .map(|name| tree_sitter_capture_kind(name, lang))
                    .unwrap_or(SynTok::Plain);
                active.push(kind);
            }
            HighlightEvent::HighlightEnd => {
                active.pop();
            }
        }
    }

    if had_highlight {
        Some(out)
    } else {
        None
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn tree_sitter_config(lang: ParserLang) -> Option<HighlightConfiguration> {
    match lang {
        ParserLang::Rust => HighlightConfiguration::new(
            tree_sitter_rust::LANGUAGE.into(),
            "rust",
            tree_sitter_rust::HIGHLIGHTS_QUERY,
            tree_sitter_rust::INJECTIONS_QUERY,
            "",
        )
        .ok(),
        ParserLang::Javascript => HighlightConfiguration::new(
            tree_sitter_javascript::LANGUAGE.into(),
            "javascript",
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::INJECTIONS_QUERY,
            tree_sitter_javascript::LOCALS_QUERY,
        )
        .ok(),
        ParserLang::Jsx => HighlightConfiguration::new(
            tree_sitter_javascript::LANGUAGE.into(),
            "javascript",
            &format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY
            ),
            tree_sitter_javascript::INJECTIONS_QUERY,
            tree_sitter_javascript::LOCALS_QUERY,
        )
        .ok(),
        ParserLang::Typescript => HighlightConfiguration::new(
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            "typescript",
            &format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
            tree_sitter_javascript::INJECTIONS_QUERY,
            tree_sitter_typescript::LOCALS_QUERY,
        )
        .ok(),
        ParserLang::Tsx => HighlightConfiguration::new(
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            "tsx",
            &format!(
                "{}\n{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
            tree_sitter_javascript::INJECTIONS_QUERY,
            tree_sitter_typescript::LOCALS_QUERY,
        )
        .ok(),
        ParserLang::Python => HighlightConfiguration::new(
            tree_sitter_python::LANGUAGE.into(),
            "python",
            tree_sitter_python::HIGHLIGHTS_QUERY,
            "",
            "",
        )
        .ok(),
        ParserLang::Go => HighlightConfiguration::new(
            tree_sitter_go::LANGUAGE.into(),
            "go",
            tree_sitter_go::HIGHLIGHTS_QUERY,
            "",
            "",
        )
        .ok(),
        ParserLang::Lua => HighlightConfiguration::new(
            tree_sitter_lua::LANGUAGE.into(),
            "lua",
            tree_sitter_lua::HIGHLIGHTS_QUERY,
            tree_sitter_lua::INJECTIONS_QUERY,
            tree_sitter_lua::LOCALS_QUERY,
        )
        .ok(),
        ParserLang::Toml => HighlightConfiguration::new(
            tree_sitter_toml_ng::LANGUAGE.into(),
            "toml",
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
            "",
            "",
        )
        .ok(),
        ParserLang::Json => HighlightConfiguration::new(
            tree_sitter_json::LANGUAGE.into(),
            "json",
            tree_sitter_json::HIGHLIGHTS_QUERY,
            "",
            "",
        )
        .ok(),
        ParserLang::Make => HighlightConfiguration::new(
            tree_sitter_make::LANGUAGE.into(),
            "make",
            tree_sitter_make::HIGHLIGHTS_QUERY,
            "",
            "",
        )
        .ok(),
        ParserLang::Nix => HighlightConfiguration::new(
            tree_sitter_nix::LANGUAGE.into(),
            "nix",
            tree_sitter_nix::HIGHLIGHTS_QUERY,
            "",
            "",
        )
        .ok(),
        ParserLang::Bash => HighlightConfiguration::new(
            tree_sitter_bash::LANGUAGE.into(),
            "bash",
            tree_sitter_bash::HIGHLIGHT_QUERY,
            "",
            "",
        )
        .ok(),
        ParserLang::C => HighlightConfiguration::new(
            tree_sitter_c::LANGUAGE.into(),
            "c",
            tree_sitter_c::HIGHLIGHT_QUERY,
            "",
            "",
        )
        .ok(),
        ParserLang::Cpp => HighlightConfiguration::new(
            tree_sitter_cpp::LANGUAGE.into(),
            "cpp",
            &format!(
                "{}\n{}",
                tree_sitter_c::HIGHLIGHT_QUERY,
                tree_sitter_cpp::HIGHLIGHT_QUERY
            ),
            "",
            "",
        )
        .ok(),
        ParserLang::Yaml => HighlightConfiguration::new(
            tree_sitter_yaml::LANGUAGE.into(),
            "yaml",
            tree_sitter_yaml::HIGHLIGHTS_QUERY,
            "",
            "",
        )
        .ok(),
        ParserLang::Css => HighlightConfiguration::new(
            tree_sitter_css::LANGUAGE.into(),
            "css",
            tree_sitter_css::HIGHLIGHTS_QUERY,
            "",
            "",
        )
        .ok(),
        ParserLang::Html => HighlightConfiguration::new(
            tree_sitter_html::LANGUAGE.into(),
            "html",
            tree_sitter_html::HIGHLIGHTS_QUERY,
            tree_sitter_html::INJECTIONS_QUERY,
            "",
        )
        .ok(),
    }
}

/// Whole-source tree-sitter highlight: global byte spans over the full
/// text, correct across multi-line constructs (block comments, raw
/// strings, triple-quoted strings) that the per-line path mis-colors.
/// Returns None when no parser exists for `lang` (caller falls back to
/// the per-line highlighter).
#[cfg(not(target_arch = "wasm32"))]
pub fn highlight_source(source: &str, lang: Lang) -> Option<Vec<(SynTok, usize, usize)>> {
    let parser_lang = ParserLang::from_lang(lang)?;
    TREE_SITTER_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let config = cache.config(parser_lang)?;
        tree_sitter_highlight_ranges(source, lang, config)
    })
}

#[cfg(target_arch = "wasm32")]
pub fn highlight_source(
    _source: &str,
    _lang: Lang,
) -> Option<Vec<(SynTok, usize, usize)>> {
    None
}

#[cfg(not(target_arch = "wasm32"))]
fn tree_sitter_capture_kind(capture: &str, lang: Lang) -> SynTok {
    // Nix's query captures nearly every identifier as the broad
    // `@variable` (last-pattern-wins over `@property`), which left
    // whole files plain white; the classic nix look colors them.
    if matches!(lang, Lang::Nix) && capture.starts_with("variable") {
        return SynTok::Function;
    }
    match capture {
        "comment"
        | "comment.documentation"
        | "comment.error"
        | "comment.note"
        | "comment.todo"
        | "comment.warning" => SynTok::Comment,
        "string" | "string.escape" | "string.regexp" | "string.special" | "symbol"
        | "character" | "character.special" | "escape" => SynTok::String,
        "number" | "boolean" | "constant.builtin" => SynTok::Number,
        "keyword"
        | "keyword.conditional"
        | "keyword.conditional.ternary"
        | "keyword.coroutine"
        | "keyword.directive"
        | "keyword.exception"
        | "keyword.export"
        | "keyword.function"
        | "keyword.import"
        | "keyword.modifier"
        | "keyword.operator"
        | "keyword.repeat"
        | "keyword.return" => SynTok::Keyword,
        "keyword.type" | "type" | "type.builtin" | "type.definition" | "constructor"
        | "tag" | "module" | "module.builtin" | "namespace" | "annotation" => {
            SynTok::Type
        }
        "function"
        | "function.builtin"
        | "function.call"
        | "function.method"
        | "function.method.call" => SynTok::Function,
        "punctuation"
        | "punctuation.bracket"
        | "punctuation.delimiter"
        | "punctuation.special"
        | "operator"
        | "delimiter"
        | "tag.delimiter" => SynTok::Punct,
        "constant" | "constant.macro" | "macro" | "label" | "variable.builtin"
        | "variable.super" => SynTok::Type,
        "property" | "field" | "variable.member" | "tag.attribute" | "attribute" => {
            SynTok::Function
        }
        _ => SynTok::Plain,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn push_range(
    out: &mut Vec<(SynTok, usize, usize)>,
    kind: SynTok,
    start: usize,
    end: usize,
) {
    if start < end {
        out.push((kind, start, end));
    }
}

/// Map a token kind to a theme-derived color. The `syn_*` palette on
/// `IdeTheme` mirrors `nvim_runtime/lua/rio/theme.lua` per theme so the
/// preview reads with the SAME colors the editor's treesitter / LSP
/// highlighter paints — no theme-mismatch between the finder and the
/// editor.
pub fn syn_color(tok: SynTok, theme: &IdeTheme, dim: bool) -> [u8; 4] {
    let alpha = if dim { 220 } else { 255 };
    let mut c = match tok {
        SynTok::Plain => theme.u8(theme.fg),
        SynTok::Keyword => theme.u8(theme.syn_keyword),
        SynTok::Type => theme.u8(theme.syn_type),
        SynTok::String => theme.u8(theme.syn_string),
        SynTok::Number => theme.u8(theme.syn_number),
        SynTok::Comment => theme.u8(theme.syn_comment),
        SynTok::Function => theme.u8(theme.syn_func),
        SynTok::Punct => theme.u8(theme.muted),
    };
    c[3] = alpha;
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_token(spans: &[(SynTok, &str)], kind: SynTok, text: &str) -> bool {
        spans
            .iter()
            .any(|(span_kind, span_text)| *span_kind == kind && *span_text == text)
    }

    #[test]
    fn typescript_uses_parser_backed_highlight_categories() {
        let spans = highlight_line(
            "const value = client.fetch<User>(url, true)",
            Lang::Typescript,
        );

        assert!(has_token(&spans, SynTok::Keyword, "const"));
        assert!(has_token(&spans, SynTok::Function, "fetch"));
        assert!(has_token(&spans, SynTok::Type, "User"));
        assert!(has_token(&spans, SynTok::Number, "true"));
        assert!(has_token(&spans, SynTok::Punct, "("));
    }

    /// `tree_sitter_config` returns `None` (silent scanner fallback) when a
    /// grammar's highlight query fails to load — so every compiled-in parser
    /// must prove its config constructs, or a bad query ships invisibly.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[test]
    fn nix_attr_keys_color_as_property() {
        let src = "{\n  name = \"x\";\n  services.foo.enable = true;\n}\n";
        let toks = highlight_source(src, Lang::Nix).unwrap_or_default();
        let name_pos = src.find("name").unwrap();
        let hit = toks
            .iter()
            .find(|(_, s, e)| *s <= name_pos && name_pos < *e)
            .map(|(k, _, _)| *k);
        eprintln!("nix tokens: {toks:?}");
        assert_eq!(hit, Some(SynTok::Function), "attr key should color as property");
    }

    #[test]
    fn new_grammars_produce_real_tokens() {
        // A grammar can "construct" yet yield zero styled tokens when
        // its query captures miss the configured name list — assert
        // actual output per language on a representative snippet.
        let cases: &[(Lang, &str)] = &[
            (Lang::Make, "# c\nall: build\n\t$(CC) -o out main.c\n.PHONY: all\n"),
            (Lang::Nix, "{ pkgs, ... }:\n{\n  # comment\n  services.foo.enable = true;\n  environment.systemPackages = [ pkgs.hello ];\n}\n"),
            (Lang::Bash, "#!/bin/bash\n# c\nfor f in *.txt; do\n  echo \"$f\"\ndone\n"),
            (Lang::C, "// c\nint main(void) {\n  return 42;\n}\n"),
            (Lang::Cpp, "// c\nclass Foo {\n public:\n  int bar() { return 1; }\n};\n"),
            (Lang::Yaml, "# c\nname: test\nitems:\n  - one\n  - two\n"),
            (Lang::Css, "/* c */\n.foo { color: red; }\n"),
            (Lang::Html, "<!-- c -->\n<div class=\"x\">hi</div>\n"),
        ];
        for (lang, src) in cases {
            let toks = highlight_source(src, *lang);
            assert!(
                toks.as_ref().is_some_and(|t| t.len() >= 3),
                "{lang:?} produced {:?} tokens",
                toks.map(|t| t.len())
            );
        }
    }

    fn every_compiled_parser_config_constructs() {
        for lang in [
            ParserLang::Rust,
            ParserLang::Javascript,
            ParserLang::Jsx,
            ParserLang::Typescript,
            ParserLang::Tsx,
            ParserLang::Python,
            ParserLang::Go,
            ParserLang::Lua,
            ParserLang::Toml,
            ParserLang::Json,
            ParserLang::Nix,
            ParserLang::Bash,
            ParserLang::C,
            ParserLang::Cpp,
            ParserLang::Yaml,
            ParserLang::Css,
            ParserLang::Html,
        ] {
            assert!(
                tree_sitter_config(lang).is_some(),
                "{lang:?} highlight query failed to load"
            );
        }
    }

    #[test]
    fn bash_and_c_use_parser_backed_highlighting() {
        let bash = highlight_line("if [ -f x ]; then echo \"hi\"; fi", Lang::Bash);
        assert!(has_token(&bash, SynTok::Keyword, "if"));
        assert!(has_token(&bash, SynTok::String, "\"hi\""));

        let c = highlight_line("static unsigned count = 42;", Lang::C);
        assert!(has_token(&c, SynTok::Number, "42"));
        assert!(c
            .iter()
            .any(|(kind, text)| *kind == SynTok::Keyword && *text == "static"));
    }

    #[test]
    fn header_extensions_split_between_c_and_cpp() {
        assert_eq!(Lang::from_path("src/list.h"), Lang::C);
        assert_eq!(Lang::from_path("src/list.hpp"), Lang::Cpp);
        assert_eq!(Lang::from_path("src/list.hh"), Lang::Cpp);
        assert_eq!(Lang::from_path("flake.nix"), Lang::Nix);
        assert_eq!(Lang::from_path("run.zsh"), Lang::Bash);
        assert_eq!(Lang::from_path("ci.yml"), Lang::Yaml);
    }

    #[test]
    fn rust_uses_parser_backed_operator_and_escape_highlighting() {
        let spans = highlight_line("let path = format!(\"a\\nb\");", Lang::Rust);

        assert!(has_token(&spans, SynTok::Keyword, "let"));
        assert!(has_token(&spans, SynTok::Function, "format"));
        assert!(has_token(&spans, SynTok::String, "\\n"));
        assert!(spans
            .iter()
            .any(|(kind, text)| *kind == SynTok::Punct && *text == "("));
    }
}
