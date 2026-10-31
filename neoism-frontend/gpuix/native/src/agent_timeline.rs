//! Original agent timeline: the shared `NeoismAgentPane` model, its cached
//! layout/scroll/selection, and the original renderer recorded for one host
//! canvas. Daemon Agent events go through the same dispatcher as the web host.
use neoism_protocol::agent::AgentServerMessage;
use neoism_ui::{
    editor::scroll_model::agent_timeline_wheel,
    panels::{
        agent_pane::{
        server_events::{agent_event_session_id, apply_agent_event_to_pane,apply_agent_event_to_pane_with_history_kind,apply_agent_event_to_cache_with_history_kind},
            state::NeoismAgentPane,
            view::{
                layout::chat_column,
                markdown::{copied_code_from_link_target, mermaid_toggle_key_from_link_target},
                timeline::{
                    render_timeline_in_paint_viewport_with, render_timeline_scrollbar_with,
                    SharedTimelineDelegate,
                },
            },
        },
        completion_menu::ScrollDelta,
    },
    primitives::{ide_theme::IdeTheme, paint::Paint, paint_record::RecordingPainter},
};
use serde_json::{json, Value};
use std::{cell::RefCell, collections::HashMap};

struct View {
    scroll_trace: Option<ScrollTrace>,
    compact_paint: bool,
    canvas_paint: bool,
    font_family: String,
    font_weight: u16,
    pane: NeoismAgentPane,
    painter: RecordingPainter,
    session_id: String,
    width: f32,
    height: f32,
    /// Composer top: content layout ends here; paint runs to `height`.
    content_bottom: f32,
    scale: f32,
    theme: String,
    mouse: Option<(f32, f32)>,
    selecting: bool,
    /// Last finished drag selection, for Ctrl+C after mouse-up.
    selection: Option<String>,
    scrollbar_drag: bool,
    horizontal_drag: bool,
}

struct ScrollTrace {
    origin: std::time::Instant,
    last: std::time::Instant,
    text_y: HashMap<u64, f32>,
    input_pixels: f32,
    input_events: u32,
    precise_events: u32,
}

fn trace_scroll(view: &mut View) {
    use std::hash::{Hash, Hasher};
    use neoism_ui::primitives::paint_record::PaintCommand;
    let Some(trace) = view.scroll_trace.as_mut() else { return };
    let now = std::time::Instant::now();
    let mut positions = HashMap::new();
    for command in view.painter.borrowed_commands().iter() {
        if let PaintCommand::Text { text, x, y, .. } = command {
            if *y < 0.0 || *y > view.content_bottom || text.is_empty() { continue; }
            let mut key = std::collections::hash_map::DefaultHasher::new();
            text.hash(&mut key); x.round().to_bits().hash(&mut key);
            positions.insert(key.finish(), *y);
        }
    }
    let mut movements: Vec<_> = positions.iter().filter_map(|(key, y)| trace.text_y.get(key).map(|before| y-before)).collect();
    movements.sort_by(f32::total_cmp);
    let movement = movements.get(movements.len()/2).copied();
    let metrics = view.pane.timeline_scrollbar_state().map(|(offset,content,viewport,_)| [offset,content,viewport]);
    eprintln!("[neoism-scroll-frame] {}", json!({"timeUs":now.duration_since(trace.origin).as_micros(),"dtUs":now.duration_since(trace.last).as_micros(),"medianTextDy":movement,"matchedRuns":movements.len(),"metrics":metrics,"inputPixels":trace.input_pixels,"inputEvents":trace.input_events,"preciseEvents":trace.precise_events,"reason":view.pane.animation_reason()}));
    trace.last=now; trace.text_y=positions; trace.input_pixels=0.0;trace.input_events=0;trace.precise_events=0;
}

#[derive(Default)]
struct Store {
    next: u32,
    views: HashMap<u32, View>,
}
thread_local! { static STORE: RefCell<Store> = RefCell::new(Store::default()); }

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn number(v: &Value, key: &str, fallback: f32) -> f32 {
    v[key].as_f64().map_or(fallback, |n| n as f32)
}

/// Wrapped phase, never raw epoch seconds (f32 loses sub-second resolution).
fn now_seconds() -> f32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| neoism_ui::render_policy::animation_phase_from_unix_secs(d.as_secs(), d.subsec_nanos()))
        .unwrap_or(0.0)
}

fn render_frame(view: &mut View) -> Value {
    let s = view.scale;
    let theme = IdeTheme::by_name(&view.theme);
    let rect = [0.0, 0.0, view.width, view.height];
    // Same column and question gutter as the original `render_chat`.
    let (content_x, content_w) = chat_column(rect, s);
    let bottom = view.content_bottom.clamp(0.0, view.height);

    let gutter_x = content_x + content_w;
    let gutter = [gutter_x, 0.0, (view.width - gutter_x).max(0.0), bottom];
    // Advance kinetic scroll before layout, like the original pane render, so
    // inertia and paint stay in lockstep.
    let scrolling = view.pane.tick_timeline_scroll();
    view.painter.begin_frame();
    let painter: &mut dyn Paint = &mut view.painter;
    let top=neoism_ui::panels::agent_pane::view::chat::render_conversation_header(painter,&mut view.pane,rect,&theme,view.mouse,s,&[]);
    let metrics=[content_x,top,content_w,(bottom-top).max(0.0)];
    // The composer floats over the transcript. Scroll metrics still stop at
    // its top, but content paints behind its rounded corners and material.
    // Markdown controls use the metrics viewport independently of this clip.
    let paint=[content_x,top,content_w,(view.height-top).max(0.0)];
    let header={use neoism_ui::panels::agent_pane::view::chat::AgentChatPane;
      view.pane.conversation_header().visible_links().map(|(direction,target,rects)|json!({"direction":format!("{:?}",direction).to_lowercase(),"target":target,"rects":rects})).collect::<Vec<_>>()};
    let rail = render_timeline_in_paint_viewport_with::<NeoismAgentPane, SharedTimelineDelegate>(
        painter,
        &mut view.pane,
        metrics,
        paint,
        &theme,
        s,
        now_seconds(),
        view.mouse,
        &[],
        &[],
        None,
        Some(gutter),
    );
    render_timeline_scrollbar_with(painter, &mut view.pane, &theme, s, rail.as_ref());
    let pointer = view.mouse.is_some_and(|(x, y)| {
        view.pane.conversation_navigation_at(x, y).is_some()
            || view.pane.link_at(x, y).is_some()
            || view.pane.tool_hit(x, y)
            || view.pane.activity_status_contains(x, y)
            || view.pane.wordmark_hit(x, y)
    });
    trace_scroll(view);
    let send={
        use neoism_ui::panels::agent_pane::{protocol_mapping::{map_outbound_command,AgentProtocolMapping,AgentProtocolMappingContext},outbound::OutboundAgentCommand};
        let context=AgentProtocolMappingContext{active_session_id:Some(view.session_id.clone()),default_directory:None,default_agent:None,default_model:None,default_connection_id:None,default_thinking:None,local_author:view.pane.local_presence_name().map(str::to_owned)};
        view.pane.drain_pending_outbound().into_iter().filter(|command|!matches!(command,OutboundAgentCommand::SwitchSession{..})).flat_map(|command|match map_outbound_command(command,&context){AgentProtocolMapping::Messages(messages)=>messages.into_iter().filter_map(|message|serde_json::to_value(message).ok()).collect::<Vec<_>>(),_=>vec![]}).collect::<Vec<_>>()
    };
    json!({
        "animating": scrolling || view.pane.animation_reason().is_some(),
        "reason": view.pane.animation_reason(),
        "selecting": view.selecting,
        "pointer": pointer,
        "hasConversation": view.pane.has_conversation(),
        "send":send,"header":header,"subagent":view.pane.is_subagent_session(),"sessionId":view.session_id,
    })
}

