//! Original settings, appearance palette, About modal and extensions painters.
use neoism_ui::{
    event::{
        KeyDescriptor, KeyState, LogicalKey, Modifiers, NamedKey, PhysicalKey,
        PointerButton,
    },
    panels::{
        command_palette::{
            CommandPalette, PaletteAction, PaletteBackgroundEntry, PaletteDirectoryEntry,
            PaletteServerEntry, PaletteWorkspaceEntry, WorkspaceDirectoryTarget,
        },
        extensions_page::{
            ExtensionEntry, ExtensionKind, ExtensionStatus, NeoismExtensionsPane,
        },
        settings_page::{NeoismSettingsPane, SettingsAction},
        ServerIndicatorStatus,
    },
    primitives::{
        ide_theme::{all_ide_theme_names, IdeTheme},
        paint::Paint,
        paint_record::RecordingPainter,
    },
    widgets::modal::{ModalAction, ModalButton, ModalSpec, UniversalModal},
};
use serde_json::{json, Value};
use std::{cell::RefCell, collections::HashMap};
#[path = "../../../../neoism-backend/src/config/background/catalog.rs"]
mod background_catalog;
#[path = "server_forms.rs"]
mod server_forms;

enum Panel {
    Settings(NeoismSettingsPane),
    Extensions(NeoismExtensionsPane),
    About(UniversalModal),
    Appearance(CommandPalette),
}
struct Host {
    panel: Panel,
    painter: RecordingPainter,
    width: f32,
    height: f32,
    scale: f32,
    theme: String,
    mouse: Option<(f32, f32)>,
    kind: String,
    context: Value,
}
#[derive(Default)]
struct Store {
    next: u32,
    hosts: HashMap<u32, Host>,
}
thread_local! { static STORE:RefCell<Store>=RefCell::new(Store::default()); }
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn n(v: &Value, key: &str, fallback: f32) -> f32 {
    v[key]
        .as_f64()
        .filter(|n| n.is_finite())
        .map(|n| n as f32)
        .unwrap_or(fallback)
}
fn setting(action: Option<SettingsAction>) -> Value {
    match action {
        Some(SettingsAction::Set { key, value }) => {
            json!({"kind":"setting","key":key,"value":value})
        }
        Some(SettingsAction::SetKeybind { action, key, with }) => {
            json!({"kind":"keybind","action":action,"key":key,"with":with})
        }
        Some(SettingsAction::OpenConfigFile) => json!({"kind":"config-document"}),
        Some(SettingsAction::RunAction(action)) => json!({"kind":"run","action":action}),
        None => Value::Null,
    }
}
fn extensions(values: &Value) -> Vec<ExtensionEntry> {
    values
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| {
            let strings = |key| {
                v[key]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            };
            let status = match text(v, "status") {
                "built_in" => ExtensionStatus::BuiltIn,
                "detected" => ExtensionStatus::Detected,
                "unavailable" => ExtensionStatus::Unavailable,
                "installed" => ExtensionStatus::Installed {
                    version: v["installed_version"]
                        .as_str()
                        .unwrap_or(text(v, "version"))
                        .into(),
                },
                _ => ExtensionStatus::NotInstalled,
            };
            ExtensionEntry {
                kind: ExtensionKind::ManagedPackage,
                id: text(v, "id").into(),
                name: text(v, "name").into(),
                version: text(v, "version").into(),
                description: text(v, "description").into(),
                author: text(v, "author").into(),
                downloads: v["downloads"].as_u64(),
                categories: strings("categories"),
                languages: strings("languages"),
                status,
                repository_url: v["repository_url"].as_str().map(str::to_owned),
                lsp_source: v["lsp_source"].as_str().map(str::to_owned),
                lua_plugin: None,
            }
        })
        .collect()
}
fn backgrounds(values: &Value) -> Vec<PaletteBackgroundEntry> {
    values
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| PaletteBackgroundEntry {
            id: text(v, "id").into(),
            name: text(v, "name").into(),
            description: text(v, "description").into(),
            path: v["path"].as_str().map(str::to_owned),
            opacity: n(v, "opacity", 1.0),
            blur: n(v, "blur", 0.0),
            fit: serde_json::from_value(v["fit"].clone()).unwrap_or_default(),
            position_x: n(v, "position_x", 0.5),
            position_y: n(v, "position_y", 0.5),
        })
        .collect()
}
fn key(v: &Value) -> KeyDescriptor {
    let name = text(v, "key");
    let named = match name {
        "enter" => Some(NamedKey::Enter),
        "escape" => Some(NamedKey::Escape),
        "backspace" => Some(NamedKey::Backspace),
        "tab" => Some(NamedKey::Tab),
        "up" => Some(NamedKey::ArrowUp),
        "down" => Some(NamedKey::ArrowDown),
        "left" => Some(NamedKey::ArrowLeft),
        "right" => Some(NamedKey::ArrowRight),
        "home" => Some(NamedKey::Home),
        "end" => Some(NamedKey::End),
        "pageup" => Some(NamedKey::PageUp),
        "pagedown" => Some(NamedKey::PageDown),
        _ => None,
    };
    let mut modifiers = Modifiers::empty();
    for (name, flag) in [
        ("ctrl", Modifiers::CTRL),
        ("alt", Modifiers::ALT),
        ("shift", Modifiers::SHIFT),
        ("cmd", Modifiers::META),
    ] {
        if v[name].as_bool().unwrap_or(false) {
            modifiers |= flag;
        }
    }
    KeyDescriptor {
        physical: PhysicalKey(0),
        logical: named.map(LogicalKey::Named).unwrap_or_else(|| {
            LogicalKey::Character(v["char"].as_str().unwrap_or(name).into())
        }),
        state: KeyState::Pressed,
        modifiers,
        repeat: v["repeat"].as_bool().unwrap_or(false),
    }
}
impl Host {
    fn frame(&mut self, action: Value) -> Value {
        self.painter.begin_frame();
        let theme = IdeTheme::by_name(&self.theme);
        let mut image = Value::Null;
        let (active, animating) = match &mut self.panel {
            Panel::Settings(p) => {
                p.render(
                    &mut self.painter,
                    self.width,
                    self.height,
                    &theme,
                    self.scale,
                    self.mouse,
                );
                (p.is_active(), false)
            }
            Panel::Extensions(p) => {
                let animating = p.tick_scroll();
                p.render(
                    &mut self.painter,
                    [0.0, 0.0, self.width, self.height],
                    &theme,
                    self.scale,
                    self.mouse,
                    &[],
                );
                (true, animating)
            }
            Panel::About(p) => {
                p.render_portable(
                    &mut self.painter,
                    (self.width, self.height, 1.0),
                    &theme,
                );
                (p.is_active(), p.needs_redraw())
            }
            Panel::Appearance(p) => {
                p.render_with_background(
                    &mut self.painter,
                    (self.width, self.height, 1.0),
                    &theme,
                    |_, _, entry, rect, _| {
                        if let Some(path) = &entry.path {
                            image = json!({"path":path,"rect":rect});
                            true
                        } else {
                            false
                        }
                    },
                );
                (p.is_enabled(), p.is_animating())
            }
        };
        let target=match &self.panel {Panel::Appearance(p)=>p.get_selected_theme().map(|name|json!({"kind":"theme","name":name})).or_else(||p.selected_background().map(|entry|json!({"kind":"background","id":entry.id,"path":entry.path,"opacity":entry.opacity,"blur":entry.blur,"fit":entry.fit,"position_x":entry.position_x,"position_y":entry.position_y}))).or_else(||p.get_selected_workspace_target().map(|target|json!({"kind":"workspace","id":target.workspace_id}))).or_else(||match p.get_selected_action(){Some(action)=>Some(server_forms::palette_action(action)),_=>None}).or_else(||p.get_selected_cd_directory().map(|entry|json!({"kind":"directory","path":entry.absolute_path}))).or_else(||p.get_typed_cd_target().and_then(|path|neoism_ui::panels::command_palette::actions::parse_cd_operand(&path).ok()).map(|path|json!({"kind":"directory","path":path}))).unwrap_or(Value::Null),_=>Value::Null};
        let rect = match &self.panel {
            Panel::Appearance(p) => p.active_visual_rect(self.width, 1.0),
            Panel::About(p) if self.kind.starts_with("server-")||self.kind=="system-form" => p.active_rect(self.width,1.0),
            _ => None,
        };
        json!({"kind":self.kind,"active":active,"animating":animating,"commands":self.painter.commands(),"rect":rect,"image":image,"action":action,"target":target,"snapshot":match &self.panel {Panel::Settings(p)=>p.host_snapshot(),_=>Value::Null},"query":match &self.panel {Panel::Appearance(p)=>Some(p.query.as_str()),_=>None}})
    }
}
pub fn dispatch(v: &Value) -> Option<Result<Value, String>> {
    let op = text(v, "op");
    if !op.starts_with("menu.") {
        return None;
    }
    if op == "menu.background-catalog" {
        let configured =
            serde_json::from_value::<sugarloaf::ImageProperties>(v["image"].clone()).ok();
        let packs = v["packs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|pack| background_catalog::PackWallpaper {
                id: text(pack, "id").into(),
                name: text(pack, "name").into(),
                description: text(pack, "description").into(),
                wallpaper: serde_json::from_value(pack["wallpaper"].clone()).ok(),
            })
            .collect::<Vec<_>>();
        let entries=background_catalog::entries_at(configured.as_ref(),std::path::Path::new(text(v,"base")),&packs).into_iter().map(|entry|{let image=entry.image;json!({"id":image.path,"name":entry.name,"description":entry.description,"path":image.path,"opacity":image.opacity,"blur":image.blur,"fit":image.fit,"position_x":image.position_x,"position_y":image.position_y})}).collect::<Vec<_>>();
        return Some(Ok(json!({"entries":entries})));
    }
    if op == "menu.servers" || op == "menu.server-connect" {
        use neoism_agent_service_api::server_registry::{
            registry_directory, ServerRegistry,
        };
        let registry = match ServerRegistry::load(registry_directory()) {
            Ok(registry) => registry,
            Err(error) => return Some(Err(error.to_string())),
        };
        if op == "menu.server-connect" {
            return Some(registry.server(text(v,"serverId")).map(|server|json!({"id":server.id,"endpoint":server.endpoint,"pairingToken":registry.token(&server.id)})).ok_or_else(||"Saved server no longer exists".into()));
        }
        return Some(Ok(
            json!({"entries":registry.servers().iter().filter(|server|!server.agent_api).map(|server|json!({"id":server.id,"name":server.name,"address":server.endpoint,"local":false,"temporary":false,"hosted":server.hosted.is_some(),"active":server.endpoint==text(v,"endpoint")})).collect::<Vec<_>>()}),
        ));
    }
    Some(STORE.with(|store| {let mut store=store.borrow_mut();
        if op=="menu.new" {
            let scale=n(v,"scale",1.0);let kind=text(v,"kind");
            let panel=match kind {
                "settings"=>{let mut p=NeoismSettingsPane::new();p.set_values(v["values"].clone());p.set_descriptors(serde_json::from_value(v["descriptors"].clone()).map_err(|e|e.to_string())?);p.set_font_families(v["fontFamilies"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect());p.open();Panel::Settings(p)},
                "extensions"=>{let mut p=NeoismExtensionsPane::new();p.set_entries(extensions(&v["entries"]));p.set_host_read_only(true);p.focus_search();Panel::Extensions(p)},
                "directories"=>{let mut p=CommandPalette::new();p.set_scale(scale);p.host_set_supported_actions(Some(v["supportedActions"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()));p.open_commands_for_workspace("cd ",WorkspaceDirectoryTarget{workspace_id:v["workspaceId"].as_str().map(str::to_owned),root:text(v,"root").into()});Panel::Appearance(p)},
                "workspaces"=>{let mut p=CommandPalette::new();p.set_scale(scale);p.host_set_management_enabled(false);let entries=v["entries"].as_array().into_iter().flatten().map(|entry|{let mut row=PaletteWorkspaceEntry::local(text(entry,"title").into(),text(entry,"detail").into(),text(entry,"id").into());row.host_id=text(entry,"hostId").into();row.host_label=text(entry,"hostLabel").into();row.current=entry["current"].as_bool().unwrap_or(false);row}).collect();p.enter_workspaces_mode(entries);Panel::Appearance(p)},
                "servers"=>{let mut p=CommandPalette::new();p.set_scale(scale);p.host_set_management_enabled(true);let entries=v["entries"].as_array().into_iter().flatten().map(|entry|PaletteServerEntry{id:text(entry,"id").into(),name:text(entry,"name").into(),address:text(entry,"address").into(),local:entry["local"].as_bool().unwrap_or(false),temporary:entry["temporary"].as_bool().unwrap_or(false),hosted:entry["hosted"].as_bool().unwrap_or(false),status:if entry["active"].as_bool().unwrap_or(false){match text(entry,"status"){"connected"=>ServerIndicatorStatus::Online,"connecting"|"authenticating"|"waiting"=>ServerIndicatorStatus::Connecting,"closed"|"auth-rejected"=>ServerIndicatorStatus::Offline,_=>ServerIndicatorStatus::Unknown}}else{ServerIndicatorStatus::Unknown},active:entry["active"].as_bool().unwrap_or(false)}).collect();p.enter_servers_mode(entries);Panel::Appearance(p)},
                "systems"|"system-settings"=>{let mut p=CommandPalette::new();p.set_scale(scale);p.host_set_management_enabled(true);let entries=v["entries"].as_array().into_iter().flatten().map(server_forms::system).collect::<Vec<_>>();if kind=="system-settings"{p.enter_system_settings_mode(entries.into_iter().next().ok_or("System no longer exists")?);}else{p.enter_systems_mode(entries);}Panel::Appearance(p)},
                "server-form"|"system-form"|"server-confirm"|"server-message"=>Panel::About(server_forms::open(v)),
                "themes"|"backgrounds"=>{let mut p=CommandPalette::new();p.set_scale(scale);if kind=="themes"{p.enter_themes_mode(all_ide_theme_names());}else{p.enter_backgrounds_mode(backgrounds(&v["entries"]));p.select_clicked(v["selected"].as_u64().unwrap_or(0) as usize);}Panel::Appearance(p)},
                "about"=>{let mut p=UniversalModal::new();p.set_scale(scale);let cargo=include_str!("../../../../Cargo.toml");let version=cargo.split("[workspace.package]").nth(1).and_then(|s|s.lines().find(|s|s.trim().starts_with("version ="))).and_then(|s|s.split('"').nth(1)).unwrap_or("dev");p.open(ModalSpec{title:"About Neoism".into(),body:format!("Neoism  v{version}\n\nA terminal-first workspace for code, notes,\nagents, and multiplayer editing.\n\nCommit\n{}",option_env!("GIT_HASH").unwrap_or("dev")),meta:String::new(),input:None,buttons:vec![ModalButton::new("OK","Enter",ModalAction::Close)],busy:false,blocking:true});Panel::About(p)},
                _=>return Err("Unknown menu panel".into()),
            };
            store.next+=1;let id=0xA0000000+store.next;let mut host=Host{panel,painter:RecordingPainter::new(v["fontFamily"].as_str().unwrap_or("Cascadia Code NF"),v["fontWeight"].as_u64().unwrap_or(400) as u16,v["extraFamilies"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()),width:n(v,"width",1440.0),height:n(v,"height",900.0),scale,theme:v["theme"].as_str().unwrap_or("pastel_dark").into(),mouse:None,kind:kind.into(),context:v.clone()};
            if let Panel::Settings(p)=&mut host.panel {p.set_font_families(host.painter.font_library().family_names());p.host_restore_snapshot(&v["snapshot"]);}
            let frame=host.frame(Value::Null);store.hosts.insert(id,host);return Ok(json!({"id":id,"frame":frame}));
        }
        let id=v["id"].as_u64().unwrap_or(0) as u32;if op=="menu.dispose"{store.hosts.remove(&id);return Ok(json!({}));}
        let host=store.hosts.get_mut(&id).ok_or("Unknown menu panel")?;let mut action=Value::Null;
        if op=="menu.layout"{host.width=n(v,"width",host.width);host.height=n(v,"height",host.height);}
        if op=="menu.data"{match &mut host.panel {Panel::Settings(p)=>{p.set_values(v["values"].clone());},Panel::Extensions(p)=>p.set_entries(extensions(&v["entries"])),_=>{}}}
        let x=n(v,"x",0.0);let y=n(v,"y",0.0);
        if op=="menu.pointer"{host.mouse=Some((x,y));match &mut host.panel {Panel::Appearance(p)=>{p.hover(x,y,host.width,1.0);},Panel::Settings(p)=>p.pointer_move(x,y),Panel::About(p)=>{p.pointer_move(x,y,host.width,1.0);},_=>{}}}
        if op=="menu.scroll"{let delta=n(v,"delta",0.0);match &mut host.panel{Panel::Settings(p)=>{p.scroll_by(-delta);},Panel::Extensions(p)=>p.scroll_pixels(delta),Panel::Appearance(p)=>p.scroll_pixels(delta),Panel::About(p)=>{p.scroll_at(x,y,host.width,1.0,-delta);}}}
        if op=="menu.directories" {if let Panel::Appearance(p)=&mut host.panel {p.set_cd_directory_results(v["entries"].as_array().into_iter().flatten().filter_map(Value::as_str).map(PaletteDirectoryEntry::new).collect());}}
        if op=="menu.query"{if let Panel::Appearance(p)=&mut host.panel{p.set_query(text(v,"query").into());}}
        if op=="menu.click"{match &mut host.panel{
            Panel::Settings(p)=>action=setting(p.pointer_down(x,y).action),
            Panel::Extensions(p)=>{if let Some(neoism_ui::panels::extensions_page::PaneAction::OpenRepository(url))=p.on_pointer_down(x,y,PointerButton::Left){action=json!({"kind":"url","url":url});}},
            Panel::About(p) if host.kind!="about"=>action=server_forms::click(p,v,host.width,&host.kind,&host.context),
            Panel::About(p)=>{if p.close_button_hit(x,y)||p.hit_test(x,y,host.width,1.0).is_err(){p.close();}else if let Ok(Some(index))=p.hit_test(x,y,host.width,1.0){p.set_selected_index(index);p.close();}},
            Panel::Appearance(p)=>if let Some(category)=p.server_action_at(x,y){action=server_forms::palette_action(category);}else{match p.hit_test(x,y,host.width,1.0){Err(())=>p.set_enabled(false),Ok(Some(index))=>{p.select_clicked(index);action=json!({"kind":"accept"});},_=>{}}},
        }}
        if op=="menu.key"||op=="menu.text"{let event=key(v);let name=text(v,"key");let chars=v["text"].as_str().unwrap_or(text(v,"char"));match &mut host.panel{
            Panel::Settings(p)=>{if p.capturing().is_some(){if name=="escape"{p.cancel_capture();}else if !name.is_empty(){let with=[("ctrl","Control"),("shift","Shift"),("alt","Alt"),("cmd","Super")].iter().filter(|(key,_)|v[*key].as_bool().unwrap_or(false)).map(|(_,name)|*name).collect::<Vec<_>>().join(" | ");action=setting(p.finish_capture(name.into(),with));}}else{match name{"escape"=>{p.on_escape();},"backspace"=>p.backspace(),"enter"=>action=setting(p.commit_edit()),_=>for c in chars.chars().filter(|c|!c.is_control()){p.input_char(c);}}}},
            Panel::Extensions(p)=>{let response=p.on_key(&event);if !response.consumed&&!chars.is_empty(){p.on_text(chars);}},
            Panel::About(p) if host.kind!="about"=>action=server_forms::key(p,v,&host.kind,&host.context),
            Panel::About(p)=>match name{"escape"|"enter"=>p.close(),"up"=>p.move_selection_up(),"down"=>p.move_selection_down(),_=>{}},
            Panel::Appearance(p)=>match name{"escape"=>{if let Some(back)=p.system_settings_back_action(){action=server_forms::palette_action(back);}else{p.set_enabled(false);}},"up"=>p.move_selection_up(),"down"=>p.move_selection_down(),"backspace"=>{p.backspace_query();},"left"=>p.move_query_cursor_left(),"right"=>p.move_query_cursor_right(),"home"=>p.set_query_cursor(0),"end"=>{let end=p.query.len();p.set_query_cursor(end);},"tab"=>{if let Some(category)=p.category_switch_action(){action=server_forms::palette_action(category);}else if p.is_cd_query(){let reverse=v["shift"].as_bool().unwrap_or(false);if reverse||!p.tab_complete(){p.cycle_cd_selection(reverse);}}else if !p.tab_complete(){p.move_selection_down();}},"enter"=>{if host.kind!="directories"||p.get_selected_cd_directory().is_some()||p.typed_change_workspace_directory_intent().is_some(){action=json!({"kind":"accept"});}},_=>p.insert_query_text(chars)},
        }}
        Ok(host.frame(action))
    }))
}
