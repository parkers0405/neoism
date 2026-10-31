//! The same context menu and confirmation/input modal as the Sugarloaf host.
//! This bridge returns commands; filesystem and session effects stay on the host.
use neoism_ui::{panels::context_menu::{ContextMenu,ContextMenuAction,ContextMenuItem},widgets::modal::{UniversalModal,ModalAction,ModalButton,ModalInputSpec,ModalSpec},primitives::{paint_record::RecordingPainter,IdeTheme}};
use serde_json::{json,Value};
use std::{cell::RefCell,collections::HashMap};
enum Panel { Menu(ContextMenu), Form(UniversalModal) }
struct Host { panel:Panel,paint:RecordingPainter,width:f32,height:f32,theme:String }
thread_local! { static HOSTS:RefCell<HashMap<u64,Host>>=RefCell::new(HashMap::new()); static NEXT:RefCell<u64>=RefCell::new(0xe200_0000); }
fn text<'a>(v:&'a Value,key:&str)->&'a str{v[key].as_str().unwrap_or("")}
fn n(v:&Value,key:&str,default:f32)->f32{v[key].as_f64().filter(|n|n.is_finite()).map_or(default,|n|n as f32)}
fn outcome(action:Option<ModalAction>)->Value{match action{Some(ModalAction::RunEditorCommand{command})=>json!({"command":command}),Some(ModalAction::RunEditorCommandWithInput{command,value})=>json!({"command":command,"value":value}),Some(ModalAction::Close)=>json!({"cancelled":true}),_=>Value::Null}}
fn menu_outcome(action:Option<ContextMenuAction>)->Value{match action{Some(ContextMenuAction::Modal(action))=>outcome(Some(action)),_=>Value::Null}}
impl Host {
 fn frame(&mut self,result:Value)->Value{
  self.paint.begin_frame();let theme=IdeTheme::by_name(&self.theme);
  let(active,animating,rect)=match &mut self.panel{
   Panel::Menu(menu)=>{menu.render(&mut self.paint,(self.width,self.height,1.0),&theme);(menu.is_visible(),menu.is_animating(),menu.rect())},
   Panel::Form(form)=>{form.render_portable(&mut self.paint,(self.width,self.height,1.0),&theme);(form.is_active(),form.needs_redraw(),form.active_rect(self.width,1.0))}
  };json!({"commands":self.paint.commands(),"active":active,"animating":animating,"rect":rect,"result":result})
 }
}
pub fn dispatch(v:&Value)->Option<Result<Value,String>>{
 let op=text(v,"op");if !op.starts_with("sidebarcontext."){return None;}
 Some(HOSTS.with(|hosts|{let mut hosts=hosts.borrow_mut();
  if op=="sidebarcontext.new"{
   let scale=n(v,"scale",1.0);let width=n(v,"width",960.0);let height=n(v,"height",680.0);
   let panel=if text(v,"kind")=="form"{
    let mut form=UniversalModal::new();form.set_scale(scale);
    let input=v["input"].as_str().map(|value|ModalInputSpec{value:value.into(),placeholder:text(v,"placeholder").into()});
    let action=if input.is_some(){ModalAction::RunEditorCommandWithInput{command:"accept".into(),value:String::new()}}else{ModalAction::RunEditorCommand{command:"accept".into()}};
    form.open(ModalSpec{title:text(v,"title").into(),body:text(v,"body").into(),meta:text(v,"meta").into(),input,buttons:vec![ModalButton::new(text(v,"accept"),"Enter",action),ModalButton::new("Cancel","Esc",ModalAction::Close)],busy:false,blocking:true});Panel::Form(form)
   }else{
    let mut menu=ContextMenu::new();menu.set_scale(scale);
    let items=v["items"].as_array().into_iter().flatten().map(|item|ContextMenuItem::new(text(item,"label"),text(item,"hint"),ContextMenuAction::Modal(ModalAction::RunEditorCommand{command:text(item,"id").into()}))).collect();
    menu.open(text(v,"title"),items,n(v,"x",0.0),n(v,"y",0.0),width,height);Panel::Menu(menu)
   };
   let id=NEXT.with(|next|{let mut next=next.borrow_mut();*next+=1;*next});let mut host=Host{panel,paint:RecordingPainter::new(text(v,"fontFamily"),v["fontWeight"].as_u64().unwrap_or(400)as u16,vec![]),width,height,theme:text(v,"theme").into()};let frame=host.frame(Value::Null);hosts.insert(id,host);return Ok(json!({"id":id,"frame":frame}));
  }
  let id=v["id"].as_u64().ok_or("Missing sidebar menu ID")?;if op=="sidebarcontext.dispose"{hosts.remove(&id);return Ok(Value::Null);}
  let host=hosts.get_mut(&id).ok_or("Unknown sidebar menu")?;let mut result=Value::Null;
  if op=="sidebarcontext.layout"{host.width=n(v,"width",host.width);host.height=n(v,"height",host.height);}
  match &mut host.panel{
   Panel::Menu(menu)=>match op{
    "sidebarcontext.key"=>match text(v,"key"){"escape"=>result=json!({"cancelled":true}),"up"|"k"=>menu.move_selection(-1),"down"|"j"=>menu.move_selection(1),"enter"=>result=menu_outcome(menu.selected_action()),_=>if let Some(ch)=text(v,"char").chars().next(){result=menu_outcome(menu.select_shortcut(ch));}},
    "sidebarcontext.pointer"=>{let x=n(v,"x",0.0);let y=n(v,"y",0.0);if text(v,"kind")=="down"{result=if menu.hit_test(x,y).is_err(){json!({"cancelled":true})}else{menu_outcome(menu.action_at(x,y))};}else{menu.hover(x,y);}},
    "sidebarcontext.scroll"=>menu.scroll_pixels(n(v,"delta",0.0)),_=>{}
   },
   Panel::Form(form)=>match op{
    "sidebarcontext.text"=>form.push_input(text(v,"text")),
    "sidebarcontext.key"=>match text(v,"key"){
     "escape"=>result=json!({"cancelled":true}),"enter"=>result=outcome(form.selected_action()),"up"=>form.move_selection_up(),"down"|"tab"=>if v["shift"].as_bool().unwrap_or(false){form.move_selection_up()}else{form.move_selection_down()},
     "backspace"=>form.pop_input(),"delete"=>form.delete_input(),"left"=>form.move_input_caret_left(),"right"=>form.move_input_caret_right(),"home"=>form.input_caret_to_start(),"end"=>form.input_caret_to_end(),_=>form.push_input(text(v,"char"))
    },
    "sidebarcontext.pointer"=>{let x=n(v,"x",0.0);let y=n(v,"y",0.0);if text(v,"kind")=="down"{result=if form.close_button_hit(x,y){json!({"cancelled":true})}else{match form.hit_test(x,y,host.width,1.0){Ok(Some(index))=>{form.set_selected_index(index);outcome(form.selected_action())},Err(())=>json!({"cancelled":true}),Ok(None)=>Value::Null}};}else{form.pointer_move(x,y,host.width,1.0);}},_=>{}
   }
  }
  Ok(host.frame(result))
 }))
}
