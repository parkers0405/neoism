//! The playground's window: the bar, the sidebar of stories, the stage and the side column.

use super::*;

impl<S: 'static> Live<S> {
    /// The window's root: the app's view above the bar, or the playground around its stage.
    pub(crate) fn view(&self, app: &S, view: fn(&S, &mut Cx<S>) -> El, cx: &mut Cx<S>, viewport: (f32, f32)) -> El {
        let dark = self.app_dark.map_or_else(|| cx.system_dark(), |dark| dark(app));
        let t = tok::theme(dark);
        if !self.open {
            if !self.bar { return call(view, app, cx); }
            let root = call(view, app, cx);
            // Laid out as the root of a window BAR shorter, as the runtime sizes a root. The
            // runtime finds the root's handlers on it here (`Runtime::app_root`).
            return col().key("playground-closed").w(viewport.0).h(viewport.1)
                .child(root.w(viewport.0).h((viewport.1 - BAR).max(0.0)))
                .child(self.bar_view(t));
        }
        let (sidebar_w, side_w) = self.widths(viewport.0);
        let stage_w = (viewport.0 - sidebar_w - side_w - 2.0).max(0.0);
        let stage = self.stage(app, view, cx, (stage_w, viewport.1), t, dark);
        let stage = match self.region {
            // The stage has no selection to colour, so a ring inside its edge shows it has the
            // keyboard. It takes no input, so presses reach the story under it.
            Region::Stage => stage.child(el().key("playground-stage-ring").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).border(2.0, cx.focus_color())),
            _ => stage,
        };
        let side = self.side(app, cx, t, dark, side_w, viewport.1);
        row().key("playground").w(viewport.0).h(viewport.1).fill(t.bg)
            .child(self.sidebar(t, dark, cx.accent(), cx.titlebar_inset(), sidebar_w, viewport.1))
            .child(el().w(1.0).h(viewport.1).fill(t.separator))
            .child(stage)
            .child(el().w(1.0).h(viewport.1).fill(t.separator))
            .child(side)
            .child(self.sidebar_edge(sidebar_w, viewport))
            .child(self.side_edge(side_w, viewport))
    }

    fn bar_view(&self, t: Theme) -> El {
        let stories = match self.order.len() { 0 => String::new(), 1 => "1 story".into(), n => format!("{n} stories") };
        let hint = self.shortcut.as_ref().map(keycaps);
        let summary = [Some(stories).filter(|s| !s.is_empty()), hint].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        let open = row().key("playground-open").h(20.0).ml(4.0).px(6.0).gap(6.0).center_y().radius(radius::SM - 1.0)
            .hover(t.hover).press(t.press)
            .child(icon(&GRID, 12.0).color(t.text_secondary))
            .child(text("Playground").font(type_::LABEL).color(t.text_secondary))
            .role("button").label("Open the playground")
            .on_press_tx(|_: &mut Controls, tx: &mut Tx| tx.open_playground());
        col().w_full().h(BAR).fill(t.bg)
            .child(el().w_full().h(1.0).fill(t.separator))
            .child(
                row().w_full().h(BAR - 1.0).center_y().justify_between()
                    .child(open)
                    .child(text(summary).font(type_::LABEL).color(t.text_secondary).pr(10.0)),
            )
    }

    /// Along the sidebar's foot: the stage's size, motion and appearance.
    pub(super) fn stage_controls(&self, t: Theme, window_dark: bool, sidebar_w: f32) -> El {
        let dark = self.dark.unwrap_or(window_dark);
        let (glyph, appearance) = if dark { (&MOON, "Dark appearance") } else { (&SUN, "Light appearance") };
        let theme = icon_button(t, "playground-theme", glyph, appearance, false, window_dark, Request::Dark(!dark));
        let reduce = icon_button(t, "playground-reduce-motion", &MOTION, "Reduce Motion", self.reduce_motion, window_dark, Request::ReduceMotion(!self.reduce_motion));
        let sizes = (!self.sizes.is_empty()).then(|| self.size_menu(t));
        row().key("playground-stage-controls").w(sidebar_w).h(HEADER).px(8.0).gap(2.0).center_y()
            .children(sizes)
            .child(spacer())
            .child(reduce)
            .child(theme)
    }

    /// The stage's size: the window's room or a preset, from a pop-up menu (a press steps
    /// through them where there's no native one).
    fn size_menu(&self, t: Theme) -> El {
        let current = self.size.checked_sub(1).and_then(|i| self.sizes.get(i)).map_or("Window", |(n, _)| *n);
        let options: Vec<(String, String)> = std::iter::once(("0".to_owned(), "Window".to_owned()))
            .chain(self.sizes.iter().enumerate().map(|(i, (n, (w, h)))| ((i + 1).to_string(), format!("{n}  {w}×{h}"))))
            .collect();
        let (size, count) = (self.size, self.sizes.len() + 1);
        row().key("playground-size").h(CONTROL).pl(8.0).pr(4.0).gap(2.0).center_y().radius(radius::SM)
            .hover(t.hover).press(t.press)
            .child(text(current).font(type_::LABEL).color(t.text))
            .child(icon(&CHEVRON_DOWN, 16.0).color(t.text_secondary))
            .role("button").label("Stage size")
            .on_press_tx(move |_: &mut Controls, tx: &mut Tx| {
                if !crate::popup_menu_supported() {
                    tx.playground.push(Request::Size((size + 1) % count));
                    return;
                }
                let menu = crate::PopupMenu::picker(options.clone(), &size.to_string());
                tx.popup_menu(menu, "playground-size", |_: &mut Controls, picked, tx| {
                    if let Some(i) = picked.and_then(|id| id.parse().ok()) { tx.playground.push(Request::Size(i)); }
                });
            })
    }

    /// The column right of the stage: the story's group and name, the app's controls for it,
    /// its triggers, and a key hint.
    fn side(&self, app: &S, cx: &mut Cx<S>, t: Theme, dark: bool, side_w: f32, height: f32) -> El {
        let i = self.selected;
        let title = (i < self.count()).then(|| {
            col().key("playground-title").px(4.0).gap(2.0)
                .child(text(self.group(i).replace('/', " › ")).font(type_::SECTION).color(t.text_secondary).truncate())
                .child(text(self.name(i)).font(type_::TITLE).color(t.text).truncate())
        });
        // Keyed by story, not generation, so a replay keeps the keyboard on the control it's on.
        let slot = self.side_slot(app, cx).map(|slot| {
            let slot = el().key(format!("playground-slot-{i}")).w_full().child(slot);
            // Its view marks what the keys move, so no ring.
            if self.side_keys() { slot.focusable(true).no_focus_ring() } else { slot }
        });
        let variants = self.variants();
        let at = self.variant_at();
        // The control the keyboard is on takes the accent, as the list's selection does while
        // the list has it; the picked variant stays grey.
        let on_fill = super::list::focused(cx.accent());
        let picker = (!variants.is_empty()).then(|| {
            col().key("playground-variants").w_full().padding(4.0).gap(2.0).radius(radius::LG).fill(t.card).outline(1.0, 0.0, t.card_border)
                .children(variants.iter().enumerate().take(9).map(|(n, &name)| {
                    let on = n == at;
                    row().key(format!("playground-variant-{n}")).w_full().h(ROW).px(4.0).gap(8.0).center_y().radius(radius::MD)
                        .fill(if on { selected(t, dark) } else { Color::TRANSPARENT })
                        .focusable(true).focus_highlight(on_fill).press(t.press)
                        .child(keycap(t, &(n + 1).to_string()))
                        .child(text(name).font(type_::BODY).color(if on { t.text } else { t.text_secondary }).truncate())
                        .role("button").label(name)
                        .on_press_tx(move |_: &mut Controls, tx: &mut Tx| tx.playground.push(Request::Variant(n)))
                }))
        });
        let triggers = self.triggers();
        let card = (!triggers.is_empty()).then(|| {
            col().key("playground-triggers").w_full().padding(4.0).gap(2.0).radius(radius::LG).fill(t.card).outline(1.0, 0.0, t.card_border)
                .children(triggers.iter().map(|&(key, label)| {
                    let request = if key == 'r' { Request::Replay } else { Request::Trigger(key) };
                    row().key(format!("playground-trigger-{key}")).w_full().h(ROW).px(4.0).gap(8.0).center_y().radius(radius::MD)
                        .focusable(true).focus_highlight(on_fill).press(t.press)
                        .child(keycap(t, &cap(key)))
                        .child(text(label).font(type_::BODY).color(t.text).truncate())
                        .role("button").label(label)
                        .on_press_tx(move |_: &mut Controls, tx: &mut Tx| tx.playground.push(request.clone()))
                }))
        });
        let hint = self.hint(variants.len(), slot.is_some() || !variants.is_empty() || !triggers.is_empty());
        // The Windows caption buttons sit over the column's top.
        let chrome = cx.chrome_insets();
        let under_caption = if chrome.trailing > 0.0 { chrome.height } else { 0.0 };
        col().key("playground-side").w(side_w).h(height).px(12.0).pt(12.0 + under_caption).pb(12.0).gap(12.0)
            .children(title)
            .children(slot)
            .children(picker)
            .children(card)
            .child(spacer())
            .child(text(hint).key("playground-hint").font(type_::SECTION).color(t.text_secondary).px(4.0).wrap())
    }

    /// The keys of the column that has the keyboard, under the side column.
    fn hint(&self, variants: usize, controls: bool) -> String {
        let variants = match variants.min(9) { 0 => None, 1 => Some("1\u{a0}variant".into()), n => Some(format!("1–{n}\u{a0}variants")) };
        let parts: Vec<Option<String>> = match self.region {
            Region::List => vec![
                Some("↑↓\u{a0}stories".into()),
                Some("←→\u{a0}groups".into()),
                Some("/\u{a0}filter".into()),
                controls.then(|| "Tab\u{a0}controls".into()),
                Some("Esc\u{a0}back".into()),
            ],
            Region::Side => vec![Some("↑↓\u{a0}controls".into()), variants, Some("Esc\u{a0}stories".into())],
            Region::Stage => vec![Some("Keys go to the story".into()), Some("Esc\u{a0}stories".into())],
        };
        // No-break spaces keep each part whole when the hint wraps.
        parts.into_iter().flatten().collect::<Vec<_>>().join("\u{a0}· ")
    }

    /// The app's controls for the open story, from `Playground::side` or `Story::side` (or
    /// their `side_keys`), acting on its stage state.
    fn side_slot(&self, app: &S, cx: &mut Cx<S>) -> Option<El> {
        if let Some(scene) = &self.scene {
            let (_, view) = self.sides.iter().find(|(s, _)| *s == self.selected)?;
            let mut stage = cx.retype::<S>();
            let root = match view {
                Side::Stage(view) => view(&scene.state, &mut stage),
                Side::Real(view) => view(&scene.state, app, &mut stage),
                Side::Keys(view, _) => view(&scene.state, self.side_active, &mut stage),
            };
            cx.absorb(stage);
            return Some(root);
        }
        let (own, stage) = self.own()?;
        let mut unit = cx.retype::<()>();
        let root = own.side(stage, self.side_active, &mut unit);
        cx.absorb(unit);
        root
    }

    fn stage(&self, app: &S, view: fn(&S, &mut Cx<S>) -> El, cx: &mut Cx<S>, area: (f32, f32), t: Theme, dark: bool) -> El {
        let mut env = cx.env();
        env.system_dark = self.dark.unwrap_or(dark);
        let page = self.page();
        let frame = self.own().and_then(|(own, _)| own.frame());
        let preset = self.size.checked_sub(1).and_then(|i| self.sizes.get(i)).map(|(_, size)| *size);
        let layout = self.layout();
        let (w, h) = if layout == Stage::Fill { area } else { frame.or(preset).unwrap_or(area) };
        let content = match (&self.scene, page) {
            (Some(scene), _) => Some(self.nested(cx, env, |c| call(view, &scene.state, c))),
            (None, Some((_, Page { kind: Kind::Live(page), .. }))) => Some(self.nested(cx, env, |c| page(app, c))),
            (None, Some(_)) => self.own().map(|(own, stage)| {
                let mut unit = cx.retype::<()>();
                unit.set_env(env);
                unit.clear_chrome();
                if self.reduce_motion { unit.reduced_motion(true); }
                let root = own.view(stage, &mut unit);
                cx.absorb(unit);
                root
            }),
            (None, None) => None,
        };
        let inner = tok::theme(env.system_dark);
        let content = content.unwrap_or_else(|| {
            col().center().child(text("Add scenes to AppSpec::scenes, or stories to the playground").font(type_::BODY).color(inner.text_secondary))
        });
        let key = format!("playground-stage-{}", self.generation);
        let stage = match layout {
            // A scroll measures its first child, so the margin goes on a column around the content.
            Stage::Scroll => col().key(key).w(w).h(h).scroll(ScrollSpec { stick_bottom: false })
                .child(col().key("playground-margin").w(w).padding(SCROLL_MARGIN).child(content.w((w - 2.0 * SCROLL_MARGIN).max(0.0)))),
            // Sized as a window sizes its root.
            _ => el().key(key).w(w).h(h).child(content.w(w).h(h)),
        };
        let stage = match (layout, frame, preset) {
            (Stage::Fill, _, _) => stage,
            // An overlay's clear parts stay clear.
            (_, Some(_), _) => stage.border(1.0, t.text_secondary.alpha(0.35)).border_dash(4.0, 3.0),
            (_, None, Some(_)) => stage.outline(1.0, 0.0, inner.separator),
            (_, None, None) => stage,
        };
        col().w(area.0).h(area.1).center().fill(inner.bg).clip_rounded(0.0).child(stage)
    }

    /// Runs a stage view of the app's own type with the stage's appearance and motion.
    fn nested(&self, cx: &mut Cx<S>, env: crate::Env, f: impl FnOnce(&mut Cx<S>) -> El) -> El {
        let mut stage = cx.retype::<S>();
        stage.set_env(env);
        stage.clear_chrome();
        if self.reduce_motion { stage.reduced_motion(true); }
        let root = f(&mut stage);
        cx.absorb(stage);
        root
    }
}

