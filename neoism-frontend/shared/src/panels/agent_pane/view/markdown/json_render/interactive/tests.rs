use super::*;
use crate::primitives::paint_record::{PaintCommand,RecordingPainter};

fn prepared(v:Value)->NativeLayout {
    let source=v.to_string();let spec=parse(&source).expect("valid test spec");
    let mut l=layout(&spec,400.0,1.0,&mut |s,size,_|s.chars().count() as f32*size*0.5);
    Arc::make_mut(l.prepared.as_mut().unwrap()).source=source;l
}
fn one(kind:&str,props:Value)->NativeLayout {prepared(json!({"root":"a","elements":{"a":{"type":kind,"props":props}}}))}
fn paint(r:&RefCell<Runtime>,l:&NativeLayout,namespace:&str,y:f32,clip:[f32;4],cover:&[[f32;4]],mouse:Option<(f32,f32)>)->Vec<PaintCommand> {
    let mut p=RecordingPainter::new("monospace",900,vec![]);
    render_interactive(&mut p,l,Some(r),namespace,0,0.0,y,&IdeTheme::default(),1.0,clip,cover,mouse);p.commands()
}
fn frame_session(r:&RefCell<Runtime>,l:&NativeLayout,ns:&str,session:&str)->Value {
    r.borrow_mut().begin_frame(session,[0.0,0.0,500.0,2000.0]);paint(r,l,ns,0.0,[0.0,0.0,500.0,2000.0],&[],None);r.borrow().metadata()
}
fn frame(r:&RefCell<Runtime>,l:&NativeLayout,ns:&str)->Value {frame_session(r,l,ns,"chat")}
fn control(meta:&Value,label:&str)->String {meta["controls"].as_array().unwrap().iter().find(|v|v["label"]==label).unwrap_or_else(||panic!("missing {label}: {meta}"))["id"].as_str().unwrap().to_owned()}

#[test]
fn json_render_rejects_execution_sources_and_action_results() {
    for (kind,props) in [("Button",json!({"label":"Go","prompt":"p"})),("Form",json!({"submitLabel":"Go","prompt":"p"}))] {
        assert!(parse(&json!({"root":"a","elements":{"a":{"type":kind,"props":props}}}).to_string()).is_none());
    }
    let l=one("Input",json!({"label":"Name"}));let r=RefCell::new(Runtime::default());let m=frame(&r,&l,"m");let id=control(&m,"Name");
    for status in ["sending","sent","failed","cancelled"] {assert!(r.borrow_mut().event(&id,"action-result",Some(&json!(status))).is_err());}
    assert!(r.borrow_mut().event(&id,"activate",Some(&json!({"prompt":"evil"}))).is_err());
    assert!(!r.borrow().metadata().as_object().unwrap().contains_key("action"));
}

#[test]
fn json_render_local_fields_validate_and_never_submit() {
    let l=prepared(json!({"root":"s","elements":{
        "s":{"type":"Stack","props":{},"children":["name","size","check"]},
        "name":{"type":"Input","props":{"label":"Name","multiline":true}},
        "size":{"type":"Select","props":{"label":"Size","options":["S","L"]}},
        "check":{"type":"Checkbox","props":{"label":"Consent"}}
    }}));let r=RefCell::new(Runtime::default());
    for (label,v) in [("Name",json!("literal ${state.secret}\nnotes")),("Size",json!("L")),("Consent",json!(true))] {
        let m=frame(&r,&l,"m");r.borrow_mut().event(&control(&m,label),"change",Some(&v)).unwrap();
        let m=frame(&r,&l,"m");assert_eq!(m["controls"].as_array().unwrap().iter().find(|c|c["label"]==label).unwrap()["value"],v);assert!(m.get("action").is_none());
    }
    let m=frame(&r,&l,"m");
    for (label,v) in [("Name",json!("x".repeat(2001))),("Name",json!(false)),("Size",json!("XL")),("Consent",json!("true"))] {assert!(r.borrow_mut().event(&control(&m,label),"change",Some(&v)).is_err());}
}

