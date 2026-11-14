//! Hidden semantic DOM mirror for the browser shell: landmarks, headings, text runs with inline
//! links, links, images and buttons positioned over the canvas so assistive tech, the keyboard
//! and browser Find-in-page see the scene. In a page (`App::web_document`) the rects are in
//! document coordinates and the mirror carries the page's scroll, selection and anchors.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
use std::hash::{Hash, Hasher};

use serde_json::{Value, json};

use super::*;

#[derive(Default)]
pub(crate) struct A11yMirror {
    commits: u64,
    viewport: (u32, u32),
    scroll_digest: u64,
    json: String,
}

#[derive(Debug, PartialEq)]
pub(crate) struct A11yItem {
    pub kind: &'static str,
    pub path: String,
    /// `[x, y, w, h]` in CSS pixels: viewport points, or document points in a page.
    pub rect: [f32; 4],
    pub font_size: f32,
    pub level: Option<u32>,
    pub expanded: Option<bool>,
    pub text: String,
    pub runs: Vec<A11yRun>,
    /// The HTML element a container stands for (`main`, `nav`, `section`, `ul`, ...).
    pub tag: Option<&'static str>,
    /// The index of the container item it sits in.
    pub parent: Option<usize>,
    /// The element id an anchor link scrolls to (`#key`).
    pub id: Option<String>,
    /// A link's URL.
    pub href: Option<String>,
    /// In a page, text's font, so the browser's selection and find highlights land on the
    /// drawn glyphs.
    pub font: Option<A11yFont>,
    /// `El::selectable` text in a page: the browser selects and copies it.
    pub selectable: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct A11yFont {
    /// `"sans"`, `"mono"` or a family name.
    pub family: String,
    pub weight: f32,
    pub line_height: f32,
    pub tracking: f32,
    pub italic: bool,
    /// `"left"`, `"center"` or `"right"`.
    pub align: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct A11yRun {
    pub text: String,
    pub href: Option<String>,
    pub path: Option<String>,
    pub link: Option<usize>,
}

impl A11yItem {
    fn new(kind: &'static str, path: &str, rect: [f32; 4]) -> Self {
        A11yItem { kind, path: path.to_owned(), rect, font_size: 0.0, level: None, expanded: None, text: String::new(),
            runs: Vec::new(), tag: None, parent: None, id: None, href: None, font: None, selectable: false }
    }

    fn to_json(&self) -> Value {
        let mut obj = serde_json::Map::new();
        obj.insert("k".into(), json!(self.kind));
        obj.insert("p".into(), json!(self.path));
        obj.insert("r".into(), json!(self.rect));
        if self.font_size != 0.0 { obj.insert("s".into(), json!(self.font_size)); }
        if let Some(l) = self.level { obj.insert("l".into(), json!(l)); }
        if let Some(x) = self.expanded { obj.insert("x".into(), json!(x)); }
        if !self.text.is_empty() { obj.insert("t".into(), json!(self.text)); }
        if let Some(g) = self.tag { obj.insert("g".into(), json!(g)); }
        if let Some(a) = self.parent { obj.insert("a".into(), json!(a)); }
        if let Some(id) = &self.id { obj.insert("i".into(), json!(id)); }
        if let Some(h) = &self.href { obj.insert("h".into(), json!(h)); }
        if let Some(f) = &self.font {
            obj.insert("f".into(), json!([f.family, f.weight, f.line_height, f.tracking, f.italic, f.align]));
        }
        if self.selectable { obj.insert("e".into(), json!(true)); }
        if !self.runs.is_empty() {
            let runs: Vec<Value> = self.runs.iter().map(|r| {
                let mut ro = serde_json::Map::new();
                ro.insert("t".into(), json!(r.text));
                if let Some(h) = &r.href { ro.insert("h".into(), json!(h)); }
                if let Some(p) = &r.path { ro.insert("p".into(), json!(p)); }
                if let Some(i) = r.link { ro.insert("i".into(), json!(i)); }
                Value::Object(ro)
            }).collect();
            obj.insert("u".into(), Value::Array(runs));
        }
        Value::Object(obj)
    }
}

/// The HTML element a landmark or list role stands for. These wrap their children in the
/// mirror, so the page outline reads as the app built it.
pub(crate) fn container_tag(role: &str) -> Option<&'static str> {
    Some(match role {
        "main" => "main",
        "navigation" => "nav",
        "banner" => "header",
        "contentinfo" => "footer",
        "region" => "section",
        "article" => "article",
        "complementary" => "aside",
        "list" => "ul",
        "listitem" => "li",
        "figure" => "figure",
        _ => return None,
    })
}

/// An element id from a key, for anchors: letters, digits, `-` and `_`, starting with a letter.
pub(crate) fn anchor_id(key: Option<&str>) -> Option<String> {
    let key = key?;
    let ok = key.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then(|| key.to_owned())
}

pub(crate) fn family_name(family: crate::tok::Family) -> String {
    match family {
        crate::tok::Family::Sans => "sans".into(),
        crate::tok::Family::Mono | crate::tok::Family::SystemMono => "mono".into(),
        crate::tok::Family::Named(name) => name.into(),
    }
}

pub(crate) fn align_name(align: crate::el::Align) -> &'static str {
    match align {
        crate::el::Align::Center => "center",
        crate::el::Align::End => "right",
        crate::el::Align::Start => "left",
    }
}