/// A selected row's fill, a step stronger in dark mode where the track is faint.
pub(super) fn selected(t: Theme, dark: bool) -> Color {
    if dark { t.control_off } else { t.control_track }
}

/// Text a step quieter than secondary: the list's counts and motion marks.
pub(super) fn tertiary(t: Theme) -> Color {
    t.text_secondary.mix(t.bg, 0.45)
}

/// A key's cap beside its control in the side column.
fn keycap(t: Theme, key: &str) -> El {
    row().key("cap").h(20.0).min_w(22.0).px(6.0).center().radius(radius::SM - 2.0).fill(t.control_track)
        .child(text(key.to_owned()).font(type_::LABEL).color(t.text))
}

/// A trigger's key as its cap shows it.
fn cap(key: char) -> String {
    if key == ' ' { "Space".into() } else { key.to_ascii_uppercase().to_string() }
}

/// A shortcut as the bar shows it: "⇧⌘D", or "Ctrl+Shift+D" off macOS. Its menu item and the
/// window both match the character, so this is the key on every layout.
fn keycaps(shortcut: &Shortcut) -> String {
    shortcut.menu_keycaps().join(if cfg!(target_os = "macos") { "" } else { "+" })
}

/// A quiet square button showing `glyph`, named by `label` in its tooltip; `on` fills it as the
/// list's selection is filled.
fn icon_button(t: Theme, key: &str, glyph: &'static Icon, label: &str, on: bool, dark: bool, request: Request) -> El {
    row().key(key.to_owned()).size(CONTROL, CONTROL).center().radius(radius::SM)
        .fill(if on { selected(t, dark) } else { Color::TRANSPARENT })
        .hover(if on { selected(t, dark) } else { t.hover }).press(t.press)
        .child(icon(glyph, 16.0).color(if on { t.text } else { t.text_secondary }))
        .role("button").label(label.to_owned()).help(label.to_owned())
        .on_press_tx(move |_: &mut Controls, tx: &mut Tx| tx.playground.push(request.clone()))
}

