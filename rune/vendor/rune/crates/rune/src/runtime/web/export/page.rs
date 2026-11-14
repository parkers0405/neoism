//! Folds the scene taken at several widths into one HTML body and one stylesheet: the first
//! width's tree, with each element's CSS for the narrowest width as its base and `@media
//! (min-width)` overrides where a wider one differs. Identical rules share a class.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

use super::ExportNode;

/// The rule every exported element starts from: a Rune element is a flex row, positioned, with
/// its border inside its size. A text element (class `t`) holds inline runs instead.
pub(crate) const BASE_CSS: &str = "[data-rune-static],[data-rune-static] *{box-sizing:border-box;display:flex;position:relative;margin:0;padding:0;min-width:0;border:0 solid transparent}[data-rune-static] a{color:inherit;text-decoration:none}[data-rune-static] .t>*{display:inline;position:static}[data-rune-static] img{display:block}:where([data-rune-static]) img{height:auto}[data-rune-static] svg{display:block;position:absolute;inset:0}";

/// The value a property takes where a width sets nothing, matching `BASE_CSS`.
fn initial(prop: &str) -> &'static str {
    match prop {
        "display" => "flex",
        "position" => "relative",
        "min-width" => "0",
        "border-width" => "0",
        "margin" | "padding" => "0",
        "flex-direction" => "row",
        "flex-wrap" => "nowrap",
        "flex-grow" | "row-gap" | "column-gap" | "border-radius" => "0",
        "flex-shrink" | "opacity" => "1",
        "background" | "box-shadow" | "filter" | "backdrop-filter" => "none",
        _ => "unset",
    }
}

const RANGE_PREFIX: [char; 6] = ['a', 'b', 'c', 'd', 'e', 'f'];

/// `widths`: the scene at each `(min-width, nodes)`, narrowest first. Returns the body (the
/// first width's tree) and the stylesheet.
pub(crate) fn page(widths: &[(f32, Vec<ExportNode>)]) -> (String, String) {
    let Some((_, primary)) = widths.first() else { return (String::new(), String::new()) };
    let widths = &widths[..widths.len().min(RANGE_PREFIX.len())];
    let by_path: Vec<HashMap<&str, &ExportNode>> =
        widths.iter().map(|(_, nodes)| nodes.iter().map(|n| (n.path.as_str(), n)).collect()).collect();
    // Per range: declaration text to class name.
    let mut classes: Vec<BTreeMap<String, String>> = vec![BTreeMap::new(); widths.len()];
    let mut node_classes: Vec<Vec<String>> = Vec::with_capacity(primary.len());
    for node in primary {
        let found: Vec<Option<&ExportNode>> = by_path.iter().map(|m| m.get(node.path.as_str()).copied()).collect();
        let mut props: Vec<&'static str> = Vec::new();
        for n in found.iter().flatten() {
            for (p, _) in &n.decls {
                if !props.contains(p) { props.push(p); }
            }
        }
        if found.iter().any(Option::is_none) && !props.contains(&"display") { props.push("display"); }
        let mut mine = Vec::new();
        let mut last: Option<String> = None;
        for (k, n) in found.iter().enumerate() {
            let text = match n {
                None => "display:none".to_owned(),
                Some(n) => props.iter().map(|p| {
                    let v = n.decls.iter().rev().find(|(q, _)| q == p).map_or(initial(p), |(_, v)| v.as_str());
                    format!("{p}:{v}")
                }).collect::<Vec<_>>().join(";"),
            };
            // Only the base width writes a rule that sets nothing.
            if last.as_deref() != Some(&text) && !(k == 0 && text.is_empty()) {
                let next = classes[k].len();
                let name = classes[k].entry(text.clone()).or_insert_with(|| format!("{}{}", RANGE_PREFIX[k], base36(next))).clone();
                mine.push(name);
            }
            last = Some(text);
        }
        node_classes.push(mine);
    }
    let mut css = String::from(BASE_CSS);
    for (k, rules) in classes.iter().enumerate() {
        let mut block = String::new();
        let mut sorted: Vec<(&String, &String)> = rules.iter().collect();
        sorted.sort_by(|a, b| a.1.len().cmp(&b.1.len()).then(a.1.cmp(b.1)));
        for (decls, name) in sorted { let _ = write!(block, ".{name}{{{decls}}}"); }
        if k == 0 { css.push_str(&block); } else if !block.is_empty() {
            let _ = write!(css, "@media (min-width:{}px){{{block}}}", widths[k].0);
        }
    }
    let mut body = String::new();
    let mut open: Vec<&'static str> = Vec::new();
    for (node, cls) in primary.iter().zip(&node_classes) {
        while open.len() > node.depth { let _ = write!(body, "</{}>", open.pop().unwrap()); }
        let names: Vec<&str> = node.text.then_some("t").into_iter().chain(cls.iter().map(String::as_str)).collect();
        let class = if names.is_empty() { String::new() } else { format!(" class=\"{}\"", names.join(" ")) };
        let root = if node.depth == 0 { " data-rune-static" } else { "" };
        let sizes = if node.attrs.contains(" srcset=") { sizes(node, widths, &by_path) } else { String::new() };
        let _ = write!(body, "<{}{root}{class}{}{sizes}>", node.tag, node.attrs);
        if node.tag == "img" { continue; }
        body.push_str(&node.inner);
        open.push(node.tag);
    }
    while let Some(tag) = open.pop() { let _ = write!(body, "</{tag}>"); }
    (body, css)
}

/// An image's `sizes`: the share of the viewport it took at each width, widest first, so the
/// browser picks a `srcset` file for the width it shows at.
fn sizes(node: &ExportNode, widths: &[(f32, Vec<ExportNode>)], by_path: &[HashMap<&str, &ExportNode>]) -> String {
    let vw = |k: usize| by_path[k].get(node.path.as_str()).map(|n| (n.width / widths[k].0 * 100.0).ceil());
    let mut parts: Vec<String> = Vec::new();
    for k in (1..widths.len()).rev() {
        if let Some(v) = vw(k) { parts.push(format!("(min-width:{}px) {v}vw", widths[k].0)); }
    }
    parts.push(format!("{}vw", vw(0).unwrap_or(100.0)));
    format!(" sizes=\"{}\"", parts.join(","))
}

fn base36(mut n: usize) -> String {
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(digits[n % 36]);
        n /= 36;
        if n == 0 { break; }
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}