fn round1(v: f32) -> f32 { (v * 10.0).round() / 10.0 }

pub(crate) fn clean_slice(spec: &crate::el::TextSpec, a: usize, b: usize) -> String {
    let a = a.min(spec.content.len());
    let b = b.min(spec.content.len()).max(a);
    if spec.uncopied.is_empty() {
        return spec.content[a..b].to_owned();
    }
    let mut out = String::with_capacity(b - a);
    let mut cur = a;
    for &(ua, ub) in &spec.uncopied {
        let (ua, ub) = (ua as usize, ub as usize);
        if ub <= cur || ua >= b { continue; }
        let stop = ua.max(cur).min(b);
        if cur < stop { out.push_str(&spec.content[cur..stop]); }
        cur = ub.min(b);
    }
    if cur < b { out.push_str(&spec.content[cur..b]); }
    out
}

fn text_runs(path: &str, spec: &crate::el::TextSpec) -> Vec<A11yRun> {
    if spec.links.is_empty() {
        let t = clean_slice(spec, 0, spec.content.len());
        return if t.is_empty() { Vec::new() } else { vec![A11yRun { text: t, href: None, path: None, link: None }] };
    }
    let mut out = Vec::new();
    let mut at = 0usize;
    for (idx, link) in spec.links.iter().enumerate() {
        let (la, lb) = ((link.start as usize).min(spec.content.len()), (link.end as usize).min(spec.content.len()));
        if la > at {
            let before = clean_slice(spec, at, la);
            if !before.is_empty() {
                out.push(A11yRun { text: before, href: None, path: None, link: None });
            }
        }
        if lb > la {
            let label = clean_slice(spec, la, lb);
            if !label.is_empty() {
                out.push(A11yRun {
                    text: label,
                    href: Some(link.id.clone()),
                    path: Some(path.to_owned()),
                    link: Some(idx),
                });
            }
        }
        at = at.max(lb);
    }
    if at < spec.content.len() {
        let tail = clean_slice(spec, at, spec.content.len());
        if !tail.is_empty() {
            out.push(A11yRun { text: tail, href: None, path: None, link: None });
        }
    }
    out
}

