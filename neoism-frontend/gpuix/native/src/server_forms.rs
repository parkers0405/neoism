//! The original desktop server forms rendered by the same UniversalModal.
use neoism_ui::{widgets::modal::{UniversalModal,ModalAction,ModalFormSpec,ModalFormField,ModalSpec,ModalButton},panels::command_palette::{PaletteAction,PaletteSystemEntry,SystemConnectionStatus}};
use serde_json::{json,Value};
fn text<'a>(v:&'a Value,key:&str)->&'a str{v[key].as_str().unwrap_or("")}
pub fn palette_action(action:PaletteAction)->Value{match action{
 PaletteAction::SelectServer{id}=>json!({"kind":"server","id":id}),
 PaletteAction::AddServer|PaletteAction::CreateServer=>json!({"kind":"server-add"}),
 PaletteAction::EditServer{id}=>json!({"kind":"server-edit","id":id}),
 PaletteAction::RemoveServer{id}=>json!({"kind":"server-remove","id":id}),
 PaletteAction::CopyFastServerInvite{id}=>json!({"kind":"server-invite","id":id}),
 PaletteAction::ShowSystems=>json!({"kind":"systems"}),
 PaletteAction::ShowServers=>json!({"kind":"servers"}),
 PaletteAction::ShowSystemSettings{id}=>json!({"kind":"system-settings","id":id}),
 PaletteAction::SshToSystem{id}|PaletteAction::ConnectSystem{id}=>json!({"kind":"system-connect","id":id}),
 PaletteAction::DisconnectSystem{id}=>json!({"kind":"system-disconnect","id":id}),
 PaletteAction::SetSystemEnabled{id,enabled}=>json!({"kind":"system-enabled","id":id,"enabled":enabled}),
 PaletteAction::RemoveSystem{id}=>json!({"kind":"system-remove","id":id}),
 PaletteAction::AddSystem=>json!({"kind":"system-add"}),
 action=>json!({"kind":"run","action":format!("{action:?}")})
}}
pub fn system(entry:&Value)->PaletteSystemEntry{PaletteSystemEntry{id:text(entry,"id").into(),name:text(entry,"name").into(),address:text(entry,"target").into(),local:entry["local"].as_bool().unwrap_or(false),enabled:entry["enabled"].as_bool().unwrap_or(false),status:match text(entry,"connection"){"connected"=>SystemConnectionStatus::Connected,"connecting"=>SystemConnectionStatus::Connecting,_=>SystemConnectionStatus::Disconnected}}}
pub fn open(v:&Value)->UniversalModal{
 let mut modal=UniversalModal::new();modal.set_scale(v["scale"].as_f64().unwrap_or(1.0)as f32);
 let kind=text(v,"kind");
 if kind=="server-form"{
  let host=text(v,"mode")!="join";let fast=v["fast"].as_bool().unwrap_or(false);let edit=!text(v,"serverId").is_empty();
  let(target,label,value,placeholder)=if edit{("address","Server address",text(v,"address"),"ws://192.168.1.20:7981/session")}else if host{(if fast{"fast_host_dir"}else{"create_dir"},"Project folder",text(v,"directory"),"~/code/myproject")}else{(if fast{"fast_join_address"}else{"address"},if fast{"Server link"}else{"Server address"},text(v,"address"),if fast{"Paste the Fast Server invite link"}else{"ws://192.168.1.20:7981/session"})};
  let mut fields=vec![ModalFormField{id:target.into(),label:label.into(),value:value.into(),placeholder:placeholder.into(),secret:false},ModalFormField{id:"name".into(),label:"Server name (optional)".into(),value:text(v,"name").into(),placeholder:"Team server".into(),secret:false},ModalFormField{id:"token".into(),label:if fast&&host{"Password (required)"}else{"Password (optional)"}.into(),value:text(v,"password").into(),placeholder:"password".into(),secret:true}];
  if edit{fields.push(ModalFormField{id:"server_id".into(),label:String::new(),value:text(v,"serverId").into(),placeholder:String::new(),secret:false});}
  modal.open_form(ModalFormSpec{title:if edit{"Edit Server"}else{"Add Server"}.into(),fields,submit_label:if edit{"Save server"}else if host{"Host server"}else{"Join server"}.into()});
  if !edit{modal.set_form_tabs(vec!["Host".into(),"Join".into()],if host{0}else{1});modal.set_form_switch("Fast tunnel",fast,true);}
 }else if kind=="system-form"{
  modal.open_form_with_action(ModalFormSpec{title:"Add system".into(),fields:vec![ModalFormField{id:"system_target".into(),label:"Computer".into(),value:String::new(),placeholder:"laptop, hostname, or SSH alias".into(),secret:false},ModalFormField{id:"system_name".into(),label:"Name (optional)".into(),value:String::new(),placeholder:"Mac Studio".into(),secret:false}],submit_label:"Add system".into()},ModalAction::SystemFormSubmit);
  modal.set_form_description("New systems are saved deactivated. Turn on Active in Systems when you want to connect.");
 }else if kind=="server-confirm"{
  modal.open(ModalSpec{title:text(v,"title").into(),body:text(v,"body").into(),meta:String::new(),input:None,buttons:vec![ModalButton::new("Remove","Enter",ModalAction::ServerFormSubmit),ModalButton::new("Cancel","Esc",ModalAction::Close)],busy:false,blocking:true});
 }else{
  modal.open(ModalSpec{title:text(v,"title").into(),body:text(v,"body").into(),meta:String::new(),input:None,buttons:vec![ModalButton::new(if v["busy"].as_bool().unwrap_or(false){"Cancel"}else{"OK"},"Esc",ModalAction::Close)],busy:v["busy"].as_bool().unwrap_or(false),blocking:true});
 }
 modal
}
pub fn submit(modal:&mut UniversalModal,kind:&str,context:&Value)->Value{
 if kind=="server-confirm"{modal.close();return json!({"kind":"confirm-remove","id":context["serverId"],"system":context["system"]});}
 let values=modal.form_values();json!({"kind":"form-submit","values":values,"system":kind=="system-form"})
}
pub fn click(modal:&mut UniversalModal,v:&Value,width:f32,kind:&str,context:&Value)->Value{
 let x=v["x"].as_f64().unwrap_or(0.0)as f32;let y=v["y"].as_f64().unwrap_or(0.0)as f32;
 if modal.close_button_hit(x,y){modal.close();return Value::Null;}
 if let Some(tab)=modal.form_tab_hit(x,y){return json!({"kind":"form-mode","mode":if tab==0{"host"}else{"join"},"fast":modal.form_switch_checked(),"values":modal.form_values()});}
 if modal.form_switch_hit(x,y){let fast=!modal.form_switch_checked().unwrap_or(false);return json!({"kind":"form-mode","mode":if modal.form_active_tab()==0{"host"}else{"join"},"fast":fast,"values":modal.form_values()});}
 match modal.hit_test(x,y,width,1.0){Err(())=>modal.close(),Ok(Some(hit))=>{if !modal.focus_form_hit(hit){modal.set_selected_index(hit);if matches!(modal.selected_action(),Some(ModalAction::Close)){modal.close();}else{return submit(modal,kind,context);}}},_=>{}}
 Value::Null
}
pub fn key(modal:&mut UniversalModal,v:&Value,kind:&str,context:&Value)->Value{
 let name=text(v,"key");let shift=v["shift"].as_bool().unwrap_or(false);
 match name{
 "escape"=>modal.close(),"tab"=>{if shift{modal.focus_previous_form_field();}else{modal.focus_next_form_field();}},
 "up"=>{modal.form_focus_move(false);},"down"=>{modal.form_focus_move(true);},
 "enter"=>{if modal.form_switch_focused(){let fast=!modal.form_switch_checked().unwrap_or(false);return json!({"kind":"form-mode","mode":if modal.form_active_tab()==0{"host"}else{"join"},"fast":fast,"values":modal.form_values()});}if modal.form_buttons_focused()&&matches!(modal.selected_action(),Some(ModalAction::Close)){modal.close();}else{return submit(modal,kind,context);}},
 "backspace"=>modal.pop_input(),"delete"=>modal.delete_input(),"left"=>modal.move_input_caret_left(),"right"=>modal.move_input_caret_right(),"home"=>modal.input_caret_to_start(),"end"=>modal.input_caret_to_end(),
 _=>{let chars=v["text"].as_str().unwrap_or(text(v,"char"));if !chars.is_empty(){modal.push_input(chars);}}
 }
 Value::Null
}
