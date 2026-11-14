//! Headless `El::on_screen_layout`: a window opens on the script's virtual desktop, and moving
//! it with `place()` reports its elements' new global rects without a relayout.
use rune::*;
use std::sync::{Mutex, OnceLock};

static HANDLE: OnceLock<AppHandle<()>> = OnceLock::new();
static RECTS: Mutex<Vec<[f32; 4]>> = Mutex::new(Vec::new());

fn view(_: &(), _: &mut Cx<()>) -> El {
    col().w_full().h_full().padding(10.0).child(
        el().key("card").w(100.0).h(20.0)
            .on_press(|_: &mut ()| {
                HANDLE.get().unwrap().place(DisplayTarget::UnderPointer,
                    OverlayPlacement::Edge { edge: ScreenEdge::Top, margin: 160.0, align: Align::Center, inset: 0.0 });
            })
            .on_screen_layout(|_: &mut (), r| RECTS.lock().unwrap().push(r)),
    )
}

#[test]
fn screen_rects_follow_the_window_on_the_virtual_desktop() {
    let dir = std::env::temp_dir().join(format!("rune-headless-screen-layout-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (script, tree) = (dir.join("desktop.json"), dir.join("tree.json"));
    std::fs::write(&script, r#"{
        "displays": [{"id": 1, "frame": [0, 0, 1512, 982], "scale": 2, "primary": true},
                     {"id": 2, "frame": [1512, 0, 1920, 1080], "scale": 1}],
        "pointer": [2000, 400],
        "events": [{"t": 0.1, "type": "tap", "key": "card"}, {"t": 0.5, "type": "move", "x": 1, "y": 1}]
    }"#).unwrap();
    let app = App::new(AppSpec::new("screen-layout", (600.0, 80.0), || (), view));
    HANDLE.set(app.handle()).ok().unwrap();
    app.args(["--script", script.to_str().unwrap(), "--tree", tree.to_str().unwrap()].map(String::from)).run();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(*RECTS.lock().unwrap(), [
        // Opened centred on the primary display, the card 10 points in.
        [466.0, 461.0, 100.0, 20.0],
        // Placed at the top of the display under the pointer.
        [2182.0, 170.0, 100.0, 20.0],
    ]);
}