fn frame(view: &mut View) -> Value {
    let mut metadata = render_frame(view);
    if view.canvas_paint {
        metadata["commands"] = json!([]);
        metadata["canvas"] = serde_json::to_value(crate::paint_canvas::encode(&view.painter.borrowed_commands(), &view.font_family, view.font_weight)).unwrap();
    } else { metadata["commands"] = serde_json::to_value(view.painter.serializable_commands(view.compact_paint)).unwrap(); }
    metadata
}

/// The animation hot path writes borrowed paint commands straight into the
/// response buffer. Building a serde_json::Value tree first allocates a map
/// and strings for every primitive on every display frame.
pub fn serialized_frame(v: &Value) -> Option<Result<String, String>> {
    if text(v, "op") != "agenttimeline.frame" { return None; }
    Some(STORE.with(|store| {
        let mut store = store.borrow_mut();
        let id = v["id"].as_u64().unwrap_or(0) as u32;
        let view = store.views.get_mut(&id).ok_or_else(|| "Unknown agent timeline".to_owned())?;
        if let Some(compact) = v["compactPaint"].as_bool() { view.compact_paint = compact; }
        if let Some(canvas) = v["canvasPaint"].as_bool() { view.canvas_paint = canvas; }
        let metadata = render_frame(view);
        #[derive(serde::Serialize)]
        struct Frame<C: serde::Serialize> {
            #[serde(flatten)]
            metadata: Value,
            commands: C,
            #[serde(skip_serializing_if="Option::is_none")]
            canvas: Option<crate::paint_canvas::CanvasPacket>,
        }
        #[derive(serde::Serialize)]
        struct Response<T: serde::Serialize> { ok: T }
        let canvas = view.canvas_paint.then(|| crate::paint_canvas::encode(&view.painter.borrowed_commands(), &view.font_family, view.font_weight));
        if canvas.is_some() && !v["includeCommands"].as_bool().unwrap_or(false) {
            return serde_json::to_string(&Response {ok: Frame {metadata,commands:Vec::<u8>::new(),canvas}}).map_err(|error|error.to_string());
        }
        let response = serde_json::to_string(&Response { ok: Frame {
            metadata, commands: view.painter.serializable_commands(view.compact_paint), canvas,
        }}).map_err(|error| error.to_string());
        response
    }))
}

fn pointer(view: &mut View, v: &Value) -> Value {
    let (x, y) = (number(v, "x", 0.0), number(v, "y", 0.0));
    view.mouse = Some((x, y));
    let pane = &mut view.pane;
    let mut copy = None;
    let mut link = None;
    match text(v, "kind") {
        "down" => {
            if pane.activate_conversation_navigation(x,y){
                view.session_id=pane.session_id_str().unwrap_or_default().to_owned();
                pane.drain_pending_outbound();
                return json!({"navigate":view.session_id});
            }
            pane.clear_selection();
            view.selection = None;
            // Original chain order (web `agent_pointer_down`, desktop bridges):
            // scrollbars, links/code copy, tool and activity toggles, then text
            // selection last.
            if pane.begin_scrollbar_drag(x, y) {
                view.scrollbar_drag = true;
            } else if pane.begin_markdown_horizontal_scrollbar_drag(x, y) {
                view.horizontal_drag = true;
            } else if let Some(target) = pane.link_at(x, y) {
                if let Some(key) = mermaid_toggle_key_from_link_target(&target) {
                    pane.toggle_mermaid_raw_mode(key);
                } else if let Some(text) = copied_code_from_link_target(&target) {
                    pane.mark_code_copied(&target);
                    pane.push_copied_notice(text.chars().count());
                    copy = Some(text);
                } else {
                    link = Some(target);
                }
            } else if pane.pop_activity_status_click(x, y) || pane.toggle_tool_at(x, y) || pane.pop_wordmark_click(x, y) {
            } else if pane.begin_selection_at(x, y) {
                view.selecting = true;
            }
        }
        "move" => {
            if view.scrollbar_drag {
                pane.drag_scrollbar_to(x, y);
            } else if view.horizontal_drag {
                pane.drag_markdown_horizontal_scrollbar_to(x);
            } else if view.selecting {
                pane.scroll_for_drag_edge(y);
                pane.drag_selection_to(x, y);
            }
        }
        "up" => {
            if view.scrollbar_drag {
                pane.end_scrollbar_drag();
            }
            view.scrollbar_drag = false;
            view.horizontal_drag = false;
            if view.selecting {
                view.selecting = false;
                // Desktop copies a finished drag selection on mouse-up.
                copy = pane.end_selection();
                view.selection = copy.clone();
            }
        }
        "leave" => view.mouse = None,
        _ => {}
    }
    json!({"copy": copy, "link": link})
}

