//! Leaf content painting inside a box: vector shapes, images, icons and rich text.
use super::*;

pub(super) struct LeafBox {
    pub r: [f32; 4],
    pub r_exact: [f32; 4],
    pub raw_size: [f32; 2],
    pub radii: [f32; 4],
    pub sc: f32,
    pub o: f32,
    pub draw: f32,
    pub clip: [f32; 4],
    pub fade: [f32; 4],
    pub group: f32,
    pub loop_value: Option<f32>,
    pub cover: Option<(f32, [f32; 2])>,
    pub scale_motion: ScaleMotion,
    /// The scale of the nearest still ancestor, which moving text rasterizes at.
    pub rest: f32,
    pub gliding: bool,
    pub caret: Option<[f32; 4]>,
}

impl<S: 'static> Runtime<S> {
    pub(super) fn paint_content(
        &mut self,
        i: usize,
        b: LeafBox,
        sel_ranges: &[(usize, usize, usize)],
        glyph_count: &mut usize,
    ) {
        let s = self.env.scale;
        let srgb = self.srgb_blend;
        let now = self.t;
        let LeafBox {
            r,
            r_exact,
            raw_size,
            radii,
            sc,
            o,
            draw,
            clip,
            fade,
            group,
            loop_value,
            cover,
            scale_motion,
            rest,
            gliding,
            caret: input_caret,
        } = b;
        let (w, h) = ((r[2] - r[0]).max(0.0), (r[3] - r[1]).max(0.0));
        let (w_exact, h_exact) = ((r_exact[2] - r_exact[0]).max(0.0), (r_exact[3] - r_exact[1]).max(0.0));
        let k_px = sc * s;
        let link_fx = self.nodes[i].text.is_some().then(|| self.link_fx(i));
        let node_id = self.nodes[i].id;
        let text_color = self.nodes[i].vis.icon.map(|(_, c)| c)
            .or_else(|| self.nodes[i].text.as_ref().map(|sp| sp.color))
            .map(|c| self.color(node_id, P_TEXT, c));
        let n = &self.nodes[i];
        let text_tint = |c: Color| match n.vis.group_color {
            Some(g) if group > 0.0 => c.mix(g, group),
            _ => c,
        };
        if let Some(shape) = &n.vis.shape {
            // When pixel snapping adjusts `r`'s width or height from `r_exact`, keep a point-sized
            // shape centred in `r` so it shares `r`'s centre with fills, borders and icons.
            let (origin, k) = match shape.view_box {
                Some([vw, vh]) => ([r[0], r[1]], [w / vw, h / vh]),
                None => ([r[0] + (w - w_exact) * 0.5, r[1] + (h - h_exact) * 0.5], [k_px, k_px]),
            };
            if let Some(t) = self.scene.get(node_id, P_TRIM) {
                // A presence draw traces the trimmed span on from its start.
                let end = t.x + (t.y - t.x) * draw;
                push_shape(&mut self.instances, &shape.trimmed(t.x, end), origin, k, k_px, o, srgb, clip, fade);
            } else if draw < 1.0 {
                push_shape(&mut self.instances, &shape.drawn(draw), origin, k, k_px, o, srgb, clip, fade);
            } else {
                push_shape(&mut self.instances, shape, origin, k, k_px, o, srgb, clip, fade);
            }
        }
        if let Some(pixels) = &n.vis.pixels {
            let snap_u = if cover.is_none() { (1.0 - (sc - 1.0).abs() * 32.0).clamp(0.0, 1.0) } else { 0.0 };
            let (rect, crop) = crate::pixels::fit_in(n.vis.pixels_fit, n.vis.surface_place, [r[0], r[1], w, h], pixels.size(), snap_u);
            let span = [crop[2] - crop[0], crop[3] - crop[1]];
            // The mip covers the whole image's drawn size, not just its visible window.
            let placed = self.images.place_at(pixels, [rect[2] / span[0], rect[3] / span[1]], Some(now));
            // An animated image wakes the window for its next frame only while drawn in a shown
            // window: off screen it isn't painted, so it asks for nothing.
            if let Some((_, Some(at))) = placed && self.surface_visible && self.time_scale > 0.0 {
                let wake = self.clock + (at - now).max(0.0) / self.time_scale as f64;
                if self.loop_wake.is_none_or(|w| wake < w.0) { self.loop_wake = Some((wake, at)); }
            }
            if let Some((image, _)) = placed {
                let [x, y, iw, ih] = image.rect.map(|v| v as f32);
                // A letterboxed image rounds its own, smaller corners.
                let half = rect[2].min(rect[3]) * 0.5;
                self.instances.push(Instance {
                    rect,
                    radii: if n.vis.pixels_fit.is_some() { radii.map(|v| v.min(half)) } else { radii },
                    fill: [0.0, 0.0, 0.0, o],
                    border: [0.0; 4],
                    params: [image.page as f32, KIND_IMAGE, image.premultiplied as u8 as f32, 1.0],
                    // The shader samples from a whole-texel origin; the span may be fractional.
                    uv: [x + (crop[0] * iw).round(), y + (crop[1] * ih).round(), span[0] * iw, span[1] * ih],
                    clip,
                    fade: crate::render::pack_fade(fade, clip, clip),
                });
            }
        }
        if let Some((ic, _)) = n.vis.icon {
            let color = text_tint(text_color.unwrap());
            let looped = if matches!(n.looping.as_ref().map(|l| l.kind), Some(rune_motion::LoopKind::Rotation)) { loop_value.unwrap_or(0.0) } else { 0.0 };
            let turn = looped + if self.scene.store().has(n.id, P_ICON_TURN) {
                self.scene.get(n.id, P_ICON_TURN).unwrap_or(n.vis.icon_turn)
            } else { n.vis.icon_turn };
            // `El::mirror_in_rtl`: the drawing mirrors (its turn already runs the other way).
            let facing = match (n.flow, n.vis.mirror_rtl) {
                (crate::el::Flow::Rtl, true) => crate::text::Facing::Mirrored,
                _ => crate::text::Facing::Upright,
            };
            let tk = if cover.is_some() { 1.0 } else { sc };
            let raster = if cover.is_some() { 1.0 } else { scale_motion.raster(sc, rest) };
            if cover.is_none() && scale_motion == ScaleMotion::Live {
                self.static_scale_settling = true;
            }
            let zoom = tk / raster.max(1e-4);
            let exact_side = w_exact.min(h_exact);
            let raw_side = raw_size[0].min(raw_size[1]).max(0.0) * s;
            let snap_u = if cover.is_none() && (raw_side - raw_side.round()).abs() < 1.0 / 512.0 {
                (1.0 - (sc - 1.0).abs() * 32.0).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let drawn = exact_side + (w.min(h) - exact_side) * snap_u;
            let px = if cover.is_none() && scale_motion != ScaleMotion::Still {
                (raw_side * raster).round().max(1.0) as u32
            } else {
                (drawn / zoom.max(1e-4)).round().max(1.0) as u32
            };
            let pos = [(r[0] + r[2] - drawn) * 0.5, (r[1] + r[3] - drawn) * 0.5];
            // An icon that paints its own colours draws as its image (or its silhouette in
            // `color`, `El::icon_one_color`), upright, faded only by the
            // element's opacity and its colour's alpha.
            if drawn > 1e-4 && let Some(pixels) = self.text.color_icon(ic, px, color, n.vis.icon_one_color) {
                if let Some(image) = self.images.place(pixels, [drawn, drawn]) {
                    self.instances.push(Instance {
                        rect: [pos[0], pos[1], drawn, drawn],
                        radii: [0.0; 4],
                        fill: [0.0, 0.0, 0.0, premul(color, o, srgb)[3]],
                        border: [0.0; 4],
                        params: [image.page as f32, KIND_IMAGE, image.premultiplied as u8 as f32, 1.0],
                        uv: image.rect.map(|v| v as f32),
                        clip,
                        fade: crate::render::pack_fade(fade, clip, clip),
                    });
                }
            } else if drawn > 1e-4 && let Some(q) = self.text.icon_quad_facing(ic, px, pos, facing) {
                self.push_icon(icon_inst(&q, pos, drawn, turn, premul(color, o, srgb), clip, fade));
            }
        } else if let (Some(spec), Some(layout)) = (&n.text, &n.text_layout) {
            let link_fx = link_fx.unwrap();
            let color = text_tint(text_color.unwrap());
            // A matched flight places its own text in its scaled content box. Its glyphs keep
            // their rest size so a label doesn't pop at the swap.
            let (zk, zr) = match cover {
                Some((k, c)) => (k, [r[0] + c[0] * s, r[1] + c[1] * s,
                    r[0] + (c[0] + n.local.w * k) * s, r[1] + (c[1] + n.local.h * k) * s]),
                None => (sc, r),
            };
            // Everywhere else text scales with its box (`El::scale`, `live_scale`,
            // `press_scale`, transitions): `tk` about the text's origin. It rasterizes at
            // `raster` and draws filtered by the rest, `tk / raster`.
            let tk = if cover.is_some() { 1.0 } else { sc };
            let raster = if cover.is_some() { 1.0 } else { scale_motion.raster(sc, rest) };
            if cover.is_none() && scale_motion == ScaleMotion::Live {
                self.static_scale_settling = true;
            }
            // Text whose scale moves draws from its outlines, magnified to `tk`: crisp all the way,
            // and the same outline on the frame it lands as the atlas mask it rests as.
            let place_masks = self.text.begin_text(node_id.0, cover.is_none() && scale_motion != ScaleMotion::Still)
                .into_iter().any(|e| e == crate::text::outline::PathEffect::PlaceMasks);
            let direct = self.text.emit() == crate::text::outline::Emit::Outline;
            let ins = n.inset.map(|v| v * zk * s);
            let (x0, x1) = (zr[0] + ins[0], zr[2] - ins[2]);
            let mut origin = [text_x(spec, x0, x1, (layout.full_width() * tk).ceil(), crate::el::Flow::of_layout(layout)), zr[1] + ins[1]];
            // A gliding box's glyphs and decorations lay out from its nearest whole pixel and move the rest.
            // Within a 64th of a pixel they don't move, but still lay out from that pixel, as at rest.
            let (glide, off) = if gliding && cover.is_none() && scale_motion == ScaleMotion::Still {
                ([pixel_snap::text_glide(r[0]), pixel_snap::text_glide(r[1])], [r[0] - r[0].round(), r[1] - r[1].round()])
            } else {
                ([0.0; 2], [0.0; 2])
            };
            let mut clip = clip;
            if let Some(inp) = n.input.as_ref().filter(|i| i.single_line) {
                let scroll = single_line_scroll(&mut self.input_scroll, &n.path, inp, &spec.content, layout, s, (x1 - x0) / tk.max(1e-4)) * tk;
                origin[0] -= scroll;
                if layout.full_width() * tk > x1 - x0 {
                    clip = [clip[0].max(x0), clip[1], clip[2].min(x1), clip[3]];
                }
            }
            let rest_origin = [origin[0] - off[0], origin[1] - off[1]];
            // Paragraph block backgrounds, quote bars and list markers.
            if spec.blocks.iter().any(|&b| b != crate::edit::Block::Paragraph) {
                emit_block_decorations(
                    &mut self.instances,
                    &mut self.text,
                    &mut self.images,
                    &mut self.glyph_scratch,
                    spec,
                    layout,
                    rest_origin,
                    (x0 - glide[0], x1 - glide[0]),
                    s,
                    tk,
                    raster,
                    scale_motion,
                    glide,
                    color,
                    o,
                    srgb,
                    clip,
                    fade,
                );
            }
            // Text fields: selection under the text, placeholder, caret over it.
            let mut caret = None;
            // A field's selected display bytes and the ink its `El::selection_style` gives them.
            let mut field_ink: Option<(usize, usize, [f32; 4])> = None;
            if let Some(inp) = &n.input {
                let e = &inp.edit;
                let departing = self.flights.iter().any(|f| f.departs(&n.path, n.key.as_deref())
                    && f.alphas(f.destination, now, self.reduced_motion).0 > 0.0);
                if e.text.is_empty() && e.preedit().is_none() && !inp.placeholder.is_empty() && !departing {
                    let pl_indent = spec.blocks.first().map_or(0.0, |b| (b.indent_pt(spec.style.size) * s).round() * tk);
                    // A one-line field's placeholder stays on its line, cut by the field's clip, as its
                    // text does; a multi-line field's wraps at its width.
                    let wrap = (!inp.single_line).then(|| (w - pl_indent).max(1.0) / tk.max(1e-4));
                    let options = crate::text::RichLayoutOptions::new().flow(spec.flow);
                    let pl = self.text.layout_rich_with(&inp.placeholder, &[], spec.style, s, wrap, options);
                    // Right to left it ends at the field's start edge, its right.
                    let pl_x = match (spec.flow, wrap) {
                        (crate::el::Flow::Ltr, _) => rest_origin[0] + pl_indent,
                        (crate::el::Flow::Rtl, Some(_)) => rest_origin[0],
                        (crate::el::Flow::Rtl, None) => rest_origin[0] - pl_indent - (pl.full_width() * tk).ceil(),
                    };
                    let pl_origin = [pl_x, rest_origin[1]];
                    let pl_r_origin = if scale_motion == ScaleMotion::Still { pl_origin } else { [0.0, 0.0] };
                    self.glyph_scratch.clear();
                    self.text.set_glyph_inks(inp.placeholder_color, &[]);
                    self.text.set_glide(glide);
                    self.text.glyphs_for_layout_scaled(&pl, pl_r_origin, raster, false, &mut self.glyph_scratch);
                    self.text.set_glide([0.0; 2]);
                    let pc = premul(inp.placeholder_color, o, srgb);
                    for q in &self.glyph_scratch {
                        self.instances.extend(glyph_or_image(q, &self.text, &mut self.images, pc, clip, fade)
                            .map(|g| zoom_glyph(g, pl_origin, pl_r_origin, tk / raster)));
                    }
                }
                if e.focused && !departing {
                    let (a, b) = e.range();
                    if a != b && e.preedit().is_none() {
                        let (da, db) = e.display_selection_range(a, b);
                        // An inactive window paints a field's selection in its inactive pair, as a browser does.
                        let (highlight, ink) = inp.selection_style.map_or((inp.selection, None), |st| st.pick(self.window_focused != Some(false)));
                        selectable::push_field_selection(&mut self.instances, layout, da, db,
                            origin, tk, premul(highlight, o, srgb), s, clip, fade);
                        field_ink = ink.map(|c| (da, db, premul(c, o, srgb)));
                    }
                    // A range selection hides the caret, as a browser's field and a native one do.
                    if a == b || e.is_composing() {
                        caret = input_caret;
                    }
                    if e.is_composing()
                        && let (_, Some((pa, pb))) = e.display()
                    {
                        for (bb, _) in crate::text::range_boxes(layout, pa, pb, false) {
                            self.instances.push(rect_inst(
                                [origin[0] + bb.x0 as f32 * tk, origin[1] + bb.y1 as f32 * tk - s, (bb.x1 - bb.x0) as f32 * tk, s],
                                premul(color, o, srgb),
                                0.0,
                                clip,
                                fade,
                            ));
                        }
                    }
                }
                // Misspelled words (`El::spellcheck`), but the one the caret is typing.
                if inp.spellcheck && !e.is_secure() && e.preedit().is_none() && !departing {
                    let found = crate::spell::misspelled(&e.text);
                    // The word typing left the caret in waits unmarked until the pause ends (`spell::pause`).
                    let caret = if e.focused { crate::spell::pause::observe(&mut self.spell_watch, &n.path, &e.text, e.range(), self.clock) } else { None };
                    let color = premul(SPELLING, o, srgb);
                    for &(a, b) in found.iter().filter(|&&(a, b)| caret.is_none_or(|c| c < a || c > b)) {
                        let (da, db) = e.display_selection_range(a, b);
                        for (bb, line) in crate::text::range_boxes(layout, da, db, false) {
                            let Some(m) = layout.get(line).map(|l| *l.metrics()) else { continue };
                            // Its top a point under the baseline, as Chromium's.
                            let top = origin[1] + (m.baseline + s) * tk;
                            let (x0, x1) = (origin[0] + bb.x0 as f32 * tk, origin[0] + bb.x1 as f32 * tk);
                            push_spelling_marker(&mut self.instances, x0, x1, top, s * tk, color, clip, fade);
                        }
                    }
                }
            }
            // Span backgrounds (inline code), strikethroughs, a selection over them, then glyphs.
            let reveal = self.reveals.get(&n.id).filter(|st| !st.words.is_empty() && spec.advance_in.is_none());
            self.deco_scratch.clear();
            let mut pill_icons = Vec::new();
            let pill_bg = spec.pill_colors.map_or(spec.rich_style.link.alpha(0.2), |(bg, _)| bg);
            let d_origin = if scale_motion == ScaleMotion::Still && tk == 1.0 { rest_origin } else { [0.0, 0.0] };
            crate::text::pill_chips(layout, &spec.pills, spec.style.size, s, d_origin, pill_bg, &mut self.deco_scratch, &mut pill_icons);
            // Elided text draws its decorations over the drawn string's offsets.
            let drawn_spans = n.elided.map(|e| e.spans(&spec.spans));
            let spans = drawn_spans.as_deref().unwrap_or(&spec.spans);
            let drawn_rings = n.elided.map(|e| e.rings(&spec.rings));
            self.text.decorations_with_rings(layout, spans, drawn_rings.as_deref().unwrap_or(&spec.rings), spec.span_radius, s, d_origin, &mut self.deco_scratch);
            let drawn_len = n.elided.map_or(spec.content.len(), |e| e.drawn_len(spec.content.len()));
            if !spec.highlights.is_empty() {
                let drawn = n.elided.map(|e| e.highlights(&spec.highlights));
                self.text.highlight_decorations(layout, drawn.as_deref().unwrap_or(&spec.highlights), drawn_len, s, d_origin, &mut self.deco_scratch);
            }
            if !spec.strikes.is_empty() {
                let drawn = n.elided.map(|e| e.strikes(&spec.strikes));
                self.text.strike_decorations(layout, spans, drawn.as_deref().unwrap_or(&spec.strikes), color, s, d_origin, &mut self.deco_scratch);
            }
            link_fx.decos(layout, drawn_len, s, d_origin, &mut self.deco_scratch);
            if !spec.rings.is_empty() || spec.highlights.iter().any(|h| h.ring.is_some()) {
                // Adjacent backgrounds overlap by bg_pad. Paint ringed chips after
                // plain fills so a neighboring fill cannot erase an inset edge;
                // keep underlines above both. No sorting on the ringless path.
                self.deco_scratch.sort_by_key(|d| if d.radius == 0.0 { 2 } else { u8::from(d.ring.is_some()) });
            }
            if d_origin != rest_origin || tk != 1.0 {
                for d in &mut self.deco_scratch {
                    d.rect = scale_about(d.rect, rest_origin, d_origin, tk);
                    d.radius *= tk;
                    if let Some((_, rw)) = &mut d.ring {
                        *rw *= tk;
                    }
                }
            }
            if glide != [0.0; 2] {
                for d in &mut self.deco_scratch {
                    d.rect[0] += glide[0];
                    d.rect[1] += glide[1];
                }
            }
            for d in &self.deco_scratch {
                let alpha = if d.ring.is_some() {
                    reveal.map_or(1.0, |st| Self::reveal_alpha(st, d.byte, self.scene.store().node_time(n.id)))
                } else { 1.0 };
                let c = premul(d.color, o * alpha, srgb);
                let mut inst = rect_inst(d.rect, c, d.radius, clip, fade);
                if let Some((rc, rw)) = d.ring {
                    inst.border = premul(rc, o * alpha, srgb);
                    inst.params[0] = rw;
                    // The opacity the colours carry, so the ring hides the chip's fill by its own alpha.
                    inst.params[3] = o * alpha;
                }
                self.instances.push(inst);
            }
            let pill_fg = spec.pill_colors.and_then(|(_, fg)| fg).unwrap_or(color);
            for (icon, px, at, fg) in pill_icons {
                let pill_fg = fg.unwrap_or(pill_fg);
                let at = [
                    rest_origin[0] + (at[0] - d_origin[0]) * tk + glide[0],
                    rest_origin[1] + (at[1] - d_origin[1]) * tk + glide[1],
                ];
                let drawn = px as f32 * tk;
                let px_r = (px as f32 * raster).round().max(1.0) as u32;
                if let Some(q) = self.text.icon_quad(icon, px_r, at) {
                    let inst = icon_inst(&q, at, drawn, 0.0, premul(pill_fg, o, srgb), clip, fade);
                    if self.paint_rank.is_some() && inst.uv[2] > 0.0 { self.icon_aprons.push(apron_key(&inst)); }
                    self.instances.push(inst);
                }
            }
            let mut bare = Vec::new();
            crate::text::bare_icons(layout, &spec.pills, s, d_origin, &mut bare);
            for (k, at, px) in bare {
                let pill = &spec.pills[k];
                let Some(b) = pill.bare.as_deref() else { continue };
                let at = [
                    rest_origin[0] + (at[0] - d_origin[0]) * tk + glide[0],
                    rest_origin[1] + (at[1] - d_origin[1]) * tk + glide[1],
                ];
                let drawn = px * tk;
                // An image fits the square, keeping its shape; a glyph takes the text's colour.
                if let Some(pixels) = &b.image
                    && let (w, h) = pixels.size()
                    && w > 0
                    && h > 0
                {
                    let (w, h) = (w as f32, h as f32);
                    let k = drawn / w.max(h);
                    let rect = [at[0] + (drawn - w * k) * 0.5, at[1] + (drawn - h * k) * 0.5, w * k, h * k];
                    if let Some(image) = self.images.place(pixels, [rect[2], rect[3]]) {
                        self.instances.push(Instance {
                            rect,
                            radii: [(b.radius * s * tk).min(rect[2].min(rect[3]) * 0.5); 4],
                            fill: [0.0, 0.0, 0.0, o],
                            border: [0.0; 4],
                            params: [image.page as f32, KIND_IMAGE, image.premultiplied as u8 as f32, 1.0],
                            uv: image.rect.map(|v| v as f32),
                            clip,
                            fade: crate::render::pack_fade(fade, clip, clip),
                        });
                    }
                    continue;
                }
                let Some(icon) = pill.icon else { continue };
                let px_r = (px * raster).round().max(1.0) as u32;
                if let Some(q) = self.text.icon_quad(icon, px_r, at) {
                    let inst = icon_inst(&q, at, drawn, 0.0, premul(pill.fg.unwrap_or(color), o, srgb), clip, fade);
                    if self.paint_rank.is_some() && inst.uv[2] > 0.0 { self.icon_aprons.push(apron_key(&inst)); }
                    self.instances.push(inst);
                }
            }
            if spec.selectable && let Some((a, b)) = selectable::range_in(sel_ranges, i) {
                let (a, b) = n.elided.map_or((a, b), |e| e.range(a, b));
                selectable::push_selection(&mut self.instances, layout, a, b, origin, tk, premul(spec.selection, o, srgb), s, clip, fade);
            }
            self.glyph_scratch.clear();
            self.text.set_glyph_inks(color, &spec.spans);
            let origin = rest_origin;
            self.text.set_glide(glide);
            let r_origin = if scale_motion == ScaleMotion::Still { origin } else { [0.0, 0.0] };
            let has_hl_fg = spec.highlights.iter().any(|h| h.fg.is_some());
            // A cell of a longer run draws its slice of the run's glyphs, shaped in context.
            let zoom = if let (Some(r), Some((_, at))) = (&n.run_layout, &spec.advance_in)
                && let Some(x) = r.x(*at)
            {
                self.text.glyphs_in(r, *at, s, [r_origin[0] - x, r_origin[1]], &mut self.glyph_scratch);
                tk
            } else {
                // Only the lines the clip shows, with two ems either side for ink past
                // the line box; the per-glyph test below still decides what draws.
                let band = if tk > 1e-4 {
                    let em = spec.spans.iter().filter_map(|sp| sp.size).fold(spec.style.size.max(spec.style.line_px()), f32::max);
                    let pad = 2.0 * em * s + 2.0 / raster.max(1e-4);
                    (clip[1] - origin[1]) / tk - pad..(clip[3] - origin[1]) / tk + pad
                } else {
                    f32::NEG_INFINITY..f32::INFINITY
                };
                let bytes = reveal.is_some() || !link_fx.is_empty() || has_hl_fg || field_ink.is_some();
                // A secret's glyphs aren't kept past the frame that draws them.
                let secret = spec.redacted || n.input.as_ref().is_some_and(|i| i.edit.is_secure());
                if direct || secret {
                    self.text.glyphs_in_band(layout, r_origin, raster, bytes, band.clone(), &mut self.glyph_scratch);
                } else {
                    self.text.glyphs_in_band_cached(layout, r_origin, raster, bytes, band.clone(), &mut self.glyph_scratch);
                }
                if place_masks && !secret {
                    // Landing: the masks the next frame draws, placed now and not drawn.
                    let mut masks = Vec::new();
                    self.text.set_emit(crate::text::outline::Emit::Atlas);
                    self.text.glyphs_in_band_cached(layout, r_origin, raster, bytes, band, &mut masks);
                    self.text.set_emit(crate::text::outline::Emit::Outline);
                }
                tk / raster
            };
            self.text.set_glide([0.0; 2]);
            let base = premul(color, o, srgb);
            let lands = self.flights.iter().any(|f| f.lands(&n.path, n.key.as_deref()));
            for q in if lands { &[][..] } else { &self.glyph_scratch[..] } {
                let [x0, y0] = if zoom == 1.0 && origin == r_origin { [q.rect[0], q.rect[1]] } else {
                    std::array::from_fn(|k| origin[k] + (q.rect[k] - r_origin[k]) * zoom)
                };
                let (x1, y1) = (x0 + q.rect[2] * zoom, y0 + q.rect[3] * zoom);
                // Wholly outside the clip, a glyph draws nothing: a long line in an
                // `El::scroll_x` skips the glyphs scrolled out of sight.
                if y0 > clip[3] || y1 < clip[1] || x0 > clip[2] || x1 < clip[0] {
                    continue;
                }
                let mut f = base;
                if q.ink > 0
                    && let Some(c) = spec.spans.get(q.ink as usize - 1).and_then(|sp| sp.color)
                {
                    f = premul(c, o, srgb);
                }
                if let Some((a, b, c)) = field_ink
                    && (a..b).contains(&(q.byte as usize))
                {
                    f = c;
                }
                if has_hl_fg
                    && let Some(c) = spec.highlights.iter().rev().find_map(|h| {
                        let (a, b) = n.elided.map_or((h.start as usize, h.end as usize), |e| e.mark(h.start as usize, h.end as usize, false));
                        ((q.byte as usize) >= a && (q.byte as usize) < b).then_some(h.fg).flatten()
                    })
                {
                    f = premul(c, o, srgb);
                }
                if !link_fx.is_empty() && let Some(c) = link_fx.glyph_color(q.byte) {
                    f = premul(c, o, srgb);
                }
                if let Some(st) = reveal {
                    let a = Self::reveal_alpha(st, q.byte, self.scene.store().node_time(n.id));
                    if a <= 0.0 {
                        continue;
                    }
                    f = f.map(|v| v * a);
                }
                self.instances.extend(glyph_or_image(q, &self.text, &mut self.images, f, clip, fade)
                    .map(|g| zoom_glyph(g, origin, r_origin, zoom)));
                *glyph_count += 1;
            }
            if let (Some(c), Some(inp)) = (caret, &n.input) {
                self.instances.push(rect_inst(c, premul(inp.caret, o, srgb), 0.0, clip, fade));
            }
            self.text.set_emit(crate::text::outline::Emit::Atlas);
        }
    }

    fn push_icon(&mut self, inst: Instance) {
        if self.paint_rank.is_some() && inst.uv[2] > 0.0 { self.icon_aprons.push(apron_key(&inst)); }
        self.instances.push(inst);
    }

    /// The rectangle a live or truncated text clips its glyphs to (edges, device pixels), its
    /// box, and whether it runs past that box; `None` for other text.
    pub(super) fn text_clip(&self, i: usize, (r, r_exact): ([f32; 4], [f32; 4]), sc: f32, clip: [f32; 4]) -> Option<([f32; 4], [f32; 4], bool)> {
        let s = self.env.scale;
        let n = &self.nodes[i];
        let (Some(spec), Some(layout)) = (&n.text, &n.text_layout) else { return None };
        if !(spec.truncate_fade || spec.live.is_some()) { return None; }
        let ins = n.inset.map(|v| v * sc * s);
        let mut bounds = [r[0] + ins[0], r[1] + ins[1], r[2] - ins[2], r[3] - ins[3]];
        // Layout width already includes display scale `s`; bounds also include the
        // element/ancestor transform `sc`. Compare both after that transform. A text fades
        // only when it runs past its laid-out box and the box drawn now: snapping each edge
        // to a whole pixel can leave the drawn box a sliver short, and a box springing to a
        // new layout (`animate_layout`, a flight) is short until it lands. A text that fits
        // isn't clipped by either.
        let exact = r_exact[2] - r_exact[0] - ins[0] - ins[2];
        let laid_out = (n.local.w - n.inset[0] - n.inset[2]).max(0.0) * s;
        let overflows = layout.full_width() > laid_out + 0.5
            && layout.full_width() * sc > (bounds[2] - bounds[0]).max(exact) + 0.5 * sc;
        // A right-to-left paragraph runs out past its left edge.
        match (overflows, crate::el::Flow::of_layout(layout)) {
            (true, _) => {}
            (false, crate::el::Flow::Ltr) => bounds[2] = bounds[2].max(bounds[0] + exact.max(layout.full_width() * sc)),
            (false, crate::el::Flow::Rtl) => bounds[0] = bounds[0].min(bounds[2] - exact.max(layout.full_width() * sc)),
        }
        Some((intersect(clip, bounds), bounds, overflows))
    }

    /// Text-only clipping leaves the box's fill/border and descendants untouched.
    pub(super) fn clip_text_instances(
        &mut self,
        i: usize,
        first_instance: usize,
        (r, r_exact): ([f32; 4], [f32; 4]),
        sc: f32,
        clip: [f32; 4],
        fade: [f32; 4],
        fade_origin: [f32; 4],
    ) {
        let s = self.env.scale;
        if let Some((text_clip, bounds, overflows)) = self.text_clip(i, (r, r_exact), sc, clip)
            && let Some(spec) = &self.nodes[i].text
        {
            let mut widths = fade;
            let mut origins = fade_origin;
            if spec.truncate_fade && overflows {
                let width = (spec.style.size * spec.fade_em * sc * s).min((bounds[2] - bounds[0]).max(0.0));
                // The fade is on the end of the line it sits in, as a web mask on its parent flips
                // with `dir`, so a Latin title (`Direction::Auto`) in a right-to-left row fades on
                // the left.
                let n = &self.nodes[i];
                let line = n.parent.map_or(n.flow, |p| self.nodes[p].flow);
                let own = if (spec.align == Align::End) != (line == crate::el::Flow::Rtl) { [width, 0.0, 0.0, 0.0] }
                    else { [0.0, 0.0, width, 0.0] };
                inherit_fade(&mut widths, &mut origins, bounds, own);
            }
            for instance in &mut self.instances[first_instance..] {
                if instance.params[1] == KIND_GLYPH {
                    instance.clip = text_clip;
                    instance.fade = crate::render::pack_fade(widths, origins, text_clip);
                }
            }
        }
    }
}