impl<S: 'static> Runtime<S> {
    fn scroll_digest(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        // A page hands horizontal scrolling to the DOM, whose container scrolls its own children:
        // its offset changes nothing in the mirror, and hashing it would rewrite the whole page's
        // items on every frame of a horizontal flick.
        let mut entries: Vec<_> = self.scrolls.iter()
            .map(|(id, s)| (id.0, s.offset.to_bits(), if self.page.on { 0 } else { s.x.offset.to_bits() }))
            .collect();
        entries.sort_unstable_by_key(|e| e.0);
        entries.hash(&mut h);
        h.finish()
    }

    pub(crate) fn subtree_end(&self, root: usize) -> usize {
        let mut j = root + 1;
        while j < self.nodes.len() {
            if std::iter::successors(self.nodes[j].parent, |&p| self.nodes[p].parent).any(|p| p == root) {
                j += 1;
            } else {
                break;
            }
        }
        j
    }

    /// Every node's [`Self::subtree_end`], in one reverse pass: a node's children always come
    /// after it, so each subtree's end is known before the parent that extends to it. Walking the
    /// parent chain per node instead costs a page's depth per node on every mirror build.
    fn subtree_ends(&self) -> Vec<usize> {
        let mut ends: Vec<usize> = (1..=self.nodes.len()).collect();
        for i in (0..self.nodes.len()).rev() {
            if let Some(p) = self.nodes[i].parent { ends[p] = ends[p].max(ends[i]); }
        }
        ends
    }

    pub(crate) fn descendant_label(&self, root: usize, end: usize) -> String {
        let mut parts = Vec::new();
        for j in root..end {
            if !self.allowed(j) || self.subtree_exiting(j) || self.nodes[j].vis.opacity <= 0.01 { continue; }
            if let Some(lbl) = self.nodes[j].label.as_deref().filter(|s| !s.trim().is_empty()) {
                parts.push(lbl.trim().to_owned());
                continue;
            }
            if let Some(spec) = self.nodes[j].text.as_deref().filter(|t| !t.redacted && self.nodes[j].input.is_none()) {
                let s = clean_slice(spec, 0, spec.content.len());
                if !s.trim().is_empty() { parts.push(s.trim().to_owned()); }
            }
        }
        parts.join(" ")
    }

    /// Collect semantic DOM items for the current laid-out scene.
    pub fn a11y_json(&self) -> String {
        let values: Vec<Value> = self.web_a11y_items().iter().map(A11yItem::to_json).collect();
        serde_json::to_string(&values).unwrap_or_else(|_| "[]".into())
    }

    /// Node `i`'s drawn box in the mirror's coordinates: the band's, plus where the band starts in
    /// the document, and `dx` points right for content inside a horizontal scroller the browser
    /// scrolls itself. Both are added in `f64` and rounded once, so a fractional device-pixel
    /// scroll offset can't round the same document position two ways between frames.
    fn mirror_rect(&self, i: usize, dx: f64) -> [f32; 4] {
        let b = self.drawn_box(i);
        let (dx, dy) = (dx, f64::from(self.page_offset()));
        let (left, right) = (f64::from(b[0]) + dx, f64::from(b[2]) + dx);
        let (top, bottom) = (f64::from(b[1]) + dy, f64::from(b[3]) + dy);
        [round1(left as f32), round1(top as f32), round1((right - left).max(0.0) as f32), round1((bottom - top).max(0.0) as f32)]
    }

    pub(crate) fn web_a11y_items(&self) -> Vec<A11yItem> {
        let mut out: Vec<A11yItem> = Vec::new();
        let mut last_flow_parent: Option<usize> = None;
        let mut inline: Option<Inline> = None;
        let ends = self.subtree_ends();
        // Open containers: (end of their node subtree, their item index).
        let mut open: Vec<(usize, usize)> = Vec::new();
        // Open horizontal scrollers and how far the browser has scrolled each, so their content
        // mirrors where it sits inside them rather than where the scroll puts it.
        let mut scrollers: Vec<(usize, f64)> = Vec::new();
        let mut i = 0;
        while i < self.nodes.len() {
            while open.last().is_some_and(|&(end, _)| i >= end) { open.pop(); }
            while scrollers.last().is_some_and(|&(end, _)| i >= end) { scrollers.pop(); }
            let parent = open.last().map(|&(_, item)| item);
            let n = &self.nodes[i];
            let end = ends[i];
            if !self.allowed(i) || self.subtree_exiting(i) || n.vis.opacity <= 0.01 || n.a11y.hidden
                || matches!(n.role, "none" | "presentation")
            {
                i = end;
                last_flow_parent = None;
                continue;
            }
            if n.input.is_some() {
                i = end;
                last_flow_parent = None;
                continue;
            }
            // A page hands every horizontal viewport to the DOM: iOS scrolls it natively, at the
            // compositor's rate, with axis locking, momentum and text selection.
            if self.page.on && n.scroll_axes == crate::el::ScrollAxes::X {
                let rect = self.mirror_rect(i, 0.0);
                let offset = self.scrolls.get(&n.id).map_or(0.0, |st| st.x.offset);
                let mut item = A11yItem::new("scroll_x", &n.path, rect);
                item.tag = Some("div");
                item.parent = parent;
                out.push(item);
                open.push((end, out.len() - 1));
                scrollers.push((end, offset));
                last_flow_parent = None;
                i += 1;
                continue;
            }
            let shift: f64 = scrollers.iter().map(|&(_, offset)| offset).sum();
            let rect = self.mirror_rect(i, shift);
            let push = |out: &mut Vec<A11yItem>, mut item: A11yItem| {
                item.parent = parent;
                out.push(item);
            };

            if let Some(tag) = container_tag(n.role) {
                let mut item = A11yItem::new("group", &n.path, rect);
                item.tag = Some(tag);
                item.id = anchor_id(n.key.as_deref());
                item.text = n.label.clone().filter(|s| !s.trim().is_empty()).unwrap_or_default();
                push(&mut out, item);
                open.push((end, out.len() - 1));
                last_flow_parent = None;
                i += 1;
                continue;
            }
            if let Some(href) = n.a11y.more.as_ref().and_then(|m| m.href.clone()) {
                let label = n.label.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| self.descendant_label(i, end));
                if !label.is_empty() && rect[2] > 0.0 && rect[3] > 0.0 {
                    let mut item = A11yItem::new("link", &n.path, rect);
                    item.text = label;
                    item.href = Some(href);
                    item.id = anchor_id(n.key.as_deref());
                    push(&mut out, item);
                }
                i = end;
                last_flow_parent = None;
                continue;
            }
            if matches!(n.role, "image" | "img") && let Some(label) = n.label.clone().filter(|s| !s.trim().is_empty()) {
                let mut item = A11yItem::new("image", &n.path, rect);
                item.text = label;
                push(&mut out, item);
                i = end;
                last_flow_parent = None;
                continue;
            }
            let button_like = (n.role == "button" || n.handlers.on_press.is_some()) && n.window_drag != crate::el::WindowDrag::Drag;
            // Only collapse a pressable container into one button when it doesn't wrap an entire
            // multi-block section with independent buttons inside.
            let nested_press = (i + 1..end).any(|j| self.nodes[j].handlers.on_press.is_some() || self.nodes[j].role == "button");
            if button_like && !nested_press && rect[2] > 0.0 && rect[3] > 0.0 {
                let label = n.label.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| self.descendant_label(i, end));
                if !label.is_empty() {
                    let mut item = A11yItem::new("button", &n.path, rect);
                    item.expanded = n.a11y.expanded;
                    item.text = label;
                    push(&mut out, item);
                    i = end;
                    last_flow_parent = None;
                    continue;
                }
            }
            if let Some(spec) = n.text.as_deref().filter(|t| !t.redacted && rect[2] > 0.0 && rect[3] > 0.0) {
                let runs = text_runs(&n.path, spec);
                if !runs.is_empty() {
                    let is_heading = n.role == "heading";
                    // Flowed word-by-word paragraphs (`pr_chips::flow`) put sibling words (or 1-word
                    // wrapper rows) inside one flex-wrap line row: merge consecutive inline text runs
                    // sharing that flow container so screen readers and Find-in-page see whole sentences.
                    let flow_group = n.parent.and_then(|p| {
                        if self.nodes[p].key.as_deref().is_none() && self.nodes[p].parent.is_some()
                            && self.nodes[i].key.as_deref().is_some_and(|k| k.starts_with("flow-"))
                        {
                            self.nodes[p].parent
                        } else if self.nodes[i].key.as_deref().is_some_and(|k| k.starts_with("flow-")) {
                            Some(p)
                        } else {
                            None
                        }
                    });
                    if !is_heading && flow_group.is_some() && flow_group == last_flow_parent
                        && let Some(prev) = out.last_mut().filter(|it| it.kind == "text")
                    {
                        merge_flow(prev, rect, runs);
                        i += 1;
                        continue;
                    }
                    let text_rect = if self.page.on { inset_rect(rect, n.inset) } else { rect };
                    if !is_heading && flow_group.is_none()
                        && let Some(prev) = inline.filter(|l| l.item + 1 == out.len() && l.parent == n.parent)
                        && let Some(gap) = prev.gap(n.target, spec.style, spec.selectable)
                        && let Some(item) = out.last_mut().filter(|it| it.kind == "text")
                    {
                        join_inline(item, text_rect, runs, gap > spec.style.size * 0.15);
                        inline = Some(Inline { right: n.target[2], ..prev });
                        i += 1;
                        continue;
                    }
                    let (plain, rich_runs) = if runs.len() == 1 && runs[0].href.is_none() && flow_group.is_none() {
                        (runs.into_iter().next().unwrap().text, Vec::new())
                    } else {
                        (String::new(), runs)
                    };
                    let mut item = A11yItem::new(if is_heading { "heading" } else { "text" }, &n.path, rect);
                    item.font_size = round1(spec.style.size);
                    item.level = is_heading.then(|| n.a11y.level.unwrap_or(2).clamp(1, 6));
                    item.text = plain;
                    item.runs = rich_runs;
                    item.id = if is_heading { anchor_id(n.key.as_deref()) } else { None };
                    if self.page.on {
                        item.rect = text_rect;
                        let style = spec.style;
                        item.font = Some(A11yFont {
                            family: family_name(style.family),
                            weight: style.weight,
                            line_height: round1(if style.line_height > 0.0 { style.line_height } else { style.size * 1.25 }),
                            tracking: style.tracking,
                            italic: style.italic,
                            align: align_name(spec.align),
                        });
                        item.selectable = spec.selectable;
                    }
                    push(&mut out, item);
                    last_flow_parent = if is_heading { None } else { flow_group };
                    inline = (!is_heading && flow_group.is_none()).then(|| Inline {
                        parent: n.parent,
                        item: out.len() - 1,
                        right: n.target[2],
                        top: n.target[1],
                        bottom: n.target[3],
                        style: spec.style,
                        selectable: spec.selectable,
                    });
                }
            } else if n.role == "heading" && let Some(lbl) = n.label.clone().filter(|s| !s.trim().is_empty()) {
                let mut item = A11yItem::new("heading", &n.path, rect);
                item.font_size = 16.0;
                item.level = Some(n.a11y.level.unwrap_or(2).clamp(1, 6));
                item.text = lbl;
                item.id = anchor_id(n.key.as_deref());
                push(&mut out, item);
                last_flow_parent = None;
            }
            i += 1;
        }
        out
    }

    /// The page's horizontal scroll viewports: their paths and how far each is scrolled right.
    /// The browser's mirror holds the containers, so the DOM keeps an offset Rune scrolls too.
    pub(crate) fn page_scroll_x(&self) -> Vec<(String, f32)> {
        if !self.page.on { return Vec::new(); }
        self.nodes.iter()
            .filter(|n| n.scroll_axes == crate::el::ScrollAxes::X)
            .filter_map(|n| self.scrolls.get(&n.id).map(|st| (n.path.clone(), st.x.offset as f32)))
            .collect()
    }

    /// Returns updated JSON only when layout commits, viewport or scroll positions changed.
    pub(crate) fn web_a11y<'a>(&self, mirror: &'a mut A11yMirror) -> Option<&'a str> {
        let vp = (self.viewport.0.to_bits(), if self.page.on { 0 } else { self.viewport.1.to_bits() });
        let sd = self.scroll_digest();
        if mirror.commits == self.commits && mirror.viewport == vp && mirror.scroll_digest == sd && !mirror.json.is_empty() {
            return None;
        }
        mirror.commits = self.commits;
        mirror.viewport = vp;
        mirror.scroll_digest = sd;
        let values: Vec<Value> = self.web_a11y_items().iter().map(A11yItem::to_json).collect();
        let next = serde_json::to_string(&values).unwrap_or_else(|_| "[]".into());
        if mirror.json == next {
            return None;
        }
        mirror.json = next;
        Some(&mirror.json)
    }

    /// Dispatches assistive-tech activation of a mirrored button or inline link.
    pub(crate) fn web_a11y_action(&mut self, event: &serde_json::Value) -> bool {
        let Some(path) = event["path"].as_str() else { return false };
        match event["kind"].as_str() {
            Some("a11y_press") => {
                let Some(i) = self.live(path) else { return false };
                if self.nodes[i].handlers.on_press.is_none() { return false; }
                self.queue.push(Action::Press(path.to_owned(), crate::PointerButton::Primary));
                true
            }
            Some("a11y_link") => {
                let Some(k) = event["link"].as_u64().and_then(|n| usize::try_from(n).ok()) else { return false };
                let Some(i) = self.live(path) else { return false };
                if self.nodes[i].text.as_ref().is_none_or(|t| k >= t.links.len()) { return false; }
                self.queue.push(Action::Link(path.to_owned(), k));
                true
            }
            _ => false,
        }
    }
}