pub fn dispatch(v: &Value) -> Option<Result<Value, String>> {
    let op = v["op"].as_str()?;
    if op == "agenttimeline.diagnostics" {
        return Some(Ok(STORE.with(|store| {
            json!(store.borrow().views.values().map(|view| {
                use neoism_ui::primitives::paint_record::PaintCommand;
                let mut counts = [0usize; 8];
                for command in view.painter.borrowed_commands().iter() {
                    match command {
                        PaintCommand::Rect { .. } => counts[0] += 1,
                        PaintCommand::Ring { .. } => counts[1] += 1,
                        PaintCommand::Polygon { .. } => counts[2] += 1,
                        PaintCommand::Image { .. } => counts[3] += 1,
                        PaintCommand::Text { raster, text, .. } => {
                            counts[4] += 1;
                            if raster.is_some() { counts[5] += 1; } else { counts[6] += 1; }
                            if raster.is_none() && text.bytes().all(|byte| matches!(byte,b' '|b'\t'|b'\r'|b'\n')) { counts[7] += 1; }
                        }
                    }
                }
                json!({
                "sessionId": view.session_id,
                "offset": view.pane.timeline_scroll_offset(),
                "scrollbar": view.pane.timeline_scrollbar_state().map(|(offset, content, viewport, _)| json!({"offset":offset,"content":content,"viewport":viewport})),
                "visibleRange": view.pane.virtual_timeline_visible_source_range(),
                "animation": view.pane.animation_reason(),
                "commandsByKind": {"rect":counts[0],"ring":counts[1],"polygon":counts[2],"image":counts[3],"text":counts[4],"rasterText":counts[5],"unrasterizedText":counts[6],"blankText":counts[7]},
            })}).collect::<Vec<_>>())
        })));
    }
    if !op.starts_with("agenttimeline.") {
        return None;
    }
    if op == "agenttimeline.identity" {
        let host = v["host"].as_str().filter(|host|!host.trim().is_empty()).unwrap_or("neoism");
        let user = v["user"].as_str().unwrap_or("user");
        let display_name = neoism_ui::editor::crdt::resolve_presence_display_name(v["envName"].as_str(),v["configName"].as_str(),host);
        let peer_id = format!("{user}@{host}");
        return Some(Ok(json!({"peerId":peer_id,"displayName":display_name,"color":neoism_ui::editor::crdt::stable_presence_color(&peer_id)})));
    }
    Some(STORE.with(|store| {
        let mut store = store.borrow_mut();
        if op == "agenttimeline.new" {
            store.next += 1;
            let id = 0x73000000 + store.next;
            let family = text(v, "fontFamily");
            let document = text(v, "markdownFontFamily");
            let painter = RecordingPainter::new(
                if family.is_empty() { "Cascadia Code NF" } else { family },
                v["fontWeight"].as_u64().unwrap_or(400) as u16,
                if document.is_empty() { vec![] } else { vec![document.to_owned()] },
            );
            let session_id = text(v, "sessionId").to_owned();
            let mut pane = NeoismAgentPane::new();
            pane.set_local_presence_name(v["localName"].as_str().map(str::to_owned));
            pane.set_session_id(Some(v["rootSessionId"].as_str().unwrap_or(&session_id).to_owned()));
            store.views.insert(id, View {
                scroll_trace: std::env::var_os("NEOISM_GPUIX_SCROLL_TRACE").map(|_| {let now=std::time::Instant::now();ScrollTrace{origin:now,last:now,text_y:HashMap::new(),input_pixels:0.0,input_events:0,precise_events:0}}),
                compact_paint: v["compactPaint"].as_bool().unwrap_or(false),
                canvas_paint: v["canvasPaint"].as_bool().unwrap_or(false),
                font_family: if family.is_empty() {"Cascadia Code NF".to_owned()} else {family.to_owned()},
                font_weight: v["fontWeight"].as_u64().unwrap_or(400) as u16,
                pane,
                painter,
                session_id,
                width: 800.0,
                height: 600.0,
                content_bottom: 600.0,
                scale: number(v, "scale", 1.0),
                theme: text(v, "theme").to_owned(),
                mouse: None,
                selecting: false,
                selection: None,
                scrollbar_drag: false,
                horizontal_drag: false,
            });
            return Ok(json!({"id": id}));
        }
        let id = v["id"].as_u64().unwrap_or(0) as u32;
        if op == "agenttimeline.dispose" {
            store.views.remove(&id);
            return Ok(Value::Null);
        }
        let view = store.views.get_mut(&id).ok_or("Unknown agent timeline")?;
        match op {
            "agenttimeline.event" => {
                let parsed: AgentServerMessage =
                    serde_json::from_value(v["message"].clone()).map_err(|error| error.to_string())?;
                let history_older = match v["historyKind"].as_str() { Some("head") => Some(false), Some("older") => Some(true), _ => None };
                let visible = agent_event_session_id(&parsed).is_none_or(|session| session == view.session_id);
                // Cached text/history changes cannot change the viewed transcript.
                // Keep lifecycle/roster events paintable: they may update family
                // activity in the conversation header even for another session.
                let needs_paint = visible || !matches!(&parsed,
                    AgentServerMessage::ContentDelta { .. }
                    | AgentServerMessage::MessageUpdated { .. }
                    | AgentServerMessage::HistoryChunk { .. }
                    | AgentServerMessage::MessageEnd { .. });
                // Only this conversation's events reach its pane (the hosts'
                // live gate); session-less events (notices) always apply.
                if visible {
                    apply_agent_event_to_pane_with_history_kind(&mut view.pane, parsed, history_older);
                }else{apply_agent_event_to_cache_with_history_kind(&mut view.pane,parsed,history_older);}
                // Events never publish paint; the host schedules a frame.
                Ok(json!({"animating": view.pane.animation_reason().is_some(), "needsPaint": needs_paint}))
            }
            "agenttimeline.family"=>{
                use neoism_ui::panels::agent_pane::state::side_panel::NeoismAgentSessionEntry;
                let root=text(v,"rootSessionId");
                let mut rows=vec![NeoismAgentSessionEntry::new(root,text(v,"rootTitle"),"")];
                if let Some(roster)=v["roster"].as_array(){for row in roster{
                    let id=text(row,"id");if id.is_empty()||id==root{continue;}
                    rows.push(NeoismAgentSessionEntry::new(id,text(row,"title"),text(row,"agent"))
                        .with_depth(row["depth"].as_u64().unwrap_or(1)as usize)
                        .with_runtime_status(Some(text(row,"status").to_owned())));
                }}
                let session=text(v,"sessionId");
                // A direct child link still has a Back target before its first
                // runtime/roster response. Preserve the canonical viewed-row rule.
                if !session.is_empty()&&session!=root&&!rows.iter().any(|row|row.id==session){rows.push(NeoismAgentSessionEntry::new(session,text(v,"title"),"").with_depth(1).with_runtime_status(Some("running".into())));}
                view.pane.side_panel_mut().set_subagents(rows);
                if !session.is_empty(){view.pane.switch_session(session.to_owned());view.session_id=session.to_owned();}
                view.pane.drain_pending_outbound();
                Ok(frame(view))
            }
            "agenttimeline.layout" => {
                if v.get("localName").is_some(){view.pane.set_local_presence_name(v["localName"].as_str().map(str::to_owned));}
                view.width = number(v, "width", view.width).max(1.0);
                view.height = number(v, "height", view.height).max(1.0);
                view.content_bottom = number(v, "contentBottom", view.height);
                view.scale = number(v, "scale", view.scale);
                if let Some(theme) = v["theme"].as_str() {
                    view.theme = theme.to_owned();
                }
                Ok(frame(view))
            }
            "agenttimeline.frame" => {
                if let Some(compact) = v["compactPaint"].as_bool() { view.compact_paint = compact; }
                Ok(frame(view))
            },
            "agenttimeline.scroll" => {
                let precise=v["precise"].as_bool().unwrap_or(false);
                let dx=number(v,"deltaX",0.0);let dy=number(v,"deltaY",0.0);
                let horizontal=if precise {
                    if dx.abs()>dy.abs() && dx.abs()>0.5 {-dx} else if v["shift"].as_bool().unwrap_or(false) && dy.abs()>0.5 {-dy} else {0.0}
                } else if dx.abs()>0.01 {-dx*48.0} else if v["shift"].as_bool().unwrap_or(false) {-dy*48.0} else {0.0};
                let point=v["x"].as_f64().zip(v["y"].as_f64()).map(|(x,y)|(x as f32,y as f32)).or(view.mouse);
                if horizontal.abs()>0.01 {
                    if let Some((x,y))=point {
                        if view.pane.scroll_markdown_horizontal_at(x,y,horizontal).is_some() {
                            // Consume at both edges too: sideways input must
                            // never leak into vertical timeline movement.
                            if v["deferFrame"].as_bool().unwrap_or(false) {return Ok(Value::Null);}
                            return Ok(frame(view));
                        }
                    }
                }
                let delta = if precise {
                    ScrollDelta::Pixels { x: number(v, "deltaX", 0.0), y: number(v, "deltaY", 0.0) }
                } else {
                    ScrollDelta::Lines { x: number(v, "deltaX", 0.0), y: number(v, "deltaY", 0.0) }
                };
                let wheel = agent_timeline_wheel(&delta, number(v, "lineHeight", 24.0));
                if let Some(trace) = view.scroll_trace.as_mut() {trace.input_pixels+=wheel.pixels;trace.input_events+=1;trace.precise_events+=u32::from(!wheel.smooth);}
                if wheel.smooth {
                    view.pane.scroll_timeline_wheel_pixels(wheel.pixels);
                } else {
                    view.pane.scroll_timeline_pixels(wheel.pixels);
                }
                if v["deferFrame"].as_bool().unwrap_or(false) { return Ok(Value::Null); }
                Ok(frame(view))
            }
            "agenttimeline.half" => {
                view.pane.scroll_timeline_half_page(v["older"].as_bool().unwrap_or(false));
                if v["deferFrame"].as_bool().unwrap_or(false) {return Ok(Value::Null);}
                Ok(frame(view))
            }
            "agenttimeline.pointer" => {
                let result = pointer(view, v);
                // Hover/drag moves coalesce: the host paints once per display frame.
                if v["deferFrame"].as_bool().unwrap_or(false) {
                    return Ok(result);
                }
                let mut out = frame(view);
                out["copy"] = result["copy"].clone();
                out["link"] = result["link"].clone();
                out["navigate"] = result["navigate"].clone();
                Ok(out)
            }
            "agenttimeline.copy" => Ok(json!({"text": view.selection})),
            _ => Err("Unknown agent timeline operation".into()),
        }
    }))
}