/// A misspelled word's marker: Chromium's red, on macOS a row of dots and elsewhere a wavy line.
/// (Linear light: #FF3B30 on macOS, #FF0000 elsewhere.)
const SPELLING: Color = if cfg!(target_os = "macos") { Color { r: 1.0, g: 0.0437, b: 0.0296, a: 1.0 } } else { Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 } };

/// The spelling marker from device x `x0` to `x1`, its top at `y`, `k` device px a point: on
/// macOS 3 pt dots every 4 pt, elsewhere Chromium's 1 pt line waving 2 pt down and back every 6 pt.
#[allow(clippy::too_many_arguments)]
fn push_spelling_marker(out: &mut Vec<Instance>, x0: f32, x1: f32, y: f32, k: f32, color: [f32; 4], clip: [f32; 4], fade: [f32; 4]) {
    if cfg!(target_os = "macos") {
        let (d, step) = (3.0 * k, 4.0 * k);
        let count = ((x1 - x0 - d) / step).floor().max(0.0) as usize + 1;
        let x = x0 + ((x1 - x0) - (count - 1) as f32 * step - d) / 2.0;
        out.extend((0..count).map(|n| rect_inst([x + n as f32 * step, y, d, d], color, d / 2.0, clip, fade)));
        return;
    }
    const WAVE: [f32; 6] = [0.0, 1.0, 2.0, 2.0, 1.0, 0.0];
    let t = k.max(1.0).round();
    let start = x0.round();
    out.extend((0..(x1.round() - start).max(0.0) as usize).map(|n| {
        let wave = WAVE[(n as f32 / k) as usize % WAVE.len()];
        rect_inst([start + n as f32, (y + wave * k).round(), 1.0, t], color, 0.0, clip, fade)
    }));
}
