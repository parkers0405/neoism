//! Canonical config descriptors and host-side suggestion discovery.

use neoism_protocol::config::{
    ConfigCategory as C, ConfigConstraints, ConfigControl as Control,
    ConfigDescriptor as D, ConfigOption, ConfigSuggestionProvider as Provider,
    ConfigValueKind as Kind,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;

fn d(
    path: &str,
    label: &str,
    description: &str,
    kind: Kind,
    default: Value,
    suggestions: &[&str],
    extensible: bool,
    category: C,
    control: Control,
) -> D {
    let nullable = default.is_null();
    let options = suggestions
        .iter()
        .map(|value| ConfigOption {
            value: suggestion_value(value, kind),
            label: None,
            description: None,
        })
        .collect();
    D {
        path: path.into(),
        label: label.into(),
        description: description.into(),
        value_kind: kind,
        default,
        static_suggestions: suggestions.iter().map(|value| (*value).into()).collect(),
        runtime_suggestions: Vec::new(),
        options,
        provider: None,
        constraints: ConfigConstraints {
            nullable,
            ..ConfigConstraints::default()
        },
        accepted_kinds: Vec::new(),
        extensible,
        category,
        control,
        settings_visible: true,
    }
}

fn suggestion_value(value: &str, kind: Kind) -> Value {
    match kind {
        Kind::Integer => value
            .parse::<i64>()
            .map_or_else(|_| json!(value), |number| json!(number)),
        Kind::Number => value
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map_or_else(|| json!(value), Value::Number),
        Kind::Boolean => value
            .parse::<bool>()
            .map_or_else(|_| json!(value), Value::Bool),
        _ => json!(value),
    }
}

/// Every leaf currently surfaced by Neoism's grouped Settings/config UI,
/// plus canonical programmable fields whose host suggestions are useful.
pub fn config_descriptors() -> Vec<D> {
    let mut rows = vec![
        d(
            "ui.confirm-before-quit",
            "Confirm before quit",
            "Ask before closing Neoism.",
            Kind::Boolean,
            json!(false),
            &[],
            false,
            C::General,
            Control::Toggle,
        ),
        d(
            "terminal.copy-on-select",
            "Copy on select",
            "Copy terminal selections to the clipboard.",
            Kind::Boolean,
            json!(false),
            &[],
            false,
            C::General,
            Control::Toggle,
        ),
        d(
            "terminal.hide-mouse-cursor-when-typing",
            "Hide mouse cursor when typing",
            "Hide the pointer while typing.",
            Kind::Boolean,
            json!(false),
            &[],
            false,
            C::General,
            Control::Toggle,
        ),
        d(
            "terminal.use-fork",
            "Use fork",
            "Spawn new terminals with fork where supported.",
            Kind::Boolean,
            json!(true),
            &[],
            false,
            C::General,
            Control::Toggle,
        ),
        d(
            "terminal.option-as-alt",
            "Option as Alt",
            "Treat macOS Option keys as Alt.",
            Kind::String,
            json!("none"),
            &["none", "left", "right", "both"],
            false,
            C::General,
            Control::Select,
        ),
        d(
            "appearance.theme",
            "Theme",
            "IDE theme for chrome, editor, and terminal.",
            Kind::String,
            json!("pastel_dark"),
            &[
                "pastel_dark",
                "nvchad_one",
                "tokyo_night",
                "catppuccin_mocha",
                "retro_95",
            ],
            true,
            C::Appearance,
            Control::Select,
        ),
        d(
            "appearance.palette",
            "Terminal palette",
            "Terminal color palette from the themes directory.",
            Kind::String,
            json!(""),
            &[],
            true,
            C::Appearance,
            Control::Select,
        ),
        d(
            "appearance.mashup-pack",
            "Mash Up Pack",
            "Active visual Mash Up Pack identifier.",
            Kind::String,
            Value::Null,
            &[],
            true,
            C::Appearance,
            Control::Select,
        ),
        d(
            "appearance.fonts.family",
            "Font family",
            "Installed font used by terminal and editor.",
            Kind::String,
            json!("Cascadia Code"),
            &[],
            true,
            C::Appearance,
            Control::FontFamily,
        ),
        d(
            "appearance.fonts.size",
            "Font size",
            "Terminal and editor font size.",
            Kind::Number,
            json!(14.0),
            &["12", "13", "14", "16", "18", "19", "20", "22"],
            true,
            C::Appearance,
            Control::Number,
        ),
        d(
            "appearance.fonts.weight",
            "Font weight",
            "Base text weight.",
            Kind::Integer,
            json!(400),
            &["300", "400", "500", "600", "700", "800"],
            true,
            C::Appearance,
            Control::Number,
        ),
        d(
            "appearance.fonts.hinting",
            "Font hinting",
            "Hint glyphs to the pixel grid.",
            Kind::Boolean,
            json!(true),
            &[],
            false,
            C::Appearance,
            Control::Toggle,
        ),
        d(
            "appearance.fonts.use-drawable-chars",
            "Native box drawing",
            "Draw box and block glyphs natively.",
            Kind::Boolean,
            json!(true),
            &[],
            false,
            C::Appearance,
            Control::Toggle,
        ),
        d(
            "appearance.line-height",
            "Line height",
            "Terminal line-height multiplier.",
            Kind::Number,
            json!(1.0),
            &["1.0", "1.1", "1.2", "1.3", "1.4", "1.5"],
            true,
            C::Appearance,
            Control::Number,
        ),
        d(
            "appearance.effects.trail-cursor",
            "Cursor trail",
            "Animate a trail behind the caret.",
            Kind::Boolean,
            json!(true),
            &[],
            false,
            C::Appearance,
            Control::Toggle,
        ),
        d(
            "appearance.effects.custom-mouse-cursor",
            "Custom mouse cursor",
            "Render Neoism's own pointer.",
            Kind::Boolean,
            json!(false),
            &[],
            false,
            C::Appearance,
            Control::Toggle,
        ),
        d(
            "presence.display-name",
            "Display name",
            "Name shown to collaborators.",
            Kind::String,
            Value::Null,
            &[],
            true,
            C::Presence,
            Control::Text,
        ),
        d(
            "presence.cursor-color",
            "Cursor color",
            "Collaborative caret color in CSS hex form.",
            Kind::String,
            Value::Null,
            &[],
            true,
            C::Presence,
            Control::Color,
        ),
        d(
            "presence.cursor-style",
            "Cursor style",
            "Solid or animated rainbow collaborative caret.",
            Kind::String,
            json!("solid"),
            &["solid", "rainbow"],
            false,
            C::Presence,
            Control::Select,
        ),
        d(
            "ui.status-fps",
            "Status bar FPS",
            "Show frame rate in the status bar.",
            Kind::Boolean,
            json!(true),
            &[],
            false,
            C::Ui,
            Control::Toggle,
        ),
        d(
            "ui.window.opacity",
            "Window opacity",
            "Overall window opacity.",
            Kind::Number,
            json!(1.0),
            &["0.8", "0.9", "0.95", "1.0"],
            true,
            C::Ui,
            Control::Number,
        ),
        d(
            "ui.window.blur",
            "Background blur",
            "Blur behind translucent windows.",
            Kind::Boolean,
            json!(false),
            &[],
            false,
            C::Ui,
            Control::Toggle,
        ),
        d(
            "ui.navigation.hide-if-single",
            "Hide tab bar when single",
            "Hide tabs when only one exists.",
            Kind::Boolean,
            json!(true),
            &[],
            false,
            C::Ui,
            Control::Toggle,
        ),
        d(
            "ui.navigation.use-split",
            "Enable splits",
            "Allow split panes.",
            Kind::Boolean,
            json!(true),
            &[],
            false,
            C::Ui,
            Control::Toggle,
        ),
        d(
            "ui.navigation.current-working-directory",
            "New tab inherits CWD",
            "Start new tabs in the active terminal directory.",
            Kind::Boolean,
            json!(true),
            &[],
            false,
            C::Ui,
            Control::Toggle,
        ),
        d(
            "editor.vim-mode",
            "Vim mode",
            "Use Vim keybindings in code and Markdown editors.",
            Kind::Boolean,
            json!(true),
            &[],
            false,
            C::Editor,
            Control::Toggle,
        ),
        d(
            "editor.format-on-save",
            "Format on save",
            "Run the language-server formatter before save.",
            Kind::Boolean,
            json!(true),
            &[],
            false,
            C::Editor,
            Control::Toggle,
        ),
        d(
            "editor.minimap",
            "Minimap",
            "Show the code editor minimap.",
            Kind::Boolean,
            json!(false),
            &[],
            false,
            C::Editor,
            Control::Toggle,
        ),
        d(
            "editor.markdown.spellcheck",
            "Markdown spell check",
            "Underline misspelled words in Markdown editors.",
            Kind::Boolean,
            json!(true),
            &[],
            false,
            C::Editor,
            Control::Toggle,
        ),
        d(
            "editor.external.program",
            "External editor",
            "External editor executable.",
            Kind::String,
            json!("vi"),
            &["vi", "vim", "nvim", "code", "zed"],
            true,
            C::Editor,
            Control::Text,
        ),
        d(
            "terminal.shell.program",
            "Shell",
            "Program used for new terminal panes.",
            Kind::String,
            json!(""),
            &["bash", "zsh", "fish", "pwsh", "nu"],
            true,
            C::Terminal,
            Control::Select,
        ),
        d(
            "terminal.shell.args",
            "Shell arguments",
            "Arguments passed to the terminal shell.",
            Kind::Array,
            json!([]),
            &[],
            true,
            C::Terminal,
            Control::StringList,
        ),
        d(
            "terminal.cursor.shape",
            "Cursor shape",
            "Terminal caret shape.",
            Kind::String,
            json!("block"),
            &["block", "underline", "beam", "hidden"],
            false,
            C::Terminal,
            Control::Select,
        ),
        d(
            "terminal.cursor.blinking",
            "Blinking cursor",
            "Blink the terminal caret.",
            Kind::Boolean,
            json!(false),
            &[],
            false,
            C::Terminal,
            Control::Toggle,
        ),
        d(
            "terminal.cursor.blinking-interval",
            "Blink interval",
            "Caret blink half-period in milliseconds.",
            Kind::Integer,
            json!(530),
            &["400", "530", "700", "1000"],
            true,
            C::Terminal,
            Control::Number,
        ),
        d(
            "terminal.enable-scroll-bar",
            "Scrollbar",
            "Show the terminal scrollbar.",
            Kind::Boolean,
            json!(true),
            &[],
            false,
            C::Terminal,
            Control::Toggle,
        ),
        d(
            "terminal.scrollback-history-limit",
            "Scrollback lines",
            "Maximum retained terminal history.",
            Kind::Integer,
            json!(10000),
            &["1000", "5000", "10000", "50000", "100000"],
            true,
            C::Terminal,
            Control::Number,
        ),
        d(
            "terminal.scroll.multiplier",
            "Scroll speed",
            "Terminal wheel multiplier.",
            Kind::Number,
            json!(3.0),
            &["1", "2", "3", "4", "5"],
            true,
            C::Terminal,
            Control::Number,
        ),
        d(
            "terminal.draw-bold-text-with-light-colors",
            "Bold uses bright colors",
            "Draw bold ANSI text with bright colors.",
            Kind::Boolean,
            json!(false),
            &[],
            false,
            C::Terminal,
            Control::Toggle,
        ),
        d(
            "terminal.keyboard.ime-cursor-positioning",
            "IME at cursor",
            "Position the IME popup at the caret.",
            Kind::Boolean,
            json!(true),
            &[],
            false,
            C::Terminal,
            Control::Toggle,
        ),
        d(
            "terminal.bell.audio",
            "Audible bell",
            "Play sound for terminal bell events.",
            Kind::Boolean,
            json!(false),
            &[],
            false,
            C::Terminal,
            Control::Toggle,
        ),
        d(
            "keybinds.keys",
            "Keybindings",
            "User keybinding overrides.",
            Kind::Array,
            json!([]),
            &[],
            true,
            C::Keybinds,
            Control::Keybinding,
        ),
        d(
            "agent.model",
            "Agent model",
            "Default provider/model identifier.",
            Kind::String,
            Value::Null,
            &[],
            true,
            C::Agent,
            Control::Select,
        ),
        d(
            "agent.small-model",
            "Small model",
            "Model used for lightweight agent tasks.",
            Kind::String,
            Value::Null,
            &[],
            true,
            C::Agent,
            Control::Select,
        ),
        d(
            "agent.default-agent",
            "Default agent",
            "Agent selected for new sessions.",
            Kind::String,
            json!("build"),
            &["build", "plan", "general", "explore"],
            true,
            C::Agent,
            Control::Select,
        ),
        d(
            "agent.reasoning-effort",
            "Reasoning effort",
            "How much supported models reason.",
            Kind::String,
            json!("medium"),
            &["low", "medium", "high", "xhigh", "max"],
            false,
            C::Agent,
            Control::Select,
        ),
        d(
            "agent.text-verbosity",
            "Response length",
            "Final-answer detail for supported models.",
            Kind::String,
            json!("low"),
            &["low", "medium", "high"],
            false,
            C::Agent,
            Control::Select,
        ),
        d(
            "agent.dangerously-skip-permissions",
            "Skip permission prompts",
            "Allow actions that would otherwise ask.",
            Kind::Boolean,
            json!(false),
            &[],
            false,
            C::Agent,
            Control::Toggle,
        ),
        d(
            "agent.lsp",
            "Language servers",
            "Language-server configuration keyed by identifier.",
            Kind::Object,
            json!({}),
            &[
                "rust",
                "typescript",
                "python",
                "go",
                "c",
                "cpp",
                "json",
                "yaml",
                "toml",
                "markdown",
            ],
            true,
            C::Agent,
            Control::Object,
        ),
        d(
            "developer.log-level",
            "Log level",
            "Tracing verbosity on next launch.",
            Kind::String,
            json!("off"),
            &["off", "error", "warn", "info", "debug", "trace"],
            false,
            C::Developer,
            Control::Select,
        ),
        d(
            "developer.enable-log-file",
            "Write log file",
            "Persist Neoism logs to disk.",
            Kind::Boolean,
            json!(false),
            &[],
            false,
            C::Developer,
            Control::Toggle,
        ),
        d(
            "developer.enable-fps-counter",
            "FPS counter",
            "Show the developer FPS overlay.",
            Kind::Boolean,
            json!(false),
            &[],
            false,
            C::Developer,
            Control::Toggle,
        ),
    ];
    append_grouped_backend_fields(&mut rows);
    apply_schema_metadata(&mut rows);
    rows.sort_by(|left, right| left.path.cmp(&right.path));
    enrich_runtime_suggestions(&mut rows);
    rows
}

fn apply_schema_metadata(rows: &mut Vec<D>) {
    // `renderer.backend` is intentionally omitted from default serialization,
    // but it is still a supported setting and must not disappear from hints.
    if !rows.iter().any(|row| row.path == "renderer.backend") {
        rows.push(d(
            "renderer.backend",
            "Renderer backend",
            "Graphics API used to render Neoism. Automatic is recommended unless diagnosing a driver issue.",
            Kind::String,
            json!("Automatic"),
            &[],
            false,
            C::Renderer,
            Control::Select,
        ));
    }

    for (path, options) in [
        ("appearance.force-theme", &["dark", "light"][..]),
        (
            "appearance.look.markdown.checkbox",
            &["modern", "retro95"][..],
        ),
        (
            "ui.window.mode",
            &["Maximized", "Fullscreen", "Windowed"][..],
        ),
        (
            "ui.window.decorations",
            &["Enabled", "Disabled", "Transparent", "Buttonless"][..],
        ),
        (
            "ui.window.colorspace",
            &["Srgb", "DisplayP3", "Rec2020"][..],
        ),
        (
            "ui.window.windows-corner-preference",
            &["Default", "DoNotRound", "Round", "RoundSmall"][..],
        ),
        ("ui.navigation.mode", navigation_modes()),
        ("renderer.backend", renderer_backends()),
        ("renderer.strategy", &["Events", "Game"][..]),
        ("agent.share", &["manual", "auto", "disabled"][..]),
    ] {
        make_select(rows, path, options);
    }

    let font_styles = &["Normal", "Italic"][..];
    let font_widths = &[
        "UltraCondensed",
        "ExtraCondensed",
        "Condensed",
        "SemiCondensed",
        "Normal",
        "SemiExpanded",
        "Expanded",
        "ExtraExpanded",
        "UltraExpanded",
    ][..];
    for face in ["regular", "bold", "italic", "bold-italic"] {
        make_select(rows, &format!("appearance.fonts.{face}.style"), font_styles);
        make_select(rows, &format!("appearance.fonts.{face}.width"), font_widths);
        set_kind(
            rows,
            &format!("appearance.fonts.{face}.weight"),
            Kind::Integer,
            Control::Number,
            true,
        );
    }

    for os in ["linux", "windows", "macos"] {
        let prefix = format!("platform.{os}");
        for (suffix, options) in [
            ("window.mode", &["Maximized", "Fullscreen", "Windowed"][..]),
            (
                "window.decorations",
                &["Enabled", "Disabled", "Transparent", "Buttonless"][..],
            ),
            ("window.colorspace", &["Srgb", "DisplayP3", "Rec2020"][..]),
            (
                "window.windows-corner-preference",
                &["Default", "DoNotRound", "Round", "RoundSmall"][..],
            ),
            ("navigation.mode", navigation_modes()),
            ("renderer.backend", renderer_backends()),
            ("renderer.strategy", &["Events", "Game"][..]),
        ] {
            make_select(rows, &format!("{prefix}.{suffix}"), options);
        }
        for suffix in [
            "window.blur",
            "window.macos-use-unified-titlebar",
            "window.macos-use-shadow",
            "window.windows-use-undecorated-shadow",
            "window.windows-use-no-redirection-bitmap",
            "navigation.clickable",
            "navigation.current-working-directory",
            "navigation.use-terminal-title",
            "navigation.hide-if-single",
            "navigation.use-split",
            "navigation.open-config-with-split",
            "renderer.disable-unfocused-render",
            "renderer.disable-occluded-render",
        ] {
            set_kind(
                rows,
                &format!("{prefix}.{suffix}"),
                Kind::Boolean,
                Control::Toggle,
                true,
            );
        }
        for suffix in [
            "window.width",
            "window.height",
            "window.columns",
            "window.rows",
        ] {
            set_kind(
                rows,
                &format!("{prefix}.{suffix}"),
                Kind::Integer,
                Control::Number,
                true,
            );
        }
        for suffix in [
            "window.opacity",
            "window.macos-traffic-light-position-x",
            "window.macos-traffic-light-position-y",
            "navigation.unfocused-split-opacity",
        ] {
            set_kind(
                rows,
                &format!("{prefix}.{suffix}"),
                Kind::Number,
                Control::Number,
                true,
            );
        }
    }

    for (path, min, max, step, unit) in [
        ("appearance.fonts.size", 6.0, 96.0, 0.5, "pt"),
        ("appearance.fonts.weight", 100.0, 900.0, 100.0, "weight"),
        ("appearance.line-height", 0.5, 3.0, 0.05, "x"),
        ("ui.window.opacity", 0.1, 1.0, 0.05, "opacity"),
        (
            "terminal.cursor.blinking-interval",
            100.0,
            5000.0,
            10.0,
            "ms",
        ),
        (
            "terminal.scrollback-history-limit",
            0.0,
            1_000_000.0,
            1000.0,
            "lines",
        ),
        ("terminal.scroll.multiplier", 0.1, 20.0, 0.1, "x"),
    ] {
        if let Some(row) = rows.iter_mut().find(|row| row.path == path) {
            row.constraints.min = Some(min);
            row.constraints.max = Some(max);
            row.constraints.step = Some(step);
            row.constraints.unit = Some(unit.to_string());
        }
    }

    if let Some(row) = rows.iter_mut().find(|row| row.path == "agent.autoupdate") {
        row.value_kind = Kind::String;
        row.control = Control::Select;
        row.options = vec![
            ConfigOption {
                value: json!("notify"),
                label: Some("Notify only".into()),
                description: None,
            },
            ConfigOption {
                value: json!(true),
                label: Some("Automatically update".into()),
                description: None,
            },
            ConfigOption {
                value: json!(false),
                label: Some("Disabled".into()),
                description: None,
            },
        ];
        row.accepted_kinds = vec![Kind::Boolean];
        row.constraints.nullable = true;
        row.extensible = false;
    }
    for path in [
        "agent.model",
        "agent.small-model",
        "agent.agent.*.model",
        "agent.mode.*.model",
    ] {
        if let Some(row) = rows.iter_mut().find(|row| row.path == path) {
            row.provider = Some(Provider::Models);
        }
    }
    for path in ["agent.enabled-providers", "agent.disabled-providers"] {
        if let Some(row) = rows.iter_mut().find(|row| row.path == path) {
            row.provider = Some(Provider::ProviderIds);
        }
    }
}

fn make_select(rows: &mut [D], path: &str, options: &[&str]) {
    if let Some(row) = rows.iter_mut().find(|row| row.path == path) {
        row.value_kind = Kind::String;
        row.control = Control::Select;
        row.static_suggestions =
            options.iter().map(|value| (*value).to_string()).collect();
        row.options = options
            .iter()
            .map(|value| ConfigOption {
                value: json!(value),
                label: None,
                description: None,
            })
            .collect();
        row.extensible = false;
    }
}

fn set_kind(rows: &mut [D], path: &str, kind: Kind, control: Control, nullable: bool) {
    if let Some(row) = rows.iter_mut().find(|row| row.path == path) {
        row.value_kind = kind;
        row.control = control;
        row.constraints.nullable = nullable;
    }
}

fn navigation_modes() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &["Plain", "Tab", "NativeTab"]
    } else {
        &["Plain", "Tab"]
    }
}

