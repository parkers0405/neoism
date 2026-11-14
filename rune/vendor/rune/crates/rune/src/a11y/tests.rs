//! The accessibility tree the runtime builds, and the actions it runs.

use accesskit::{Action as Ak, ActionData, ActionRequest, Role, TreeId};
use serde_json::Value;

use crate::{Cx, El, Env, GrowthSpec, LiveRegion, Runtime, ScrollSpec, TextEdit, col, row, text, text_input};

fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    if v["key"] == key { return Some(v); }
    v["children"].as_array()?.iter().find_map(|c| find(c, key))
}

fn count(v: &Value) -> usize {
    1 + v["children"].as_array().map_or(0, |c| c.iter().map(count).sum())
}

fn states(v: &Value) -> Vec<&str> {
    v["states"].as_array().map_or(Vec::new(), |s| s.iter().filter_map(Value::as_str).collect())
}

fn actions(v: &Value) -> Vec<&str> {
    v["actions"].as_array().map_or(Vec::new(), |s| s.iter().filter_map(Value::as_str).collect())
}

fn request<S>(rt: &Runtime<S>, key: &str, action: Ak, data: Option<ActionData>) -> ActionRequest {
    let tree = rt.accessibility_tree();
    let (id, _) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some(key)).expect("node keyed so");
    ActionRequest { action, target_tree: TreeId::ROOT, target_node: *id, data }
}

#[derive(Default)]
struct Form {
    presses: u32,
    on: bool,
    open: bool,
    level: i32,
    name: TextEdit,
    menu: u32,
}

fn form(s: &Form, _: &mut Cx<Form>) -> El {
    col().w_full().gap(8.0)
        .child(text("Settings").key("title").role("heading").level(1))
        .child(row().key("save").role("button").focusable(true).on_press(|s: &mut Form| s.presses += 1)
            .on_secondary_press(|s: &mut Form, _, _| s.menu += 1)
            .child(text("Save")))
        .child(row().key("close").role("button").label("Close window").focusable(true).on_press(|s: &mut Form| s.presses += 10)
            .child(crate::el().key("x").role("icon").size(12.0, 12.0)))
        .child(row().key("wifi").role("switch").checked(s.on).focusable(true).on_press(|s: &mut Form| s.on = !s.on)
            .child(text("Wi-Fi")))
        .child(row().key("more").role("button").expanded(s.open).focusable(true).on_press(|s: &mut Form| s.open = !s.open)
            .child(text("More")))
        .child(crate::el().key("volume").role("slider").range_value(f64::from(s.level), 0.0, 10.0).range_step(1.0)
            .label("Volume").size(100.0, 12.0)
            .on_step(|s: &mut Form, d, _| s.level = (s.level + d).clamp(0, 10)))
        .child(text_input(&s.name, crate::tok::type_::BODY).key("name").single_line().w(200.0)
            .on_edit(|s: &mut Form| &mut s.name))
        .child(row().key("off").role("button").disabled(true).child(text("Unavailable")))
        .child(text("Saved").key("status").role("status"))
        .child(text("Decoration").key("deco").a11y_hidden())
}

