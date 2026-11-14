use super::*;
use crate::el::ScrollSpec;

const PROSE: &str = "Amber lantern orchard quiet harbor, copper meadow signal drift. Pebble willow \
    ember tide under a juniper kestrel; nimbus opal prism and a quiet harbor beneath the lantern. ";

fn prose(_: &(), _: &mut Cx<()>) -> El {
    col().key("scroll").size(320.0, 240.0).scroll(ScrollSpec { stick_bottom: false }).children((0..40).map(|i| {
        crate::text(format!("{i}. {}", PROSE.repeat(1 + i % 3))).key(format!("p{i}"))
            .font(crate::TextStyle::new(13.0, 400.0)).color(crate::Color::WHITE).w_full()
    }))
}

fn instance_bytes(rt: &Runtime<()>) -> Vec<u8> {
    bytemuck::cast_slice(&rt.instances).to_vec()
}

/// Scrolling replays each paragraph's glyphs: a frame walks Parley only for lines it hasn't
/// drawn before, and draws exactly what a fresh window scrolled to the same place draws.
#[test]
fn scrolling_replays_glyphs_it_has_drawn() {
    let mut rt = Runtime::new((), prose, (320.0, 240.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..12 {
        rt.wheel(160.0, 120.0, -7.0);
        rt.frame(1.0 / 120.0);
    }
    let flattened = rt.text.glyph_cache.flattened;
    for _ in 0..12 {
        rt.wheel(160.0, 120.0, 7.0);
        rt.frame(1.0 / 120.0);
    }
    assert_eq!(rt.text.glyph_cache.flattened, flattened, "scrolling back walks no Parley lines");
    for _ in 0..12 {
        rt.wheel(160.0, 120.0, -7.0);
        rt.frame(1.0 / 120.0);
    }
    assert_eq!(rt.text.glyph_cache.flattened, flattened);
    for _ in 0..30 { rt.frame(1.0 / 120.0); }

    let mut fresh = Runtime::new((), prose, (320.0, 240.0), Env::new(true, 2.0));
    fresh.frame(0.0);
    fresh.wheel(160.0, 120.0, -84.0);
    for _ in 0..30 { fresh.frame(1.0 / 120.0); }
    assert_eq!(fresh.scrolls.values().next().map(|s| s.offset), rt.scrolls.values().next().map(|s| s.offset));
    assert!(rt.instances.len() > 200);
    assert!(instance_bytes(&rt) == instance_bytes(&fresh), "replayed glyphs draw as freshly walked ones");
}

/// A secret's glyphs never stay in the replay cache.
#[test]
fn secure_fields_keep_no_glyphs() {
    fn view(s: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
        crate::text_input(s, crate::TextStyle::new(16.0, 400.0)).single_line().secure()
            .key("secret").size(200.0, 40.0).on_edit(|s: &mut crate::TextEdit| s)
    }
    let mut rt = Runtime::new(crate::TextEdit::new("hunter2"), view, (200.0, 40.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.instances.iter().any(|q| q.params[1] == KIND_GLYPH));
    assert_eq!(rt.text.glyph_cache.len(), 0);
}
