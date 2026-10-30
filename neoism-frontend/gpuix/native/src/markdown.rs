//! Original rich MarkdownPane, original renderer, original host-neutral input.
use neoism_ui::{
    editor::markdown::{
        self as md,
        bridge_policy::{MarkdownBridgeModifiers, markdown_leader_transition},
        dispatch::{dispatch_markdown_pane_key, parse_browser_markdown_key},
        MarkdownCursorLink, MarkdownMode, MarkdownPane,
    },
    primitives::{ide_theme::IdeTheme, paint::Paint, paint_record::RecordingPainter},
};
use serde_json::{json, Value};
use neoism_ui::panels::context_menu::{ContextMenu,ContextMenuItem,ContextMenuAction};
use std::{cell::RefCell, collections::HashMap, path::PathBuf};
struct View {
    pane: MarkdownPane,
    painter: RecordingPainter,
    width: f32,
    height: f32,
    scale: f32,
    theme: String,
    mouse: Option<[f32; 2]>,
    effects: Value,
    trail:neoism_ui::panels::trail_cursor::TrailCursor,
    last_cursor:(usize,usize),
    last_frame:std::time::Instant,
    leader_pending:Option<std::time::Instant>,
    menu:ContextMenu,
    menu_painter:Option<RecordingPainter>,
    presence:crate::presence_cache::PresenceCache,
    canvas_style:Option<(String,u16)>,
}
#[derive(Default)]
struct Store {
    next: u32,
    views: HashMap<u32, View>,
}
thread_local! {static STORE:RefCell<Store>=RefCell::new(Store::default());}
fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
fn number(v: &Value, key: &str, fallback: f32) -> f32 {
    v[key].as_f64().map_or(fallback, |n| n as f32)
}
fn cursor_link(pane: &MarkdownPane, link: MarkdownCursorLink) -> Value {
    match link {
        MarkdownCursorLink::External(url) => json!({"url":url}),
        MarkdownCursorLink::Internal { target, code_ref } => {
            pane.resolve_markdown_link(&target).map_or(
                json!({"target":target,"codeRef":code_ref}),
                |link| json!({"path":link.path,"line":link.line,"codeRef":code_ref}),
            )
        }
    }
}
fn open_block_menu(view:&mut View,anchor:Option<[f32;4]>){
 let anchor=anchor.or(view.pane.cursor_rect).unwrap_or([20.0,20.0,0.0,20.0]);
 let items=md::menus::markdown_block_menu_entries().iter().map(|entry|ContextMenuItem::new(entry.label,entry.hint,ContextMenuAction::MarkdownBlock(entry.template)).with_preview(entry.preview)).collect();
 view.menu.set_scale(view.scale);view.menu.open_markdown_block("Add block",items,view.pane.slash_block_query_before_cursor().unwrap_or_default(),anchor[0],anchor[1]+anchor[3]+6.0,view.width,view.height);
 view.menu.set_viewport([0.0,0.0,view.width,view.height]);view.menu.avoid_row(anchor[1],anchor[1]+anchor[3]);
}
fn menu_action(view:&mut View,action:ContextMenuAction){
 view.menu.close();match action{
  ContextMenuAction::MarkdownBlock(template)=>view.pane.apply_block_template(template),
  ContextMenuAction::MarkdownTable(action)=>{view.pane.apply_table_action(action);},
  ContextMenuAction::MarkdownLinkCompletion(target)=>{view.pane.apply_wiki_link_completion(&target);},
  ContextMenuAction::MarkdownSpellingReplace{line,start,end,expected,replacement}=>{view.pane.replace_spelling_word(line,start,end,&expected,&replacement);},
  ContextMenuAction::MarkdownSpellingIgnore(word)=>{md::ignore_spelling_word(&word);},
  ContextMenuAction::MarkdownSpellingAddToDictionary(word)=>{let _=md::add_spelling_word_to_dictionary(&word);},_=>{}
 }
}
fn open_spelling_menu(view:&mut View,x:f32,y:f32){
 let Some(target)=view.pane.spelling_word_at(x,y)else{return;};
 let mut items=md::render::spelling_suggestions(&target.word).into_iter().map(|replacement|ContextMenuItem::new(replacement.clone(),"fix",ContextMenuAction::MarkdownSpellingReplace{line:target.line,start:target.start,end:target.end,expected:target.word.clone(),replacement}).with_preview("\u{f0eb}")).collect::<Vec<_>>();
 items.push(ContextMenuItem::new("Ignore","Session",ContextMenuAction::MarkdownSpellingIgnore(target.word.clone())).with_preview("\u{f05e}"));
 items.push(ContextMenuItem::new("Add to Dictionary","Global",ContextMenuAction::MarkdownSpellingAddToDictionary(target.word.clone())).with_preview("\u{f02d}"));
 view.menu.set_scale(view.scale);view.menu.open(format!("Spelling: {}",target.word),items,x,y+8.0,view.width,view.height);view.menu.set_viewport([0.0,0.0,view.width,view.height]);
}
fn frame(view: &mut View) -> Value {
    let moving = view.pane.tick_scroll();
    view.painter.begin_frame();
    md::render::render(
        &mut view.painter,
        &mut view.pane,
        [0.0, 0.0, view.width, view.height],
        &IdeTheme::by_name(&view.theme),
        view.mouse,
        &[],
        view.scale,
        0.0,
    );
    if std::env::var("NEOISM_RM_OVERLAY").as_deref()!=Ok("0"){
      use neoism_ui::editor::neodraw::{ink_sidecar_path,legacy_ink_sidecar_path,Scene,strokes_only,render_scene,Camera,Vec2};
      let ink=view.painter.asset_text(&ink_sidecar_path(&view.pane.path)).filter(|text|!text.trim().is_empty()).or_else(||view.painter.asset_text(&legacy_ink_sidecar_path(&view.pane.path)).filter(|text|!text.trim().is_empty()));
      if let Some(mut scene)=ink.and_then(|text|Scene::from_json(&text).ok()){strokes_only(&mut scene);render_scene(&mut view.painter,&scene,&Camera{pan:Vec2::new(0.0,-view.pane.scroll_y),zoom:1.0},[0.0,0.0,view.width,view.height],0.0,12);}
    }
    let assets=view.painter.take_asset_requests();
    let cover=view.pane.frontmatter_cover().zip(view.pane.cover_overlay_rect).map(|(value,rect)|json!({"value":value,"rect":rect}));
    let reveal=view.pane.scroll_cursor_into_view(0.0,view.height);
    let elapsed=view.last_frame.elapsed().as_secs_f32();view.last_frame=std::time::Instant::now();
    if let Some(rect)=view.pane.cursor_rect{let was_animating=view.trail.is_animating();view.trail.set_cursor_shape(view.pane.cursor_shape());
      // The original Markdown overlay follows the rendered caret each frame.
      view.trail.set_destination(rect[0],rect[1],rect[2],rect[3]);
      view.last_cursor=(view.pane.cursor_line,view.pane.cursor_col);
      view.trail.animate(rect[2],rect[3],if was_animating{elapsed}else{0.0});
    }
    let menu=if view.menu.is_visible(){if view.menu_painter.is_none(){view.menu_painter=Some(RecordingPainter::from_fonts(view.painter.font_library().clone()));}
      let painter=view.menu_painter.as_mut().unwrap();painter.begin_frame();view.menu.render(painter,(view.width,view.height,1.0),&IdeTheme::by_name(&view.theme));Some(json!({"rect":view.menu.rect(),"commands":&*painter.borrowed_commands()}))
    }else{None};
    let effects = std::mem::replace(&mut view.effects, Value::Null);
    let presence=view.presence.frame(view.pane.source_revision(),&view.pane.lines,(view.pane.cursor_line,view.pane.cursor_col),view.pane.visual_selection().map(|(a,b,_)|((a.line,a.col),(b.line,b.col))),view.pane.mode==MarkdownMode::Insert);
    let history=view.pane.take_doc_history_requests().into_iter().map(|request|matches!(request,md::MarkdownDocHistoryRequest::Redo)).collect::<Vec<_>>();
    let canvas=view.canvas_style.as_ref().map(|(font,weight)|crate::paint_canvas::encode(&view.painter.borrowed_commands(),font,*weight));
    let commands=if canvas.is_some(){json!([])}else{json!(&*view.painter.borrowed_commands())};
    json!({"presence":presence,"docHistory":history,"commands":commands,"canvas":canvas,"menu":menu,"assets":assets,"cover":cover,"revision":view.pane.source_revision(),"mode":format!("{:?}",view.pane.mode),"vim":view.pane.vim_enabled,"cursor":{"line":view.pane.cursor_line,"col":view.pane.cursor_col,"rect":view.pane.cursor_rect},"dirty":view.pane.is_dirty(),"scrollY":view.pane.scroll_y,"contentHeight":view.pane.content_height(),"animating":moving||reveal||view.pane.has_pending_scroll_animation()||view.trail.is_animating()||view.menu.is_animating(),"cursorQuad":view.trail.quad_points(),"cursorHold":view.trail.blink_hold_visible(neoism_ui::editor::optimistic::DEFAULT_BLINK_PERIOD_MS),"hostAction":effects.get("hostAction"),"dragging":view.pane.is_dragging(),"effects":effects})
}
fn snapshot(view: &mut View) -> Value {
    let anchor=view.pane.capture_host_scroll_anchor();
    let pane=&view.pane;
    json!({"anchor":anchor,"width":view.width,"height":view.height,"version":1,"line":pane.cursor_line,"col":pane.cursor_col,"scrollY":pane.scroll_y,"mode":format!("{:?}",pane.mode),"vim":pane.vim_enabled})
}
fn restore(pane: &mut MarkdownPane, value: &Value) {
    pane.vim_enabled = value["vim"].as_bool().unwrap_or(pane.vim_enabled);
    pane.mode = match text(value, "mode") {
        "Insert" => MarkdownMode::Insert,
        "Visual" => MarkdownMode::Normal,
        _ => {
            if pane.vim_enabled {
                MarkdownMode::Normal
            } else {
                MarkdownMode::Insert
            }
        }
    };
    pane.jump_to_line(value["line"].as_u64().unwrap_or(0) as usize + 1);
    pane.cursor_col = value["col"].as_u64().unwrap_or(0) as usize;
    let source = &pane.lines[pane.cursor_line];
    pane.cursor_col = pane.cursor_col.min(source.len());
    while !source.is_char_boundary(pane.cursor_col) {
        pane.cursor_col -= 1;
    }
    pane.restore_scroll_position(number(value, "scrollY", 0.0));
}
pub fn dispatch(v: &Value) -> Option<Result<Value, String>> {
    let op = text(v, "op");
    if !op.starts_with("markdown.") {
        return None;
    }
    Some(STORE.with(|store|{let mut store=store.borrow_mut();
 if op=="markdown.new"{
  store.next+=1;let id=0x71000000+store.next;
  let family=text(v,"fontFamily");let document=text(v,"markdownFontFamily");
  let mut look=(*neoism_ui::primitives::look::active_look()).clone();look.markdown.font_family=(!document.is_empty()).then(||document.to_owned());neoism_ui::primitives::look::set_active_look(look);
  let mut pane=MarkdownPane::from_source(PathBuf::from(text(v,"path")),text(v,"text"));pane.vim_enabled=v["vim"].as_bool().unwrap_or(true);if !pane.vim_enabled{pane.enter_insert();}
  let painter=RecordingPainter::new(if family.is_empty(){"Cascadia Code NF"}else{family},v["fontWeight"].as_u64().unwrap_or(400) as u16,if document.is_empty(){vec![]}else{vec![document.to_owned()]});
  store.views.insert(id,View{pane,painter,width:800.0,height:600.0,scale:number(v,"scale",1.0),theme:text(v,"theme").to_owned(),mouse:None,effects:Value::Null,trail:neoism_ui::panels::trail_cursor::TrailCursor::new(),last_cursor:(usize::MAX,usize::MAX),last_frame:std::time::Instant::now(),leader_pending:None,menu:ContextMenu::new(),menu_painter:None,presence:Default::default(),canvas_style:None});return Ok(json!({"id":id}));
 }
 let id=v["id"].as_u64().unwrap_or(0) as u32;
 if op=="markdown.dispose"{store.views.remove(&id);return Ok(Value::Null);}
 if op=="markdown.block-options"{return Ok(json!(md::menus::markdown_block_menu_entries().iter().enumerate().map(|(i,e)|json!({"title":e.label,"value":i.to_string(),"description":e.hint,"footer":e.preview})).collect::<Vec<_>>()));}
 let view=store.views.get_mut(&id).ok_or("Unknown rich Markdown view")?;
 if op!="markdown.frame"{view.presence.clear();}
 match op{
  "markdown.canvas-paint"=>{view.canvas_style=if v["enabled"].as_bool()==Some(false){None}else{Some((text(v,"fontFamily").to_owned(),v["fontWeight"].as_u64().unwrap_or(400) as u16))};},
  "markdown.text"=>return Ok(json!({"text":view.pane.lines.join("\n")})),
  "markdown.snapshot"=>return Ok(snapshot(view)),
  "markdown.restore"=>restore(&mut view.pane,&v["snapshot"]),
  "markdown.restore-anchor"=>{let anchor=serde_json::from_value::<neoism_ui::neoism_paint::VirtualScrollAnchor>(v["anchor"].clone()).map_err(|error|error.to_string())?;view.pane.restore_host_scroll_anchor(anchor);},
  "markdown.source"=>{
    if view.pane.doc_history_bound(){if let Some(delta)=md::doc_sync::diff_doc_texts(&view.pane.lines.join("\n"),text(v,"text")){md::doc_sync::apply_remote_delta_to_pane(&mut view.pane,&delta);}}
    else{view.pane.set_source_preserving_view(text(v,"text"));}
  },
  "markdown.path"=>{view.pane.path=PathBuf::from(text(v,"path"));},
  "markdown.doc-bound"=>view.pane.set_doc_history_bound(v["bound"].as_bool().unwrap_or(false)),
  "markdown.presence"=>{let peers=serde_json::from_value::<Vec<neoism_protocol::crdt::CrdtPeerPresence>>(v["peers"].clone()).map_err(|e|e.to_string())?;view.pane.remote_cursors=peers.into_iter().map(|peer|md::MarkdownRemoteCursor{name:peer.display_name,client_os:peer.client_os,color:[peer.color.r,peer.color.g,peer.color.b],rainbow:peer.rainbow,insert:peer.insert,line:peer.cursor.line as usize,col_utf16:peer.cursor.column as usize}).collect();},
  "markdown.asset"=>view.painter.set_asset(PathBuf::from(text(v,"path")),v["text"].as_str().map(str::to_owned)),
  "markdown.cover-paths"=>{let covers=std::env::var("NEOISM_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_|{if cfg!(target_os="windows"){dirs::home_dir().unwrap_or_default().join("AppData/Local/neoism")}else if cfg!(target_os="macos"){dirs::home_dir().unwrap_or_default().join(".config/neoism")}else{std::env::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_|dirs::home_dir().unwrap_or_default().join(".config")).join("neoism")}}).join("covers");return Ok(json!(md::host_assets::cover_path_candidates(&view.pane.path,text(v,"value"),&covers)));},
  "markdown.saved"=>view.pane.mark_saved(),
  "markdown.configure"=>{
   view.scale=number(v,"scale",1.0);view.theme=text(v,"theme").to_owned();view.menu.close();view.menu_painter=None;let family=text(v,"fontFamily");let document=text(v,"markdownFontFamily");
   let mut look=(*neoism_ui::primitives::look::active_look()).clone();look.markdown.font_family=(!document.is_empty()).then(||document.to_owned());neoism_ui::primitives::look::set_active_look(look);
   view.painter=RecordingPainter::new(if family.is_empty(){"Cascadia Code NF"}else{family},v["fontWeight"].as_u64().unwrap_or(400) as u16,if document.is_empty(){vec![]}else{vec![document.to_owned()]});
  },
  "markdown.layout"=>{view.width=number(v,"width",800.0);view.height=number(v,"height",600.0);if view.menu.is_visible(){view.menu.set_viewport([0.0,0.0,view.width,view.height]);}},
  "markdown.goto"=>{view.pane.jump_to_line(v["line"].as_u64().unwrap_or(0) as usize+1);view.pane.cursor_col=v["col"].as_u64().unwrap_or(0) as usize;let source=&view.pane.lines[view.pane.cursor_line];view.pane.cursor_col=view.pane.cursor_col.min(source.len());while !source.is_char_boundary(view.pane.cursor_col){view.pane.cursor_col-=1;}},
  "markdown.toggle-vim"=>{view.pane.vim_enabled=!view.pane.vim_enabled;if view.pane.vim_enabled{view.pane.enter_normal()}else{view.pane.enter_insert()}},
  "markdown.scroll"=>{
   let x=number(v,"x",0.0);let y=number(v,"y",0.0);let dx=number(v,"deltaX",0.0);
   // GPUI already delivers compositor/natural-scroll-adjusted deltas, just
   // like Code. Page/outline APIs negate internally; table offsets instead
   // use content coordinates (the original desktop bridge negates X once).
   if dx==0.0||!view.pane.scroll_table_at(x,y,-dx){
    let delta=number(v,"delta",0.0);
    if !view.pane.outline_wheel_at(x,y,delta){view.pane.scroll_pixels(delta,view.height);}
   }
  },
  "markdown.hover"=>{if v["x"].is_number(){let x=number(v,"x",0.0);let y=number(v,"y",0.0);view.mouse=Some([x,y]);view.pane.hover_at(x,y);}else{view.mouse=None;view.pane.clear_hover();}},
  "markdown.menu.close"=>view.menu.close(),
  "markdown.menu.scroll"=>view.menu.scroll_pixels(number(v,"delta",0.0)),
  "markdown.menu.pointer"=>{let x=number(v,"x",0.0);let y=number(v,"y",0.0);if text(v,"kind")=="hover"{view.menu.hover(x,y);}else if let Some(action)=view.menu.action_at(x,y){menu_action(view,action);}else{view.menu.close();}},
  "markdown.context"=>open_spelling_menu(view,number(v,"x",0.0),number(v,"y",0.0)),
  "markdown.pointer"=>{
   let x=number(v,"x",0.0);let y=number(v,"y",0.0);view.mouse=Some([x,y]);
   match text(v,"kind"){
    "move"=>{view.pane.update_drag(x,y);},"up"=>{let line=view.pane.dragging_line;view.pane.end_drag();if let Some(rect)=view.pane.take_pending_block_menu_rect(){if let Some(line)=line{view.pane.jump_to_line(line+1);view.pane.enter_insert();}open_block_menu(view,Some(rect));}},
    _=>{if let Some(payload)=view.pane.copy_at(x,y){view.effects=json!({"clipboard":payload});}
      else if view.pane.toggle_task_at(x,y)||view.pane.activate_table_action_at(x,y)||view.pane.roster_jump_at(x,y){}
      else if v["ctrl"].as_bool().unwrap_or(false){view.pane.click_at(x,y);if let Some(link)=view.pane.link_at_cursor(){view.effects=json!({"link":cursor_link(&view.pane,link)});}else if let Some(link)=view.pane.link_at(x,y){view.effects=json!({"link":{"path":link.path,"line":link.line,"codeRef":link.code_ref}});}}
      else if let Some(rect)=view.pane.block_conversion_at(x,y){open_block_menu(view,Some(rect));}
      else if let Some((line,column,count))=view.pane.table_column_menu_at(x,y){view.menu.set_scale(view.scale);view.menu.open_table_column(line,column,count,x,y,view.width,view.height);view.menu.set_viewport([0.0,0.0,view.width,view.height]);}
      else if !view.pane.begin_drag_at(x,y){if v["clicks"].as_u64().unwrap_or(1)>1{view.pane.select_word_at(x,y);}else{view.pane.click_at(x,y);}}
    }
   }
  },
  "markdown.paste"=>view.pane.insert_text(text(v,"text")),
  "markdown.block-template"=>{if let Some(entry)=md::menus::markdown_block_menu_entries().get(v["index"].as_u64().unwrap_or(0) as usize){view.pane.apply_block_template(entry.template);}},
  "markdown.search"=>{let matches=view.pane.search_scan(text(v,"query"));if let Some((line,col,_))=matches.first(){view.pane.search_preview(*line,*col);}view.effects=json!({"matches":matches});},
  "markdown.search.cancel"=>view.pane.search_cancel(),
  "markdown.search.commit"=>view.pane.search_commit(v["line"].as_u64().unwrap_or(1),v["col"].as_u64().unwrap_or(1)),
  "markdown.command"=>{
   use neoism_ui::editor::scroll_model::{parse_ex_command,MarkdownExCommandPlan as Plan};
   let command=text(v,"command").trim().trim_start_matches(':').trim();
   if let Some((head,_))=parse_ex_command(command){match Plan::classify(&head){
    Plan::JumpToLastLine=>view.pane.jump_to_last_line(),Plan::JumpToLine(line)=>view.pane.jump_to_line(line),
    Plan::Save=>view.effects=json!({"hostAction":"save"}),Plan::SaveAndCloseFocusedBuffer=>view.effects=json!({"hostAction":"save-close"}),
    _ if head=="q"||head=="quit"=>view.effects=json!({"hostAction":"close"}),
    _ if head=="noh"||head=="nohlsearch"=>{view.pane.search_cancel();view.pane.vim.search=None;},
    _=>return Err(format!("Unsupported rich document command :{command}"))
   }}
  },
  "markdown.key"=>{
   let key=text(v,"key");
   if view.menu.is_visible(){let ctrl=v["ctrl"].as_bool().unwrap_or(false);
    match key{"escape"=>{view.menu.close();return Ok(frame(view));},"up"|"down"=>{view.menu.move_selection(if key=="up"{-1}else{1});return Ok(frame(view));},"n"|"p" if ctrl=>{view.menu.move_selection(if key=="p"{-1}else{1});return Ok(frame(view));},"enter"|"tab"=>{if let Some(action)=view.menu.selected_action(){menu_action(view,action);}return Ok(frame(view));},_=>{}}
   }
   // The shared dispatcher deliberately leaves the desktop leader timer to
   // its host. Apply the canonical 800ms policy before virtual title input.
   let mods=MarkdownBridgeModifiers{shift:v["shift"].as_bool().unwrap_or(false),control:v["ctrl"].as_bool().unwrap_or(false),alt:v["alt"].as_bool().unwrap_or(false),super_key:v["cmd"].as_bool().unwrap_or(false)};
   let now=std::time::Instant::now();
   let pending=view.leader_pending;
   let normal=view.pane.vim_enabled&&view.pane.mode==MarkdownMode::Normal;
   let title_normal=normal&&view.pane.title_edit.is_some();
   let transition=markdown_leader_transition(normal.then_some(MarkdownMode::Normal),pending.is_some(),pending.map_or(0,|started|now.duration_since(started).as_millis()));
   if transition.clear_pending{view.leader_pending=None;}
   let chord=normal&&pending.is_some()&&!transition.flushed;
   let ch=if text(v,"char").is_empty(){key}else{text(v,"char")};
   if chord&&mods.classify().plain&&(ch=="x"||title_normal&&ch.eq_ignore_ascii_case("x")){
    view.effects=json!({"hostAction":"close"});return Ok(frame(view));
   }
   if title_normal&&mods.classify().plain&&(key=="space"||ch==" "){
    view.leader_pending=Some(now);return Ok(frame(view));
   }
   let flushed=transition.flushed||chord&&!title_normal;
   let browser=match key{"escape"=>"Escape","enter"=>"Enter","backspace"=>"Backspace","delete"=>"Delete","tab"=>"Tab","left"=>"ArrowLeft","right"=>"ArrowRight","up"=>"ArrowUp","down"=>"ArrowDown","home"=>"Home","end"=>"End","pageup"=>"PageUp","pagedown"=>"PageDown","space"=>" ",_=>if text(v,"char").is_empty(){key}else{text(v,"char")}};
   if let Some(key)=parse_browser_markdown_key(browser){let fx=dispatch_markdown_pane_key(&mut view.pane,key,text(v,"char"),mods,view.height,v["clipboard"].as_str(),flushed||v["flushedLeader"].as_bool().unwrap_or(false));
    if fx.arm_leader{view.leader_pending=Some(now);}
    if fx.open_block_menu{open_block_menu(view,view.pane.cursor_rect);}else if view.menu.is_markdown_block_completion(){if let Some(query)=view.pane.slash_block_query_before_cursor(){view.menu.set_markdown_block_query(query);}else{view.menu.close();}}
    view.effects=json!({"handled":fx.handled,"clipboard":fx.clipboard_out,"search":fx.open_search,"palette":fx.open_palette,"blockMenu":fx.open_block_menu,"yank":fx.yank_message,"titleRename":fx.title_rename,"link":fx.open_cursor_link.map(|link|cursor_link(&view.pane,link))});
   }
  },
  "markdown.frame"=>{},_=>return Err(format!("Unknown Markdown operation {op}"))
 }
 Ok(frame(view))
 }))
}
