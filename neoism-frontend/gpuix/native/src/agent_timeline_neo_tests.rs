use super::*;

fn send(id:u32, op:&str, mut args:Value)->Value {
    args["op"]=json!(format!("agenttimeline.{op}")); args["id"]=json!(id);
    dispatch(&args).unwrap().unwrap()
}
fn create(neo:bool)->u32 {
    dispatch(&json!({"op":"agenttimeline.new","sessionId":"neo-fixture","neoMode":neo,"reducedMotion":true})).unwrap().unwrap()["id"].as_u64().unwrap() as u32
}
fn settled(id:u32)->Value {
    for _ in 0..64 { let frame=send(id,"frame",json!({})); if frame["preparing"]==false {return frame;} }
    panic!("timeline preparation did not settle")
}
fn texts(frame:&Value)->String {
    frame["commands"].as_array().unwrap().iter().filter_map(|c|c["text"].as_str()).collect::<Vec<_>>().join("")
}
fn history(id:u32) {
    send(id,"event",json!({"deferFrame":true,"message":{"HistoryChunk":{"session_id":"neo-fixture","next_cursor":null,"messages":[
        {"id":"u","kind":"User","role":"User","text":"Hello Neo"},
        {"id":"t","kind":"Tool","role":"Assistant","tool":"bash","title":"SECRET_TOOL","text":"SECRET_RESULT","status":"completed"},
        {"id":"r","kind":"Reasoning","role":"Assistant","text":"SECRET_REASONING"},
        {"id":"sys","kind":"System","role":"System","tool":"location_notice","text":"SECRET_SYSTEM"},
        {"id":"sub","kind":"Subtask","role":"Assistant","text":"SECRET_SUBTASK"},
        {"id":"runtime","kind":"User","role":"User","text":"Subagent finished. SECRET_RUNTIME"},
        {"id":"a","kind":"Assistant","role":"Assistant","text":"First thought\n\nSecond thought","status":"Build · MODEL_SECRET · 20s"},
        {"id":"b","kind":"Assistant","role":"Assistant","text":"Another reply"}
    ]}}}));
    send(id,"layout",json!({"width":700,"height":900,"contentBottom":850}));
}

#[test]
fn neo_projection_bubbles_history_live_reload_and_normal_mode() {
    let id=create(true); history(id);
    let frame=settled(id); let text=texts(&frame);
    for expected in ["Hello Neo","First thought","Second thought","Another reply"] {assert!(text.contains(expected),"{text}");}
    assert!(!text.contains("SECRET"),"{text}");
    let bubbles:Vec<_>=frame["commands"].as_array().unwrap().iter().filter(|c|c["kind"]=="rect" && c["radii"][0].as_f64()==Some(16.0)).collect();
    assert_eq!(bubbles.len(),4,"user + two assistant paragraphs + distinct next assistant: {bubbles:?}");
    assert!(bubbles[0]["rect"][0].as_f64().unwrap()>bubbles[1]["rect"][0].as_f64().unwrap(),"user right, Neo left");
    for pair in bubbles.windows(2) {
        let bottom=pair[0]["rect"][1].as_f64().unwrap()+pair[0]["rect"][3].as_f64().unwrap();
        let next=pair[1]["rect"][1].as_f64().unwrap();
        assert!(next>=bottom && next-bottom<40.0,"no overlap or hidden-tool gap: {pair:?}");
    }
    // Select the whole painted projection through the same pointer path as UI.
    let commands=frame["commands"].as_array().unwrap();
    let first=commands.iter().find(|c|c["text"]=="Hello Neo").unwrap();
    let last=commands.iter().rev().find(|c|c["text"].as_str().is_some()).unwrap();
    let start_x=first["x"].as_f64().unwrap();let start_y=first["y"].as_f64().unwrap()+3.0;
    let end_y=last["y"].as_f64().unwrap()+3.0;
    send(id,"pointer",json!({"kind":"down","x":start_x,"y":start_y}));
    send(id,"pointer",json!({"kind":"move","x":699,"y":end_y}));
    send(id,"pointer",json!({"kind":"up","x":699,"y":end_y}));
    let copy=send(id,"copy",json!({}))["text"].as_str().unwrap_or("").to_owned();
    assert!(!copy.contains("SECRET"),"hidden execution data leaked: {copy}");
    assert!(copy.contains("First thought") && copy.contains("Second thought"),"all visible paragraphs selectable: {copy}");
    send(id,"event",json!({"message":{"MessageUpdated":{"session_id":"neo-fixture","message":{"id":"b","kind":"Assistant","role":"Assistant","text":"Another reply updated"}}}}));
    assert!(texts(&settled(id)).contains("updated"));
    send(id,"event",json!({"message":{"MessageUpdated":{"session_id":"neo-fixture","message":{"id":"t","kind":"Tool","role":"Assistant","tool":"bash","title":"SECRET_LIVE_TOOL","text":"SECRET_LIVE_RESULT","status":"running"}}}}));
    assert!(!texts(&settled(id)).contains("SECRET"),"live tool updates stay filtered");
    STORE.with(|store|assert_eq!(store.borrow().views[&id].pane.messages().len(),8,"projection must not delete raw history"));
    history(id); assert!(!texts(&settled(id)).contains("SECRET"));
    send(id,"layout",json!({"neoMode":false}));
    let normal=texts(&settled(id));
    assert!(normal.contains("SECRET_SYSTEM") && normal.contains("MODEL_SECRET"),"regular projection retains its scaffolding/footer behavior: {normal}");
    send(id,"layout",json!({"neoMode":true}));
    assert!(!texts(&settled(id)).contains("SECRET"));
    let regular=create(false);history(regular);
    assert!(texts(&settled(regular)).contains("SECRET_SYSTEM"),"Neo cannot leak into another Build view");
    send(id,"dispose",json!({}));send(regular,"dispose",json!({}));
}