/// Joins one more flowed word run into the paragraph item before it.
fn merge_flow(prev: &mut A11yItem, rect: [f32; 4], runs: Vec<A11yRun>) {
    prev.rect = union(prev.rect, rect);
    let needs_space = prev.runs.last().is_some_and(|r| !r.text.ends_with(char::is_whitespace))
        && runs.first().is_some_and(|r| !r.text.starts_with(char::is_whitespace) && !r.text.starts_with([')', ']', '.', ',', ';', ':']));
    if needs_space {
        if let Some(last) = prev.runs.last_mut().filter(|r| r.href.is_none()) {
            last.text.push(' ');
        } else {
            prev.runs.push(A11yRun { text: " ".into(), href: None, path: None, link: None });
        }
    }
    if runs.len() == 1 && runs[0].href.is_none()
        && let Some(last) = prev.runs.last_mut().filter(|r| r.href.is_none())
    {
        last.text.push_str(&runs[0].text);
    } else {
        prev.runs.extend(runs);
    }
}

fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let (x0, y0) = (a[0].min(b[0]), a[1].min(b[1]));
    let (x1, y1) = ((a[0] + a[2]).max(b[0] + b[2]), (a[1] + a[3]).max(b[1] + b[3]));
    [x0, y0, round1(x1 - x0), round1(y1 - y0)]
}

