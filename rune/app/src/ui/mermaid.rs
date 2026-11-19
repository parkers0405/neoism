//! Native chart presentation. Parsing, layout and image preparation belong to the worker.
use crate::{app::App, mermaid::Entry, ui::theme::rgb};
use rune::{El, col, el, row, text};
use std::{borrow::Cow, collections::HashMap, hash::{Hash, Hasher}};

pub fn height(entry: &Entry, width: f32, scale: f32) -> f32 {
    match entry {
        Entry::Ready { width: w, height: h, .. } => {
            let (w, h) = (w * scale, h * scale);
            let fit = (width / w.max(1.0)).min(1.0).min(800.0 * scale / h.max(1.0));
            (h * fit).max(1.0)
        }
        _ => 120.0 * scale,
    }
}

pub fn body(app: &App, entry: &Entry, width: f32) -> El {
    let frame = el().key("mermaid-diagram").w(width.max(1.0)).h(height(entry, width.max(1.0), app.ty.scale())).center();
    match entry {
        Entry::Ready { image, .. } => frame.pixels_fit(image.pixels(), rune::SurfaceFit::Fit),
        Entry::Pending => frame.child(text("Rendering diagram...").font(app.ty.small()).color(rgb(app.pal.muted))),
        Entry::Failed(error) => frame.padding(12.0 * app.ty.scale()).child(text(format!("Diagram unavailable: {error}"))
            .font(app.ty.small()).color(rgb(app.pal.red)).wrap().w_full().max_lines(4)),
    }
}

fn source_view(app: &App, source: &str) -> El {
    let end = source.char_indices().nth(4096).map_or(source.len(), |(byte, _)| byte);
    let shown = &source[..end];
    let h = (shown.lines().count().max(1) as f32 * app.ty.body().line_px() + 24.0).min(400.0 * app.ty.scale());
    let mut view = col().key("mermaid-source").w_full().h(h).scroll(rune::ScrollSpec { stick_bottom: false }).padding(12.0)
        .child(text(shown.to_owned()).font(app.ty.body()).color(rgb(app.pal.fg)).selectable(true).wrap().w_full());
    if end < source.len() { view = view.child(text("Preview truncated. Copy retains the complete source.").font(app.ty.small()).color(rgb(app.pal.muted))); }
    view
}

pub fn card(app: &App, id: &str, source: &str, width: f32) -> El {
    let entry = app.mermaid_cache.request(fence_source(source), app.pal, app.waker.clone());
    let raw = app.mermaid_sources.contains(id);
    let id = id.to_owned();
    let control = |label: &'static str| el().h(26.0).px(10.0).radius(6.0).center()
        .cursor(rune::Cursor::Pointer).hover(rgb(app.pal.fg).alpha(0.085))
        .child(text(label).font(app.ty.small()).color(rgb(app.pal.dim)));
    let mut controls = row().center_y().gap(4.0).child(control(if raw { "Diagram" } else { "Source" })
        .key("mermaid-toggle-source").on_press(move |a: &mut App| {
            if !a.mermaid_sources.remove(&id) { a.mermaid_sources.insert(id.clone()); }
            a.mermaid_view_version = a.mermaid_view_version.wrapping_add(1);
        }));
    if let Entry::Ready { image, .. } = &entry {
        let image = image.clone();
        controls = controls.child(control("Open").key("mermaid-open").on_press(move |a: &mut App| a.preview_rendered_diagram(image.clone())));
    }
    controls = controls.child(crate::agent::timeline::copy_button(app.pal, app.ty, source.to_owned()));
    let mut chart = crate::agent::timeline::code_surface(app.pal)
        .child(row().w_full().h(38.0).pl(22.0).pr(12.0).center_y().justify_between().fill(rgb(app.pal.surface).alpha(0.55))
            .child(text("mermaid").font(app.ty.small()).color(rgb(app.pal.yellow))).child(controls));
    if raw { return chart.child(source_view(app, source)); }
    chart = chart.child(body(app, &entry, (width - 24.0).max(1.0)).mx(12.0).my(12.0));
    if matches!(entry, Entry::Failed(_)) { chart = chart.child(source_view(app, source)); }
    chart
}

