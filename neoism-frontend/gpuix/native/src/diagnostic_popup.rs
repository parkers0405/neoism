//! Original status badge diagnostics popover, reading the active native LSP set.
use neoism_ui::{panels::{diagnostics_popup::DiagnosticsPopup,status_line::DiagnosticPill},primitives::{IdeTheme,paint_record::RecordingPainter}};
use serde_json::{json,Value};
use std::{cell::RefCell,collections::HashMap};
struct Host {popup:DiagnosticsPopup,paint:RecordingPainter,editor:u32,pill:DiagnosticPill,signature:String,wheel:f32}
thread_local! {static HOSTS:RefCell<HashMap<u64,Host>>=RefCell::new(HashMap::new());static NEXT:RefCell<u64>=RefCell::new(0xb200_0000);}
fn n(v:&Value,k:&str,d:f32)->f32{v[k].as_f64().unwrap_or(d as f64)as f32}
pub fn dispatch(v:&Value,engines:&super::Engines)->Option<Result<Value,String>> {
 let op=v["op"].as_str()?;if !op.starts_with("diagnosticpopup."){return None;}
 Some(HOSTS.with(|hosts|{
  let mut hosts=hosts.borrow_mut();
  if op=="diagnosticpopup.new"{
   let id=NEXT.with(|n|{let mut n=n.borrow_mut();*n+=1;*n});
   let extras=vec![v["iconFontFamily"].as_str().unwrap_or("Symbols Nerd Font Mono").into()];
   hosts.insert(id,Host{popup:DiagnosticsPopup::new(),paint:RecordingPainter::new(v["fontFamily"].as_str().unwrap_or("Cascadia Code NF"),v["fontWeight"].as_u64().unwrap_or(400)as u16,extras),editor:v["editorId"].as_u64().unwrap_or(0)as u32,pill:if v["pill"].as_str()==Some("Warn"){DiagnosticPill::Warn}else{DiagnosticPill::Error},signature:String::new(),wheel:0.0});
   return Ok(json!({"id":id}));
  }
  let id=v["id"].as_u64().ok_or("Missing diagnostic popup id")?;
  if op=="diagnosticpopup.dispose"{hosts.remove(&id);return Ok(json!({}));}
  let host=hosts.get_mut(&id).ok_or("Unknown diagnostic popup")?;
  let Some(editor)=engines.editors.get(&host.editor)else{return Ok(json!({"visible":false,"commands":[],"rows":[],"animating":false}));};
  let items=editor.lsp.session.diagnostics.popup_items(&editor.pane.path,host.pill);
  let counts=editor.lsp.session.diagnostics.counts_for(&editor.pane.path);
  let count=match host.pill{DiagnosticPill::Error=>counts.error,DiagnosticPill::Warn=>counts.warn}as u64;
  let signature=json!([count,items.iter().map(|item|json!([item.lnum,item.message])).collect::<Vec<_>>()]).to_string();
  let scale=n(v,"scale",1.0);host.popup.set_scale(scale);host.popup.set_host_viewport_height(v["height"].as_f64().map(|height|height as f32));
  if op=="diagnosticpopup.open"{host.popup.open_with_total(host.pill,items,count,n(v,"anchorX",0.0),n(v,"anchorY",600.0));host.signature=signature;}
  else if signature!=host.signature{host.popup.refresh_items_with_total(items,count);host.signature=signature;}
  host.popup.set_host_anchor(n(v,"anchorX",0.0),n(v,"anchorY",600.0));
  let mut target=None;
  match op {
   "diagnosticpopup.key"=>match v["key"].as_str().unwrap_or(""){"up"=>host.popup.move_up(),"down"=>host.popup.move_down(),"escape"=>host.popup.close(),"enter"=>{target=host.popup.selected_lnum();host.popup.close();},_=>{}},
   "diagnosticpopup.pointer"=>if host.popup.is_interactive(){if let Ok(Some(index))=host.popup.hit_test(n(v,"x",-1.0),n(v,"y",-1.0)){host.popup.set_selected_index(index);if v["activate"].as_bool()==Some(true){target=host.popup.selected_lnum();host.popup.close();}}},
   "diagnosticpopup.wheel"=>{
    let dx=n(v,"deltaX",0.0);if dx.abs()>0.0{if let Some(row)=host.popup.row_at_y(n(v,"y",-1.0)){host.popup.scroll_message(row,-dx);}}
    let dy=n(v,"deltaY",0.0);host.wheel-=if v["precise"].as_bool().unwrap_or(false){dy/(24.0*scale)}else{dy};let rows=host.wheel.trunc()as i32;if rows!=0{host.wheel-=rows as f32;host.popup.scroll_by(rows);}
   },
   "diagnosticpopup.close"=>host.popup.close(),_=>{}
  }
  host.paint.begin_frame();host.popup.render(&mut host.paint,n(v,"width",800.0),1.0,&IdeTheme::by_name(v["theme"].as_str().unwrap_or("pastel_dark")));
  Ok(json!({"visible":host.popup.is_visible(),"interactive":host.popup.is_interactive(),"animating":host.popup.is_animating(),"rect":host.popup.occlusion_rect(),"rows":host.popup.host_rows().into_iter().map(|(index,rect,line,message)|json!({"index":index,"rect":rect,"line":line,"message":message})).collect::<Vec<_>>(),"selected":host.popup.host_selected(),"target":target.map(|line|json!({"line":line.saturating_sub(1),"column":0})),"commands":host.paint.commands()}))
 }))
}