// ── Conversations side panel ─────────────────────────────────────────
// The desktop left sidebar (`conversations_pane` + `render_side_panel_with_icons`):
// catalog, project groups, running/sub-agent/background indicators. Clicks
// mirror desktop `handle_conversations_click`; navigation is the host's.

struct Panel {
    pane: NeoismAgentPane,
    painter: RecordingPainter,
    directory: Option<String>,
    width: f32,
    height: f32,
    scale: f32,
    theme: String,
    mouse: Option<(f32, f32)>,
    viewed_session: Option<String>,
    catalog: CatalogPump,
}

/// The web host's catalog driver (`wasm/src/rendered/agent.rs`
/// `refresh_agent_catalog` / `pump_project_sessions`) over the same daemon
/// socket: a root dashboard discovers projects and pages each one; a declared
/// workspace pages only its own chats. Replies are matched to this panel's own
/// requests so another consumer's `ListThreads` never replaces loaded pages.
#[derive(Default)]
struct CatalogPump {
    identity: String,
    discovery_generation: u64,
    discovery_identity: Option<String>,
    discovery_retry_at: Option<std::time::Instant>,
    thread_lists_pending: u32,
    refresh_requested: bool,
    /// The workspace directory is not known yet: no scope, no requests.
    unscoped: bool,
}

/// Desktop `catalog_scope_for_workspace`: the filesystem root or the local
/// home directory is the all-workspace dashboard; anything else is exact.
fn workspace_catalog_scope(directory: Option<&str>) -> Option<String> {
    let directory = directory?.trim_end_matches(['/', '\\']);
    let bytes = directory.as_bytes();
    let filesystem_root = directory.is_empty()
        || (bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':');
    let home = dirs::home_dir().map(|home| home.to_string_lossy().trim_end_matches(['/', '\\']).to_owned());
    if filesystem_root || home.as_deref() == Some(directory) {
        None
    } else {
        Some(directory.to_owned())
    }
}

fn refresh_catalog(panel: &mut Panel, out: &mut Vec<Value>) {
    use neoism_protocol::agent::AgentClientMessage;
    let directory = panel.pane.side_panel().catalog_directory().map(str::to_owned);
    if !panel.pane.side_panel().project_pagination_enabled() {
        panel.pane.side_panel_mut().mark_refresh_kicked();
    }
    let message = if directory.is_some() {
        panel.catalog.thread_lists_pending += 1;
        AgentClientMessage::ListThreads { directory, limit: Some(50), cursor: None }
    } else {
        panel.catalog.discovery_generation = panel.catalog.discovery_generation.wrapping_add(1);
        panel.catalog.discovery_identity = Some(panel.catalog.identity.clone());
        panel.catalog.discovery_retry_at = Some(std::time::Instant::now() + std::time::Duration::from_millis(850));
        AgentClientMessage::ListSessionProjects {
            catalog_identity: panel.catalog.identity.clone(),
            generation: panel.catalog.discovery_generation,
        }
    };
    out.extend(serde_json::to_value(message).ok());
}