#[test]
fn json_render_local_tabs_collapse_filter_reflow_and_stale_targets() {
    let l=prepared(json!({"root":"tabs","elements":{
        "tabs":{"type":"Tabs","props":{"labels":["First","Second"]},"children":["collapse","other"]},
        "collapse":{"type":"Collapsible","props":{"title":"Details"},"children":["filter"]},
        "filter":{"type":"Filter","props":{"label":"Search"},"children":["alpha","beta"]},
        "alpha":{"type":"Checkbox","props":{"label":"ÉCLAIR"}},"beta":{"type":"Checkbox","props":{"label":"Beta"}},
        "other":{"type":"Text","props":{"text":"Other"}}
    }}));let r=RefCell::new(Runtime::default());let m=frame(&r,&l,"m");let closed=height(&l,Some(&r),"m",0);
    assert_eq!(m["controls"].as_array().unwrap().len(),3);
    r.borrow_mut().event(&control(&m,"Details"),"activate",None).unwrap();let m=frame(&r,&l,"m");let open=height(&l,Some(&r),"m",0);assert!(open>closed);
    let alpha=control(&m,"ÉCLAIR");let beta=control(&m,"Beta");r.borrow_mut().event(&control(&m,"Search"),"change",Some(&json!("éclAir"))).unwrap();
    assert!(r.borrow_mut().event(&beta,"change",Some(&json!(true))).is_err());let m=frame(&r,&l,"m");assert_eq!(control(&m,"ÉCLAIR"),alpha);assert!(height(&l,Some(&r),"m",0)<open);
    r.borrow_mut().event(&control(&m,"Second"),"activate",None).unwrap();let m=frame(&r,&l,"m");assert_eq!(m["controls"].as_array().unwrap().len(),2);assert!(r.borrow_mut().event(&alpha,"change",Some(&json!(true))).is_err());
    r.borrow_mut().event(&control(&m,"First"),"activate",None).unwrap();let m=frame(&r,&l,"m");assert_eq!(control(&m,"ÉCLAIR"),alpha);assert!(m.get("action").is_none());
    let key=instance_key("chat","m",0);let expected=reflow(&l,&r.borrow().instances[&key]).0.height;assert_eq!(expected,height(&l,Some(&r),"m",0));
}

#[test]
fn json_render_instance_isolation_resize_source_revision_and_bounded_eviction() {
    let l=one("Input",json!({"label":"Name"}));let r=RefCell::new(Runtime::default());let a=control(&frame(&r,&l,"message-a"),"Name");r.borrow_mut().event(&a,"change",Some(&json!("Alice"))).unwrap();
    assert_eq!(frame(&r,&l,"message-b")["controls"][0]["value"],"");assert_eq!(frame_session(&r,&l,"message-a","other")["controls"][0]["value"],"");assert_eq!(frame(&r,&l,"message-a")["controls"][0]["value"],"Alice");
    let mut resized=l.clone();resized.width=500.0;assert_eq!(frame(&r,&resized,"message-a")["controls"][0]["value"],"Alice");
    let edited=one("Input",json!({"label":"Name","defaultValue":"Bob"}));let m=frame(&r,&edited,"message-a");assert_ne!(control(&m,"Name"),a);assert_eq!(m["controls"][0]["value"],"Bob");assert!(r.borrow_mut().event(&a,"change",Some(&json!("late"))).is_err());
    r.borrow_mut().observe_source("chat","message-a","");assert!(r.borrow_mut().event(&control(&m,"Name"),"change",Some(&json!("late"))).is_err());
    for n in 0..140 {height(&l,Some(&r),&format!("m{n}"),0);}assert_eq!(r.borrow().instances.len(),128);
}

#[test]
fn json_render_fully_usable_visible_targets_only() {
    let l=one("Collapsible",json!({"title":"Details"}));let r=RefCell::new(Runtime::default());let id=control(&frame(&r,&l,"m"),"Details");
    for (y,clip,covers) in [(-1000.0,[0.0,0.0,500.0,200.0],vec![]),(0.0,[0.0,0.0,500.0,24.0],vec![]),(0.0,[0.0,0.0,500.0,200.0],vec![[0.0,20.0,500.0,40.0]])] {
        r.borrow_mut().begin_frame("chat",[0.0,0.0,500.0,200.0]);paint(&r,&l,"m",y,clip,&covers,None);assert!(r.borrow().metadata()["controls"].as_array().unwrap().is_empty());assert!(r.borrow_mut().event(&id,"activate",None).is_err());
    }
}