fn renderer_backends() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &["Automatic", "GL", "Vulkan", "DX12", "WgpuMetal", "Metal"]
    } else {
        &["Automatic", "GL", "Vulkan", "DX12", "WgpuMetal"]
    }
}

/// Keep the schema exhaustive as grouped backend structs evolve. Richly
/// curated rows above win; this fills advanced leaves from the serialized
/// canonical defaults so a new supported field cannot silently disappear.
fn append_grouped_backend_fields(rows: &mut Vec<D>) {
    let Ok(defaults) = serde_json::to_value(super::Config::default()) else {
        return;
    };
    let mut generated = Vec::new();
    collect_defaults("", &defaults, &mut generated);
    if let Ok(agent_defaults) =
        serde_json::to_value(neoism_agent_core::api::AgentConfigDocument::default())
    {
        collect_defaults("agent", &agent_defaults, &mut generated);
    }
    let agent_definition = json!({
        "name": null,
        "model": null,
        "variant": null,
        "temperature": null,
        "topP": null,
        "prompt": null,
        "tools": {},
        "disable": false,
        "description": null,
        "mode": null,
        "hidden": null,
        "options": {},
        "color": null,
        "steps": null,
        "maxSteps": null,
        "permission": {},
    });
    for prefix in ["agent.agent.*", "agent.mode.*"] {
        collect_defaults(prefix, &agent_definition, &mut generated);
    }
    let platform_template = super::PlatformConfig {
        shell: Some(super::Shell::default()),
        navigation: Some(super::platform::PlatformNavigation::default()),
        window: Some(super::platform::PlatformWindow::default()),
        renderer: Some(super::platform::PlatformRenderer::default()),
        env_vars: Some(Vec::new()),
        theme: Some(String::new()),
    };
    if let Ok(template) = serde_json::to_value(platform_template) {
        for os in ["linux", "windows", "macos"] {
            collect_defaults(&format!("platform.{os}"), &template, &mut generated);
        }
    }
    let existing = rows
        .iter()
        .map(|row| row.path.clone())
        .collect::<BTreeSet<_>>();
    rows.extend(
        generated
            .into_iter()
            .filter(|row| !existing.contains(&row.path)),
    );
}