fn pump_catalog(panel: &mut Panel, out: &mut Vec<Value>) {
    use neoism_protocol::agent::AgentClientMessage;
    if panel.catalog.unscoped {
        return;
    }
    let grouped = panel.pane.side_panel().catalog_directory().is_none();
    let identity_changed = grouped && panel.catalog.discovery_identity.as_deref() != Some(panel.catalog.identity.as_str());
    let retry_due = panel.pane.side_panel().should_refresh_sessions()
        && panel.catalog.discovery_retry_at.is_none_or(|at| std::time::Instant::now() >= at);
    if std::mem::take(&mut panel.catalog.refresh_requested) || identity_changed || retry_due {
        refresh_catalog(panel, out);
    }
    if !grouped {
        return;
    }
    for (directory, cursor, generation) in panel.pane.side_panel_mut().begin_project_session_requests() {
        out.extend(serde_json::to_value(AgentClientMessage::ListProjectSessions {
            catalog_identity: panel.catalog.identity.clone(),
            directory,
            cursor,
            generation,
        }).ok());
    }
}

/// Whether a daemon reply belongs to this panel's catalog requests.
fn accept_catalog_reply(panel: &mut Panel, message: &AgentServerMessage) -> bool {
    match message {
        AgentServerMessage::SessionProjects { catalog_identity, generation, .. }
        | AgentServerMessage::SessionProjectsFailed { catalog_identity, generation, .. } => {
            let current = *catalog_identity == panel.catalog.identity && *generation == panel.catalog.discovery_generation;
            if current && matches!(message, AgentServerMessage::SessionProjects { .. }) {
                panel.catalog.discovery_retry_at = None;
            }
            current
        }
        AgentServerMessage::ProjectSessionPage { catalog_identity, .. }
        | AgentServerMessage::ProjectSessionPageFailed { catalog_identity, .. } => *catalog_identity == panel.catalog.identity,
        AgentServerMessage::ThreadList { threads, .. } => {
            // ThreadList carries no scope; other consumers (the model's own
            // startup listing) share the socket. A list naming only other
            // directories is not ours and must not consume our pending reply.
            let scope = panel.pane.side_panel().catalog_directory();
            let foreign = scope.is_some_and(|scope| {
                !threads.is_empty() && threads.iter().all(|thread| thread.directory.as_deref() != Some(scope))
            });
            if foreign || panel.catalog.thread_lists_pending == 0 {
                return false;
            }
            panel.catalog.thread_lists_pending -= 1;
            true
        }
        AgentServerMessage::ThreadListFailed { .. } => {
            if panel.catalog.thread_lists_pending == 0 {
                return false;
            }
            panel.catalog.thread_lists_pending -= 1;
            true
        }
        // Lifecycle edges (rename/pin ack, finished run) re-read the catalog;
        // head refreshes keep already-loaded pages and the viewport.
        AgentServerMessage::SessionProjectsChanged
        | AgentServerMessage::ThreadUpdated { .. }
        | AgentServerMessage::SessionIdle { .. } => {
            panel.catalog.refresh_requested = true;
            true
        }
        _ => true,
    }
}

thread_local! { static PANELS: RefCell<HashMap<u32, Panel>> = RefCell::new(HashMap::new()); }
static NEXT_PANEL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// Transcript content belongs to conversation panes; the catalog pane only
/// takes catalog, activity and runtime events (desktop `catalog_accepts_family`).
fn catalog_event(message: &AgentServerMessage) -> bool {
    !matches!(
        message,
        AgentServerMessage::HistoryChunk { .. }
            | AgentServerMessage::MessageUpdated { .. }
            | AgentServerMessage::PartRemoved { .. }
            | AgentServerMessage::ContentDelta { .. }
            | AgentServerMessage::ToolUseRequest { .. }
            | AgentServerMessage::ToolUseResult { .. }
    )
}

fn outbound(panel: &mut Panel) -> Vec<Value> {
    use neoism_ui::panels::agent_pane::protocol_mapping::{
        map_outbound_command, AgentProtocolMapping, AgentProtocolMappingContext,
    };
    let context = AgentProtocolMappingContext {
        active_session_id: panel.viewed_session.clone(),
        default_directory: panel.directory.clone(),
        default_agent: None,
        default_model: None,
        default_connection_id: None,
        default_thinking: None,
        local_author: None,
    };
    let mut out = Vec::new();
    for command in panel.pane.drain_pending_outbound() {
        use neoism_ui::panels::agent_pane::outbound::OutboundAgentCommand;
        match &command {
            // The all-workspace catalog discovers projects instead of listing threads.
            OutboundAgentCommand::RefreshSessions { directory: None, .. } => {
                panel.catalog.refresh_requested = true;
                continue;
            }
            OutboundAgentCommand::RefreshSessions { .. } => panel.catalog.thread_lists_pending += 1,
            _ => {}
        }
        if let AgentProtocolMapping::Messages(messages) = map_outbound_command(command, &context) {
            out.extend(messages.into_iter().filter_map(|message| serde_json::to_value(message).ok()));
        }
    }
    pump_catalog(panel, &mut out);
    out
}

fn panel_frame(panel: &mut Panel) -> Value {
    use neoism_ui::panels::agent_pane::view::side_panel::{render_side_panel_with_icons, SharedAgentSidePanelIcons};
    let theme = IdeTheme::by_name(&panel.theme);
    panel.pane.side_panel_mut().set_navigation_header_enabled(true);
    let viewed = panel.viewed_session.clone();
    panel.pane.side_panel_mut().set_catalog_viewed_session(viewed.as_deref());
    panel.pane.side_panel_mut().set_catalog_viewed_root(viewed.as_deref());
    panel.painter.begin_frame();
    let painter: &mut dyn Paint = &mut panel.painter;
    render_side_panel_with_icons::<NeoismAgentPane, SharedAgentSidePanelIcons>(
        painter,
        &mut panel.pane,
        [0.0, 0.0, panel.width, panel.height],
        &theme,
        panel.scale,
        now_seconds(),
        panel.mouse,
        &[],
    );
    let side=panel.pane.side_panel();
    let selected=if side.new_chat_selected(){"new-chat".to_owned()}else if let Some(action)=side.selected_header_action(){format!("{:?}",action).to_lowercase()}else{side.selected_row().map(|row|format!("row:{}",row.id)).unwrap_or_default()};
    json!({
        "commands": &*panel.painter.borrowed_commands(),
        // This pane owns only the catalog. Its empty conversation would keep
        // the unrelated home wordmark animation alive forever.
        "animating": panel.pane.side_panel().is_animating(),
        "send": outbound(panel),
        "cursor": panel.pane.side_panel().selected_cursor_rect(),
        "focused": panel.pane.side_panel().is_focused(),
        "selected": selected,
    })
}

