enum InlineMarkKind {
    Bold,
    Italic,
    Code,
    Strike,
}

#[derive(Clone, Debug, PartialEq)]
enum InlineRunStyle {
    Normal,
    Bold,
    Italic,
    Code,
    Strike,
    Link(String),
    Tag,
    Illuminated(IlluminatedToken),
}

#[derive(Clone, Debug, PartialEq)]
struct InlineRun {
    text: String,
    style: InlineRunStyle,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct InlineWord {
    text: String,
    runs: Vec<InlineRun>,
    /// Number of literal whitespace chars that preceded this word in the
    /// source. Used to preserve multiple/interior spaces while typing
    /// instead of collapsing every run to a single space (browser-style).
    /// Applied even when the word starts a wrapped visual line. Markdown edit
    /// mode must render real source spaces; otherwise the caret/source can
    /// advance while following text appears frozen.
    lead_ws: usize,
}

#[derive(Clone, Debug, Default)]
struct InlineWrappedLine {
    text: String,
    words: Vec<InlineWord>,
    visible_start: usize,
    x_offset: f32,
    row_width: f32,
}

impl InlineWrappedLine {
    fn visible_len(&self) -> usize {
        self.text.chars().count()
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_inline_unwrapped_text(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    raw: &str,
    x: f32,
    y: f32,
    opts: &DrawOpts,
    theme: &IdeTheme,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    occlusions: &[[f32; 4]],
) {
    let words = inline_words_for_text(raw);
    let mut line = InlineWrappedLine::default();
    for word in words {
        push_word_to_line(&mut line, word);
    }
    draw_inline_wrapped_lines(
        sugarloaf,
        pane,
        &[line],
        x,
        y,
        0.0,
        opts,
        theme,
        clip,
        clip_top,
        clip_bottom,
        occlusions,
    );
}

fn inline_wrapped_lines(
    sugarloaf: &mut Sugarloaf,
    raw: &str,
    max_w: f32,
    opts: &DrawOpts,
) -> Vec<InlineWrappedLine> {
    wrap_inline_words(sugarloaf, inline_words_for_text(raw), max_w, 0.0, opts)
}

fn inline_wrapped_lines_dropcap(
    sugarloaf: &mut Sugarloaf,
    raw: &str,
    max_w: f32,
    opts: &DrawOpts,
) -> (Vec<InlineWrappedLine>, f32) {
    let words = inline_words_for_text(raw);
    let (drop_rows, drop_offset) = inline_dropcap_wrap(words.first(), sugarloaf, opts);
    (
        wrap_inline_words_with_dropcap(
            sugarloaf,
            words,
            max_w,
            0.0,
            drop_rows,
            drop_offset,
            opts,
        ),
        drop_offset,
    )
}

/// Wrap a line WITHOUT interpreting inline markup — every character (including
/// `**`, `` ` ``, `[`, `]`, `~~`, `#`) is a literal `Normal`-styled glyph.
/// Used for the cursor's own line under Obsidian-style Live Preview so the drawn
/// text equals the buffer and the caret maps 1:1 (paired with an identity
/// `InlineSourceMap`). `hang_px` is the hanging indent for wrapped continuation
/// rows (the width of the line's indent + list-marker prefix), so a revealed
/// `- [ ]` item keeps its body column across wraps like the rendered view.
fn inline_wrapped_lines_raw(
    sugarloaf: &mut Sugarloaf,
    raw: &str,
    max_w: f32,
    hang_px: f32,
    opts: &DrawOpts,
) -> Vec<InlineWrappedLine> {
    wrap_inline_words(
        sugarloaf,
        plain_inline_words_for_text(raw),
        max_w,
        hang_px,
        opts,
    )
}

/// Width of the raw indent + list-marker prefix as it is actually drawn on the
/// revealed cursor line (each whitespace char renders as one space). This is
/// the hanging indent wrapped continuation rows are shifted by.
fn raw_line_hang_px(sugarloaf: &mut Sugarloaf, raw: &str, opts: &DrawOpts) -> f32 {
    let Some(marker) = parse_markdown_list_marker(raw) else {
        return 0.0;
    };
    let marker_len = marker.marker_len.min(raw.len());
    let prefix: String = raw[..marker_len]
        .chars()
        .map(|ch| if ch.is_whitespace() { ' ' } else { ch })
        .collect();
    sugarloaf.text_mut().measure(&prefix, opts)
}

fn plain_inline_words_for_text(raw: &str) -> Vec<InlineWord> {
    let mut words = Vec::new();
    let mut current = InlineWord::default();
    let mut pending_ws = 0usize;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            if !current.text.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            pending_ws += 1;
        } else {
            if current.text.is_empty() {
                current.lead_ws = pending_ws;
                pending_ws = 0;
            }
            push_char_to_word(&mut current, InlineRunStyle::Normal, ch);
        }
    }
    if !current.text.is_empty() {
        words.push(current);
    } else if pending_ws > 0 {
        // Trailing spaces must survive the wrap: a just-typed space at
        // the end of the line otherwise vanishes from the wrap rows and
        // the caret can't advance past it until the next char lands.
        words.push(InlineWord {
            lead_ws: pending_ws,
            ..InlineWord::default()
        });
    }
    words
}

fn inline_wrap_rows(lines: &[InlineWrappedLine]) -> Vec<MarkdownWrapRow> {
    lines
        .iter()
        .map(|line| MarkdownWrapRow {
            start: line.visible_start,
            len: line.visible_len(),
        })
        .collect()
}

fn inline_visual_row_count(lines: &[InlineWrappedLine]) -> usize {
    lines
        .len()
        .max(lines.iter().map(inline_line_occupied_rows).max().unwrap_or(1))
        .max(1)
}

fn inline_line_occupied_rows(line: &InlineWrappedLine) -> usize {
    line.words
        .iter()
        .flat_map(|word| word.runs.iter())
        .filter_map(|run| match &run.style {
            InlineRunStyle::Illuminated(token) if token.lines > 1.25 => {
                Some(token.lines.ceil().clamp(1.0, 8.0) as usize)
            }
            _ => None,
        })
        .max()
        .unwrap_or(1)
}

fn measured_inline_wrap_hit_rows(
    sugarloaf: &mut Sugarloaf,
    lines: &[InlineWrappedLine],
    hang_px: f32,
    opts: &DrawOpts,
) -> Vec<MarkdownWrapHitRow> {
    lines
        .iter()
        .enumerate()
        .map(|(ix, line)| {
            let mut stops = measured_inline_stops_for_line(sugarloaf, line, opts);
            // Continuation rows draw at x + hang_px; bake the shift into the
            // stops so pointer hit-testing matches the drawn glyphs.
            let x_offset = line.x_offset + if ix > 0 { hang_px } else { 0.0 };
            if x_offset > 0.0 {
                for stop in &mut stops {
                    *stop += x_offset;
                }
            }
            MarkdownWrapHitRow {
                start: line.visible_start,
                stops,
            }
        })
        .collect()
}

fn measured_inline_stops_for_line(
    sugarloaf: &mut Sugarloaf,
    line: &InlineWrappedLine,
    opts: &DrawOpts,
) -> Vec<f32> {
    measured_stops_for_text(sugarloaf, &line.text, opts)
}

fn measured_stops_for_text(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    opts: &DrawOpts,
) -> Vec<f32> {
    let mut stops = Vec::with_capacity(text.chars().count().saturating_add(1));
    let mut prefix = String::new();
    stops.push(0.0);
    for ch in text.chars() {
        prefix.push(ch);
        stops.push(sugarloaf.text_mut().measure(&prefix, opts));
    }
    stops
}

#[allow(clippy::too_many_arguments)]
fn draw_inline_wrapped_lines(
    sugarloaf: &mut Sugarloaf,
    pane: &mut MarkdownPane,
    lines: &[InlineWrappedLine],
    x: f32,
    y: f32,
    hang_px: f32,
    opts: &DrawOpts,
    theme: &IdeTheme,
    clip: [f32; 4],
    clip_top: f32,
    clip_bottom: f32,
    occlusions: &[[f32; 4]],
) {
    let line_h = line_height(opts);
    let space_w = sugarloaf.text_mut().measure(" ", opts);
    for (visual_ix, line) in lines.iter().enumerate() {
        let line_y = y + visual_ix as f32 * line_h;
        let mut run_x = x + line.x_offset + if visual_ix > 0 { hang_px } else { 0.0 };
        // Link target of the last-drawn run, so a link that spans several
        // words underlines the inter-word gaps too — one continuous
        // underline instead of one dash per word.
        let mut prev_link_target: Option<&str> = None;
        for (word_ix, word) in line.words.iter().enumerate() {
            let lead_ws = if word_ix == 0 {
                word.lead_ws
            } else {
                word.lead_ws.max(1)
            };
            if lead_ws > 0 {
                let gap = space_w * lead_ws as f32;
                // When the same link spans this word gap, underline the
                // gap so the link reads as one continuous underline.
                let word_link = word
                    .runs
                    .iter()
                    .find(|r| !r.text.is_empty())
                    .and_then(|r| match &r.style {
                        InlineRunStyle::Link(inner) => Some(inner.as_str()),
                        _ => None,
                    });
                if let (Some(prev), Some(cur)) = (prev_link_target, word_link) {
                    if prev == cur {
                        draw_rect_clipped(
                            sugarloaf,
                            clip,
                            run_x,
                            line_y + line_h - 3.0,
                            gap,
                            1.4,
                            theme.f32_alpha(theme.blue, 0.92),
                            DEPTH,
                            ORDER_TEXT + 1,
                        );
                    }
                }
                run_x += gap;
            }
            for run in &word.runs {
                if run.text.is_empty() {
                    continue;
                }
                if let InlineRunStyle::Illuminated(token) = &run.style {
                    let run_w = draw_illuminated_inline(
                        sugarloaf,
                        token,
                        run_x,
                        line_y,
                        opts,
                        theme,
                        clip,
                        clip_top,
                        clip_bottom,
                        occlusions,
                    );
                    run_x += run_w;
                    continue;
                }
                let mut run_opts = *opts;
                match &run.style {
                    InlineRunStyle::Normal => {}
                    InlineRunStyle::Bold => {
                        run_opts.bold = true;
                    }
                    InlineRunStyle::Italic => {
                        run_opts.italic = true;
                    }
                    InlineRunStyle::Code => {
                        run_opts.color = theme.u8(theme.yellow);
                        // Inline code chips keep the monospace terminal
                        // font even when a pack overrides the markdown
                        // body font. Mirrored in `inline_word_width` so
                        // wrap widths match the drawn advance.
                        run_opts.font_id = None;
                    }
                    InlineRunStyle::Strike => {
                        run_opts.color = theme.u8(theme.muted);
                    }
                    InlineRunStyle::Link(_) => {
                        run_opts.color = theme.u8(theme.blue);
                    }
                    InlineRunStyle::Tag => {
                        run_opts.color = theme.u8(theme.green);
                        run_opts.bold = true;
                    }
                    InlineRunStyle::Illuminated(_) => {}
                }
                let run_w = sugarloaf.text_mut().measure(&run.text, &run_opts);
                let decoration_w = run_w.max(cursor_cell_width(opts));
                if matches!(run.style, InlineRunStyle::Code) {
                    draw_rounded_rect_clipped(
                        sugarloaf,
                        clip,
                        run_x - 4.0,
                        line_y + 1.0,
                        decoration_w + 8.0,
                        line_h - 2.0,
                        4.0,
                        theme.f32_alpha(theme.surface, 0.82),
                        DEPTH,
                        ORDER_BG + 2,
                    );
                }
                draw_if_visible(
                    sugarloaf,
                    run_x,
                    line_y,
                    &run.text,
                    &run_opts,
                    clip_top,
                    clip_bottom,
                    occlusions,
                );
                match &run.style {
                    InlineRunStyle::Strike => {
                        draw_rect_clipped(
                            sugarloaf,
                            clip,
                            run_x,
                            line_y + line_h * 0.56,
                            decoration_w,
                            1.4,
                            theme.f32_alpha(theme.muted, 0.86),
                            DEPTH,
                            ORDER_TEXT + 2,
                        );
                    }
                    InlineRunStyle::Link(inner) => {
                        draw_rect_clipped(
                            sugarloaf,
                            clip,
                            run_x,
                            line_y + line_h - 3.0,
                            decoration_w,
                            1.4,
                            theme.f32_alpha(theme.blue, 0.92),
                            DEPTH,
                            ORDER_TEXT + 1,
                        );
                        if let Some(target) = pane.resolve_markdown_link(inner) {
                            pane.register_link_rect(
                                [run_x, line_y, decoration_w, line_h],
                                target,
                            );
                        }
                    }
                    InlineRunStyle::Tag => {
                        draw_rect_clipped(
                            sugarloaf,
                            clip,
                            run_x,
                            line_y + line_h - 3.0,
                            decoration_w,
                            1.2,
                            theme.f32_alpha(theme.green, 0.75),
                            DEPTH,
                            ORDER_TEXT + 1,
                        );
                    }
                    InlineRunStyle::Normal
                    | InlineRunStyle::Bold
                    | InlineRunStyle::Italic
                    | InlineRunStyle::Code
                    | InlineRunStyle::Illuminated(_) => {}
                }
                run_x += run_w;
                prev_link_target = match &run.style {
                    InlineRunStyle::Link(inner) => Some(inner.as_str()),
                    _ => None,
                };
            }
        }
    }
}

fn wrap_inline_words(
    sugarloaf: &mut Sugarloaf,
    words: Vec<InlineWord>,
    max_w: f32,
    hang_px: f32,
    opts: &DrawOpts,
) -> Vec<InlineWrappedLine> {
    wrap_inline_words_with_dropcap(sugarloaf, words, max_w, hang_px, 0, 0.0, opts)
}

fn wrap_inline_words_with_dropcap(
    sugarloaf: &mut Sugarloaf,
    words: Vec<InlineWord>,
    max_w: f32,
    hang_px: f32,
    drop_rows: usize,
    drop_offset: f32,
    opts: &DrawOpts,
) -> Vec<InlineWrappedLine> {
    // Continuation rows are drawn `hang_px` further right, so they wrap that
    // much sooner; the right edge stays aligned with the first row's.
    let cont_w = (max_w - hang_px).max(24.0);
    let space_w = sugarloaf.text_mut().measure(" ", opts);
    let mut out = Vec::new();
    let mut line = inline_line_for_row(0, max_w, hang_px, drop_rows, drop_offset, 0);
    let mut visible_cursor = 0usize;
    for word in words {
        let row_w = line.row_width.max(8.0);
        let lead_ws = if line.text.is_empty() {
            word.lead_ws
        } else {
            word.lead_ws.max(1)
        };
        let word_w = inline_word_width(sugarloaf, &word, opts, space_w, lead_ws);
        if line.text.is_empty() && word_w > row_w {
            let mut chunks =
                split_inline_word_to_fit(sugarloaf, &word, cont_w.min(row_w), opts);
            if let Some(last) = chunks.pop() {
                for chunk in chunks {
                    let row = line_from_word(
                        chunk,
                        visible_cursor,
                        out.len(),
                        max_w,
                        hang_px,
                        drop_rows,
                        drop_offset,
                    );
                    visible_cursor =
                        visible_cursor.saturating_add(row.text.chars().count());
                    out.push(row);
                }
                line = line_from_word(
                    last,
                    visible_cursor,
                    out.len(),
                    max_w,
                    hang_px,
                    drop_rows,
                    drop_offset,
                );
            }
            continue;
        }
        let line_w = inline_line_width(sugarloaf, &line, opts, space_w);
        if line_w + word_w <= row_w || line.text.is_empty() {
            push_word_to_line(&mut line, word);
            continue;
        }
        visible_cursor = visible_cursor.saturating_add(line.text.chars().count());
        // The separator whitespace stays on the END of the finished row — it's
        // still a real visible char the caret can occupy — so the continuation
        // row starts flush with the body column instead of indented by a stray
        // space (which visually mis-aligned every wrapped row by one space).
        let mut word = word;
        let sep_ws = word.lead_ws.max(1);
        word.lead_ws = 0;
        for _ in 0..sep_ws {
            line.text.push(' ');
        }
        visible_cursor = visible_cursor.saturating_add(sep_ws);
        out.push(line);
        // The just-taken `line` is reset to `default()` (visible_start = 0).
        // The next continuation row begins at the running visible offset —
        // without this, every wrapped row after the first reported
        // `start = 0`, corrupting the wrap-row map that vertical cursor
        // motion AND caret placement consult, so down/up arrow jumped to the
        // wrong visual row (or fell through to the next source line). The
        // oversize branch reassigns `line` via `line_from_word`
        // (which sets visible_start itself), so this only matters for the
        // common `push_word_to_line` continuation below — set it for both.
        line = inline_line_for_row(
            out.len(),
            max_w,
            hang_px,
            drop_rows,
            drop_offset,
            visible_cursor,
        );
        let bare_w = inline_word_width(sugarloaf, &word, opts, space_w, 0);
        let row_w = line.row_width.max(8.0);
        if bare_w > row_w {
            let mut chunks = split_inline_word_to_fit(sugarloaf, &word, row_w, opts);
            if let Some(last) = chunks.pop() {
                for chunk in chunks {
                    let row = line_from_word(
                        chunk,
                        visible_cursor,
                        out.len(),
                        max_w,
                        hang_px,
                        drop_rows,
                        drop_offset,
                    );
                    visible_cursor =
                        visible_cursor.saturating_add(row.text.chars().count());
                    out.push(row);
                }
                line = line_from_word(
                    last,
                    visible_cursor,
                    out.len(),
                    max_w,
                    hang_px,
                    drop_rows,
                    drop_offset,
                );
            }
        } else {
            push_word_to_line(&mut line, word);
        }
    }
    if !line.text.is_empty() {
        out.push(line);
    }
    if out.is_empty() {
        out.push(inline_line_for_row(
            0,
            max_w,
            hang_px,
            drop_rows,
            drop_offset,
            0,
        ));
    }
    out
}

fn inline_line_for_row(
    row_ix: usize,
    max_w: f32,
    hang_px: f32,
    drop_rows: usize,
    drop_offset: f32,
    visible_start: usize,
) -> InlineWrappedLine {
    let x_offset = if row_ix > 0 && row_ix < drop_rows {
        drop_offset
    } else {
        0.0
    };
    let hang_offset = if row_ix > 0 { hang_px } else { 0.0 };
    InlineWrappedLine {
        visible_start,
        x_offset,
        row_width: (max_w - x_offset - hang_offset).max(8.0),
        ..InlineWrappedLine::default()
    }
}

fn inline_dropcap_wrap(
    first_word: Option<&InlineWord>,
    sugarloaf: &mut Sugarloaf,
    opts: &DrawOpts,
) -> (usize, f32) {
    let Some(first_word) = first_word else {
        return (0, 0.0);
    };
    let Some(first_run) = first_word.runs.first() else {
        return (0, 0.0);
    };
    let InlineRunStyle::Illuminated(token) = &first_run.style else {
        return (0, 0.0);
    };
    if token.lines <= 1.25 {
        return (0, 0.0);
    }
    let metrics = illuminated_inline_metrics(sugarloaf, token, opts);
    let rows = token.lines.ceil().clamp(1.0, 8.0) as usize;
    (
        rows,
        metrics.width + sugarloaf.text_mut().measure(" ", opts).max(6.0),
    )
}

fn split_inline_word_to_fit(
    sugarloaf: &mut Sugarloaf,
    word: &InlineWord,
    max_w: f32,
    opts: &DrawOpts,
) -> Vec<InlineWord> {
    let mut out = Vec::new();
    let mut chunk = InlineWord {
        lead_ws: word.lead_ws,
        ..InlineWord::default()
    };
    let space_w = sugarloaf.text_mut().measure(" ", opts);
    for run in &word.runs {
        for ch in run.text.chars() {
            let mut candidate = chunk.clone();
            push_char_to_word(&mut candidate, run.style.clone(), ch);
            if !chunk.text.is_empty()
                && inline_word_width(
                    sugarloaf,
                    &candidate,
                    opts,
                    space_w,
                    candidate.lead_ws,
                ) > max_w
            {
                out.push(std::mem::take(&mut chunk));
                chunk.lead_ws = 0;
            }
            push_char_to_word(&mut chunk, run.style.clone(), ch);
        }
    }
    if !chunk.text.is_empty() {
        out.push(chunk);
    }
    out
}

fn inline_line_width(
    sugarloaf: &mut Sugarloaf,
    line: &InlineWrappedLine,
    opts: &DrawOpts,
    space_w: f32,
) -> f32 {
    let mut width = 0.0;
    for (word_ix, word) in line.words.iter().enumerate() {
        let lead_ws = if word_ix == 0 {
            word.lead_ws
        } else {
            word.lead_ws.max(1)
        };
        width += inline_word_width(sugarloaf, word, opts, space_w, lead_ws);
    }
    width
}

fn inline_word_width(
    sugarloaf: &mut Sugarloaf,
    word: &InlineWord,
    opts: &DrawOpts,
    space_w: f32,
    lead_ws: usize,
) -> f32 {
    let mut width = space_w * lead_ws as f32;
    for run in &word.runs {
        width += match &run.style {
            InlineRunStyle::Illuminated(token) => {
                illuminated_inline_metrics(sugarloaf, token, opts).width
            }
            // Inline code chips draw in the monospace terminal font
            // (font_id = None) regardless of the pack's markdown font;
            // measure them the same way so wrapping matches the drawn
            // advance. No-op when no override is set.
            InlineRunStyle::Code if opts.font_id.is_some() => {
                let code_opts = DrawOpts {
                    font_id: None,
                    ..*opts
                };
                sugarloaf.text_mut().measure(&run.text, &code_opts)
            }
            _ => sugarloaf.text_mut().measure(&run.text, opts),
        };
    }
    width
}

fn line_from_word(
    word: InlineWord,
    visible_start: usize,
    row_ix: usize,
    max_w: f32,
    hang_px: f32,
    drop_rows: usize,
    drop_offset: f32,
) -> InlineWrappedLine {
    let mut line =
        inline_line_for_row(row_ix, max_w, hang_px, drop_rows, drop_offset, visible_start);
    push_word_to_line(&mut line, word);
    line
}

fn push_word_to_line(line: &mut InlineWrappedLine, word: InlineWord) {
    let lead_ws = if line.text.is_empty() {
        word.lead_ws
    } else {
        word.lead_ws.max(1)
    };
    for _ in 0..lead_ws {
        line.text.push(' ');
    }
    line.text.push_str(&word.text);
    line.words.push(word);
}

fn inline_words_for_text(raw: &str) -> Vec<InlineWord> {
    let mut words = Vec::new();
    let mut current = InlineWord::default();
    // Count whitespace runs between words so multiple/interior spaces are
    // preserved (see `InlineWord::lead_ws`).
    let mut pending_ws = 0usize;
    for run in inline_runs_for_text(raw) {
        for ch in run.text.chars() {
            if ch.is_whitespace() {
                if !current.text.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
                pending_ws += 1;
            } else {
                if current.text.is_empty() {
                    current.lead_ws = pending_ws;
                    pending_ws = 0;
                }
                push_char_to_word(&mut current, run.style.clone(), ch);
            }
        }
    }
    if !current.text.is_empty() {
        words.push(current);
    } else if pending_ws > 0 {
        // Keep trailing spaces in the wrap rows (see
        // `plain_inline_words_for_text`) so the caret tracks a
        // just-typed space at the end of the line.
        words.push(InlineWord {
            lead_ws: pending_ws,
            ..InlineWord::default()
        });
    }
    words
}

fn inline_runs_for_text(raw: &str) -> Vec<InlineRun> {
    let text = strip_html_comments_for_inline(raw);
    let mut runs = Vec::new();
    let mut ix = 0usize;
    while ix < text.len() {
        let rest = &text[ix..];
        if let Some(token) = parse_illuminate_token(rest) {
            let letter = token.letter.to_string();
            let source_len = token.source_len;
            push_inline_run(&mut runs, InlineRunStyle::Illuminated(token), &letter);
            ix += source_len;
            continue;
        }
        if rest.starts_with("[[") {
            let inner_start = ix + 2;
            if let Some(end_rel) = text[inner_start..].find("]]") {
                let inner_end = inner_start + end_rel;
                let raw_end = inner_end + 2;
                if let Some(inner) = text.get(inner_start..inner_end) {
                    if let Some(label) = markdown_link_label(inner) {
                        push_inline_run(
                            &mut runs,
                            InlineRunStyle::Link(inner.to_string()),
                            &label,
                        );
                        ix = raw_end;
                        continue;
                    }
                }
            }
        }
        if let Some((open, close, kind)) = inline_marker_at(rest) {
            let inner_start = ix + open.len();
            if let Some(close_rel) = text[inner_start..].find(close) {
                let inner_end = inner_start + close_rel;
                if inner_end > inner_start {
                    if let Some(inner) = text.get(inner_start..inner_end) {
                        let visible = clean_inline_with_active_link(inner, None);
                        push_inline_run(&mut runs, style_for_mark(kind), &visible);
                        ix = inner_end + close.len();
                        continue;
                    }
                }
            }
        }
        if let Some(tag_end) = inline_tag_end(&text, ix) {
            if let Some(tag) = text.get(ix..tag_end) {
                push_inline_run(&mut runs, InlineRunStyle::Tag, tag);
                ix = tag_end;
                continue;
            }
        }
        let Some(ch) = rest.chars().next() else {
            break;
        };
        push_inline_char(&mut runs, InlineRunStyle::Normal, ch);
        ix += ch.len_utf8();
    }
    runs
}

fn push_char_to_word(word: &mut InlineWord, style: InlineRunStyle, ch: char) {
    word.text.push(ch);
    push_inline_char(&mut word.runs, style, ch);
}

fn push_inline_char(runs: &mut Vec<InlineRun>, style: InlineRunStyle, ch: char) {
    if matches!(style, InlineRunStyle::Illuminated(_)) {
        runs.push(InlineRun {
            text: ch.to_string(),
            style,
        });
        return;
    }
    if let Some(last) = runs.last_mut() {
        if last.style == style {
            last.text.push(ch);
            return;
        }
    }
    runs.push(InlineRun {
        text: ch.to_string(),
        style,
    });
}

fn push_inline_run(runs: &mut Vec<InlineRun>, style: InlineRunStyle, text: &str) {
    if text.is_empty() {
        return;
    }
    if matches!(style, InlineRunStyle::Illuminated(_)) {
        runs.push(InlineRun {
            text: text.to_string(),
            style,
        });
        return;
    }
    if let Some(last) = runs.last_mut() {
        if last.style == style {
            last.text.push_str(text);
            return;
        }
    }
    runs.push(InlineRun {
        text: text.to_string(),
        style,
    });
}

fn style_for_mark(kind: InlineMarkKind) -> InlineRunStyle {
    match kind {
        InlineMarkKind::Bold => InlineRunStyle::Bold,
        InlineMarkKind::Italic => InlineRunStyle::Italic,
        InlineMarkKind::Code => InlineRunStyle::Code,
        InlineMarkKind::Strike => InlineRunStyle::Strike,
    }
}

fn inline_tag_end(text: &str, ix: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(ix) != Some(&b'#') {
        return None;
    }
    let prev_ok = ix == 0
        || text[..ix]
            .chars()
            .next_back()
            .is_none_or(|ch| !is_inline_tag_char(ch));
    let mut end = ix + 1;
    while end < bytes.len() && is_inline_tag_char(bytes[end] as char) {
        end += 1;
    }
    if prev_ok && end > ix + 1 {
        Some(end)
    } else {
        None
    }
}

fn is_inline_tag_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '/')
}