/// Closed fenced blocks only. Partial streaming syntax stays literal until its closer arrives.
/// The Markdown kit keeps its own code-frame source, so use the same CommonMark text events.
pub fn contains_fence(markdown: &str) -> bool {
    markdown.lines().any(|line| {
        let line = line.trim_start_matches(|c: char| c == '>' || c.is_whitespace());
        let marker = line.as_bytes().first().copied().unwrap_or_default();
        if !matches!(marker, b'`' | b'~') { return false; }
        let n = line.bytes().take_while(|&b| b == marker).count();
        n >= 3 && line[n..].trim_start().split(|c: char| c.is_whitespace() || c == ',').next().is_some_and(|s| s.eq_ignore_ascii_case("mermaid"))
    })
}

pub fn prepare<'a>(app: &App, markdown: &'a str, id: &str, width: f32) -> (Cow<'a, str>, HashMap<usize, El>) {
    if !contains_fence(markdown) { return (Cow::Borrowed(markdown), HashMap::new()); }
    use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
    let mut frames = HashMap::new();
    let mut rendered = Cow::Borrowed(markdown);
    let mut index = 0;
    let mut current: Option<(usize, std::ops::Range<usize>, String)> = None;
    for (event, range) in Parser::new(markdown).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                let word = match &kind { CodeBlockKind::Fenced(info) => info.split(|c: char| c.is_whitespace() || c == ',').next().unwrap_or(""), _ => "" };
                if word.eq_ignore_ascii_case("mermaid") { current = Some((index, range, String::new())); }
                if !word.eq_ignore_ascii_case("diff") && !word.eq_ignore_ascii_case("patch") { index += 1; }
            }
            Event::Text(value) if current.is_some() => current.as_mut().unwrap().2.push_str(&value),
            Event::End(TagEnd::CodeBlock) => {
                let Some((index, range, source)) = current.take() else { continue };
                let fenced = &markdown[range.clone()];
                let first = fenced.lines().next().unwrap_or_default().trim_start();
                let marker = first.as_bytes().first().copied().unwrap_or_default();
                let count = first.bytes().take_while(|b| *b == marker).count();
                let last = fenced.lines().last().unwrap_or_default().trim_start_matches(|c: char| c == '>' || c.is_whitespace()).trim_end();
                if count < 3 || last.len() < count || !last.bytes().all(|b| b == marker) || fenced.lines().count() < 2 { continue; }
                let opening = fenced.lines().next().unwrap_or_default();
                let Some(word) = opening.as_bytes().windows(7).position(|word| word.eq_ignore_ascii_case(b"mermaid")) else { continue };
                // The kit reserves `mermaid` for its synchronous optional engine. Route just
                // this closed fence through its ordinary code callback instead. Equal byte
                // lengths preserve every Markdown source offset and reference definition.
                rendered.to_mut().replace_range(range.start + word..range.start + word + 7, "diagram");
                let mut hash = std::collections::hash_map::DefaultHasher::new();
                source.hash(&mut hash);
                let key = format!("{id}:{:x}", hash.finish());
                frames.insert(index, card(app, &key, &source, width));
            }
            _ => {}
        }
    }
    (rendered, frames)
}

pub const EXAMPLE: &str = "## Native Mermaid\n\nDiagrams render off-thread; the source stays unchanged.\n\n```mermaid\nflowchart LR\n  subgraph App[Native app]\n    Note[Markdown editor] --> Cache{Cached?}\n  end\n  Cache -->|Hit| Pixels[Native image]\n  Cache -->|Miss| Worker[Background worker]\n  Worker --> Cache\n```\n\n```mermaid\nsequenceDiagram\n  participant Reader\n  participant App\n  participant Worker\n  Reader->>App: Open document\n  App->>Worker: Render once\n  App-->>Reader: Keep scrolling\n  Worker-->>App: Cached chart\n```\n\n```mermaid\nstateDiagram-v2\n  [*] --> Queued\n  Queued --> Rendering\n  Rendering --> Cached\n  Cached --> [*]\n```\n";

// CommonMark includes the content line's final EOL; the source editor stores lines
// without it. Share one cache identity across both without changing Copy's text.
fn fence_source(source: &str) -> &str {
    source.strip_suffix('\n').map_or(source, |s| s.strip_suffix('\r').unwrap_or(s))
}