fn panel_activate(panel:&mut Panel)->Value{
    use neoism_ui::panels::agent_pane::state::side_panel::CatalogHeaderAction;
    let side=panel.pane.side_panel_mut();
    if side.selected_header_action()==Some(CatalogHeaderAction::Search){return json!({"action":"search"});}
    if side.projects_selected(){return json!({"action":"projects","anchor":side.project_button_rect(),"projects":side.session_project_directories(),"root":side.catalog_directory().is_none()});}
    if side.new_chat_selected(){return json!({"action":"new-chat"});}
    if side.toggle_selected_project(){return Value::Null;}
    side.selected_session().map(|entry|json!({"open":{"id":entry.id,"title":entry.title,"directory":entry.directory,"familyRootId":entry.branch_root_id,"pointer":false}})).unwrap_or(Value::Null)
}

fn panel_key(panel:&mut Panel,v:&Value)->Value{
    let key=text(v,"key");let ctrl=v["ctrl"].as_bool().unwrap_or(false);
    let rows=panel.pane.side_panel().last_panel_height_rows();
    let side=panel.pane.side_panel_mut();
    match key{
        "d" if ctrl=>side.select_next_by((rows/2).max(1)),
        "u" if ctrl=>side.select_prev_by((rows/2).max(1)),
        "f" if ctrl=>return json!({"action":"search"}),
        "down"=>side.select_next(),
        "up"=>side.select_prev(),
        "left"=>{side.navigate_project(false);},
        "right"=>{side.navigate_project(true);},
        "enter"=>return panel_activate(panel),
        "escape"=>{side.set_focused(false);return json!({"action":"focus-content"});},
        _=>return Value::Null,
    }
    if key=="down"||(ctrl&&key=="d"){panel.pane.maybe_request_side_panel_session_page();}
    Value::Null
}

fn panel_pointer(panel: &mut Panel, v: &Value) -> Value {
    use neoism_ui::panels::agent_pane::state::side_panel::CatalogHeaderAction;
    let (x, y) = (number(v, "x", 0.0), number(v, "y", 0.0));
    panel.mouse = Some((x, y));
    if text(v, "kind") == "leave" {
        panel.mouse = None;
        return Value::Null;
    }
    if text(v, "kind") != "down" {
        return Value::Null;
    }
    let side = panel.pane.side_panel_mut();
    side.set_focused(true);
    if let Some(action) = side.header_action_at(x, y) {
        let name = match action {
            CatalogHeaderAction::Search => { side.select_header_action(CatalogHeaderAction::Search); "search" }
            CatalogHeaderAction::Projects => {
                side.select_projects();
                // The desktop anchors the dropdown under this button and lists
                // discovered chat projects beside the registered workspaces.
                return json!({"action": "projects", "anchor": side.project_button_rect(),
                    "projects": side.session_project_directories(), "root": side.catalog_directory().is_none()});
            }
            CatalogHeaderAction::AddWorkspace => "add-workspace",
        };
        return json!({"action": name});
    }
    if matches!(side.new_chat_hit(x, y), Some(None)) {
        side.select_new_chat();
        return json!({"action": "new-chat"});
    }
    let Some(rect) = side.last_panel_rect() else { return Value::Null };
    let Some(row) = side.hit_test_row(x, y, rect) else { return Value::Null };
    if !side.sessions().get(row).is_some_and(|entry| entry.is_selectable()) {
        return Value::Null;
    }
    side.set_selected(row);
    if side.toggle_selected_project() {
        return Value::Null;
    }
    match side.selected_session() {
        Some(entry) => json!({"open": {"id": entry.id, "title": entry.title, "directory": entry.directory,"familyRootId":entry.branch_root_id,"pointer":true}}),
        None => Value::Null,
    }
}

