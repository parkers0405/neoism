//! A page's static first frame (`--html`): the laid-out scene as HTML elements with CSS taken
//! from each element's flexbox style, paint and text, so the browser draws the same page before
//! the wasm arrives, and readers, crawlers and link previews see real text, headings and links.
//! The layout stays the browser's (flex, percentages, wrapping), so it holds between the widths
//! it was taken at; `page` folds several widths into `@media` rules.

use std::fmt::Write;

use super::*;
use crate::runtime::web_a11y::{anchor_id, container_tag};

#[path = "export/css.rs"]
mod css;
#[path = "export/page.rs"]
mod page;
pub(crate) use page::page;

/// One element of the exported tree, in document order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExportNode {
    pub path: String,
    pub depth: usize,
    pub tag: &'static str,
    /// Attributes besides `class`, already escaped: ` id="views" href="…"`.
    pub attrs: String,
    /// Inner HTML for text and shapes; children follow otherwise.
    pub inner: String,
    /// CSS declarations, property then value.
    pub decls: Vec<(&'static str, String)>,
    /// Laid-out width in points, which an image's `sizes` follows across widths.
    pub width: f32,
    /// A text element: its children are inline runs, not elements.
    pub text: bool,
}

pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

impl<S: 'static> Runtime<S> {
    /// The visible scene as `ExportNode`s: what `--html` writes for one width.
    pub(crate) fn export_nodes(&self) -> Vec<ExportNode> {
        let mut out = Vec::new();
        // Ancestors' tags, to keep inline content inside links, headings and buttons.
        let mut stack: Vec<(usize, &'static str)> = Vec::new();
        let mut i = 0;
        while i < self.nodes.len() {
            let n = &self.nodes[i];
            let end = self.subtree_end(i);
            while stack.last().is_some_and(|&(e, _)| i >= e) { stack.pop(); }
            if !self.allowed(i) || self.subtree_exiting(i) || n.vis.opacity <= 0.01 || n.input.is_some() {
                i = end;
                continue;
            }
            let inline = stack.iter().any(|&(_, t)| matches!(t, "a" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "span" | "p"));
            let node = self.export_node(i, stack.len(), inline);
            stack.push((end, node.tag));
            out.push(node);
            // Shapes and text carry their content; an image has none.
            if out.last().is_some_and(|e| e.tag == "img") { i = end; stack.pop(); continue; }
            i += 1;
        }
        out
    }

    fn export_node(&self, i: usize, depth: usize, inline: bool) -> ExportNode {
        let n = &self.nodes[i];
        let mut attrs = String::new();
        let mut inner = String::new();
        let href = n.a11y.more.as_ref().and_then(|m| m.href.clone());
        let label = n.label.as_deref().filter(|s| !s.trim().is_empty());
        let pixels = n.vis.cold.pixels.as_ref().filter(|_| i + 1 == self.subtree_end(i));
        let pixels_url = pixels.and_then(|p| p.source_url());
        let tag: &'static str = if let Some(t) = container_tag(n.role) {
            t
        } else if href.is_some() {
            "a"
        } else if n.role == "heading" {
            ["h1", "h2", "h3", "h4", "h5", "h6"][n.a11y.level.unwrap_or(2).clamp(1, 6) as usize - 1]
        } else if pixels_url.is_some() {
            "img"
        } else if n.text.is_some() {
            if inline { "span" } else { "p" }
        } else if inline {
            "span"
        } else {
            "div"
        };
        if matches!(tag, "a" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6") || container_tag(n.role).is_some() {
            if let Some(id) = anchor_id(n.key.as_deref()) { let _ = write!(attrs, " id=\"{id}\""); }
        }
        if let Some(h) = &href { let _ = write!(attrs, " href=\"{}\"", escape(h)); }
        if let Some(url) = pixels_url {
            let [x0, y0, x1, y1] = n.target;
            let _ = write!(attrs, " src=\"{}\" alt=\"{}\" width=\"{}\" height=\"{}\" decoding=\"async\"",
                escape(url), escape(label.unwrap_or("")), (x1 - x0).round(), (y1 - y0).round());
            // `page` adds `sizes`, which needs every width.
            if let Some(set) = pixels.and_then(|p| p.source_set()) { let _ = write!(attrs, " srcset=\"{}\"", escape(set)); }
            // Images below the first screen wait until the reader scrolls near them.
            if y0 > 900.0 { attrs.push_str(" loading=\"lazy\""); }
        } else if let Some(l) = label.filter(|_| n.text.is_none() && tag != "a") {
            let role = matches!(n.role, "image" | "img").then_some(" role=\"img\"").unwrap_or("");
            let _ = write!(attrs, "{role} aria-label=\"{}\"", escape(l));
        }
        if n.a11y.hidden { attrs.push_str(" aria-hidden=\"true\""); }
        let mut decls = css::layout(&self.layout.style(n.taffy).to_style(), i == 0);
        let [x0, y0, x1, y1] = n.target;
        css::paint(&n.vis, (x1 - x0, y1 - y0), &mut decls);
        if let Some(spec) = n.text.as_deref() {
            css::text(spec, &mut decls);
            inner = css::text_html(spec);
        } else if let Some(shape) = n.vis.cold.shape.as_deref() {
            inner = css::svg(shape, (x1 - x0, y1 - y0));
        }
        if tag == "img" {
            let fit = match n.vis.cold.pixels_fit {
                None => "fill",
                Some(crate::SurfaceFit::Fit) => "contain",
                Some(crate::SurfaceFit::Fill) => "cover",
            };
            decls.push(("object-fit", fit.into()));
        }
        ExportNode { path: n.path.clone(), depth, tag, attrs, inner, decls, width: x1 - x0, text: n.text.is_some() }
    }
}

#[cfg(test)]
#[path = "export/tests.rs"]
mod tests;
