//! One horizontal position per table, shared by its virtualized row fragments.
use super::{edit, parse};
use crate::{app::App, editor::Buffer, ui::theme::rgb};
use rune::{El, Live, Phase, Tx, WheelEvent, el};
use std::{cell::Cell, ops::Range, rc::Rc};

const BAR: f32 = 16.0;
const END: f32 = 14.0;

pub(super) struct Scroll {
    key: String,
    source: Cell<(f32, f32)>,
    track: Cell<(f32, f32)>,
    pub content: Live<[f32; 2]>,
    thumb: Live<[f32; 2]>,
    bar: Live<[f32; 2]>,
    visible: Live<f32>,
}

impl Scroll {
    fn new(key: String) -> Self {
        Self { key, source: Cell::new((0.0, 0.0)), track: Cell::new((0.0, 0.0)), content: Live::new([0.0, 0.0]),
            thumb: Live::new([0.0, 0.0]), bar: Live::new([0.0, 0.0]), visible: Live::new(0.0) }
    }

    fn place(&self, to: f32) {
        let max = self.source.get().1;
        let to = to.clamp(0.0, max);
        self.source.set((to, max));
        self.content.set([-to, 0.0]);
        self.thumb.set([self.track.get().1 * to / max.max(1.0), 0.0]);
    }
}

#[derive(Clone)]
pub(super) struct Table {
    pub range: Range<usize>,
    pub widths: Vec<f32>,
    pub width: f32,
    pub top: f32,
    pub bottom: f32,
    pub scroll: Rc<Scroll>,
}

impl Table {
    pub fn new(app: &App, buffer: &Buffer, range: Range<usize>, viewport: f32) -> Self {
        let rows: Vec<Vec<String>> = range.clone().filter(|&i| !edit::separator(&buffer.doc.lines[i])).map(|i| {
            edit::cells(&buffer.doc.lines[i]).into_iter().map(|r| parse::inline_line(&buffer.doc.lines[i][r]).text).collect()
        }).collect();
        let widths = crate::agent::table::column_widths(&rows, viewport, app.ty.scale(), app.ty.size * 0.6);
        let width = widths.iter().sum::<f32>();
        let scroll = buffer.markdown_cache.tables.borrow_mut().entry(range.start).or_insert_with(|| {
            Rc::new(Scroll::new(format!("md-table:{}:{}:{}", app.workspace_id, buffer.path, range.start)))
        }).clone();
        let max = if width > viewport + 1.0 { width + END * app.ty.scale() - viewport } else { 0.0 };
        let (thumb, travel) = crate::agent::table::thumb(viewport, viewport, width + END * app.ty.scale(), app.ty.scale());
        scroll.source.set((scroll.source.get().0, max));
        scroll.track.set((thumb, travel));
        scroll.place(scroll.source.get().0);
        Self { range, widths, width, top: 0.0, bottom: 0.0, scroll }
    }

    pub fn offset(&self) -> f32 { self.scroll.source.get().0 }

    pub fn gutter(&self, scale: f32) -> f32 {
        if self.scroll.source.get().1 > 0.0 { (BAR + 4.0) * scale } else { 0.0 }
    }

    pub fn reveal(&self, x: f32, caret_w: f32, viewport: f32, scale: f32) {
        let offset = self.offset();
        if x < offset { self.scroll.place(x - 14.0 * scale); }
        else if x + caret_w > offset + viewport { self.scroll.place(x + caret_w + 14.0 * scale - viewport); }
    }

    pub fn vertical(&self, offset: f32, viewport: f32, scale: f32) {
        // Follow the viewport while the table continues below it, then settle
        // into the reserved footer without covering the closing border.
        let top = (self.bottom - offset).min(viewport - BAR * scale).max(self.top - offset);
        let visible = self.scroll.source.get().1 > 0.0 && top < viewport
            && top + BAR * scale > 0.0 && self.bottom + BAR * scale > offset;
        self.scroll.bar.set([0.0, top]);
        self.scroll.visible.set(if visible { 1.0 } else { 0.0 });
    }
}