fn inset_rect(rect: [f32; 4], inset: [f32; 4]) -> [f32; 4] {
    [round1(rect[0] + inset[0]), round1(rect[1] + inset[1]),
        round1((rect[2] - inset[0] - inset[2]).max(0.0)), round1((rect[3] - inset[1] - inset[3]).max(0.0))]
}

/// The last text item and where its node's laid-out box ends, so a sibling text that carries on
/// the same line joins it: a code line drawn one text per token mirrors as one line, which reads,
/// finds and copies as the line rather than token by token.
#[derive(Clone, Copy)]
struct Inline {
    parent: Option<usize>,
    item: usize,
    right: f32,
    top: f32,
    bottom: f32,
    style: crate::tok::TextStyle,
    selectable: bool,
}

impl Inline {
    /// The gap in points from the last text to `target`, when `target` carries on its line in the
    /// same face; a larger gap is a separate column, not the same run of text.
    fn gap(&self, target: [f32; 4], style: crate::tok::TextStyle, selectable: bool) -> Option<f32> {
        let a = self.style;
        let same_face = (a.family, a.size, a.weight, a.tracking) == (style.family, style.size, style.weight, style.tracking)
            && self.selectable == selectable;
        let height = (self.bottom - self.top).min(target[3] - target[1]);
        let overlap = self.bottom.min(target[3]) - self.top.max(target[1]);
        let gap = target[0] - self.right;
        (same_face && height > 0.0 && overlap >= height * 0.5 && (-0.5..=style.size * 0.6).contains(&gap)).then_some(gap)
    }
}

