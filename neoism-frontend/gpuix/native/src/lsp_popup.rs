//! Host bridge for the original status-line LSP details panel.
use neoism_ui::{panels::{lsp_popup::{LspPopup,LspPopupClickAction},status_line::PillRect},primitives::{IdeTheme,paint_record::RecordingPainter}};
use serde_json::{json,Value};
use std::{cell::RefCell,collections::HashMap};
struct Host {popup:LspPopup,paint:RecordingPainter,editor:u32,status_revision:Option<u64>,diagnostics_revision:Option<u64>}
thread_local! {static HOSTS:RefCell<HashMap<u64,Host>>=RefCell::new(HashMap::new());static NEXT:RefCell<u64>=RefCell::new(0xb300_0000);}
fn n(v:&Value,k:&str,d:f32)->f32{v[k].as_f64().unwrap_or(d as f64)as f32}
pub fn dispatch(v:&Value,engines:&super::Engines)->Option<Result<Value,String>> {
 let op=v["op"].as_str()?;if !op.starts_with("lsppopup."){return None;}
 Some(HOSTS.with(|hosts|{
  let mut hosts=hosts.borrow_mut();
  if op=="lsppopup.new"{
   let id=NEXT.with(|n|{let mut n=n.borrow_mut();*n+=1;*n});
   hosts.insert(id,Host{popup:LspPopup::new(),paint:RecordingPainter::new(v["fontFamily"].as_str().unwrap_or("Cascadia Code NF"),v["fontWeight"].as_u64().unwrap_or(400)as u16,vec![v["iconFontFamily"].as_str().unwrap_or("Symbols Nerd Font Mono").into()]),editor:v["editorId"].as_u64().unwrap_or(0)as u32,status_revision:None,diagnostics_revision:None});
   return Ok(json!({"id":id}));
  }
  let id=v["id"].as_u64().ok_or("Missing LSP popup id")?;
  if op=="lsppopup.dispose"{hosts.remove(&id);return Ok(json!({}));}
  let host=hosts.get_mut(&id).ok_or("Unknown LSP popup")?;
  let Some(editor)=engines.editors.get(&host.editor)else{return Ok(json!({"visible":false,"commands":[],"rows":[],"animating":false}));};
  let anchor=PillRect{x:n(v,"anchorX",0.0),y:n(v,"anchorY",600.0),w:n(v,"anchorWidth",80.0),h:n(v,"anchorHeight",22.0)};
  let scale=n(v,"scale",1.0);
  host.popup.set_host_geometry(anchor,[n(v,"width",800.0),n(v,"height",600.0)]);
  if host.status_revision != Some(editor.lsp.status_revision) {
   host.popup.set_servers(editor.lsp.servers.clone());
   host.popup.set_status(editor.lsp.status);
   host.status_revision=Some(editor.lsp.status_revision);
  }
  let diagnostics_revision=editor.lsp.session.diagnostics.version();
  if host.diagnostics_revision != Some(diagnostics_revision) {
   host.popup.set_diagnostics(editor.lsp.session.diagnostics.counts_for(&editor.pane.path));
   host.diagnostics_revision=Some(diagnostics_revision);
  }
  host.popup.set_buffer_label(editor.pane.path.file_name().map(|name|name.to_string_lossy().into_owned()));
  let mut clipboard=None;
  match op {
   "lsppopup.open"=>host.popup.open(anchor),
   "lsppopup.close"=>host.popup.close(),
   "lsppopup.key"=>match v["key"].as_str(){Some("escape")=>host.popup.close(),Some("up")=>host.popup.move_selection(-1),Some("down")=>host.popup.move_selection(1),_=>{}},
   "lsppopup.pointer"=>{let(x,y)=(n(v,"x",-1.0),n(v,"y",-1.0));host.popup.hover(x,y,scale);if v["activate"].as_bool()==Some(true){if let Some(LspPopupClickAction::CopyMessage(text))=host.popup.click(x,y,scale){clipboard=Some(text);}}},
   "lsppopup.wheel"=>{let delta=n(v,"deltaY",0.0)*if v["precise"].as_bool().unwrap_or(false){1.0}else{38.0};host.popup.scroll_at(n(v,"x",-1.0),n(v,"y",-1.0),delta,scale);},_=>{}
  }
  if editor.lsp.status.is_none(){host.popup.close();}
  host.paint.begin_frame();host.popup.render(&mut host.paint,&IdeTheme::by_name(v["theme"].as_str().unwrap_or("pastel_dark")),scale);
  Ok(json!({"visible":host.popup.is_visible(),"animating":host.popup.is_animating(),"rect":host.popup.occlusion_rect(),"selected":host.popup.host_selected(),"clipboard":clipboard,
   "rows":host.popup.host_rows().iter().map(|(index,rect)|{let server=&editor.lsp.servers[*index];json!({"index":index,"rect":rect,"name":server.name,"state":server.state.label()})}).collect::<Vec<_>>(),"commands":host.paint.commands()}))
 }))
}