fn collect_defaults(prefix: &str, value: &Value, output: &mut Vec<D>) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (key, child) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                collect_defaults(&path, child, output);
            }
        }
        _ if !prefix.is_empty() => {
            output.push(generated_descriptor(prefix, value.clone()))
        }
        _ => {}
    }
}

fn generated_descriptor(path: &str, default: Value) -> D {
    let kind = match default {
        Value::Bool(_) => Kind::Boolean,
        Value::Number(ref number) if number.is_i64() || number.is_u64() => Kind::Integer,
        Value::Number(_) => Kind::Number,
        Value::String(_) | Value::Null => Kind::String,
        Value::Array(_) => Kind::Array,
        Value::Object(_) => Kind::Object,
    };
    let control = match kind {
        Kind::Boolean => Control::Toggle,
        Kind::Integer | Kind::Number => Control::Number,
        Kind::String => Control::Text,
        Kind::Array => Control::StringList,
        Kind::Object => Control::Object,
    };
    let category = match path.split('.').next().unwrap_or_default() {
        "appearance" => C::Appearance,
        "editor" => C::Editor,
        "terminal" => C::Terminal,
        "ui" => C::Ui,
        "presence" => C::Presence,
        "keybinds" => C::Keybinds,
        "agent" => C::Agent,
        "platform" => C::Platform,
        "renderer" => C::Renderer,
        "developer" => C::Developer,
        _ => C::General,
    };
    let leaf = path.rsplit('.').next().unwrap_or(path);
    let label = leaf
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ");
    let kind_name = match kind {
        Kind::Boolean => "true/false value",
        Kind::Integer => "whole number",
        Kind::Number => "number",
        Kind::String => "text value",
        Kind::Array => "JSON list",
        Kind::Object => "JSON object",
    };
    let group = path.split('.').next().unwrap_or("Neoism");
    let mut descriptor = d(
        path,
        &label,
        &format!("Set {label} in the {group} configuration. Accepts a {kind_name}."),
        kind,
        default,
        &[],
        !matches!(kind, Kind::Boolean),
        category,
        control,
    );
    descriptor.settings_visible = false;
    descriptor
}

