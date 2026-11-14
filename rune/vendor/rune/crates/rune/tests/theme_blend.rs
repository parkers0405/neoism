//! A theme's translucent tokens are authored for a browser's sRGB blending. Composited over
//! their backdrop they give the browser's pixel in a window of either blend mode, and follow a
//! live switch, memoised rows included.
use rune::color::{over, srgb_encode};
use rune::text::TextSystem;
use rune::{Cx, El, Env, Runtime, Theme, el, memo};

const THEME: &str = r##"{ "name": "Test", "blend": { "mode": "srgb", "over": "bg" },
  "light": { "bg": "#f8f8f8", "border": "#0000001f" },
  "dark":  { "bg": "#161616", "border": "#ffffff1a" } }"##;

fn view(t: &Theme, _: &mut Cx<Theme>) -> El {
    el().size(40.0, 40.0).fill(t.color("bg"))
        .child(el().size(20.0, 20.0).fill(t.color("border")))
        .child(memo("row", (), || el().size(10.0, 10.0).fill(t.color("border"))))
}

fn runtime(dark: bool, srgb: bool) -> Runtime<Theme> {
    let theme = Theme::from_json(THEME, dark).unwrap();
    let text = TextSystem::new(&[], ("Inter", "Inter"));
    let mut rt = Runtime::with_text(theme, view, (40.0, 40.0), Env::new(dark, 1.0), text);
    rt.set_srgb_blend(srgb);
    rt.frame(0.0);
    rt
}

/// The token in the `side`-point box composited over the backdrop, as sRGB bytes.
fn composited(rt: &Runtime<Theme>, side: f32) -> [f32; 3] {
    let fill = |w: f32| rt.instances.iter().find(|i| i.rect[2] == w && i.fill[3] > 0.0).unwrap_or_else(|| panic!("a {w}-point box")).fill;
    let px = over(fill(side), fill(40.0));
    let encode = |v: f32| if rt.srgb_blend() { v } else { srgb_encode(v) };
    [0, 1, 2].map(|i| encode(px[i]) * 255.0)
}

/// CSS source-over in sRGB bytes: `a·fg + (1 − a)·bg`.
fn browser(fg: u8, bg: u8, a: u8) -> f32 {
    let a = a as f32 / 255.0;
    a * fg as f32 + (1.0 - a) * bg as f32
}

fn assert_browser(rt: &Runtime<Theme>, want: f32, what: &str) {
    for side in [20.0, 10.0] {
        let got = composited(rt, side);
        assert!(got.iter().all(|v| (v - want).abs() < 0.5), "{what}, {side}-point box: {got:?} vs {want}");
    }
}

#[test]
fn a_translucent_token_composites_to_the_browser_pixel_in_both_blend_modes() {
    for (dark, want) in [(true, browser(0xff, 0x16, 0x1a)), (false, browser(0x00, 0xf8, 0x1f))] {
        for srgb in [true, false] {
            assert_browser(&runtime(dark, srgb), want, &format!("dark {dark}, sRGB blending {srgb}"));
        }
    }
}

/// The view runs again with the new mode, memoised rows included; the fills then ease to their
/// new values as any colour change does.
#[test]
fn a_live_switch_resolves_tokens_again() {
    let want = browser(0xff, 0x16, 0x1a);
    let mut rt = runtime(true, false);
    for srgb in [true, false] {
        rt.update(|_, tx| tx.set_srgb_blend(srgb));
        rt.frame(1.0 / 60.0);
        rt.frame(1.0);
        assert_browser(&rt, want, &format!("switched to sRGB blending {srgb}"));
    }
}

#[test]
fn outside_a_runtime_tokens_resolve_as_authored() {
    let t = Theme::from_json(THEME, true).unwrap();
    assert!(rune::theme::blends_srgb());
    assert_eq!(t.color("border").a, 0x1a as f32 / 255.0);
    let linear = rune::theme::with_blend(false, || t.color("border"));
    assert!(linear.a < t.color("border").a, "linear light needs less alpha over a dark backdrop: {}", linear.a);
    assert!(rune::theme::blends_srgb(), "with_blend restores the mode");
}