pub(super) fn row(buffer: &Buffer, table: &Table, grid: El, viewport: f32, height: f32) -> El {
    let path = buffer.path.clone();
    let start = table.range.start;
    let group = format!("{}:hover", table.scroll.key);
    el().w(viewport).h(height).clip_rounded(0.0).hover_group_shared(group)
        .on_wheel(move |a: &mut App, event: WheelEvent, tx: &mut Tx| wheel(a, &path, start, event, tx))
        .child(grid.live_offset(&table.scroll.content))
}

pub(super) fn bar(app: &App, table: &Table, viewport: f32, left: f32) -> El {
    let s = app.ty.scale();
    let scroll = table.scroll.clone();
    let thumb = scroll.track.get().0;
    let group = format!("{}:hover", scroll.key);
    let wheeled = app.table_active.as_ref().is_some_and(|(key, _)| *key == scroll.key);
    let pill = |w: f32, color| el().w(w).h(5.0 * s).radius(2.5 * s).fill(color);
    let key = scroll.key.clone();
    el().key(format!("{key}:bar")).w(viewport).h(BAR * s).absolute(left, 0.0).instant_changes()
        .live_offset(&scroll.bar).live_opacity(&scroll.visible).hover_group_shared(group)
        .child(el().w(viewport).h(12.0 * s).absolute(0.0, 2.0 * s).radius(4.0 * s)
            .fill(rgb(app.pal.surface).alpha(0.94)).group_opacity(if wheeled { 1.0 } else { 0.0 }, 1.0))
        .child(pill(viewport, rgb(app.pal.border).alpha(0.72)).absolute(0.0, 5.5 * s).group_opacity(if wheeled { 1.0 } else { 0.0 }, 1.0))
        .child(pill(thumb, rgb(app.pal.fg).alpha(0.78)).absolute(0.0, 5.5 * s).live_offset(&scroll.thumb)
            .group_opacity(if wheeled { 1.0 } else { 0.0 }, 1.0))
        .on_drag(move |a: &mut App, drag: &mut rune::Drag, tx: &mut Tx| {
            a.scrollers.stop(&key);
            tx.live_only();
            if drag.phase == Phase::Down {
                let at = scroll.thumb.get()[0];
                drag.data = if drag.x >= at && drag.x <= at + thumb { drag.x - at } else { thumb / 2.0 };
            }
            let travel = scroll.track.get().1;
            if travel > 0.0 { scroll.place(((drag.x - drag.data) / travel).clamp(0.0, 1.0) * scroll.source.get().1); }
        })
}

fn wheel(app: &mut App, path: &str, start: usize, event: WheelEvent, tx: &mut Tx) -> bool {
    let Some(scroll) = app.buffers.get(path).and_then(|b| b.markdown_cache.tables.borrow().get(&start).cloned()) else { return false };
    if scroll.source.get().1 <= 0.0 { return app.editor_wheel(path, event, tx); }
    app.table_active = Some((scroll.key.clone(), std::time::Instant::now()));
    let Some((horizontal, event)) = app.table_gesture.wheel(&scroll.key, event, std::time::Instant::now()) else { tx.live_only(); return true };
    if !horizontal { return app.editor_wheel(path, event, tx); }
    app.scrollers.stop(&super::super::view::scroll_key(&app.workspace_id, path));
    let delta = if event.dx.abs() > event.dy.abs() { event.dx } else { event.dy };
    let delta = if event.precise && !cfg!(target_os = "macos") { delta * 2.0 } else { delta };
    let source = scroll.clone();
    app.scrollers.own(&scroll.key);
    let line_h = super::super::view::line_height(app);
    app.scrollers.sideways(&scroll.key, &event, delta, 3.0 * line_h, Rc::new(move || source.source.get()), tx);
    tx.live_only();
    true
}

pub(crate) fn table_wheel(app: &mut App, start: usize, event: WheelEvent, tx: &mut Tx) -> bool {
    let Some(buffer) = app.active_buffer() else { return false };
    if !super::enabled(buffer) { return false; }
    let path = buffer.path.clone();
    super::layout(app, buffer);
    wheel(app, &path, start, event, tx)
}