fn enrich_runtime_suggestions(rows: &mut [D]) {
    let fonts = installed_font_families();
    let ide_themes = installed_ide_themes();
    let palettes = installed_themes();
    let packs = installed_mashup_packs();
    let shells = installed_shells();
    let agents = installed_agents();
    for row in rows {
        let (provider, values) = if row.path == "appearance.fonts.family"
            || row.path.starts_with("appearance.fonts.") && row.path.ends_with(".family")
        {
            (Some(Provider::SystemFonts), fonts.clone())
        } else {
            match row.path.as_str() {
                "appearance.theme" => (Some(Provider::IdeThemes), ide_themes.clone()),
                "appearance.palette" => {
                    (Some(Provider::TerminalPalettes), palettes.clone())
                }
                "appearance.mashup-pack" => (Some(Provider::MashupPacks), packs.clone()),
                "terminal.shell.program" => (Some(Provider::Shells), shells.clone()),
                "agent.default-agent" | "agent.agent" | "agent.mode" => {
                    (Some(Provider::AgentNames), agents.clone())
                }
                _ => (None, Vec::new()),
            }
        };
        if provider.is_some() {
            row.provider = provider;
        }
        row.runtime_suggestions = values.clone();
        row.options
            .extend(values.into_iter().map(|value| ConfigOption {
                value: json!(value),
                label: None,
                description: None,
            }));
    }
}

