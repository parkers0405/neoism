//! Whole-window native zoom. Rune's scale transforms drawing and input, not layout;
//! lay out the app in an inverse-sized viewport so every surface still fits/reflows.
use super::App;
use rune::{Cx, El, Key, KeyEvent, Tx, el};

#[derive(Default)]
pub(super) struct Zoom {
    // Integer steps avoid drift and make reset / reciprocal shortcuts exact.
    step: i8,
}

impl Zoom {
    fn factor(&self) -> f32 { 1.1_f32.powi(i32::from(self.step)) }

    pub(super) fn key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        // `cmd` is Rune's primary modifier (Ctrl elsewhere, Command on macOS).
        // Also accept explicit Control; never steal AltGr or plain punctuation.
        if !(event.mods.cmd || event.mods.ctrl) || event.mods.alt { return false; }
        let value = match &event.key {
            Key::Text(s) => s.as_str(),
            Key::Command(c) => match c { '+' => "+", '=' => "=", '-' => "-", '0' => "0", _ => return false },
            _ => return false,
        };
        self.step = match value {
            "+" | "=" => (self.step + 1).min(8),
            "-" if !event.mods.shift => (self.step - 1).max(-6),
            "0" if !event.mods.shift => 0,
            _ => return false,
        };
        tx.instant();
        true
    }
}

pub(super) fn wrap(app: &App, cx: &mut Cx<App>, content: El) -> El {
    let [width, height] = cx.layout_size("zoom-viewport").unwrap_or([1252.0, 674.0]);
    let scale = app.zoom.factor();
    el().key("zoom-viewport").w_full().h_full().clip_soft(0.0)
        .child(content.absolute(0.0, 0.0).size(width / scale, height / scale)
            .origin(0.0, 0.0).scale(scale))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Mods, Runtime};

    fn settle(rt: &mut Runtime<App>) { for _ in 0..90 { rt.frame(1.0 / 60.0); } }
    fn primary() -> Mods { Mods { cmd: true, ..Default::default() } }
    fn press(rt: &mut Runtime<App>, key: Key, mods: Mods) {
        rt.key(KeyEvent { key, mods });
        settle(rt);
    }
    fn runtime(size: (f32, f32), dpi: f32) -> Runtime<App> {
        let mut app = App::new();
        crate::fixture::load(&mut app, true);
        let mut rt = Runtime::new(app, crate::view::root, size, Env::new(true, dpi));
        rt.on_key = Some(|a, e, tx| a.zoom.key(e, tx));
        settle(&mut rt);
        rt
    }
    fn element(rt: &Runtime<App>, key: &str) -> rune::ElementInfo {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap()
    }
    fn close(a: f32, b: f32) { assert!((a-b).abs() < 1.0, "{a} != {b}"); }

    #[test]
    fn zoom_aliases_limits_and_unrelated_keys() {
        let mut zoom = Zoom::default();
        let mut tx = Tx::default();
        for key in [Key::Text("=".into()), Key::Text("+".into()), Key::Command('+')] {
            assert!(zoom.key(&KeyEvent { key, mods: Mods { shift: true, ..primary() } }, &mut tx));
        }
        assert_eq!(zoom.step, 3);
        for s in ["=", "+", "-", "0", ",", "a"] {
            assert!(!zoom.key(&KeyEvent { key: Key::Text(s.into()), mods: Mods::default() }, &mut tx));
        }
        assert!(!zoom.key(&KeyEvent { key: Key::Command('='), mods: Mods { alt: true, ..primary() } }, &mut tx));
        for _ in 0..100 { zoom.key(&KeyEvent { key: Key::Command('='), mods: primary() }, &mut tx); }
        assert_eq!(zoom.step, 8);
        for _ in 0..100 { zoom.key(&KeyEvent { key: Key::Command('-'), mods: Mods { ctrl: true, ..Default::default() } }, &mut tx); }
        assert_eq!(zoom.step, -6);
        assert!(zoom.key(&KeyEvent { key: Key::Command('0'), mods: primary() }, &mut tx));
        assert_eq!(zoom.factor(), 1.0);
    }

    #[test]
    fn zoom_scales_native_chrome_and_reflows_to_window_at_each_dpi() {
        for dpi in [1.0, 2.0] {
            let mut rt = runtime((1252.0, 674.0), dpi);
            let before = element(&rt, "sidebar").rect;
            press(&mut rt, Key::Command('='), primary());
            let after = element(&rt, "sidebar");
            close(after.rect[2], before[2] * 1.1);
            close(after.rect[3], 674.0);
            close(element(&rt, "header").rect[3], 45.0 * 1.1);
            let root = element(&rt, "neochat").rect;
            close(root[0], 0.0); close(root[1], 0.0);
            close(root[2], 1252.0); close(root[3], 674.0);
            close(rt.state.width, 1252.0 / 1.1);
            press(&mut rt, Key::Command('0'), primary());
            close(element(&rt, "sidebar").rect[2], before[2]);
        }
    }

    #[test]
    fn zoomed_hit_testing_and_thread_breakpoint_use_scaled_layout() {
        let mut rt = runtime((1000.0, 674.0), 1.0);
        rt.update(|a, _| a.open_thread(1)); settle(&mut rt);
        assert!(rt.find("sidebar").is_some());
        press(&mut rt, Key::Command('='), primary());
        press(&mut rt, Key::Command('='), primary());
        assert!(rt.state.width < 900.0);
        assert!(rt.find("sidebar").is_none());
        let input = element(&rt, "reply-input");
        let [x,y,w,h] = input.visible;
        assert!(w > 0.0 && h > 0.0);
        rt.pointer_down(x+w/2.0, y+h/2.0);
        rt.pointer_up(x+w/2.0, y+h/2.0);
        settle(&mut rt);
        press(&mut rt, Key::Text("zoomed reply".into()), Mods::default());
        assert_eq!(rt.state.reply.text, "zoomed reply");
        assert!(rt.state.draft.text.is_empty());
        press(&mut rt, Key::Command('0'), primary());
        assert!(rt.find("sidebar").is_some());
        assert_eq!(rt.state.reply.text, "zoomed reply");
    }

    #[test]
    fn focused_composer_zoom_keeps_text_selection_undo_and_plain_punctuation() {
        let mut rt = runtime((700.0, 600.0), 1.0);
        rt.update(|_, tx| tx.focus("post-input", false)); settle(&mut rt);
        press(&mut rt, Key::Text("draft".into()), Mods::default());
        press(&mut rt, Key::Text("+".into()), Mods { shift: true, ..primary() });
        assert_eq!(rt.state.draft.text, "draft");
        assert_eq!(rt.state.zoom.step, 1);
        assert!(element(&rt, "post-input").focused);
        press(&mut rt, Key::Command('a'), primary());
        press(&mut rt, Key::Text("replacement".into()), Mods::default());
        assert_eq!(rt.state.draft.text, "replacement");
        press(&mut rt, Key::Command('z'), primary());
        assert_eq!(rt.state.draft.text, "draft");
        press(&mut rt, Key::Command('0'), primary());
        press(&mut rt, Key::Command('a'), primary());
        press(&mut rt, Key::Text("=+-0,".into()), Mods::default());
        assert_eq!(rt.state.draft.text, "=+-0,");
        assert_eq!(rt.state.zoom.step, 0);
        close(element(&rt, "neochat").rect[2], 700.0);
    }
}