pub(crate) fn place(app: &App, step: &crate::ui::scroll::Move) -> bool {
    if !step.key.starts_with("md-table:") { return false; }
    for buffer in app.buffers.values() {
        for scroll in buffer.markdown_cache.tables.borrow().values() {
            if scroll.key == step.key { scroll.place(step.to); return true; }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> rune::Runtime<App> {
        let mut app = App::new();
        app.ty.size = 14.0;
        let mut source = String::from("| Left | Middle | Right |\n| --- | --- | --- |\n");
        for i in 0..120 { source.push_str(&format!("| row-{i} | middle-{i} | right-{i} |\n")); }
        app.open_file("/notes/tables.md", None);
        app.apply_opened_file("/notes/tables.md", source.as_bytes());
        let mut rt = rune::Runtime::new(app, |a: &App, _: &mut rune::Cx<App>| {
            rune::col().w_full().h_full().child(super::super::view(a, a.active_buffer().unwrap()))
        }, (620.0, 380.0), rune::Env::new(true, 1.0));
        for _ in 0..3 { rt.frame(1.0 / 120.0); }
        rt
    }

    fn step(rt: &mut rune::Runtime<App>, from: usize, frames: usize) {
        for frame in from..from + frames {
            rt.update(|app, tx| { app.scrollers.step(frame as f64 / 120.0, tx); app.place_conversations(tx); });
            rt.frame(1.0 / 120.0);
        }
    }

    fn offset(rt: &rune::Runtime<App>) -> f32 {
        rt.state.active_buffer().unwrap().markdown_cache.tables.borrow()[&0].source.get().0
    }

    #[test]
    fn overflow_keeps_columns_and_scrolls_all_virtual_rows_with_correct_hits() {
        let mut rt = runtime();
        let b = rt.state.active_buffer().unwrap();
        let laid = super::super::layout(&rt.state, b);
        assert!(laid.tables[0].width > laid.width);
        let caret = b.doc.cursor();
        let header = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("Left")).unwrap().rect;
        let mut event = WheelEvent::new(0.0, -40.0).at(header[0] + 8.0, header[1] + 8.0);
        event.mods.shift = true;
        rt.wheel_event(event);
        step(&mut rt, 0, 180);
        let across = offset(&rt);
        assert!(across > 0.0);
        let moved = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("Left")).unwrap().rect;
        assert!((header[0] - moved[0] - across).abs() < 0.2);
        let b = rt.state.active_buffer().unwrap();
        assert_eq!(b.doc.cursor(), caret);
        assert_eq!(b.scroll.get().0, 0.0, "Shift-wheel must not move the document vertically");
        let laid = super::super::layout(&rt.state, b);
        let entry = &laid.rows[0];
        let middle = &entry.row.pieces[1];
        let x = laid.left + middle.x - across + 0.1;
        assert_eq!(super::super::hit(&rt.state, b, x, entry.y), Some((0, middle.source[0])));
        let mounted = rt.elements().iter().filter_map(|e| e.text.as_deref().filter(|text| text.starts_with("row-")).map(str::to_owned))
            .collect::<std::collections::BTreeSet<_>>().len();
        let budget = ((b.scroll.get().2 * 3.0 + 2.0 * super::super::tile_height(&rt.state))
            / super::super::font(&rt.state, &parse::Kind::Table).line_px()).ceil() as usize + 4;
        assert!(mounted <= budget && mounted < 120,
            "table rows must stay bounded by the viewport and overscan: mounted={mounted}, budget={budget}");
    }

    #[test]
    fn trackpad_axis_lock_and_scrollbar_drag_do_not_move_editor_caret() {
        let mut rt = runtime();
        let caret = rt.state.active_buffer().unwrap().doc.cursor();
        let header = rt.elements().into_iter().find(|e| e.text.as_deref() == Some("Left")).unwrap().rect;
        rt.wheel_event(WheelEvent::new(-20.0, -1.0).precise(true).phase(rune::GesturePhase::Began)
            .at(header[0] + 8.0, header[1] + 8.0));
        step(&mut rt, 0, 2);
        assert!(offset(&rt) > 0.0);
        let mut coast = WheelEvent::new(-3.0, -10.0).precise(true).at(header[0] + 8.0, header[1] + 8.0);
        coast.momentum = true;
        rt.wheel_event(coast);
        step(&mut rt, 2, 2);
        assert_eq!(rt.state.active_buffer().unwrap().scroll.get().0, 0.0, "coast noise must stay on the selected axis");
        assert_eq!(rt.state.active_buffer().unwrap().markdown_cache.tables.borrow()[&0].visible.get(), 1.0,
            "a long table keeps its rail reachable while its bottom is offscreen");
        let pinned = rt.elements().into_iter().find(|e| e.key.as_deref().is_some_and(|k| k.starts_with("md-table:") && k.ends_with(":bar"))).unwrap().rect;
        assert!((pinned[1] + pinned[3] - 380.0).abs() < 0.2);
        rt.update(|app, tx| {
            let b = app.active_buffer().unwrap();
            tx.scroll_to(crate::editor::view::scroll_key(&app.workspace_id, &b.path), 0.0, 500.0);
        });
        for _ in 0..120 { rt.frame(1.0 / 120.0); }
        let scrolled = rt.elements().into_iter().find(|e| e.key.as_deref().is_some_and(|k| k.starts_with("md-table:") && k.ends_with(":bar"))).unwrap().rect;
        assert!((scrolled[1] - pinned[1]).abs() < 0.2, "rail must follow the viewport rather than scroll away with rows");
        rt.update(|app, tx| {
            let b = app.active_buffer().unwrap();
            let laid = super::super::layout(app, b);
            let table = &laid.tables[0];
            let y = table.bottom + table.gutter(app.ty.scale()) - b.scroll.get().2;
            tx.scroll_to(crate::editor::view::scroll_key(&app.workspace_id, &b.path), 0.0, y.max(0.0));
        });
        for _ in 0..120 { rt.frame(1.0 / 120.0); }
        let bar = rt.elements().into_iter().find(|e| e.key.as_deref().is_some_and(|k| k.starts_with("md-table:") && k.ends_with(":bar"))).unwrap().rect;
        let b = rt.state.active_buffer().unwrap();
        let laid = super::super::layout(&rt.state, b);
        let border_y = b.view_rect.unwrap()[1] + laid.tables[0].bottom - b.scroll.get().0;
        assert!((bar[1] - border_y).abs() < 0.2, "the scrollbar needs its own gutter below the closing border");
        assert!(bar[1] + bar[3] <= 380.1, "bar={bar:?} viewport={:?} scroll={:?} bottom={} gutter={}",
            b.view_rect, b.scroll.get(), laid.tables[0].bottom, laid.tables[0].gutter(rt.state.ty.scale()));
        rt.pointer_move(bar[0] + bar[2] * 0.8, bar[1] + bar[3] * 0.5);
        rt.frame(1.0 / 120.0);
        rt.pointer_down(bar[0] + bar[2] * 0.8, bar[1] + bar[3] * 0.5);
        rt.frame(1.0 / 120.0);
        rt.pointer_move(bar[0] + bar[2] * 0.95, bar[1] + bar[3] * 0.5);
        rt.frame(1.0 / 120.0);
        rt.pointer_up(bar[0] + bar[2] * 0.95, bar[1] + bar[3] * 0.5);
        rt.frame(1.0 / 120.0);
        assert!(offset(&rt) > 250.0, "the visible rail must be draggable: offset={} bar={bar:?} picked={:?} visible={}", offset(&rt),
            rt.element_at(bar[0] + bar[2] * 0.8, bar[1] + bar[3] * 0.5),
            rt.state.active_buffer().unwrap().markdown_cache.tables.borrow()[&0].visible.get());
        assert_eq!(rt.state.active_buffer().unwrap().doc.cursor(), caret);
        assert_eq!(rt.state.active_buffer().unwrap().doc.mode, crate::editor::Mode::Normal);
    }
}