pub fn installed_font_families() -> Vec<String> {
    let (library, _) = sugarloaf::font::FontLibrary::new(Default::default());
    library.family_names()
}

fn installed_themes() -> Vec<String> {
    let mut values = BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(super::config_dir_path().join("themes")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) == Some("json") {
                if let Some(stem) = path.file_stem().and_then(|value| value.to_str()) {
                    values.insert(stem.to_string());
                }
            }
        }
    }
    values.into_iter().collect()
}

fn installed_ide_themes() -> Vec<String> {
    super::mashup::load_ide_theme_specs()
        .into_iter()
        .map(|theme| theme.name)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn installed_mashup_packs() -> Vec<String> {
    super::mashup::load_mashup_packs()
        .into_iter()
        .map(|pack| pack.id)
        .collect()
}

fn installed_shells() -> Vec<String> {
    let mut values = BTreeSet::new();
    if let Some(shell) = std::env::var_os("SHELL") {
        values.insert(shell.to_string_lossy().into_owned());
    }
    if let Ok(content) = std::fs::read_to_string("/etc/shells") {
        values.extend(
            content
                .lines()
                .map(str::trim)
                .filter(|line| line.starts_with('/'))
                .map(str::to_owned),
        );
    }
    values.into_iter().collect()
}

fn installed_agents() -> Vec<String> {
    let mut values = BTreeSet::from(["build".to_string(), "plan".to_string()]);
    for directory in ["agent", "mode"] {
        if let Ok(entries) = std::fs::read_dir(super::config_dir_path().join(directory)) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) == Some("md") {
                    if let Some(stem) = path.file_stem().and_then(|value| value.to_str())
                    {
                        values.insert(stem.to_string());
                    }
                }
            }
        }
    }
    values.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_are_unique_and_cover_golden_groups() {
        let descriptors = config_descriptors();
        let paths = descriptors
            .iter()
            .map(|row| row.path.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(paths.len(), descriptors.len());
        for prefix in [
            "appearance.",
            "editor.",
            "terminal.",
            "ui.",
            "presence.",
            "keybinds.",
            "agent.",
            "developer.",
        ] {
            assert!(
                paths.iter().any(|path| path.starts_with(prefix)),
                "missing {prefix}"
            );
        }
        assert!(paths.contains("platform.windows.renderer.backend"));
        assert!(paths.contains("renderer.disable-unfocused-render"));
        assert!(paths.contains("agent.enabled-providers"));
        assert!(paths.contains("agent.instructions"));
        assert!(paths.contains("agent.mcp"));
        assert!(paths.contains("agent.agent.*.model"));
        assert!(paths.contains("agent.mode.*.permission"));
        assert!(descriptors
            .iter()
            .find(|row| row.path == "agent.default-agent")
            .unwrap()
            .static_suggestions
            .contains(&"build".to_string()));

        let font = descriptors
            .iter()
            .find(|row| row.path == "appearance.fonts.family")
            .unwrap();
        assert_eq!(font.provider, Some(Provider::SystemFonts));
        assert!(font.options.iter().all(|option| option.value.is_string()));

        let platform_mode = descriptors
            .iter()
            .find(|row| row.path == "platform.windows.window.mode")
            .unwrap();
        assert_eq!(platform_mode.control, Control::Select);
        assert!(platform_mode
            .options
            .iter()
            .any(|option| option.value == json!("Fullscreen")));

        let font_size = descriptors
            .iter()
            .find(|row| row.path == "appearance.fonts.size")
            .unwrap();
        assert!(font_size
            .options
            .iter()
            .all(|option| option.value.is_number()));
        assert_eq!(font_size.constraints.unit.as_deref(), Some("pt"));
    }

    #[test]
    fn generated_completion_templates_are_not_graphical_settings() {
        let descriptors = config_descriptors();
        let visible = descriptors
            .iter()
            .filter(|row| row.settings_visible)
            .collect::<Vec<_>>();

        assert!(visible.iter().any(|row| row.path == "appearance.theme"));
        assert!(visible.iter().any(|row| row.path == "editor.vim-mode"));
        assert!(visible.iter().all(|row| !row.path.contains('*')));
        assert!(descriptors
            .iter()
            .find(|row| row.path == "platform.windows.window.mode")
            .is_some_and(|row| !row.settings_visible));
        assert!(descriptors
            .iter()
            .find(|row| row.path == "agent.agent.*.model")
            .is_some_and(|row| !row.settings_visible && row.category == C::Agent));
    }
}