#[test]
fn json_render_chart_custom_color_hover_selection_and_no_action() {
    let l=one("LineGraph",json!({"title":"Trend","color":"#00aaff","data":[{"label":"A","value":1},{"label":"B","value":4},{"label":"C","value":2}]}));let r=RefCell::new(Runtime::default());let m=frame(&r,&l,"m");let id=control(&m,"Trend");
    let t=r.borrow().visible[&id].clone();assert_eq!(t.control.points.len(),3);let point=t.control.points[1];let x=t.control.rect[0]+point.0;let y=t.control.rect[1]+point.1;
    assert!(paint(&r,&l,"m",0.0,[0.0,0.0,500.0,2000.0],&[],Some((x,y))).iter().any(|c|matches!(c,PaintCommand::Text{text,..} if text=="B: 4")));
    assert!(r.borrow_mut().pointer_activate(x,y));assert_eq!(frame(&r,&l,"m")["controls"][0]["value"],1);
    for bad in [json!(-1),json!(3),json!(0.5),json!("1")] {assert!(r.borrow_mut().event(&id,"activate",Some(&bad)).is_err());}
    r.borrow_mut().event(&id,"activate",Some(&json!(2))).unwrap();let m=frame(&r,&l,"m");assert_eq!(m["controls"][0]["value"],2);assert!(m.get("action").is_none());
}

#[test]
fn json_render_diagram_local_navigation_lifecycle_and_visibility() {
    let props=json!({"title":"Flow","nodes":[{"id":"a","label":"Start"},{"id":"b","label":"End"}],"edges":[{"from":"a","to":"b"}]});let l=one("Diagram",props.clone());let r=RefCell::new(Runtime::default());
    let initial=json!({"zoom":1.0,"panX":0.0,"panY":0.0});let m=frame(&r,&l,"m");let id=control(&m,"Flow");assert_eq!(m["controls"][0]["value"],initial);
    for command in [json!("zoom-in"),json!("pan-left"),json!("pan-right"),json!("pan-up"),json!("pan-down"),json!({"dx":35.0,"dy":-20.0}),json!("zoom-out")] {
        frame(&r,&l,"m");r.borrow_mut().event(&id,"activate",Some(&command)).unwrap();assert!(r.borrow().metadata().get("action").is_none());assert!(r.borrow_mut().event(&id,"activate",Some(&command)).is_err());
    }
    let current=frame(&r,&l,"m")["controls"][0]["value"].clone();assert_eq!(current,json!({"zoom":1.0,"panX":35.0,"panY":-20.0}));assert_eq!(frame(&r,&l,"other")["controls"][0]["value"],initial);assert_eq!(frame_session(&r,&l,"m","other")["controls"][0]["value"],initial);frame(&r,&l,"m");
    for bad in [json!({"dx":null,"dy":0}),json!({"prompt":"send"}),json!("execute")] {assert!(r.borrow_mut().event(&id,"activate",Some(&bad)).is_err());}
    r.borrow_mut().event(&id,"activate",Some(&json!("fit"))).unwrap();assert_eq!(frame(&r,&l,"m")["controls"][0]["value"],initial);
    r.borrow_mut().begin_frame("chat",[0.0,0.0,500.0,300.0]);paint(&r,&l,"m",0.0,[0.0,0.0,500.0,300.0],&[],None);assert!(r.borrow().metadata()["controls"].as_array().unwrap().is_empty());assert!(r.borrow_mut().event(&id,"activate",Some(&json!("fit"))).is_err());
    let mut edited=props;edited["nodes"][0]["label"]=json!("Changed");let edited=one("Diagram",edited);assert_ne!(control(&frame(&r,&edited,"m"),"Flow"),id);
}

#[test]
fn json_render_flat_groups_and_single_control_labels() {
    let l=prepared(json!({"root":"outer","elements":{
        "outer":{"type":"Card","props":{},"children":["inner"]},"inner":{"type":"Card","props":{},"children":["collapse","check"]},
        "collapse":{"type":"Collapsible","props":{"title":"Details"}},"check":{"type":"Checkbox","props":{"label":"Include notes"}}
    }}));let r=RefCell::new(Runtime::default());let m=frame(&r,&l,"m");let commands=paint(&r,&l,"m",0.0,[0.0,0.0,500.0,2000.0],&[],None);
    for (label,shown) in [("Details","▸ Details"),("Include notes","☐ Include notes")] {
        let rect=&m["controls"].as_array().unwrap().iter().find(|c|c["label"]==label).unwrap()["rect"];
        let lines:Vec<_>=commands.iter().filter_map(|c|match c {PaintCommand::Text{text,y,size,..} if text.contains(label)=>Some((text,y,size)),_=>None}).collect();assert_eq!(lines.len(),1);let (text,y,size)=lines[0];assert_eq!(text,shown);assert!((y+size*0.5-rect[1].as_f64().unwrap() as f32-rect[3].as_f64().unwrap() as f32*0.5).abs()<0.01);
    }
    assert!(!commands.iter().any(|c|matches!(c,PaintCommand::Rect{rect,..} if rect[2]>=l.width)));
}