#[test]
fn neo_full_user_text_quiet_activity_and_scroll() {
    let id=create(true);
    let text=(0..16).map(|i|format!("User line {i:02}")).collect::<Vec<_>>().join("\n");
    send(id,"event",json!({"message":{"HistoryChunk":{"session_id":"neo-fixture","next_cursor":null,"messages":[{"id":"u","kind":"User","role":"User","text":text}]}}}));
    send(id,"layout",json!({"width":700,"height":1000,"contentBottom":950}));
    assert!(texts(&settled(id)).contains("User line 15"),"Neo must not truncate long user messages");
    send(id,"event",json!({"message":{"StreamingState":{"session_id":"neo-fixture","state":"Generating","label":"SECRET_WORKING_STATUS"}}}));
    let active=texts(&settled(id));assert!(active.contains("•••"));assert!(!active.contains("SECRET_WORKING_STATUS"));
    send(id,"layout",json!({"width":400,"height":180,"contentBottom":160}));
    let initial=STORE.with(|s|s.borrow().views[&id].pane.timeline_scroll_offset());
    send(id,"scroll",json!({"deltaY":10000,"precise":true,"deferFrame":true}));
    let scrolled=STORE.with(|s|s.borrow().views[&id].pane.timeline_scroll_offset());
    assert!(scrolled>initial,"reuse normal timeline scrolling");
    send(id,"dispose",json!({}));
}

#[test]
fn neo_structured_markdown_stays_whole_and_copyable() {
    let id=create(true);
    let source="Here is the example.\n\n```rust\nlet first = 1;\n\nlet second = 2;\n```\n\n- one\n- two\n\n| Name | Value |\n| --- | --- |\n| test | result |";
    send(id,"event",json!({"message":{"HistoryChunk":{"session_id":"neo-fixture","next_cursor":null,"messages":[{"id":"a","kind":"Assistant","role":"Assistant","text":source}]}}}));
    send(id,"layout",json!({"width":700,"height":1200,"contentBottom":1150}));
    let frame=settled(id);
    let bubbles=frame["commands"].as_array().unwrap().iter().filter(|c|c["kind"]=="rect" && c["radii"][0].as_f64()==Some(16.0)).count();
    assert_eq!(bubbles,1,"never split fenced code, lists or tables on embedded blank lines");
    let text=texts(&frame);
    for expected in ["first", "second", "one", "two", "Name", "result"] {assert!(text.contains(expected),"structured content lost: {text}");}
    STORE.with(|s|assert_eq!(s.borrow().views[&id].pane.messages()[0].text,source));
    send(id,"dispose",json!({}));
}