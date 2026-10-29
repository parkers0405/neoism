//! Original Rust notification stack, owned across component generations.
use neoism_ui::{panels::notifications::{Notifications,NotificationLevel},primitives::{IdeTheme,paint_record::RecordingPainter}};
use serde_json::{json,Value};
use std::{cell::RefCell,collections::HashMap};
struct Host{view:Notifications,paint:RecordingPainter,style:String}
thread_local!{static HOSTS:RefCell<HashMap<u64,Host>>=RefCell::new(HashMap::new());static NEXT:RefCell<u64>=RefCell::new(0xb300_0000);}
fn n(v:&Value,key:&str,d:f32)->f32{v[key].as_f64().unwrap_or(d as f64)as f32}
pub fn dispatch(v:&Value)->Option<Result<Value,String>>{
 let op=v["op"].as_str()?;if !op.starts_with("notifications."){return None;}
 Some(HOSTS.with(|hosts|{let mut hosts=hosts.borrow_mut();
  if op=="notifications.new"{let id=NEXT.with(|n|{let mut n=n.borrow_mut();*n+=1;*n});hosts.insert(id,Host{view:Notifications::new(),paint:RecordingPainter::new("Cascadia Code NF",400,vec![]),style:String::new()});return Ok(json!({"id":id}));}
  let id=v["id"].as_u64().ok_or("Missing notifications id")?;
  if op=="notifications.dispose"{hosts.remove(&id);return Ok(json!({}));}
  let host=hosts.get_mut(&id).ok_or("Unknown notifications host")?;
  let style=json!([v["fontFamily"],v["fontWeight"]]).to_string();if style!=host.style{host.paint=RecordingPainter::new(v["fontFamily"].as_str().unwrap_or("Cascadia Code NF"),v["fontWeight"].as_u64().unwrap_or(400)as u16,vec![]);host.style=style;}
  host.view.set_scale(n(v,"scale",1.0));let width=n(v,"width",800.0);let height=n(v,"height",600.0);
  match op{
   "notifications.push"=>host.view.push(v["message"].as_str().unwrap_or(""),match v["level"].as_str(){Some("info")=>NotificationLevel::Info,Some("warn")=>NotificationLevel::Warn,_=>NotificationLevel::Error}),
   "notifications.hover"=>{host.view.hover(n(v,"x",-1.0),n(v,"y",-1.0),width,1.0,0.0);},
   "notifications.leave"=>{host.view.clear_hover();},
   "notifications.dismiss"=>host.view.dismiss(v["index"].as_u64().unwrap_or(0)as usize),_=>{}
  }
  host.paint.begin_frame();host.view.render(&mut host.paint,(width,height,1.0),0.0,&IdeTheme::by_name(v["theme"].as_str().unwrap_or("pastel_dark")));
  Ok(json!({"commands":host.paint.commands(),"rects":host.view.host_rects(width,0.0),"messages":host.view.host_messages(),"active":host.view.is_active()}))
 }))
}
