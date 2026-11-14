use rune_motion::{MotionTrace, Mount, NodeId, NodeSpec, Rect, Scene, TraceHeader};
use serde_json::{Value, json};

#[test]
fn trace_names_channels_and_events_match_motion_lint() {
    let header = TraceHeader { launch: "pill".into(), width: 200.0, height: 80.0, hz: 120.0, scale: 2.0 };
    let (trace, buffer) = MotionTrace::in_memory(&header);
    let mut scene = Scene::new();
    // Attaching to an already settled scene must describe its nodes too.
    scene.insert(NodeId(40), NodeSpec::new(Rect::new(0.0, 0.0, 100.0, 40.0)), Mount::Initial);
    scene.insert(NodeId(90), NodeSpec::new(Rect::new(10.0, 0.0, 20.0, 20.0)), Mount::Initial);
    scene.frame_at(0.0);
    scene.set_trace(Some(trace.class("gpui").host("pill")));
    scene.trace_name(NodeId(40), Some("chip:\"Area\""));
    let trace = scene.trace_mut().unwrap();
    trace.channel("shell.width", "position", 100.12345);
    trace.channel("phase:recording", "phase", 1.0);
    trace.event(format_args!("lab:{:?}", "Start\n"));
    scene.frame_at(0.01);
    scene.frame_at(0.02);
    scene.trace_name(NodeId(40), Some("chip:Region"));
    scene.trace_mut().unwrap().channel("shell.width", "position", 120.0);
    scene.trace_mut().unwrap().remove_channel("phase:recording");
    scene.frame_at(0.03);
    scene.trace_name(NodeId(40), None);
    scene.trace_mut().unwrap().channel("phase:recording", "phase", 1.0);
    scene.trace_mut().unwrap().channel("shell.width", "position", f32::NAN);
    scene.frame_at(0.04);
    scene.take_trace().unwrap().flush().unwrap();
    let lines: Vec<Value> = buffer.contents().lines().map(|s| serde_json::from_str(s).unwrap()).collect();
    let named = lines[1]["m"].as_array().unwrap().iter().find(|m| m["nm"] == "chip:\"Area\"").unwrap();
    let id = named["id"].clone();
    assert_eq!(named["host"], "pill");
    assert_eq!(named["cls"], "gpui");
    assert!(lines[1]["m"].as_array().unwrap().iter().any(|m| m["nm"] == "90"));
    assert_eq!(lines[1]["cm"], json!([
        {"id": 1, "name": "shell.width", "kind": "position"},
        {"id": 2, "name": "phase:recording", "kind": "phase"},
    ]));
    assert_eq!(lines[1]["c"], json!([[1, 100.1235], [2, 1]]));
    assert_eq!(lines[1]["ev"], json!(["lab:\"Start\\n\""]));
    for field in ["m", "cm", "c", "ev"] { assert!(lines[2][field].is_null(), "duplicate {field}"); }
    assert_eq!(lines[3]["m"][0]["id"], id);
    assert_eq!(lines[3]["m"][0]["nm"], "chip:Region");
    assert_eq!(lines[3]["c"], json!([[1, 120]]));
    assert_eq!(lines[3]["cg"], json!([2]));
    assert_eq!(lines[4]["m"][0]["nm"], "40");
    assert_eq!(lines[4]["c"], json!([[2, 1]]));
}
