//! Original code path strip and debounced tree-sitter symbol trail.
use neoism_ui::{panels::breadcrumbs::Breadcrumbs,primitives::{ide_theme::IdeTheme,paint_record::RecordingPainter}};
use serde_json::{json,Value};
use std::{cell::RefCell,collections::HashMap,path::PathBuf,time::Instant};
struct Host{view:Breadcrumbs,paint:RecordingPainter,key:Option<(u32,u64,usize)>,pending:Option<Instant>,tail:String}
thread_local!{static HOSTS:RefCell<HashMap<u64,Host>>=RefCell::new(HashMap::new());static NEXT:RefCell<u64>=RefCell::new(0xd000_0000);}
pub(super) fn dispatch(v:&Value,engines:&super::Engines)->Option<Result<Value,String>>{
 let op=v["op"].as_str()?;if !op.starts_with("breadcrumbs."){return None;}
 Some(HOSTS.with(|hosts|{let mut hosts=hosts.borrow_mut();
  if op=="breadcrumbs.new"{let id=NEXT.with(|next|{let mut next=next.borrow_mut();*next+=1;*next});hosts.insert(id,Host{view:Breadcrumbs::new(),paint:RecordingPainter::new(v["fontFamily"].as_str().unwrap_or("Cascadia Code NF"),v["fontWeight"].as_u64().unwrap_or(400)as u16,vec!["Symbols Nerd Font Mono".into()]),key:None,pending:None,tail:String::new()});return Ok(json!({"id":id}));}
  let id=v["id"].as_u64().ok_or("Missing breadcrumbs id")?;if op=="breadcrumbs.dispose"{hosts.remove(&id);return Ok(Value::Null);}
  if op!="breadcrumbs.frame"{return Err(format!("Unknown breadcrumbs operation {op}"));}
  let host=hosts.get_mut(&id).ok_or("Unknown breadcrumbs")?;let editor_id=v["editorId"].as_u64().ok_or("Missing editor id")? as u32;
  let editor=engines.editors.get(&editor_id).ok_or("Unknown editor")?;let (path,revision,line)=(editor.pane.path.clone(),editor.pane.buffer.revision,editor.pane.buffer.cursor_line);
  let key=(editor_id,revision,line);if host.key!=Some(key){host.key=Some(key);host.pending=Some(Instant::now());}
  let elapsed=host.pending.map(|since|since.elapsed().as_secs_f64()*1000.0);let mut next_tick=None;
  if let Some(elapsed)=elapsed{
   if elapsed>=150.0&&!v["scrollAnimating"].as_bool().unwrap_or(false){
    host.pending=None;
    host.tail={let source=editor.pane.buffer.text();if source.len()>neoism_ui::editor::code::outline::OUTLINE_SOURCE_CUTOFF{String::new()}else{
     let byte=source.split_inclusive('\n').take(editor.pane.buffer.cursor_line).map(str::len).sum::<usize>()+editor.pane.buffer.cursor_col;
     neoism_ui::editor::code::outline::symbol_trail(&source,editor.pane.language,byte).into_iter().map(|symbol|symbol.name).collect::<Vec<_>>().join(" › ")
    }};
   }else if elapsed<150.0{next_tick=Some((150.0-elapsed).ceil().max(1.0));}
  }
  let path=v["path"].as_str().filter(|path|!path.is_empty()).map(PathBuf::from).unwrap_or(path);let root=v["root"].as_str().filter(|root|!root.is_empty()).map(PathBuf::from);
  host.view.set_scale(v["scale"].as_f64().unwrap_or(1.0)as f32);host.view.set_from_path(&path,root.as_deref());host.view.set_tail(0,0,&host.tail);
  host.paint.begin_frame();let width=v["width"].as_f64().unwrap_or(800.0).max(0.0)as f32;
  host.view.render_with_options(&mut host.paint,0.0,0.0,width,&IdeTheme::by_name(v["theme"].as_str().unwrap_or("pastel_dark")),true);
  Ok(json!({"commands":host.paint.commands(),"height":if host.view.is_visible(){host.view.height()}else{0.0},"segments":host.view.segments(),"tail":host.tail,"nextTickMs":next_tick}))
 }))
}
