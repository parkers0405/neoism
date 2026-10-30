//! Real command-palette catalog, policies, and source-derived binding actions.
use neoism_ui::panels::command_palette::{
    host_command_catalog, CommandPalette, PaletteHostCapabilities, PaletteSurface,
};
use neoism_ui::editor::scroll_model::{parse_ex_command,GlobalExCommandPlan};
use regex::Regex;
use serde_json::{json, Value};
use std::{cell::RefCell, collections::HashMap, sync::LazyLock};
static ACTION_ALIASES: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../assets/command-aliases.json")).expect("valid command aliases")
});
fn parse_action(value: &str) -> Value {
    let lower = value.to_lowercase();
    if let Some(action) = ACTION_ALIASES.get(&lower) {
        return json!({"kind":"action","action":action});
    }
    for (pattern, action) in [
        (r"selecttab\(([^()]+)\)", "SelectTab"),
        (r"run\(([^()]+)\)", "Run"),
        (r"scroll\(([^()]+)\)", "Scroll"),
    ] {
        let regex = Regex::new(pattern).unwrap();
        if let Some(c) = regex.captures(&lower) {
            let value = &c[1];
            return match action {
                "SelectTab" => {
                    json!({"kind":"action","action":action,"argument":value.parse::<u64>().unwrap_or(0)})
                }
                "Scroll" => {
                    json!({"kind":"action","action":action,"argument":value.parse::<i32>().unwrap_or(1)})
                }
                _ => {
                    let mut args = value.split(' ');
                    let program = args.next().unwrap_or("");
                    json!({"kind":"action","action":action,"program":program,"args":args.collect::<Vec<_>>()})
                }
            };
        }
    }
    json!({"kind":"action","action":"None"})
}
struct HostCommands {
    palette: CommandPalette,
    width: f32,
    height: f32,
}
fn close_target(query:&str)->Option<(&'static str,&'static str,bool)> {
    let command=query.trim().trim_start_matches(':').trim();
    parse_ex_command(command).and_then(|(head,tail)|match GlobalExCommandPlan::classify(&head,&tail){
        GlobalExCommandPlan::CloseFocusedBufferTab{discard}=>Some(("CloseTab", "Close tab",discard)),
        GlobalExCommandPlan::CloseActiveWorkspace{discard}=>Some(("CloseWorkspace", "Close workspace",discard)),
        _=>None,
    })
}
// The desktop router resolves exact close commands before fuzzy catalog matches.
fn frame(host:&mut HostCommands)->Value {
    let mut frame=host.palette.host_frame(host.width,host.height);
    let command=host.palette.query.trim().trim_start_matches(':').trim();
    let target=close_target(command);
    if let Some((action,title,discard))=target {
        frame["target"]=json!({"action":action,"argument":if discard{"discard"}else{""}});
        frame["rows"]=json!([{"index":0,"score":0,"title":title,"displayTitle":format!("workspace: {title}"),"shortcut":command,"action":action,"service":"workspace","glyph":"\u{f00d}","exCommand":null}]);
        frame["selected"]=json!(0);frame["top"]=json!(0);frame["selectedY"]=json!(0);frame["offset"]=json!(0);
        let row_height=frame["rowHeight"].as_f64().unwrap_or(28.0);
        if frame["listRect"][3].as_f64().unwrap_or(0.0)<row_height {
            frame["listRect"][3]=json!(row_height);
            frame["rect"][3]=json!(frame["listRect"][1].as_f64().unwrap_or(0.0)+row_height+frame["stroke"].as_f64().unwrap_or(2.0));
        }
    }
    frame
}
#[derive(Default)]
struct Store {
    next: u32,
    commands: HashMap<u32, HostCommands>,
}
thread_local! {static STORE:RefCell<Store>=RefCell::new(Store::default());}
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn number(v: &Value, key: &str, default: f32) -> f32 {
    v[key]
        .as_f64()
        .map(|n| n as f32)
        .filter(|n| n.is_finite())
        .unwrap_or(default)
}
fn surface(v: &Value) -> PaletteSurface {
    match text(v, "surface") {
        "file" | "editor" => PaletteSurface::Editor,
        "markdown" => PaletteSurface::Markdown,
        "notebook" => PaletteSurface::Notebook,
        "epub" => PaletteSurface::Epub,
        _ => PaletteSurface::Terminal,
    }
}
fn capabilities(v: &Value) -> PaletteHostCapabilities {
    let read = |key: &str| v["capabilities"][key].as_bool().unwrap_or(true);
    PaletteHostCapabilities {
        backgrounds: read("backgrounds"),
        lsp_actions: read("lsp_actions"),
        notebook_kernel: read("notebook_kernel"),
        draw_notes: read("draw_notes"),
        leave_workspace: read("leave_workspace"),
        settings_editor: read("settings_editor"),
        neoworld_page: read("neoworld_page"),
        minimap: read("minimap"),
    }
}
pub fn dispatch(v: &Value) -> Option<Result<Value, String>> {
    let op = text(v, "op");
    if !op.starts_with("commands.") {
        return None;
    }
    if op=="commands.close-target" {
        return Some(Ok(close_target(text(v,"query")).map(|(action,_,discard)|json!({"action":action,"argument":if discard{"discard"}else{""}})).unwrap_or(Value::Null)));
    }
    if op == "commands.catalog" {
        return Some(Ok(host_command_catalog()));
    }
    if op == "commands.parse-action" {
        return Some(Ok(parse_action(text(v, "action"))));
    }
    if op == "commands.action-aliases" {
        return Some(Ok(json!(&*ACTION_ALIASES)));
    }
    Some(STORE.with(|store| {
        let mut store = store.borrow_mut();
        if op == "commands.new" {
            store.next += 1;
            let id = 0x80000000 + store.next;
            let mut palette = CommandPalette::new();
            palette.set_scale(number(v, "scale", 1.0));
            palette.set_surface(surface(v));
            palette.set_host_capabilities(capabilities(v));
            palette.host_set_supported_actions(v["supportedActions"].as_array().map(|items|items.iter().filter_map(Value::as_str).map(ToString::to_string).collect()));
            palette.has_adaptive_theme = v["adaptiveTheme"].as_bool().unwrap_or(false);
            palette.open_commands_with_query(text(v, "query"));
            let mut host = HostCommands {
                palette,
                width: number(v, "width", 1440.0),
                height: number(v, "height", 900.0),
            };
            let frame = frame(&mut host);
            store.commands.insert(id, host);
            return Ok(json!({"id":id,"frame":frame}));
        }
        let id = v["id"].as_u64().unwrap_or(0) as u32;
        if op == "commands.dispose" {
            store.commands.remove(&id);
            return Ok(json!({}));
        }
        let host = store
            .commands
            .get_mut(&id)
            .ok_or("Unknown command palette")?;
        match op {
            "commands.frame" => {}
            "commands.query" => host.palette.set_query(text(v, "query").into()),
            "commands.move" => {
                let delta = v["delta"].as_i64().unwrap_or(1);
                for _ in 0..delta.unsigned_abs().min(500) {
                    if delta < 0 {
                        host.palette.move_selection_up();
                    } else {
                        host.palette.move_selection_down();
                    }
                }
            }
            "commands.select" => host
                .palette
                .select_clicked(v["index"].as_u64().unwrap_or(0) as usize),
            "commands.scroll" => host.palette.scroll_pixels(number(v, "delta", 0.0)),
            "commands.layout" => {
                host.width = number(v, "width", host.width).max(1.0);
                host.height = number(v, "height", host.height).max(1.0);
            }
            "commands.surface" => host.palette.set_surface(surface(v)),
            _ => return Err(format!("Unknown command operation: {op}")),
        }
        Ok(frame(host))
    }))
}