fn strip_html_comments_for_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        let after_start = &rest[start + 4..];
        let Some(end) = after_start.find("-->") else {
            return out;
        };
        rest = &after_start[end + 3..];
    }
    out.push_str(rest);
    out
}

fn inline_marker_at(rest: &str) -> Option<(&'static str, &'static str, InlineMarkKind)> {
    if rest.starts_with("**") {
        Some(("**", "**", InlineMarkKind::Bold))
    } else if rest.starts_with("__") {
        Some(("__", "__", InlineMarkKind::Bold))
    } else if rest.starts_with("~~") {
        Some(("~~", "~~", InlineMarkKind::Strike))
    } else if rest.starts_with('`') {
        Some(("`", "`", InlineMarkKind::Code))
    } else if rest.starts_with('*') {
        Some(("*", "*", InlineMarkKind::Italic))
    } else {
        None
    }
}

fn task_marker_checked(raw: &str) -> bool {
    let Some(open) = raw.find('[') else {
        return false;
    };
    let Some(marker) = raw.get(open + 1..).and_then(|rest| rest.chars().next()) else {
        return false;
    };
    matches!(marker, 'x' | 'X')
}

fn looks_like_inline_table_line(raw: &str) -> bool {
    let trimmed = raw.trim();
    trimmed.starts_with('|')
        && trimmed.ends_with('|')
        && trimmed.matches('|').count() >= 3
}

