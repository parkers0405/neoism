//! Canonical desktop presence state, hosted without a display/window.
use neoism_protocol::crdt::{CrdtCursorPosition, CrdtServerMessage, CrdtSelectionRange};
use neoism_ui::editor::crdt::{peer_presence_to_wire, PeerCursor, PeerSelection, PresenceColor, PresencePublisher, RemotePresenceStore};
use serde_json::{json, Value};
use std::{cell::RefCell, collections::HashMap};
struct Host { publisher: PresencePublisher, remote: RemotePresenceStore, paint:Option<neoism_ui::primitives::paint_record::RecordingPainter>, style:String }
thread_local! {static HOSTS:RefCell<HashMap<u64,Host>>=RefCell::new(HashMap::new());static NEXT:RefCell<u64>=RefCell::new(0xb100_0000);}
fn cursor(v:&Value)->Result<PeerCursor,String>{let c:CrdtCursorPosition=serde_json::from_value(v.clone()).map_err(|e|e.to_string())?;Ok(PeerCursor{line:c.line,column:c.column,offset:c.offset})}
fn frame(host:&Host,send:Value,changed:bool)->Value {
 let peers:Vec<_>=host.remote.avatar_peers().into_iter().map(|p|json!({"peer_id":p.peer_id,"display_name":p.display_name,"client_os":p.client_os,"color":p.color,"rainbow":p.rainbow})).collect();
 let buffers:HashMap<_,Vec<_>>=host.remote.avatar_peers_by_buffer().into_iter().map(|(buffer,_)|{let peers=host.remote.cursors_for(&buffer).cloned().map(peer_presence_to_wire).collect();(buffer,peers)}).collect();
 json!({"peers":peers,"buffers":buffers,"send":send,"changed":changed})
}
pub fn dispatch(v:&Value)->Option<Result<Value,String>> {
 let op=v["op"].as_str()?;if !op.starts_with("presence."){return None;}
 Some(HOSTS.with(|store|{
  let mut store=store.borrow_mut();
  if op=="presence.buffer-id"{return Ok(json!({"bufferId":neoism_ui::editor::crdt::presence_buffer_id_for_path(std::path::Path::new(v["path"].as_str().ok_or("Missing host path")?))}));}
  if op=="presence.new" {
   let id=NEXT.with(|n|{let mut n=n.borrow_mut();*n+=1;*n});
   let peer=v["peerId"].as_str().ok_or("Missing peer identity")?;
   let mut publisher=PresencePublisher::new(peer,v["displayName"].as_str().unwrap_or(peer));
   publisher.set_client_os(v["clientOS"].as_str().map(str::to_owned));
   let mut remote=RemotePresenceStore::new();remote.set_local_peer_id(peer);
   store.insert(id,Host{publisher,remote,paint:None,style:String::new()});return Ok(json!({"id":id}));
  }
  let id=v["id"].as_u64().ok_or("Missing presence id")?;
  if op=="presence.dispose" {store.remove(&id);return Ok(json!({}));}
  let host=store.get_mut(&id).ok_or("Unknown presence host")?;
  if op=="presence.row" {
   let style=json!([v["family"],v["weight"]]).to_string();
   if host.paint.is_none()||host.style!=style{host.paint=Some(neoism_ui::primitives::paint_record::RecordingPainter::new(v["family"].as_str().unwrap_or("Cascadia Code NF"),v["weight"].as_u64().unwrap_or(400)as u16,vec![]));host.style=style;}
   let peers:Vec<_>=host.remote.cursors_for(v["bufferId"].as_str().unwrap_or("")).map(|peer|neoism_ui::editor::crdt::PresenceAvatarPeer{peer_id:peer.peer_id.clone(),display_name:peer.display_name.clone(),client_os:peer.client_os.clone(),color:[peer.color.r,peer.color.g,peer.color.b],rainbow:peer.rainbow}).collect();
   let painter=host.paint.as_mut().expect("initialized above");painter.begin_frame();
   let scale=v["scale"].as_f64().unwrap_or(1.0).clamp(0.5,3.0)as f32;
   let theme=neoism_ui::primitives::IdeTheme::by_name(v["theme"].as_str().unwrap_or("pastel_dark"));
   let occupied=neoism_ui::panels::file_tree::paint_host_presence_row(painter,&peers,256.0,scale,&theme);
   return Ok(json!({"commands":painter.commands(),"width":if occupied>0.0{occupied+8.0*scale}else{0.0},"height":neoism_ui::panels::file_tree::ROW_HEIGHT*scale}));
  }
  let mut changed=false;let mut send=vec![];
  match op {
   "presence.event"=>{let message:CrdtServerMessage=serde_json::from_value(v["message"].clone()).map_err(|e|e.to_string())?;changed=host.remote.apply_server_message(&message);}
   "presence.reset"=>{changed=host.remote.clear();let peer=host.publisher.peer_id().to_owned();let name=host.publisher.display_name().to_owned();let color=host.publisher.color();let os=host.publisher.client_os().map(str::to_owned);host.publisher=PresencePublisher::new(peer,name);host.publisher.set_color(color);host.publisher.set_client_os(os);}
   "presence.tick"=>{
    if let Some(color)=v["color"].as_array(){if color.len()==3{host.publisher.set_color(PresenceColor{r:color[0].as_u64().unwrap_or(0)as u8,g:color[1].as_u64().unwrap_or(0)as u8,b:color[2].as_u64().unwrap_or(0)as u8});}}
    host.publisher.set_rainbow(v["rainbow"].as_bool().unwrap_or(false));
    let active=&v["active"];let parsed=if let Some(buffer)=active["bufferId"].as_str(){
     let selection=if active["selection"].is_object(){let s:CrdtSelectionRange=serde_json::from_value(active["selection"].clone()).map_err(|e|e.to_string())?;Some(PeerSelection{anchor:PeerCursor{line:s.anchor.line,column:s.anchor.column,offset:s.anchor.offset},head:PeerCursor{line:s.head.line,column:s.head.column,offset:s.head.offset}})}else{None};
     Some((buffer,cursor(&active["cursor"])?,selection,active["insert"].as_bool().unwrap_or(false)))
    }else{None};
    let now=v["now"].as_u64().unwrap_or(0);
    send=host.publisher.tick(parsed.as_ref().map(|(b,c,s,i)|(*b,*c,*s,*i)),now);
    changed|=host.remote.prune_stale(now,10_000);
   }
   "presence.frame"=>{},_=>return Err("Unknown presence operation".into())
  }
  Ok(frame(host,serde_json::to_value(send).map_err(|e|e.to_string())?,changed))
 }))
}
