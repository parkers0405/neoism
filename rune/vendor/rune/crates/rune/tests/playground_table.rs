//! A story table made in a loop: each story's init, view, triggers and variants are closures
//! that capture their row, and a story opens by its slug.
use rune::playground::slug;
use rune::{App, AppSpec, Cx, El, Env, Playground, Runtime, Story, Tx, text};

#[derive(Default)]
struct Real;

fn real(_: &Real, _: &mut Cx<Real>) -> El {
    text("the app")
}

struct Counter {
    n: u32,
}

/// A row of the app's story table.
struct Row {
    path: &'static str,
    start: u32,
    step: u32,
    label: String,
}

fn table() -> Vec<Row> {
    (0..50u32)
        .map(|i| Row {
            path: Box::leak(format!("Group {}/Story {i}", i / 10).into_boxed_str()),
            start: i * 10,
            step: i + 1,
            label: format!("row {i}"),
        })
        .collect()
}

fn runtime() -> Runtime<Real> {
    let mut playground = Playground::<Real>::new();
    for Row { path, start, step, label } in table() {
        let story = Story::new(path, move || Counter { n: start }, move |c, _| text(format!("{label} {}", c.n)).key("counter"))
            .trigger('a', "Add", move |c, _| c.n += step)
            .trigger('l', "Add later", move |_, play| play.after(0.5, move |c: &mut Counter, _| c.n += step))
            .trigger_tx('z', "Zero", |c: &mut Counter, tx: &mut Tx| {
                c.n = 0;
                tx.instant();
            })
            .variant("As listed", |_| {})
            .variant("Doubled", move |c| c.n = start * 2);
        playground = playground.story(story);
    }
    App::new(AppSpec::new("table", (800.0, 500.0), Real::default, real)).playground(playground).headless_runtime((800.0, 500.0), Env::new(false, 1.0))
}

fn shows(rt: &Runtime<Real>) -> String {
    rt.playground_element("counter").and_then(|e| e.text).unwrap_or_default()
}

#[test]
fn fifty_stories_from_a_table_each_with_its_own_row() {
    let mut rt = runtime();
    assert_eq!(slug("Group 3/Story 37"), "group-3-story-37");
    rt.open_playground("group-3-story-37", None);
    assert_eq!(shows(&rt), "row 37 370", "its init and view captured its row");
    rt.playground_trigger('a');
    assert_eq!(rt.playground_state::<Counter>().map(|c| c.n), Some(408), "and its trigger, the row's step");
    rt.playground_trigger('l');
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(rt.playground_state::<Counter>().map(|c| c.n), Some(446), "a step it queued captured the row too");
    rt.playground_trigger('z');
    assert_eq!(rt.playground_state::<Counter>().map(|c| c.n), Some(0), "a trigger that takes just the Tx");
    rt.open_playground("group-0-story-4", Some("Doubled"));
    assert_eq!(shows(&rt), "row 4 80", "a variant that captured its row");
}
