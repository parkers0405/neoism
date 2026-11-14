//! `--html`: the app's page as static HTML and CSS, laid out without a GPU, for a web app's
//! first paint and for readers that never run its wasm.

use super::*;
use crate::runtime::web_export::{ExportNode, page};

/// How long the scene may animate before it is taken as it stands, in frames at 60 Hz.
const SETTLE_FRAMES: usize = 240;

pub(crate) fn export<S: 'static>(app: crate::app::App<S>, args: &Args) -> Result<(), String> {
    let out = args.html.clone().expect("--html");
    let mut widths = if args.html_widths.is_empty() { vec![args.size.unwrap_or(app.spec.size).0] } else { args.html_widths.clone() };
    widths.sort_by(f32::total_cmp);
    widths.dedup();
    let height = args.size.unwrap_or(app.spec.size).1;
    let scale = args.scale.unwrap_or(2.0);
    let (fit_height, document) = (app.fit_content_height, app.web_document);
    let template = match &args.html_template {
        Some(path) => std::fs::read_to_string(path).map_err(|e| format!("--html-template {}: {e}", path.display()))?,
        None => DEFAULT_TEMPLATE.to_owned(),
    };
    let (spec, state, _shared) = app.into_parts(args.scene.as_deref());
    let dark = args.theme.as_deref() != Some("light");
    let mut rt = runtime_for(&spec, state, (widths[0], height), Env::new(dark, scale), std::sync::Arc::new(|| {}));
    rt.fit_height = fit_height;
    if document { rt.page_mode(); }
    let mut taken: Vec<(f32, Vec<ExportNode>)> = Vec::with_capacity(widths.len());
    for &w in &widths {
        rt.set_viewport((w, height), scale);
        settle(&mut rt);
        taken.push((w, rt.export_nodes()));
    }
    let (body, css) = page(&taken);
    std::fs::write(&out, fill(&template, &css, &body)).map_err(|e| format!("--html {}: {e}", out.display()))?;
    eprintln!("rune: wrote {} ({} elements, widths {:?})", out.display(), taken[0].1.len(), widths);
    Ok(())
}

/// Lands entrance motion and anything the first frames start, then steps until the scene is at
/// rest or `SETTLE_FRAMES` pass (a looping animation is taken where it is).
fn settle<S: 'static>(rt: &mut Runtime<S>) {
    rt.settle_all();
    rt.frame(0.0);
    for _ in 0..SETTLE_FRAMES {
        if !rt.animating && !rt.stale() { break; }
        rt.frame(1.0 / 60.0);
    }
}

/// Puts the stylesheet and the body into `template` at `<!-- rune:css -->` and
/// `<!-- rune:body -->`; without those markers, before `</head>` and after `<body…>`.
pub(crate) fn fill(template: &str, css: &str, body: &str) -> String {
    let style = format!("<style data-rune-static>{css}</style>");
    let with_css = if template.contains(CSS_MARK) {
        template.replacen(CSS_MARK, &style, 1)
    } else if let Some(at) = template.find("</head>") {
        format!("{}{style}{}", &template[..at], &template[at..])
    } else {
        format!("{style}{template}")
    };
    if with_css.contains(BODY_MARK) { return with_css.replacen(BODY_MARK, body, 1); }
    let at = with_css.find("<body").and_then(|b| with_css[b..].find('>').map(|e| b + e + 1));
    match at {
        Some(at) => format!("{}{body}{}", &with_css[..at], &with_css[at..]),
        None => format!("{with_css}{body}"),
    }
}

const CSS_MARK: &str = "<!-- rune:css -->";
const BODY_MARK: &str = "<!-- rune:body -->";

const DEFAULT_TEMPLATE: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><style>:root{--rune-sans:system-ui,sans-serif;--rune-mono:ui-monospace,monospace}body{margin:0}</style><!-- rune:css --></head><body><!-- rune:body --></body></html>";

#[cfg(test)]
#[path = "html_tests.rs"]
mod tests;
