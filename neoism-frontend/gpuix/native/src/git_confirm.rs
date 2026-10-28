//! Pure original Git restore confirmation. It never opens a repository or
//! mutates Git; the host receives the exact validated confirmation target.
use neoism_ui::{panels::git_diff::review_actions::{restore_confirmation,restore_block_confirmation},widgets::modal::{ModalAction,UniversalModal},primitives::{paint_record::RecordingPainter,IdeTheme}};
use neoism_protocol::git::GitChangeBlock;
use serde_json::{json,Value};
use std::{cell::RefCell,collections::HashMap,path::PathBuf};
struct Host{modal:UniversalModal,paint:RecordingPainter,width:f32,height:f32,theme:String,target:Value,outcome:Option<bool>}
thread_local!{static HOSTS:RefCell<HashMap<u64,Host>>=RefCell::new(HashMap::new());static NEXT:RefCell<u64>=RefCell::new(0xe100_0000);}
fn text<'a>(v:&'a Value,key:&str)->&'a str{v[key].as_str().unwrap_or("")}
fn n(v:&Value,key:&str,default:f32)->f32{v[key].as_f64().map_or(default,|n|n as f32)}
impl Host{
 fn accept(&mut self,action:Option<ModalAction>){if self.outcome.is_some(){return;}if let Some(action)=action{self.outcome=Some(matches!(action,ModalAction::GitRestore{..}|ModalAction::GitRestoreBlock{..}));self.modal.close();}}
 fn frame(&mut self)->Value{self.paint.begin_frame();self.modal.render_portable(&mut self.paint,(self.width,self.height,1.0),&IdeTheme::by_name(&self.theme));json!({"commands":self.paint.commands(),"active":self.modal.is_active(),"animating":self.modal.needs_redraw(),"rect":self.modal.active_rect(self.width,1.0),"confirmed":self.outcome,"target":self.target})}
}
pub fn dispatch(v:&Value)->Option<Result<Value,String>>{let op=text(v,"op");if !op.starts_with("gitconfirm."){return None;}Some(HOSTS.with(|hosts|{let mut hosts=hosts.borrow_mut();
 if op=="gitconfirm.new"{let target=&v["target"];let root=PathBuf::from(text(target,"root"));let path=text(target,"path").to_owned();if root.as_os_str().is_empty()||path.is_empty(){return Err("Missing restore target".into());}
  let block=target.get("block").filter(|b|!b.is_null()).map(|b|serde_json::from_value::<GitChangeBlock>(b.clone())).transpose().map_err(|e|format!("Invalid restore block: {e}"))?;
  let exact=json!({"root":root,"path":path,"block":block});let mut modal=UniversalModal::new();let scale=n(v,"scale",1.0);modal.set_scale(scale);modal.set_top_anchor(14.0*scale);modal.open(match block{Some(block)=>restore_block_confirmation(root,path,block),None=>restore_confirmation(root,path)});
  let id=NEXT.with(|next|{let mut next=next.borrow_mut();*next+=1;*next});hosts.insert(id,Host{modal,paint:RecordingPainter::new(text(v,"fontFamily"),v["fontWeight"].as_u64().unwrap_or(400)as u16,vec![]),width:n(v,"width",960.0),height:n(v,"height",680.0),theme:text(v,"theme").to_owned(),target:exact,outcome:None});return Ok(json!({"id":id}));
 }
 let id=v["id"].as_u64().ok_or("Missing Git confirmation ID")?;if op=="gitconfirm.dispose"{hosts.remove(&id);return Ok(Value::Null);}let host=hosts.get_mut(&id).ok_or("Unknown Git confirmation")?;
 match op{
  "gitconfirm.layout"=>{host.width=n(v,"width",host.width).max(1.0);host.height=n(v,"height",host.height).max(1.0);},
  "gitconfirm.key"=>match text(v,"key"){"up"|"k"=>host.modal.move_selection_up(),"down"|"j"|"tab"=>if v["shift"].as_bool().unwrap_or(false){host.modal.move_selection_up()}else{host.modal.move_selection_down()},"enter"=>{let action=host.modal.selected_action();host.accept(action);},"escape"=>{let action=host.modal.escape_action();host.accept(action);},_=>{}},
  "gitconfirm.pointer"=>{let x=n(v,"x",0.0);let y=n(v,"y",0.0);match text(v,"kind"){
   "move"=>{host.modal.pointer_move(x,y,host.width,1.0);},"leave"=>{host.modal.pointer_leave();},"down"=>{let action=if host.modal.close_button_hit(x,y){host.modal.escape_action()}else{match host.modal.hit_test(x,y,host.width,1.0){Ok(Some(index))=>{host.modal.set_selected_index(index);host.modal.selected_action()},Err(())=>host.modal.escape_action(),Ok(None)=>None}};host.accept(action);},_=>{}
  }},"gitconfirm.frame"=>{},_=>return Err(format!("Unknown Git confirmation operation {op}"))
 }Ok(host.frame())
}))}