pub fn dispatch_panel(v: &Value) -> Option<Result<Value, String>> {
    let op = v["op"].as_str()?;
    if !op.starts_with("agentsidepanel.") {
        return None;
    }
    Some(PANELS.with(|panels| {
        let mut panels = panels.borrow_mut();
        if op == "agentsidepanel.new" {
            let id = 0x74000000 + NEXT_PANEL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let family = text(v, "fontFamily");
            let directory = v["directory"].as_str().filter(|d| !d.is_empty()).map(str::to_owned);
            let mut pane = NeoismAgentPane::new();
            pane.set_directory(directory.clone());
            let scope = workspace_catalog_scope(directory.as_deref());
            pane.set_catalog_directory(scope.clone());
            let unscoped = directory.is_none();
            panels.insert(id, Panel {
                pane,
                painter: RecordingPainter::new(
                    if family.is_empty() { "Cascadia Code NF" } else { family },
                    v["fontWeight"].as_u64().unwrap_or(400) as u16,
                    vec![],
                ),
                directory,
                width: 300.0,
                height: 600.0,
                scale: number(v, "scale", 1.0),
                theme: text(v, "theme").to_owned(),
                mouse: None,
                viewed_session: None,
                catalog: CatalogPump {
                    identity: format!("gpuix:{id}:{}", scope.as_deref().unwrap_or("*")),
                    unscoped,
                    ..CatalogPump::default()
                },
            });
            return Ok(json!({"id": id}));
        }
        let id = v["id"].as_u64().unwrap_or(0) as u32;
        if op == "agentsidepanel.dispose" {
            panels.remove(&id);
            return Ok(Value::Null);
        }
        let panel = panels.get_mut(&id).ok_or("Unknown conversations panel")?;
        match op {
            "agentsidepanel.event" => {
                let parsed: AgentServerMessage =
                    serde_json::from_value(v["message"].clone()).map_err(|error| error.to_string())?;
                if catalog_event(&parsed) && accept_catalog_reply(panel, &parsed) {
                    apply_agent_event_to_pane(&mut panel.pane, parsed);
                }
                Ok(Value::Null)
            }
            // The workspace directory changed (or became known): rescope like a
            // desktop workspace switch, dropping the old catalog and its replies.
            "agentsidepanel.scope" => {
                let directory = v["directory"].as_str().filter(|d| !d.is_empty()).map(str::to_owned);
                if directory != panel.directory || panel.catalog.unscoped != directory.is_none() {
                    let scope = workspace_catalog_scope(directory.as_deref());
                    let mut pane = NeoismAgentPane::new();
                    pane.set_directory(directory.clone());
                    pane.set_catalog_directory(scope.clone());
                    panel.pane = pane;
                    panel.catalog = CatalogPump {
                        identity: format!("gpuix:{id}:{}:{}", NEXT_PANEL.fetch_add(1, std::sync::atomic::Ordering::Relaxed), scope.as_deref().unwrap_or("*")),
                        unscoped: directory.is_none(),
                        ..CatalogPump::default()
                    };
                    panel.directory = directory;
                }
                Ok(panel_frame(panel))
            }
            "agentsidepanel.frame" => {
                if let Some(width) = v["width"].as_f64() { panel.width = (width as f32).max(1.0); }
                if let Some(height) = v["height"].as_f64() { panel.height = (height as f32).max(1.0); }
                if let Some(theme) = v["theme"].as_str() { panel.theme = theme.to_owned(); }
                if !v["viewedSession"].is_null() || v.get("viewedSession").is_some() {
                    panel.viewed_session = v["viewedSession"].as_str().map(str::to_owned);
                }
                Ok(panel_frame(panel))
            }
            "agentsidepanel.scroll" => {
                let row = panel.pane.side_panel().row_height().max(1.0);
                let dy = number(v, "deltaY", 0.0);
                // Same sign as the timeline and desktop `vertical_overlay_scroll_pixels`,
                // so the compositor's natural-scroll setting applies here too.
                let pixels = if v["precise"].as_bool().unwrap_or(false) { dy } else { dy * row * 3.0 };
                let rows = panel.pane.side_panel().last_panel_height_rows();
                panel.pane.scroll_side_panel_pixels(pixels, rows);
                Ok(panel_frame(panel))
            }
            "agentsidepanel.focus"=>{
                let focused=v["focused"].as_bool().unwrap_or(true);
                panel.pane.side_panel_mut().set_focused(focused);
                let side=panel.pane.side_panel();
                if focused&&side.sessions().is_empty()&&!side.new_chat_selected()&&side.selected_header_action().is_none(){panel.pane.side_panel_mut().select_new_chat();}
                Ok(panel_frame(panel))
            }
            "agentsidepanel.key"=>{
                let result=panel_key(panel,v);let mut frame=panel_frame(panel);frame["result"]=result;Ok(frame)
            }
            // Introspection for pagination probes (row count, viewport, paging state).
            "agentsidepanel.debug" => {
                let side = panel.pane.side_panel();
                let rows = side.sessions();
                Ok(json!({
                    "rows": rows.len(),
                    "sessions": rows.iter().filter(|entry| !entry.is_header).count(),
                    "first": rows.iter().find(|entry| !entry.is_header).map(|entry| entry.title.clone()),
                    "scrollTop": side.scroll_top(),
                    "visibleRows": side.last_panel_height_rows(),
                    "loading": side.session_page_loading(),
                    "nextCursor": side.session_next_cursor(),
                    "state": format!("{:?}", side.session_catalog_state()),
                    "grouped": side.catalog_directory().is_none(),
                    "projects": side.session_project_directories().len(),
                }))
            }
            "agentsidepanel.pointer" => {
                let result = panel_pointer(panel, v);
                let mut out = panel_frame(panel);
                out["result"] = result;
                Ok(out)
            }
            _ => Err("Unknown conversations panel operation".into()),
        }
    }))
}

// ── Directory browser ────────────────────────────────────────────────
// The desktop composer directory chooser (`panels::file_browser`, opened by
// the footer's directory chip): locations, recents, listing, keyboard.

struct Browser {
    browser: neoism_ui::panels::file_browser::FileBrowserModal,
    painter: RecordingPainter,
    root: std::path::PathBuf,
    theme: String,
}
thread_local! { static BROWSERS: RefCell<HashMap<u32, Browser>> = RefCell::new(HashMap::new()); }

fn browser_frame(view: &mut Browser, v: &Value) -> Value {
    let theme = IdeTheme::by_name(&view.theme);
    view.browser.fulfill_requests_native(&view.root);
    view.painter.begin_frame();
    let painter: &mut dyn Paint = &mut view.painter;
    view.browser.render(painter, [0.0, 0.0, number(v, "width", 800.0), number(v, "height", 600.0)], &theme);
    json!({
        "commands": &*view.painter.borrowed_commands(),
        "animating": view.browser.is_animating(),
        "active": view.browser.is_active(),
        "selection": view.browser.take_selection().map(|selection| selection.path),
    })
}