pub(super) static CHEVRON_LEFT: Icon = Icon {
    name: "playground-chevron-left",
    view: 16.0,
    body: r#"<path d="M9.5 4L5.5 8L9.5 12" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

static GRID: Icon = Icon {
    name: "playground-grid",
    view: 12.0,
    body: r#"<rect x="1" y="1" width="4" height="4" rx="1" fill="currentColor"/><rect x="7" y="1" width="4" height="4" rx="1" fill="currentColor"/><rect x="1" y="7" width="4" height="4" rx="1" fill="currentColor"/><rect x="7" y="7" width="4" height="4" rx="1" fill="currentColor"/>"#,
};

pub(super) static CHEVRON_RIGHT: Icon = Icon {
    name: "playground-chevron-right",
    view: 16.0,
    body: r#"<path d="M6.75 5L9.75 8L6.75 11" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

pub(super) static CHEVRON_DOWN: Icon = Icon {
    name: "playground-chevron-down",
    view: 16.0,
    body: r#"<path d="M5 6.75L8 9.75L11 6.75" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

static SUN: Icon = Icon {
    name: "playground-sun",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="2.75" stroke="currentColor" stroke-width="1.5" fill="none"/><path d="M8 1.75V3M8 13V14.25M1.75 8H3M13 8H14.25M3.6 3.6L4.45 4.45M11.55 11.55L12.4 12.4M3.6 12.4L4.45 11.55M11.55 4.45L12.4 3.6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>"#,
};

static MOON: Icon = Icon {
    name: "playground-moon",
    view: 16.0,
    body: r#"<path d="M13.25 9.5A5.5 5.5 0 0 1 6.5 2.75A5.5 5.5 0 1 0 13.25 9.5Z" stroke="currentColor" stroke-width="1.5" fill="none" stroke-linejoin="round"/>"#,
};

pub(super) static MOTION: Icon = Icon {
    name: "playground-motion",
    view: 16.0,
    body: r#"<circle cx="10" cy="8" r="3.25" stroke="currentColor" stroke-width="1.5" fill="none"/><path d="M1.75 5.5H4.5M1.25 8H4M1.75 10.5H4.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>"#,
};