pub fn fixture(app: &mut App, editor: bool) {
    if editor {
        app.open_file("mermaid.md", None);
        app.apply_opened_file("mermaid.md", EXAMPLE.as_bytes());
    } else {
        let messages = serde_json::json!([
            {"id":"mermaid-question", "role":"User", "kind":"User", "text":"Show the native diagram pipeline."},
            {"id":"mermaid-answer", "role":"Assistant", "kind":"Assistant", "text":EXAMPLE}
        ]);
        app.load_fixture("mermaid-fixture", "Native Mermaid", &messages.to_string());
    }
    if std::env::var_os("NEOISM_RUNE_OFFLINE").is_some() {
        // Diagnostic fixtures only: prepare images before the headless measurement begins.
        // Interactive rendering never waits for the worker.
        let sources: Vec<_> = EXAMPLE.split("```mermaid\n").skip(1).map(|s| fence_source(s.split("```").next().unwrap())).collect();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let pending = sources.iter().any(|source| matches!(app.mermaid_cache.request(source, app.pal, None), Entry::Pending));
            app.mermaid_cache.poll();
            if !pending || std::time::Instant::now() >= until { break; }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Key, KeyEvent, Runtime};
    use rune_kit::markdown::Markdown;

    fn runtime(source: &'static str) -> Runtime<App> {
        let mut app = App::new();
        app.open_file("test.md", None);
        app.apply_opened_file("test.md", source.as_bytes());
        app.focus = crate::app::Focus::Sidebar;
        let mut rt = Runtime::new(app, |a, _| {
            let source = a.active_buffer().unwrap().doc.text_for_disk();
            let (source, diagrams) = prepare(a, &source, "test", 600.0);
            let code = crate::agent::timeline::code_frame(a);
            col().w_full().h_full().scroll(rune::ScrollSpec { stick_bottom: false }).child(Markdown::new(source.into_owned())
                .code_frame(move |body, frame| diagrams.get(&frame.index).cloned().unwrap_or_else(|| code(body, frame)))
                .render(&crate::agent::timeline::markdown_style(a)))
        }, (620.0, 600.0), rune::Env::new(true, 1.0));
        for _ in 0..5 { rt.frame(1.0 / 60.0); }
        rt
    }

    #[test]
    fn closed_diagrams_replace_only_their_own_fences_and_leave_links_and_code_intact() {
        let rt = runtime("Before [reference][link].\n\n```rust\nlet mermaid = 1;\n```\n\n```diff\n-old\n+new\n```\n\n~~~~MERMAID title=chart\nflowchart LR\nA-->B\n~~~~\n\n````text\n```mermaid\nliteral example\n```\n````\n\n```mermaid\nsequenceDiagram\nA->>B: hello\n```\n\nAfter.\n\n[link]: https://example.com\n");
        let nodes = rt.elements();
        assert_eq!(nodes.iter().filter(|e| e.key.as_deref() == Some("mermaid-diagram")).count(), 2);
        assert!(nodes.iter().any(|e| e.text.as_ref().is_some_and(|s| s.contains("let mermaid = 1;"))));
        assert!(nodes.iter().any(|e| e.text.as_ref().is_some_and(|s| s.contains("literal example"))));
        assert!(nodes.iter().any(|e| e.text.as_ref().is_some_and(|s| s.contains("After."))));
        assert!(nodes.iter().any(|e| e.text.as_ref().is_some_and(|s| s.contains("reference"))));
        assert!(!nodes.iter().any(|e| e.text.as_deref() == Some("diagram")), "the internal callback alias is not user-facing");
    }

    #[test]
    fn mermaid_agent_pane_stream_completion_pixels_and_exact_copy() {
        use neoism_protocol::agent::{AgentServerMessage, HistoryMessage};
        // Long and fence-first deliberately exercises the assistant's code-slab cutoff.
        let source = format!("flowchart LR\nA[Start]-->B[Done]\n{}", "%% retained comment\n".repeat(160));
        for (kind, lang, tagged) in [("Assistant", "", false), ("Reasoning", "", false), ("Assistant", "markdown", false), ("Assistant", "mermaid", true)] {
            let partial = if tagged { source.clone() } else { format!("```mermaid\n{source}") };
            let message: HistoryMessage = serde_json::from_value(serde_json::json!({
                "id":"chart", "role":"Assistant", "kind":kind, "text":partial,
                "output_kind": if lang.is_empty() { "Text" } else { "Code" }, "lang":lang
            })).unwrap();
            let mut app = App::new();
            app.load_fixture("chart-session", "Chart", &serde_json::to_string(&vec![message.clone()]).unwrap());
            app.focus = crate::app::Focus::Sidebar;
            app.sessions.get_mut("chart-session").unwrap().streaming = true;
            // Live trace rows are deliberately hidden outside a turn.
            app.sessions.get_mut("chart-session").unwrap().live_from = Some(0);
            let mut rt = Runtime::new(app, |a, cx| crate::agent::timeline::pane(a, Some("chart-session"), cx),
                (900.0, 1100.0), rune::Env::new(true, 1.0));
            for _ in 0..60 { rt.frame(1.0 / 60.0); }
            assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("mermaid-diagram")), "partial {kind}/{lang}");
            assert!(rt.state.mermaid_cache.cached(fence_source(&source), rt.state.pal).is_none(), "partial streams must not schedule work");
            let epoch = rt.state.mermaid_cache.epoch();
            rt.update(|a, _| {
                let mut closed = message.clone();
                if !tagged { closed.text.push_str("```\n"); }
                a.apply_agent(AgentServerMessage::MessageUpdated { session_id: "chart-session".into(), message: closed, parent_message_id: None });
                a.sessions.get_mut("chart-session").unwrap().streaming = false;
                if kind == "Reasoning" { a.sessions.get_mut("chart-session").unwrap().live_from = Some(0); }
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            loop {
                rt.update(|a, _| { a.mermaid_cache.poll(); });
                rt.frame(1.0 / 60.0);
                if let Some(Entry::Ready { image, .. }) = rt.state.mermaid_cache.cached(fence_source(&source), rt.state.pal) {
                    assert!(image.is_ready());
                    break;
                }
                assert!(std::time::Instant::now() < deadline, "native chart did not complete: {kind}/{lang}");
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            for _ in 0..60 { rt.frame(1.0 / 60.0); }
            assert!(rt.state.mermaid_cache.epoch() > epoch);
            assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("mermaid-diagram")));
            assert!(!rt.elements().iter().any(|e| e.text.as_deref() == Some("Rendering diagram...")), "cache epoch must invalidate memoized rows");
            let clipboard = rune::clipboard::memory();
            rt.use_clipboard(&clipboard);
            let toggle = rt.elements().iter().find(|e| e.key.as_deref() == Some("mermaid-toggle-source")).unwrap().rect;
            rt.pointer_down(toggle[0] + 3.0, toggle[1] + 3.0);
            rt.pointer_up(toggle[0] + 3.0, toggle[1] + 3.0);
            for _ in 0..60 { rt.frame(1.0 / 60.0); }
            assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("mermaid-source")));
            assert_eq!(rt.state.focus, crate::app::Focus::Sidebar);
            let copy = rt.elements().iter().find(|e| e.text.as_deref() == Some("Copy")).unwrap().rect;
            rt.pointer_down(copy[0] + 3.0, copy[1] + 3.0);
            rt.pointer_up(copy[0] + 3.0, copy[1] + 3.0);
            rt.frame(1.0 / 60.0);
            assert_eq!(clipboard.text().unwrap().as_str(), source);
            assert_eq!(rt.state.focus, crate::app::Focus::Sidebar);
            assert!(!rt.elements().iter().any(|e| e.text.as_deref() == Some("Edit")), "history is read-only");
            let toggle = rt.elements().iter().find(|e| e.key.as_deref() == Some("mermaid-toggle-source")).unwrap().rect;
            rt.pointer_down(toggle[0] + 3.0, toggle[1] + 3.0);
            rt.pointer_up(toggle[0] + 3.0, toggle[1] + 3.0);
            for _ in 0..60 { rt.frame(1.0 / 60.0); }
            assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("mermaid-diagram")));
            assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("mermaid-source")));
            assert_eq!(rt.state.focus, crate::app::Focus::Sidebar);
        }
    }

    #[test]
    fn mermaid_agent_pane_does_not_render_literal_fence_examples() {
        let mut app = App::new();
        let example = "````text\n```mermaid\nflowchart LR\nA-->B\n```\n````\n";
        app.load_fixture("literal", "Literal", &serde_json::json!([
            {"id":"literal", "kind":"Assistant", "role":"Assistant", "text":example}
        ]).to_string());
        let mut rt = Runtime::new(app, |a, cx| crate::agent::timeline::pane(a, Some("literal"), cx),
            (900.0, 1100.0), rune::Env::new(true, 1.0));
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
        assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("mermaid-diagram")));
        assert!(rt.state.mermaid_cache.cached("flowchart LR\nA-->B", rt.state.pal).is_none());
        assert!(rt.elements().iter().any(|e| e.text.as_ref().is_some_and(|s| s.contains("A-->B"))));
    }

    #[test]
    fn incomplete_streams_never_queue_diagrams_and_native_source_copy_does_not_steal_focus() {
        let partial = runtime("```mermaid\nflowchart LR\nA-->");
        assert!(!partial.elements().iter().any(|e| e.key.as_deref() == Some("mermaid-diagram")));
        let mut rt = runtime("```mermaid\nflowchart LR\nA[Start]-->B[Done]\n```\n");
        let clipboard = rune::clipboard::memory();
        rt.use_clipboard(&clipboard);
        let button = rt.elements().iter().find(|e| e.key.as_deref() == Some("mermaid-toggle-source")).unwrap().rect;
        rt.pointer_down(button[0] + 3.0, button[1] + 3.0);
        rt.pointer_up(button[0] + 3.0, button[1] + 3.0);
        for _ in 0..6 { rt.frame(1.0 / 60.0); }
        assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("mermaid-source")));
        assert_eq!(rt.state.focus, crate::app::Focus::Sidebar);
        let copy = rt.elements().iter().find(|e| e.text.as_deref() == Some("Copy")).unwrap().rect;
        rt.pointer_down(copy[0] + 3.0, copy[1] + 3.0);
        rt.pointer_up(copy[0] + 3.0, copy[1] + 3.0);
        rt.frame(1.0 / 60.0);
        assert_eq!(clipboard.text().unwrap().as_str(), "flowchart LR\nA[Start]-->B[Done]\n");
        assert_eq!(rt.state.focus, crate::app::Focus::Sidebar);
        rt.key(KeyEvent { key: Key::Escape, mods: Default::default() });
        rt.frame(1.0 / 60.0);
    }

    #[test]
    #[ignore = "Headless GPU screenshots and frame CPU diagnostic; run explicitly with offline fixtures"]
    fn cached_chart_frame_diagnostic() {
        use rune::render::{Gpu, Offscreen, Renderer, OFFSCREEN_FORMAT};
        for editor in [false, true] {
            let mut app = App::new();
            fixture(&mut app, editor);
            for source in EXAMPLE.split("```mermaid\n").skip(1).map(|s| s.split("```").next().unwrap()) {
                assert!(matches!(app.mermaid_cache.request(fence_source(source), app.pal, None), Entry::Ready { .. }), "diagnostic requires rendered charts, not placeholders");
            }
            let mut rt = Runtime::with_text(app, crate::ui::chrome::root, (1252.0, 1100.0), rune::Env::new(true, 1.0),
                rune::text::TextSystem::new(crate::FONTS, ("Geist", "Geist Mono")));
            for _ in 0..120 { rt.frame(1.0 / 60.0); }
            let epoch = rt.state.mermaid_cache.epoch();
            let mut times = Vec::new();
            for _ in 0..240 {
                assert!(!rt.state.mermaid_cache.poll(), "settled charts do not invalidate a frame");
                let start = std::time::Instant::now();
                rt.frame(1.0 / 120.0);
                times.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            times.sort_by(f64::total_cmp);
            assert_eq!(rt.state.mermaid_cache.epoch(), epoch);
            let name = if editor { "markdown" } else { "chat" };
            eprintln!("Mermaid {name}: warm native frame CPU p50={:.3}ms p95={:.3}ms max={:.3}ms; epoch={epoch}", times[120], times[228], times[239]);
            assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("mermaid-diagram")));
            if let Some(prefix) = std::env::var_os("NEOISM_RUNE_MERMAID_PNG") {
                let gpu = Gpu::headless();
                let target = Offscreen::with_format(&gpu.device, 1252, 1100, OFFSCREEN_FORMAT);
                let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
                renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (1252, 1100));
                renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
                let mut enc = gpu.device.create_command_encoder(&Default::default());
                renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
                target.copy(&mut enc);
                renderer.submit(&gpu.queue, enc);
                let path = format!("{}-{name}.png", prefix.to_string_lossy());
                rune::render::write_png(std::path::Path::new(&path), 1252, 1100, &target.read(&gpu.device)).unwrap();
            }
        }
    }
}