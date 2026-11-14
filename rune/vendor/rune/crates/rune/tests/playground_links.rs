//! Opening a playground story from the app: `Tx::open_playground_at` by path, name or slug,
//! as a link through `App::on_open_url` does at launch.
use std::cell::Cell;

use rune::playground::slug;
use rune::{App, AppSpec, Cx, El, Env, Playground, Runtime, Story, text};

thread_local! {
    /// How many times the app's own view ran, on this test's thread.
    static APP_VIEWS: Cell<usize> = const { Cell::new(0) };
}

#[derive(Default)]
struct Real;

fn app_view(_: &Real, _: &mut Cx<Real>) -> El {
    APP_VIEWS.with(|n| n.set(n.get() + 1));
    text("the app").key("app")
}

#[derive(Default)]
struct Upload {
    bytes: u32,
}

fn upload(u: &Upload, _: &mut Cx<Upload>) -> El {
    text(format!("{} bytes", u.bytes)).key("upload")
}

fn runtime() -> Runtime<Real> {
    let playground = Playground::<Real>::new()
        .story(Story::new("Timeline/Send", Upload::default, upload).variant("Small", |u| u.bytes = 10).variant("Large", |u| u.bytes = 1_000_000))
        .story(Story::new("Lists/Many devices", Upload::default, upload));
    App::new(AppSpec::new("links", (800.0, 500.0), Real::default, app_view)).playground(playground).headless_runtime((800.0, 500.0), Env::new(false, 1.0))
}

fn bytes(rt: &Runtime<Real>) -> Option<u32> {
    rt.playground_state::<Upload>().map(|u| u.bytes)
}

#[test]
fn a_slug_is_the_path_lowercased_with_dashes() {
    assert_eq!(slug("Timeline/Send"), "timeline-send");
    assert_eq!(slug("Lists/Many devices"), "lists-many-devices");
    assert_eq!(slug("  Toasts / Sync -- toast!  "), "toasts-sync-toast");
    assert_eq!(slug("Café/Crème brûlée"), "café-crème-brûlée");
    assert_eq!(slug("Motion/Fade 2"), "motion-fade-2");
}

#[test]
fn every_lookup_takes_a_slug() {
    let mut rt = runtime();
    rt.open_playground("lists-many-devices", None);
    assert_eq!(bytes(&rt), Some(0));
    rt.open_playground("timeline-send", Some("Large"));
    assert_eq!(bytes(&rt), Some(1_000_000));
}

#[test]
fn a_link_before_the_first_frame_opens_straight_onto_the_story() {
    let mut rt = runtime();
    // As App::on_open_url delivers the link that launched the app: an update before frame 1.
    rt.update(|_, tx| tx.open_playground_at("timeline-send", Some("Large")));
    rt.frame(0.0);
    assert_eq!(rt.playground_element("upload").and_then(|e| e.text).as_deref(), Some("1000000 bytes"), "the first frame shows the story");
    assert_eq!(APP_VIEWS.with(Cell::get), 0, "and the app's view never ran");
}

#[test]
fn from_a_handler_it_moves_an_open_playground_to_the_story() {
    let mut rt = runtime();
    rt.open_playground("Lists/Many devices", None);
    rt.update(|_, tx| tx.open_playground_at("Send", Some("2")));
    rt.frame(0.0);
    assert_eq!(bytes(&rt), Some(1_000_000), "by name, on the variant by number");
}

#[test]
fn an_unknown_story_opens_the_list_instead_of_stopping_the_app() {
    let mut rt = runtime();
    rt.update(|_, tx| tx.open_playground_at("timeline-nope", None));
    rt.frame(0.0);
    assert!(rt.find("playground-list").is_some(), "the playground is open, on its list");
    assert_eq!(bytes(&rt), Some(10), "on the first story, as it would open without a name");
}

#[test]
fn an_unknown_variant_opens_the_story_as_it_would_without_one() {
    let mut rt = runtime();
    rt.update(|_, tx| tx.open_playground_at("Timeline/Send", Some("Huge")));
    rt.frame(0.0);
    assert_eq!(bytes(&rt), Some(10), "its first variant");
}