pub fn dispatch_browser(v: &Value) -> Option<Result<Value, String>> {
    use neoism_ui::event::NamedKey;
    use neoism_ui::panels::file_browser::{FileBrowserModal as FileBrowser, FileBrowserLocation, FileBrowserMode};
    let op = v["op"].as_str()?;
    if !op.starts_with("dirbrowser.") {
        return None;
    }
    Some(BROWSERS.with(|browsers| {
        let mut browsers = browsers.borrow_mut();
        if op == "dirbrowser.new" {
            let id = 0x75000000 + NEXT_PANEL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let start = text(v, "start");
            let root = std::path::PathBuf::from(text(v, "root"));
            let mut browser = FileBrowser::new();
            browser.set_font_scale(number(v, "scale", 1.0));
            browser.open(FileBrowserMode::ChooseDirectory, start, v["recents"].as_array().into_iter().flatten().filter_map(|r| r.as_str().map(str::to_owned)).collect());
            let candidates = [
                ("workspace", "Workspace", Some(root.clone())),
                ("home", "Home", dirs::home_dir()),
                ("documents", "Documents", dirs::document_dir()),
                ("downloads", "Downloads", dirs::download_dir()),
                ("pictures", "Pictures", dirs::picture_dir()),
            ];
            browser.set_locations(candidates.into_iter().filter_map(|(kind, label, path)| {
                let path = path?.canonicalize().ok()?;
                path.is_dir().then(|| FileBrowserLocation { kind: kind.into(), label: label.into(), path: path.to_string_lossy().into_owned() })
            }).collect());
            let family = text(v, "fontFamily");
            browsers.insert(id, Browser {
                browser,
                painter: RecordingPainter::new(if family.is_empty() { "Cascadia Code NF" } else { family }, v["fontWeight"].as_u64().unwrap_or(400) as u16, vec![]),
                root,
                theme: text(v, "theme").to_owned(),
            });
            return Ok(json!({"id": id}));
        }
        let id = v["id"].as_u64().unwrap_or(0) as u32;
        if op == "dirbrowser.dispose" {
            browsers.remove(&id);
            return Ok(Value::Null);
        }
        let view = browsers.get_mut(&id).ok_or("Unknown directory browser")?;
        match op {
            "dirbrowser.frame" => {}
            "dirbrowser.pointer" => view.browser.pointer_down(number(v, "x", 0.0), number(v, "y", 0.0), v["clicks"].as_u64().unwrap_or(1).min(3) as u8),
            "dirbrowser.scroll" => view.browser.scroll_pixels(number(v, "delta", 0.0)),
            "dirbrowser.text" => view.browser.input_text(text(v, "text")),
            "dirbrowser.key" => {
                let key = match text(v, "key") {
                    "enter" => Some(NamedKey::Enter), "tab" => Some(NamedKey::Tab), "escape" => Some(NamedKey::Escape),
                    "backspace" => Some(NamedKey::Backspace), "up" => Some(NamedKey::ArrowUp), "down" => Some(NamedKey::ArrowDown),
                    "left" => Some(NamedKey::ArrowLeft), "right" => Some(NamedKey::ArrowRight), "home" => Some(NamedKey::Home),
                    "end" => Some(NamedKey::End), "pageup" => Some(NamedKey::PageUp), "pagedown" => Some(NamedKey::PageDown),
                    "delete" => Some(NamedKey::Delete), "space" => Some(NamedKey::Space), _ => None,
                };
                match key {
                    Some(key) => view.browser.press_named(key),
                    None => { let t = text(v, "char"); if !t.is_empty() { view.browser.input_text(t); } }
                }
            }
            _ => return Err("Unknown directory browser operation".into()),
        }
        Ok(browser_frame(view, v))
    }))
}

// ── Conversation navigator ───────────────────────────────────────────
// The desktop sidebar's Projects dropdown (shared `ConversationNavigator`,
// opened under the header's Projects button by `open_conversation_project_picker`).

struct Navigator {
    navigator: neoism_ui::widgets::conversation_navigator::ConversationNavigator,
    painter: RecordingPainter,
    scale: f32,
    theme: String,
}

thread_local! { static NAVIGATORS: RefCell<HashMap<u32, Navigator>> = RefCell::new(HashMap::new()); }

fn navigator_frame(view: &mut Navigator, v: &Value, action: Option<String>) -> Value {
    let theme = IdeTheme::by_name(&view.theme);
    view.painter.begin_frame();
    let painter: &mut dyn Paint = &mut view.painter;
    view.navigator.render(painter, &theme, [0.0, 0.0, number(v, "width", 800.0), number(v, "height", 600.0)], view.scale, now_seconds());
    json!({
        "commands": &*view.painter.borrowed_commands(),
        "animating": view.navigator.is_animating(),
        "active": view.navigator.is_active(),
        "action": action,
    })
}

pub fn dispatch_navigator(v: &Value) -> Option<Result<Value, String>> {
    use neoism_ui::widgets::conversation_navigator::{
        ConversationNavigator, ConversationNavigatorAction as Action, ConversationNavigatorMode as Mode,
        ConversationNavigatorRow as Row,
    };
    let op = v["op"].as_str()?;
    if !op.starts_with("convnav.") {
        return None;
    }
    Some(NAVIGATORS.with(|navigators| {
        let mut navigators = navigators.borrow_mut();
        if op == "convnav.new" {
            let id = 0x76000000 + NEXT_PANEL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let rows = v["rows"].as_array().into_iter().flatten().map(|row| Row {
                value: text(row, "value").to_owned(),
                title: text(row, "title").to_owned(),
                description: text(row, "description").to_owned(),
                footer: text(row, "footer").to_owned(),
                is_current: row["current"].as_bool().unwrap_or(false),
                is_disabled: row["disabled"].as_bool().unwrap_or(false),
            }).collect();
            let mut navigator = ConversationNavigator::default();
            navigator.open(Mode::Projects, rows);
            navigator.set_anchor(v["anchor"].as_array().filter(|a| a.len() == 4).map(|a| {
                [0, 1, 2, 3].map(|i| a[i].as_f64().unwrap_or(0.0) as f32)
            }));
            let family = text(v, "fontFamily");
            navigators.insert(id, Navigator {
                navigator,
                painter: RecordingPainter::new(if family.is_empty() { "Cascadia Code NF" } else { family }, v["fontWeight"].as_u64().unwrap_or(400) as u16, vec![]),
                scale: number(v, "scale", 1.0),
                theme: text(v, "theme").to_owned(),
            });
            return Ok(json!({"id": id}));
        }
        let id = v["id"].as_u64().unwrap_or(0) as u32;
        if op == "convnav.dispose" {
            navigators.remove(&id);
            return Ok(Value::Null);
        }
        let view = navigators.get_mut(&id).ok_or("Unknown conversation navigator")?;
        let mut action = None;
        match op {
            "convnav.frame" => {}
            "convnav.pointer" => {
                let (x, y) = (number(v, "x", 0.0), number(v, "y", 0.0));
                if text(v, "kind") == "move" {
                    view.navigator.pointer_move(x, y);
                } else {
                    match view.navigator.pointer_down(x, y) {
                        Some(Action::Activate(value)) => action = Some(value),
                        Some(Action::Dismiss) => {}
                        None => {}
                    }
                }
            }
            "convnav.scroll" => { view.navigator.scroll(number(v, "delta", 0.0)); }
            "convnav.key" => match text(v, "key") {
                "down" => view.navigator.select_next(),
                "up" => view.navigator.select_previous(),
                "tab" => if v["shift"].as_bool().unwrap_or(false) { view.navigator.select_previous() } else { view.navigator.select_next() },
                "escape" => view.navigator.close(),
                "backspace" => view.navigator.pop_char(),
                "enter" => action = view.navigator.selected_value().map(str::to_owned),
                _ => { let t = text(v, "char"); if !t.is_empty() && !v["ctrl"].as_bool().unwrap_or(false) { view.navigator.push_text(t); } }
            },
            _ => return Err("Unknown conversation navigator operation".into()),
        }
        if action.is_some() {
            view.navigator.close();
        }
        Ok(navigator_frame(view, v, action))
    }))
}