/// Appends a sibling's text to the line item before it, with a space when a visible gap stands
/// for one.
fn join_inline(prev: &mut A11yItem, rect: [f32; 4], mut runs: Vec<A11yRun>, gap: bool) {
    prev.rect = union(prev.rect, rect);
    let ends_blank = if prev.runs.is_empty() { prev.text.ends_with(char::is_whitespace) } else { prev.runs.last().is_some_and(|r| r.text.ends_with(char::is_whitespace)) };
    if gap && !ends_blank && runs.first().is_some_and(|r| !r.text.starts_with(char::is_whitespace)) {
        runs.insert(0, A11yRun { text: " ".into(), href: None, path: None, link: None });
    }
    if prev.runs.is_empty() && runs.iter().all(|r| r.href.is_none()) {
        for r in runs { prev.text.push_str(&r.text); }
        return;
    }
    if prev.runs.is_empty() {
        prev.runs.push(A11yRun { text: std::mem::take(&mut prev.text), href: None, path: None, link: None });
    }
    for r in runs {
        match prev.runs.last_mut() {
            Some(last) if last.href.is_none() && r.href.is_none() => last.text.push_str(&r.text),
            _ => prev.runs.push(r),
        }
    }
}

#[cfg(test)]
#[path = "a11y_tests.rs"]
mod tests;