/// Tree metrics for a list line: the nesting indent (matching the vertical
/// list guides) and the marker slot (checkbox/bullet/number gutter) the body
/// text sits after. Shared by layout, measurement, and drawing so the guides,
/// markers, and wrapped text all line up.
fn list_marker_metrics(marker: &MarkdownListMarker, cell_w: f32) -> (usize, f32, f32) {
    let depth = list_depth_from_indent(marker.indent);
    let indent_px = list_indent_px(depth);
    let marker_slot = match &marker.kind {
        MarkdownListMarkerKind::Task { .. } => 28.0,
        MarkdownListMarkerKind::Bullet(_) => 22.0,
        MarkdownListMarkerKind::Number { width, .. } => {
            cell_w * (*width as f32 + 1.0) + 12.0
        }
        MarkdownListMarkerKind::Letter { label, .. } => {
            cell_w * (label.chars().count() as f32 + 1.0) + 12.0
        }
    };
    (depth, indent_px, marker_slot)
}

fn line_marker_layout<'a>(
    raw: &'a str,
    width: f32,
    opts: &DrawOpts,
) -> (f32, &'a str, usize) {
    let Some(marker) = parse_markdown_list_marker(raw) else {
        return (0.0, raw, 0);
    };
    let marker_len = marker.marker_len.min(raw.len());
    let cell_w = cursor_cell_width(opts).max(1.0);
    let (_, indent_px, marker_slot) = list_marker_metrics(&marker, cell_w);
    let offset = (indent_px + marker_slot).min((width - 24.0).max(0.0));
    let body = raw.get(marker_len..).unwrap_or_default();
    (offset, body, marker_len)
}
