//! Measured read-only Markdown panels above the agent composer.
use crate::{app::App, ui::theme::rgb};
use rune::{El, Tx, WheelEvent, el, text};

pub(super) fn view(app: &App, composer: [f32; 4], key: &str, title: String, footer: &str, content: El) -> El {
    let picker = app.composer.picker.as_ref().expect("read-only panel");
    let kind = picker.kind;
    let p = app.pal;
    let s = app.ty.scale();
    let tab = app.active_tab();
    let [cx, cy, cw, _] = composer;
    let top = app.rects.get(&format!("agent-area:{}", tab.id)).map_or(0.0, |r| r[1]) + 8.0 * s;
    let available = (cy - top - 6.0 * s).max(0.0);
    let header_h = (40.0 * s).min(available);
    let footer_h = if available >= header_h + 60.0 * s { 26.0 * s } else { 0.0 };
    let measure_key = format!("{key}-content:{}", tab.id);
    let content_h = app.rects.get(&measure_key).map_or(100.0 * s, |r| r[3]);
    let body_h = content_h.min((available - header_h - footer_h).max(0.0));
    let height = header_h + body_h + footer_h;
    let max_scroll = (content_h - body_h).max(0.0);
    picker.usage_max_scroll.set(max_scroll);
    let scroll = picker.usage_scroll.clamp(0.0, max_scroll);
    let content = content.on_layout_tx(move |a: &mut App, rect: [f32; 4], tx: &mut Tx| {
        let previous = a.rects.insert(measure_key.clone(), rect);
        if previous.is_some_and(|old| old[2] == rect[2] && old[3] == rect[3]) { tx.live_only(); }
    });
    let mut card = el().key(format!("{key}-picker")).w(cw).h(height).fill(rgb(p.bg))
        .radius(14.0 * s).border(s, rgb(p.border)).clip_rounded(14.0 * s)
        .on_press(|_: &mut App| {})
        .on_wheel(move |a: &mut App, e: WheelEvent, tx: &mut Tx| {
            if let Some(picker) = a.composer.picker.as_mut().filter(|p| p.kind == kind) {
                let delta = if e.precise { -e.dy } else if e.dy < 0.0 { 40.0 * s } else { -40.0 * s };
                picker.usage_scroll = (picker.usage_scroll + delta).clamp(0.0, picker.usage_max_scroll.get());
                tx.animated();
            }
            true
        })
        .child(text(title).font(app.ty.small()).color(rgb(p.fg)).w((cw - 64.0 * s).max(0.0)).truncate().absolute(14.0 * s, 12.0 * s))
        .child(text("esc").font(app.ty.small()).color(rgb(p.muted)).absolute(cw - 38.0 * s, 12.0 * s)
            .on_press(|a: &mut App| a.close_prompt_picker()));
    let mut body = el().w((cw - 2.0 * s).max(0.0)).h(body_h).clip_rounded(0.0).absolute(s, header_h)
        .child(content.absolute(0.0, 0.0).offset(0.0, -scroll).offset_motion(rune::Spring::new(0.188, 0.0)));
    if max_scroll > 0.0 && body_h > 0.0 {
        let thumb_h = (body_h * body_h / content_h).max(12.0 * s).min(body_h);
        body = body.child(el().w(2.0 * s).h(thumb_h).fill(rgb(p.dim))
            .absolute(cw - 6.0 * s, (body_h - thumb_h) * scroll / max_scroll));
    }
    card = card.child(body);
    if footer_h > 0.0 {
        card = card.child(text(footer).font(app.ty.small()).color(rgb(p.muted))
            .w((cw - 28.0 * s).max(0.0)).truncate().absolute(14.0 * s, header_h + body_h + 6.0 * s));
    }
    el().key("prompt-picker-layer").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .child(el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).on_press(|a: &mut App| a.close_prompt_picker()))
        .child(card.absolute(cx, cy - height - 6.0 * s))
}