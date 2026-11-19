//! Shared, captured edge dragging for chrome and modal sidebars.

use crate::app::App;
use super::theme::rgb;
use rune::{Cursor, Drag, El, Phase, Tx, col, el};

pub fn chrome_max(app: &App) -> f32 {
    let available = (app.window[0] - super::chrome::RAIL_W - 240.0).max(0.0);
    let count = if app.sidebar.is_some() && app.git_panel.open { 2.0 } else { 1.0 };
    (available / count).min(600.0)
}

pub fn view(app: &App, key: &'static str, content: El, default: f32, min: f32, max: f32, right: bool) -> El {
    let max = max.max(0.0);
    let min = min.min(max);
    let width = app.sidebar_widths.get(key).copied().unwrap_or(default).clamp(min, max);
    let handle = el().key(format!("sidebar-resize:{key}")).w(6.0).h_full()
        .pin(if right { Some(0.0) } else { None }, Some(0.0), if right { None } else { Some(0.0) }, Some(0.0))
        .cursor(Cursor::ResizeColumn).hover(rgb(app.pal.border))
        .on_drag(move |a: &mut App, d: &mut Drag, tx: &mut Tx| {
            if d.phase == Phase::Down { d.data = width; }
            let travel = if right { -d.dx } else { d.dx };
            a.sidebar_widths.insert(key, (d.data + travel).clamp(min, max));
            tx.instant();
        });
    col().key(format!("sidebar-frame:{key}")).w(width).h_full()
        .child(content.w_full().grow(1.0).shrink())
        .child(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Cx, Env, Runtime, row};

    #[test]
    fn sidebar_edges_capture_drag_clamp_and_remember_independent_widths() {
        fn scene(a: &App, _: &mut Cx<App>) -> El {
            let key = a.workspace_name.as_str();
            let key = match key {
                "files" => "files", "notes" => "notes", "git" => "git",
                "settings" => "settings", "browser" => "browser", _ => "conversations",
            };
            row().w_full().h_full().child(view(a, key, el(), 300.0, 180.0, 500.0, key == "git"))
        }
        fn rect(rt: &Runtime<App>, key: &str) -> [f32; 4] {
            rt.elements().iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting).unwrap().rect
        }
        fn drag(rt: &mut Runtime<App>, key: &str, dx: f32) {
            let [x, y, w, h] = rect(rt, &format!("sidebar-resize:{key}"));
            let (x, y) = (x + w / 2.0, y + h / 2.0);
            rt.pointer_move(x, y);
            rt.pointer_down(x, y);
            rt.frame(0.016);
            rt.pointer_move(x + dx, y);
            rt.frame(0.016);
            rt.pointer_up(x + dx, y);
            rt.frame(0.016);
        }
        let mut rt = Runtime::new(App::new(), scene, (1000.0, 600.0), Env::new(true, 1.0));
        rt.frame(0.016);
        for key in ["files", "notes", "conversations", "git", "settings", "browser"] {
            rt.update(|a, tx| { a.workspace_name = key.into(); tx.instant(); });
            rt.frame(0.016);
            let direction = if key == "git" { -1.0 } else { 1.0 };
            drag(&mut rt, key, direction * 80.0);
            assert_eq!(rect(&rt, &format!("sidebar-frame:{key}"))[2], 380.0);
            drag(&mut rt, key, direction * 1000.0);
            assert_eq!(rect(&rt, &format!("sidebar-frame:{key}"))[2], 500.0);
            drag(&mut rt, key, -direction * 1000.0);
            assert_eq!(rect(&rt, &format!("sidebar-frame:{key}"))[2], 180.0);
        }
        assert_eq!(rt.state.sidebar_widths.len(), 6);
        rt.update(|a, tx| { a.workspace_name = "files".into(); tx.instant(); });
        rt.frame(0.016);
        assert_eq!(rect(&rt, "sidebar-frame:files")[2], 180.0);
    }
}