fn form_rt() -> Runtime<Form> {
    let mut rt = Runtime::new(Form::default(), form, (400.0, 500.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

#[test]
fn controls_report_role_name_value_states_and_actions() {
    let rt = form_rt();
    let tree = rt.accessibility_json();
    assert_eq!(tree["role"], "window");
    let title = find(&tree, "title").unwrap();
    assert_eq!((title["role"].as_str(), title["name"].as_str(), title["level"].as_u64()), (Some("heading"), Some("Settings"), Some(1)));
    let save = find(&tree, "save").unwrap();
    assert_eq!((save["role"].as_str(), save["name"].as_str()), (Some("button"), Some("Save")));
    assert!(actions(save).contains(&"click") && actions(save).contains(&"focus") && actions(save).contains(&"context-menu"));
    let close = find(&tree, "close").unwrap();
    assert_eq!(close["name"], "Close window", "an icon-only button takes its label");
    let wifi = find(&tree, "wifi").unwrap();
    assert_eq!((wifi["role"].as_str(), wifi["name"].as_str()), (Some("switch"), Some("Wi-Fi")));
    assert!(states(wifi).contains(&"unchecked"));
    let more = find(&tree, "more").unwrap();
    assert!(states(more).contains(&"collapsed") && actions(more).contains(&"expand"));
    let volume = find(&tree, "volume").unwrap();
    assert_eq!(volume["role"], "slider");
    assert_eq!(volume["range"], serde_json::json!([0.0, 0.0, 10.0, 1.0]));
    assert!(actions(volume).contains(&"increment"));
    let name = find(&tree, "name").unwrap();
    assert_eq!(name["role"], "text-input");
    assert!(actions(name).contains(&"set-value"));
    let off = find(&tree, "off").unwrap();
    assert!(states(off).contains(&"disabled") && actions(off).is_empty());
    let status = find(&tree, "status").unwrap();
    assert!(states(status).contains(&"live-polite"));
    assert!(find(&tree, "deco").is_none(), "a11y_hidden leaves the tree");
    let b = save["bounds"].as_array().unwrap();
    assert!(b[2].as_f64().unwrap() > 0.0 && b[3].as_f64().unwrap() > 0.0, "bounds in points: {b:?}");
}

#[test]
fn actions_run_the_same_handlers_as_pointer_and_keys() {
    let mut rt = form_rt();
    let click = request(&rt, "save", Ak::Click, None);
    assert!(rt.a11y_action(&click));
    rt.frame(0.0);
    assert_eq!(rt.state.presses, 1);
    let menu = request(&rt, "save", Ak::ShowContextMenu, None);
    assert!(rt.a11y_action(&menu));
    rt.frame(0.0);
    assert_eq!(rt.state.menu, 1);
    let toggle = request(&rt, "wifi", Ak::Click, None);
    rt.a11y_action(&toggle);
    rt.frame(0.0);
    assert!(rt.state.on);
    assert!(states(find(&rt.accessibility_json(), "wifi").unwrap()).contains(&"checked"));
    let expand = request(&rt, "more", Ak::Expand, None);
    assert!(rt.a11y_action(&expand));
    rt.frame(0.0);
    assert!(rt.state.open);
    let expand_again = request(&rt, "more", Ak::Expand, None);
    assert!(!rt.a11y_action(&expand_again), "expanding an open disclosure does nothing");
    for _ in 0..3 {
        let up = request(&rt, "volume", Ak::Increment, None);
        rt.a11y_action(&up);
        rt.frame(0.0);
    }
    assert_eq!(rt.state.level, 3);
    let to_seven = request(&rt, "volume", Ak::SetValue, Some(ActionData::NumericValue(7.2)));
    assert!(rt.a11y_action(&to_seven), "a slider with a step takes a number");
    rt.frame(0.0);
    assert_eq!(rt.state.level, 7, "set as steps through on_step, rounded to a step");
    let set = request(&rt, "name", Ak::SetValue, Some(ActionData::Value("Ada".into())));
    assert!(rt.a11y_action(&set));
    rt.frame(0.0);
    assert_eq!(rt.state.name.text, "Ada");
    let tree = rt.accessibility_json();
    let name = find(&tree, "name").unwrap();
    assert_eq!(name["value"], "Ada");
    assert!(states(name).contains(&"focused"), "editing focuses the field: {name}");
    let off = request(&rt, "off", Ak::Click, None);
    assert!(!rt.a11y_action(&off), "a disabled control takes no action");
}

#[test]
fn focus_moves_through_the_tree() {
    let mut rt = form_rt();
    assert_eq!(rt.accessibility_tree().focus, super::a11y_tree::ROOT);
    let focus = request(&rt, "save", Ak::Focus, None);
    assert!(rt.a11y_action(&focus));
    rt.frame(0.0);
    let tree = rt.accessibility_tree();
    assert_eq!(tree.nodes.iter().find(|(id, _)| *id == tree.focus).and_then(|(_, n)| n.author_id()), Some("save"));
}

#[test]
fn updates_send_only_what_changed_and_nothing_while_inactive() {
    let mut rt = form_rt();
    assert!(rt.a11y_update().is_none(), "no client, no tree");
    rt.a11y_activate("Form");
    let full = rt.a11y_update().expect("first update after activation");
    assert!(full.tree.is_some() && full.nodes.len() > 10);
    assert!(rt.a11y_update().is_none(), "nothing changed");
    rt.frame(0.0);
    assert!(rt.a11y_update().is_none(), "an idle frame changes nothing");
    let toggle = request(&rt, "wifi", Ak::Click, None);
    rt.a11y_action(&toggle);
    rt.frame(0.0);
    let update = rt.a11y_update().expect("the switch changed");
    assert!(update.tree.is_none());
    assert!(update.nodes.len() <= 3, "only the switch and what moved: {:?}", update.nodes.iter().map(|(_, n)| n.role()).collect::<Vec<_>>());
    assert!(update.nodes.iter().any(|(_, n)| n.author_id() == Some("wifi")));
    rt.a11y_deactivate();
    rt.frame(0.0);
    assert!(rt.a11y_update().is_none());
}

/// A window closed and opened again keeps its runtime but gets a new adapter, which AccessKit
/// may already hold active (AT-SPI turned on): its first update must hold the whole tree, or
/// AccessKit panics building the tree from it.
#[test]
fn a_reopened_surface_gets_the_whole_tree() {
    let mut rt = form_rt();
    let mut first = crate::AccessibilityLink::new();
    first.activate();
    assert!(first.service(&mut rt, "Form").expect("activated").tree.is_some());
    let toggle = request(&rt, "wifi", Ak::Click, None);
    rt.a11y_action(&toggle);
    rt.frame(0.0);
    assert!(first.service(&mut rt, "Form").expect("the switch changed").tree.is_none());
    drop(first);
    let mut second = crate::AccessibilityLink::new();
    rt.frame(0.0);
    assert!(second.service(&mut rt, "Form").is_none(), "nothing before the new client asks");
    second.activate();
    assert!(second.service(&mut rt, "Form").expect("activated").tree.is_some());
}

#[derive(Default)]
struct List {
    rows: usize,
}

fn list(s: &List, _: &mut Cx<List>) -> El {
    col().key("scroller").w_full().h(200.0).scroll(ScrollSpec { stick_bottom: false }).child(
        col().key("rows").role("list").growth(GrowthSpec::default()).virtual_rows(s.rows, 20.0,
            |_: &List, i| format!("row-{i}").into(),
            |_: &List, i| row().role("listitem").h(20.0).child(text(format!("Entry {i}")))),
    )
}

#[test]
fn virtual_lists_report_their_size_and_rows_their_place() {
    let mut rt = Runtime::new(List { rows: 10_000 }, list, (300.0, 200.0), Env::new(true, 1.0));
    for _ in 0..4 { rt.frame(0.0); }
    let tree = rt.accessibility_json();
    let rows = find(&tree, "rows").unwrap();
    assert_eq!(rows["setSize"], 10_000);
    let first = find(&tree, "row-0").unwrap();
    assert_eq!((first["position"].as_u64(), first["name"].as_str()), (Some(1), Some("Entry 0")));
    assert!(count(&tree) < 400, "only mounted rows: {}", count(&tree));
    let scroller = find(&tree, "scroller").unwrap();
    assert_eq!(scroller["role"], "scroll-view");
    assert!(states(scroller).contains(&"clips") && actions(scroller).contains(&"scroll-down"));
    let down = request(&rt, "scroller", Ak::ScrollDown, None);
    assert!(rt.a11y_action(&down));
    for _ in 0..4 { rt.frame(0.0); }
    let tree = rt.accessibility_json();
    let y = find(&tree, "scroller").unwrap()["scroll"][1].as_f64().unwrap();
    assert!(y > 100.0, "a page down scrolled: {y}");
}

#[test]
fn scrolling_changes_only_the_viewport_node() {
    let mut rt = Runtime::new(List { rows: 200 }, list, (300.0, 200.0), Env::new(true, 1.0));
    for _ in 0..4 { rt.frame(0.0); }
    rt.a11y_activate("List");
    rt.a11y_update();
    let down = request(&rt, "scroller", Ak::SetScrollOffset, Some(ActionData::SetScrollOffset(accesskit::Point { x: 0.0, y: 40.0 })));
    rt.a11y_action(&down);
    rt.frame(0.0);
    let update = rt.a11y_update().expect("scrolled");
    let changed: Vec<_> = update.nodes.iter().map(|(_, n)| n.author_id().unwrap_or("")).collect();
    assert!(changed.contains(&"scroller"), "{changed:?}");
    assert!(update.nodes.iter().all(|(_, n)| n.role() != Role::Label), "rows keep their content-space bounds: {changed:?}");
}

#[derive(Default)]
struct Field {
    edit: TextEdit,
}

fn field(s: &Field, _: &mut Cx<Field>) -> El {
    col().child(text_input(&s.edit, crate::tok::type_::BODY).key("note").w(300.0).on_edit(|s: &mut Field| &mut s.edit))
}

#[test]
fn text_fields_expose_lines_caret_and_selection() {
    let mut edit = TextEdit::new("hello world");
    edit.focused = true;
    edit.set_selection(0, 5);
    let mut rt = Runtime::new(Field { edit }, field, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let tree = rt.accessibility_tree();
    let (_, note) = tree.nodes.iter().find(|(_, n)| n.author_id() == Some("note")).unwrap();
    assert_eq!(note.role(), Role::MultilineTextInput);
    let runs: Vec<_> = tree.nodes.iter().filter(|(_, n)| n.role() == Role::TextRun).collect();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].1.value(), Some("hello world"));
    assert_eq!(runs[0].1.character_lengths().len(), 11);
    let sel = note.text_selection().expect("a selection");
    assert_eq!((sel.anchor.character_index, sel.focus.character_index), (0, 5));
    let positions = runs[0].1.character_positions().unwrap_or_default();
    assert!(positions.windows(2).all(|w| w[1] > w[0]), "characters advance: {positions:?}");
    // A reader moves the caret to the end of "world".
    let request = ActionRequest {
        action: Ak::SetTextSelection, target_tree: TreeId::ROOT, target_node: tree.nodes.iter().find(|(_, n)| n.author_id() == Some("note")).unwrap().0,
        data: Some(ActionData::SetTextSelection(accesskit::TextSelection {
            anchor: accesskit::TextPosition { node: runs[0].0, character_index: 11 },
            focus: accesskit::TextPosition { node: runs[0].0, character_index: 11 },
        })),
    };
    assert!(rt.a11y_action(&request));
    rt.frame(0.0);
    assert_eq!(rt.state.edit.range(), (11, 11));
}

#[test]
fn secure_fields_never_expose_their_text() {
    let edit = TextEdit::new("hunter2").secure();
    let mut rt = Runtime::new(Field { edit }, |s: &Field, _: &mut Cx<Field>| {
        col().child(text_input(&s.edit, crate::tok::type_::BODY).key("pw").single_line().label("Password").w(200.0).on_edit(|s: &mut Field| &mut s.edit))
    }, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let json = rt.accessibility_json().to_string();
    assert!(!json.contains("hunter2"), "{json}");
    let tree = rt.accessibility_json();
    let pw = find(&tree, "pw").unwrap();
    assert_eq!((pw["role"].as_str(), pw["name"].as_str()), (Some("password-input"), Some("Password")));
}

#[derive(Default)]
struct Linked {
    opened: Vec<String>,
}

#[test]
fn inline_links_are_their_own_nodes() {
    let mut rt = Runtime::new(Linked::default(), |_: &Linked, _: &mut Cx<Linked>| {
        col().child(crate::rich("Read the spec now.", vec![]).key("para").w(300.0)
            .link(crate::Link::new(9..13, "https://example.com/spec"))
            .on_link(|s: &mut Linked, id, _| s.opened.push(id.to_owned())))
    }, (400.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let tree = rt.accessibility_json();
    let para = find(&tree, "para").unwrap();
    assert_eq!(para["name"], "Read the spec now.");
    let link = find(&tree, "https://example.com/spec").unwrap();
    assert_eq!((link["role"].as_str(), link["name"].as_str(), link["url"].as_str()), (Some("link"), Some("spec"), Some("https://example.com/spec")));
    let click = request(&rt, "https://example.com/spec", Ak::Click, None);
    assert!(rt.a11y_action(&click));
    rt.frame(0.0);
    assert_eq!(rt.state.opened, ["https://example.com/spec"]);
}

#[test]
fn modal_hides_the_background_and_live_regions_announce() {
    #[derive(Default)]
    struct M;
    let mut rt = Runtime::new(M, |_: &M, _: &mut Cx<M>| {
        col()
            .child(row().key("behind").role("button").on_press(|_: &mut M| {}).child(text("Behind")))
            .child(col().key("sheet").label("Confirm").modal(true, |_: &mut M| {})
                .child(text("Are you sure?").key("q").live_region(LiveRegion::Assertive)))
    }, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.frame(0.0);
    let tree = rt.accessibility_json();
    assert!(find(&tree, "behind").is_none(), "behind a modal: {tree}");
    let sheet = find(&tree, "sheet").unwrap();
    assert_eq!((sheet["role"].as_str(), sheet["name"].as_str()), (Some("dialog"), Some("Confirm")));
    assert!(states(sheet).contains(&"modal"));
    assert!(states(find(&tree, "q").unwrap_or_else(|| panic!("{tree:#}"))).contains(&"live-assertive"));
}

struct Big {
    on: Vec<bool>,
}

fn big(s: &Big, _: &mut Cx<Big>) -> El {
    col().key("big").w_full().h(600.0).scroll(ScrollSpec { stick_bottom: false }).children(s.on.iter().enumerate().map(|(i, on)| {
        row().key(format!("r{i}")).role("listitem").h(24.0).gap(8.0)
            .child(row().key("check").role("checkbox").checked(*on).focusable(true).size(16.0, 16.0)
                .on_press(move |s: &mut Big| s.on[i] = !s.on[i]))
            .child(text(format!("Row {i}: the quick brown fox")))
    }))
}

/// What the tree costs: `cargo test -p rune --profile perf a11y_cost -- --ignored --nocapture`.
#[test]
#[ignore]
fn a11y_cost() {
    super::a11y_tree::CHECK_SENT.set(false);
    let median = |mut v: Vec<f64>| { v.sort_by(f64::total_cmp); v[v.len() / 2] };
    let mut rt = Runtime::new(Big { on: vec![false; 2000] }, big, (800.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let toggle = |rt: &mut Runtime<Big>, i: usize| { rt.update(|s, _| s.on[i] = !s.on[i]); rt.frame(0.0); };
    let time = |f: &mut dyn FnMut()| { let t = std::time::Instant::now(); f(); t.elapsed().as_secs_f64() * 1e6 };
    let off: Vec<f64> = (0..30).map(|i| time(&mut || toggle(&mut rt, i))).collect();
    rt.a11y_activate("Big");
    let full: Vec<f64> = (0..10).map(|_| { rt.a11y_activate("Big"); time(&mut || { rt.a11y_update(); }) }).collect();
    let on: Vec<f64> = (0..30).map(|i| time(&mut || toggle(&mut rt, i))).collect();
    let incremental: Vec<f64> = (0..30).map(|i| { toggle(&mut rt, i); time(&mut || { rt.a11y_update(); }) }).collect();
    let idle: Vec<f64> = (0..30).map(|_| time(&mut || { rt.a11y_update(); })).collect();
    rt.pointer_move(100.0, 100.0);
    let scroll: Vec<f64> = (1..=30).map(|k| {
        rt.wheel_event(crate::WheelEvent::new(0.0, -24.0).precise(true).at(100.0, 100.0));
        rt.frame(10.0 + k as f64 / 60.0);
        let commits = rt.commits;
        let mut sent = None;
        let t = time(&mut || { sent = rt.a11y_update().map(|u| u.nodes.len()); });
        assert_eq!(rt.commits, commits);
        assert!(sent.is_some_and(|n| n > 0), "a scroll sends its viewports");
        t
    }).collect();
    let nodes = rt.accessibility_tree().nodes.len();
    println!("a11y_cost elements={} nodes={nodes} commit_us={:.0} commit_active_us={:.0} full_us={:.0} incremental_us={:.0} scroll_us={:.1} idle_turn_us={:.2}",
        rt.nodes.len(), median(off), median(on), median(full), median(incremental), median(scroll), median(idle));
}

#[test]
fn help_describes_unless_it_repeats_the_name_and_rows_point_at_their_active_descendant() {
    #[derive(Default)]
    struct H;
    let mut rt = Runtime::new(H, |_: &H, _: &mut Cx<H>| {
        col()
            .child(row().key("save").role("button").label("Save").help("Save the file").on_press(|_: &mut H| {}))
            .child(row().key("trash").role("button").label("Delete").help("Moves it to the Trash").on_press(|_: &mut H| {}))
            .child(row().key("quiet").role("button").label("Quiet").help("Hush").description("").on_press(|_: &mut H| {}))
            .child(col().key("tree").role("tree").focusable(true).active_descendant("b")
                .child(row().key("a").role("treeitem").child(text("Alpha")))
                .child(row().key("b").role("treeitem").focus_selected(true).child(text("Beta"))))
            .child(col().role("textbox").child(text_input(&TextEdit::new(""), crate::tok::type_::BODY).key("field").label("Find")))
            .child(text("Bad").key("bad").invalid(true))
    }, (400.0, 300.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let tree = rt.accessibility_json();
    assert!(find(&tree, "save").unwrap()["description"].is_null(), "help that repeats the name: {tree}");
    assert_eq!(find(&tree, "trash").unwrap()["description"], "Moves it to the Trash");
    assert!(find(&tree, "quiet").unwrap()["description"].is_null());
    assert!(states(find(&tree, "b").unwrap()).contains(&"selected"), "a selected row outside a focus group: {tree}");
    let whole = rt.accessibility_tree();
    let (_, t) = whole.nodes.iter().find(|(_, n)| n.author_id() == Some("tree")).unwrap();
    let (b, _) = whole.nodes.iter().find(|(_, n)| n.author_id() == Some("b")).unwrap();
    assert_eq!(t.active_descendant(), Some(*b));
    // The box around the field joins its parent; the field is the text box.
    let field_parent_is_root = tree["children"].as_array().unwrap().iter().any(|c| c["key"] == "field");
    assert!(field_parent_is_root, "{tree}");
    let (_, bad) = whole.nodes.iter().find(|(_, n)| n.author_id() == Some("bad")).unwrap();
    assert_eq!(bad.invalid(), Some(accesskit::Invalid::True));
}

#[test]
fn updates_report_structure_and_motion_beyond_accesskit_events() {
    let mut rt = Runtime::new(List { rows: 200 }, list, (300.0, 200.0), Env::new(true, 1.0));
    for _ in 0..4 { rt.frame(0.0); }
    rt.a11y_activate("List");
    rt.a11y_update();
    rt.update(|s, _| s.rows = 3);
    for _ in 0..4 { rt.frame(0.0); }
    rt.a11y_update();
    assert!(rt.a11y_changes().structure, "rows went");
    rt.pointer_move(50.0, 50.0);
    rt.update(|s, _| s.rows = 200);
    for _ in 0..4 { rt.frame(0.0); }
    rt.a11y_update();
    rt.wheel_event(crate::WheelEvent::new(0.0, -30.0).precise(true).at(50.0, 50.0));
    rt.frame(1.0 / 60.0);
    assert!(rt.a11y_update().is_some(), "a scroll sends its viewport");
    assert!(rt.a11y_changes().moved, "a scroll moves boxes");
}
