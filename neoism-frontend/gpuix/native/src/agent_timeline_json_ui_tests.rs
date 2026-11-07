//! Direct native view tests; host/FFI integration coverage lives with the host.
use super::*;

fn view(source:&str)->View {
    let mut pane=NeoismAgentPane::new();pane.set_session_id(Some("json-session".into()));pane.set_text_reveal_enabled(false);
    let event:AgentServerMessage=serde_json::from_value(json!({"HistoryChunk":{"session_id":"json-session","messages":[{"id":"json-message","role":"Assistant","kind":"Assistant","text":format!("```json-render\n{source}\n```")}],"next_cursor":null}})).unwrap();
    apply_agent_event_to_pane(&mut pane,event);
    View{visible:true,preparation_scope:987654,preparing:false,scroll_trace:None,compact_paint:false,canvas_paint:false,chunked_paint:false,font_family:"monospace".into(),font_weight:400,pane,painter:RecordingPainter::new("monospace",400,vec![]),session_id:"json-session".into(),width:700.0,height:1000.0,content_bottom:900.0,scale:1.0,theme:"dark".into(),mouse:None,selecting:false,selection:None,scrollbar_drag:false,horizontal_drag:false,inner_scroll_focus:None,inner_scroll_drag:None}
}
fn ready(view:&mut View)->Value {let mut m=frame(view);for _ in 0..8 {if !view.preparing {break;}m=frame(view);}m}
fn local_source()->String {json!({"root":"c","elements":{
    "c":{"type":"Collapsible","props":{"title":"Details"},"children":["b"]},
    "b":{"type":"Checkbox","props":{"label":"Include notes"}}
}}).to_string()}
fn no_submit(m:&Value) {assert!(m["jsonUi"].get("action").is_none());assert!(m["send"].as_array().unwrap().is_empty());}

#[test]
fn agent_timeline_native_json_ui_pointer_reflows_and_never_sends() {
    let mut v=view(&local_source());let m=ready(&mut v);let controls=m["jsonUi"]["controls"].as_array().unwrap();assert_eq!(controls.len(),1,"{m}");
    let rect=controls[0]["rect"].as_array().unwrap();let x=rect[0].as_f64().unwrap()+5.0;let y=rect[1].as_f64().unwrap()+5.0;
    let m=pointer(&mut v,&json!({"kind":"down","x":x,"y":y}));assert_eq!(m["jsonUi"]["controls"].as_array().unwrap().len(),2,"{m}");no_submit(&m);
    let id=m["jsonUi"]["controls"].as_array().unwrap().iter().find(|c|c["label"]=="Include notes").unwrap()["id"].as_str().unwrap().to_owned();
    let m=json_ui_event(&mut v,&json!({"control":id,"kind":"change","value":true}));assert_eq!(m["jsonUiAccepted"],true);no_submit(&m);
    for status in ["sending","sent","failed","cancelled"] {let m=json_ui_event(&mut v,&json!({"control":id,"kind":"action-result","value":status}));assert_eq!(m["jsonUiAccepted"],false);no_submit(&m);}
    neoism_ui::panels::agent_pane::view::markdown::release_preparation_scope(v.preparation_scope);
}

#[test]
fn agent_timeline_native_json_ui_namespaced_height_isolates_identical_messages() {
    let source=local_source();let mut v=view(&source);v.height=1800.0;v.content_bottom=1700.0;
    // Separate turns keep the normal conversation projection from folding parts.
    let event:AgentServerMessage=serde_json::from_value(json!({"HistoryChunk":{"session_id":"json-session","messages":[
        {"id":"a-first","created_at":1,"role":"Assistant","kind":"Assistant","text":format!("```json-render\n{source}\n```")},
        {"id":"b-next-turn","created_at":2,"role":"User","kind":"User","text":"Next independent response"},
        {"id":"c-second","created_at":3,"role":"Assistant","kind":"Assistant","text":format!("```json-render\n{source}\n```")}
    ],"next_cursor":null}})).unwrap();
    if let AgentServerMessage::HistoryChunk{session_id,messages,next_cursor}=event {v.pane.replace_session_history(&session_id,messages.into_iter().map(neoism_ui::panels::agent_pane::server_events::map_history).collect(),next_cursor);}else{unreachable!();}
    let m=ready(&mut v);let controls=m["jsonUi"]["controls"].as_array().unwrap();assert_eq!(controls.len(),2);
    let first=controls[0]["id"].as_str().unwrap().to_owned();let second=controls[1]["id"].as_str().unwrap().to_owned();assert_ne!(first,second);
    let m=json_ui_event(&mut v,&json!({"control":first,"kind":"activate"}));let controls=m["jsonUi"]["controls"].as_array().unwrap();assert_eq!(controls.len(),3);assert_eq!(controls.iter().find(|c|c["id"]==first).unwrap()["value"],true);assert_eq!(controls.iter().find(|c|c["id"]==second).unwrap()["value"],false);
    let m=json_ui_event(&mut v,&json!({"control":second,"kind":"activate"}));assert_eq!(m["jsonUi"]["controls"].as_array().unwrap().len(),4);no_submit(&m);
    for c in m["jsonUi"]["controls"].as_array().unwrap() {let r=&c["rect"];assert!(r[1].as_f64().unwrap()>=0.0);assert!(r[1].as_f64().unwrap()+r[3].as_f64().unwrap()<=v.content_bottom as f64);}
    neoism_ui::panels::agent_pane::view::markdown::release_preparation_scope(v.preparation_scope);
}

#[test]
fn agent_timeline_native_json_ui_composer_viewport_and_repaint_clear_targets() {
    let mut v=view(&local_source());let m=ready(&mut v);let id=m["jsonUi"]["controls"][0]["id"].as_str().unwrap().to_owned();
    v.content_bottom=1.0;let m=render_frame(&mut v);assert!(m["jsonUi"]["controls"].as_array().unwrap().is_empty());assert!(v.pane.json_ui.borrow_mut().event(&id,"activate",None).is_err());
    v.content_bottom=900.0;assert_eq!(ready(&mut v)["jsonUi"]["controls"][0]["id"],id);
    v.visible=false;let m=render_frame(&mut v);assert!(m["jsonUi"]["controls"].as_array().unwrap().is_empty());assert!(v.pane.json_ui.borrow_mut().event(&id,"activate",None).is_err());
    v.visible=true;assert_eq!(ready(&mut v)["jsonUi"]["controls"][0]["id"],id);
    neoism_ui::panels::agent_pane::view::markdown::release_preparation_scope(v.preparation_scope);
}

#[test]
fn agent_timeline_native_json_ui_rejects_old_execution_sources() {
    for (kind,props) in [("Button",json!({"label":"Send","prompt":"p"})),("Form",json!({"submitLabel":"Send","prompt":"p"}))] {
        let source=json!({"root":"b","elements":{"b":{"type":kind,"props":props}}}).to_string();let mut v=view(&source);let m=ready(&mut v);assert!(m["jsonUi"]["controls"].as_array().unwrap().is_empty());no_submit(&m);
        let m=json_ui_event(&mut v,&json!({"control":"Send","kind":"activate"}));assert_eq!(m["jsonUiAccepted"],false);no_submit(&m);
        neoism_ui::panels::agent_pane::view::markdown::release_preparation_scope(v.preparation_scope);
    }
}