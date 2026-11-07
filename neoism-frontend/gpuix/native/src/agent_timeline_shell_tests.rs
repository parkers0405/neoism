use super::*;

#[test]
fn shell_output_receives_wheel_focus_and_scrollbar_drag_through_public_dispatch() {
    let created=dispatch(&json!({"op":"agenttimeline.new","sessionId":"shell-chat","reducedMotion":true})).unwrap().unwrap();
    let id=created["id"].as_u64().unwrap() as u32;
    let send=|op:&str, mut args:Value| {
        args["op"]=json!(format!("agenttimeline.{op}"));args["id"]=json!(id);
        dispatch(&args).unwrap().unwrap()
    };
    send("event",json!({"message":{"HistoryChunk":{"session_id":"shell-chat","next_cursor":null,"messages":[
        {"id":"prior","kind":"Assistant","role":"Assistant","text":"Earlier conversation\n\n".repeat(50)},
        {"id":"command","kind":"User","role":"User","text":"!generate output"},
        {"id":"shell-output","kind":"Tool","role":"Assistant","tool":"user_shell","status":"completed",
            "title":"generate output","text":"/project  |  exit 0",
            "detail":(0..100).map(|i|format!("line{i:03}")).collect::<Vec<_>>().join("\n")}
    ]}}}));
    send("layout",json!({"width":700,"height":700,"contentBottom":650}));
    for _ in 0..16 {
        if send("frame",json!({}))["preparing"]==false { break; }
    }
    send("scroll",json!({"deltaY":-100000,"precise":true}));
    let key="shell:shell-output";
    let state=|| STORE.with(|store|{
        let mut store=store.borrow_mut();let view=store.views.get_mut(&id).unwrap();
        let (rect,max)=view.pane.diff_scroll_target(key).unwrap_or_else(||panic!("paint must register the shell viewport: scroll={:?}, texts={:?}",view.pane.timeline_scrollbar_state(),view.painter.borrowed_commands().iter().filter_map(|c|match c {neoism_ui::primitives::paint_record::PaintCommand::Text{text,y,..}=>Some((text.clone(),*y)),_=>None}).take(30).collect::<Vec<_>>()));
        (rect,max,view.pane.diff_scroll_offset(key,max),view.pane.timeline_scroll_offset())
    });
    let (rect,_,initial,outer)=state();assert_eq!(initial,0.0);
    let (x,y)=(rect[0]+30.0,rect[1]+20.0);
    send("scroll",json!({"x":x,"y":y,"deltaY":-20,"precise":true}));
    assert_eq!(state().2,0.0,"hover alone must not capture output scrolling");
    send("pointer",json!({"kind":"down","x":x,"y":y}));
    send("pointer",json!({"kind":"up","x":x,"y":y}));
    send("scroll",json!({"x":x,"y":y,"deltaY":-1,"lineHeight":24,"precise":false,"deferFrame":true}));
    assert_eq!(state().2,72.0);assert_eq!(state().3,outer);
    send("scroll",json!({"x":x,"y":y,"deltaY":-20,"precise":true}));
    assert_eq!(state().2,92.0);
    let frame=send("scroll",json!({"deltaY":-10000,"precise":true}));
    assert!(frame["commands"].as_array().unwrap().iter().any(|c|c["text"]=="line099"));
    assert_eq!(state().2,state().1);assert_eq!(state().3,outer);
    send("scroll",json!({"deltaY":-10000,"precise":true}));
    assert_eq!(state().3,outer,"wheel at the block's end must not move the conversation");

    let (rect,_,_,_)=state();
    let gutter=rect[0]+rect[2]-6.0;
    let down=send("pointer",json!({"kind":"down","x":gutter,"y":rect[1]+rect[3]-2.0}));
    assert_eq!(down["selecting"],true,"host must capture scrollbar drags too");
    let top=send("pointer",json!({"kind":"move","x":gutter,"y":rect[1]-20.0}));
    assert_eq!(state().2,0.0);assert_eq!(state().3,outer);
    assert!(top["commands"].as_array().unwrap().iter().any(|c|c["text"]=="line000"));
    let up=send("pointer",json!({"kind":"up","x":gutter,"y":rect[1]-20.0}));
    assert_eq!(up["selecting"],false);
    send("pointer",json!({"kind":"move","x":gutter,"y":rect[1]+rect[3]}));
    assert_eq!(state().2,0.0,"released scrollbar must stop following the pointer");
    send("pointer",json!({"kind":"down","x":x,"y":y}));
    send("pointer",json!({"kind":"up","x":x,"y":y}));
    send("scroll",json!({"x":x,"y":y,"deltaY":100,"precise":true}));
    assert_eq!(state().2,0.0);
    assert!(state().3>outer,"clicking again must let the conversation scroll from inside the block");
    let (rect,_,_,outer_after_release)=state();
    let y=rect[1]+20.0;
    send("pointer",json!({"kind":"down","x":x,"y":y}));
    send("pointer",json!({"kind":"up","x":x,"y":y}));
    let focused_outer=state().3;
    send("scroll",json!({"x":x,"y":y,"deltaY":-20,"precise":true,"deferFrame":true}));
    assert_eq!(state().2,20.0,"another click can activate the block again");
    assert_eq!(state().3,focused_outer);
    send("pointer",json!({"kind":"down","x":0,"y":0}));
    send("pointer",json!({"kind":"up","x":0,"y":0}));
    send("scroll",json!({"deltaY":100,"precise":true,"deferFrame":true}));
    STORE.with(|store|assert!(store.borrow().views[&id].pane.timeline_scroll_offset()>outer_after_release));
    send("dispose",json!({}));
}