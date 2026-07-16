use super::*;

impl Screen<'_> {
    pub(crate) fn markdown_viewport_height(&self) -> f32 {
        let scale = self.sugarloaf.scale_factor();
        self.context_manager
            .current_grid()
            .current_item()
            .map(|item| item.layout_rect[3] / scale)
            .unwrap_or_else(|| self.sugarloaf.window_size().height as f32 / scale)
    }

    pub(crate) fn render_markdown_panels(&mut self) -> bool {
        let scale = self.sugarloaf.scale_factor();
        let theme = self.renderer.theme;
        let markdown_font_scale = self.renderer.chrome_scale();
        let window_size = self.sugarloaf.window_size();
        let text_occlusions = self.renderer.active_text_occlusion_rects(
            window_size.width,
            window_size.height,
            scale,
        );
        // Black-region triage (`NEOISM_MD_OCCLUSION_LOG=1`): every text row
        // intersecting an occlusion rect is silently dropped while block
        // backgrounds still paint — a stale/oversized rect here blanks
        // whatever scrolls under it. Log the active set so the next repro
        // names the culprit panel instead of guessing.
        if !text_occlusions.is_empty()
            && std::env::var_os("NEOISM_MD_OCCLUSION_LOG").is_some()
        {
            tracing::warn!(
                target: "neoism::md_occlusion",
                rects = ?text_occlusions,
                "markdown text occlusion set active this frame"
            );
        }
        let markdown_mouse = (!self.mouse_hidden_by_typing)
            .then_some([self.mouse.x as f32 / scale, self.mouse.y as f32 / scale]);
        let (visible_nodes, scaled_margin) = {
            let grid = self.context_manager.current_grid();
            (
                grid.contexts()
                    .keys()
                    .copied()
                    .filter(|node| grid.is_context_visible(*node))
                    .collect::<Vec<_>>(),
                grid.scaled_margin,
            )
        };
        let mut markdown_needs_redraw = false;
        let show_overlay = self.show_ink_overlay;
        // Wave 7C: snapshot remote collaborator carets per visible pane
        // path BEFORE the mutable grid borrow below (presence store and
        // grid are both fields of `self`).
        let pane_buffers: Vec<(std::path::PathBuf, String)> = {
            let grid = self.context_manager.current_grid();
            grid.contexts()
                .iter()
                .filter(|(key, _)| visible_nodes.contains(key))
                .filter_map(|(_, item)| {
                    item.val
                        .markdown
                        .as_ref()
                        .map(|pane| {
                            (
                                pane.path.clone(),
                                crate::screen::markdown_crdt::buffer_id_for_markdown_path(
                                    &pane.path,
                                ),
                            )
                        })
                        .or_else(|| {
                            item.val.notebook.as_ref().map(|pane| {
                                (
                                    pane.path.clone(),
                                    crate::screen::markdown_crdt::buffer_id_for_notebook_render_path(
                                        &pane.path,
                                    ),
                                )
                            })
                        })
                })
                .collect()
        };
        let mut remote_by_path: std::collections::HashMap<
            std::path::PathBuf,
            Vec<neoism_ui::editor::markdown::MarkdownRemoteCursor>,
        > = std::collections::HashMap::new();
        for (path, buffer_id) in pane_buffers {
            let cursors = self
                .remote_presence
                .cursors_for(&buffer_id)
                .map(
                    |presence| neoism_ui::editor::markdown::MarkdownRemoteCursor {
                        name: presence.display_name.clone(),
                        color: [presence.color.r, presence.color.g, presence.color.b],
                        rainbow: presence.rainbow,
                        line: presence.cursor.line as usize,
                        col_utf16: presence.cursor.column as usize,
                    },
                )
                .collect::<Vec<_>>();
            remote_by_path.insert(path, cursors);
        }
        // (rect, note path, scroll) collected here, composited after the
        // loop to avoid borrowing `context_manager` while we render ink.
        let mut overlays: Vec<([f32; 4], std::path::PathBuf, f32)> = Vec::new();
        // Panes whose image overlays are re-synced this frame — anything
        // we pushed to LAST frame that isn't in here gets cleared below.
        let mut overlay_ids_touched: std::collections::HashSet<usize> =
            std::collections::HashSet::new();
        let markdown_animation_phase = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| {
                neoism_ui::render_policy::animation_phase_from_unix_secs(
                    duration.as_secs(),
                    duration.subsec_nanos(),
                )
            })
            .unwrap_or_default();
        for (key, item) in self
            .context_manager
            .current_grid_mut()
            .contexts_mut()
            .iter_mut()
        {
            if !visible_nodes.contains(key) {
                // A pane that left the composition (background tab,
                // collapsed split) must not leave its cover/image
                // overlays registered — switching from an md file to
                // the terminal kept compositing the page cover.
                self.sugarloaf
                    .clear_image_overlays_for(item.val.rich_text_id);
                continue;
            }
            let Some(markdown) = item.val.markdown.as_mut() else {
                if let Some(notebook) = item.val.notebook.as_mut() {
                    let rich_text_id = item.val.rich_text_id;
                    let rect = [
                        (scaled_margin.left + item.layout_rect[0]) / scale,
                        (scaled_margin.top + item.layout_rect[1]) / scale,
                        item.layout_rect[2] / scale,
                        item.layout_rect[3] / scale,
                    ];
                    {
                        let markdown = &mut notebook.markdown;
                        markdown.remote_cursors =
                            remote_by_path.remove(&markdown.path).unwrap_or_default();
                        crate::editor::markdown::render::render(
                            &mut self.sugarloaf,
                            markdown,
                            rect,
                            &theme,
                            markdown_mouse,
                            &text_occlusions,
                            markdown_font_scale,
                            markdown_animation_phase,
                        );
                        markdown_needs_redraw |=
                            markdown.scroll_cursor_into_view(rect[1], rect[3]);
                    }
                    Self::sync_notebook_image_overlays(
                        &mut self.sugarloaf,
                        rich_text_id,
                        notebook,
                        markdown_font_scale,
                    );
                    overlay_ids_touched.insert(rich_text_id);
                }
                continue;
            };
            self.sugarloaf
                .clear_image_overlays_for(item.val.rich_text_id);
            markdown.remote_cursors =
                remote_by_path.remove(&markdown.path).unwrap_or_default();
            let rect = [
                (scaled_margin.left + item.layout_rect[0]) / scale,
                (scaled_margin.top + item.layout_rect[1]) / scale,
                item.layout_rect[2] / scale,
                item.layout_rect[3] / scale,
            ];
            // Always render the rich markdown (checkboxes, headings, etc.) —
            // it scrolls as normal. The ink layer composites OVER it below.
            crate::editor::markdown::render::render(
                &mut self.sugarloaf,
                markdown,
                rect,
                &theme,
                markdown_mouse,
                &text_occlusions,
                markdown_font_scale,
                markdown_animation_phase,
            );
            markdown_needs_redraw |= markdown.scroll_cursor_into_view(rect[1], rect[3]);
            Self::sync_markdown_cover_overlay(
                &mut self.sugarloaf,
                &mut self.markdown_cover_cache,
                item.val.rich_text_id,
                markdown,
                rect,
                markdown_font_scale,
            );
            overlay_ids_touched.insert(item.val.rich_text_id);
            if show_overlay {
                overlays.push((rect, markdown.path.clone(), markdown.scroll_y));
            }
        }

        // Orphan sweep: clear overlays for every pane we fed LAST frame
        // that didn't re-sync this frame — a cover whose pane left the
        // composition (tab stashed to another grid, workspace switch,
        // pane closed) otherwise stays screen-glued over whatever
        // renders next. Terminals are untouched: their kitty overlays
        // live on ids this sweep never owned.
        for id in std::mem::take(&mut self.markdown_image_overlay_ids) {
            if !overlay_ids_touched.contains(&id) {
                self.sugarloaf.clear_image_overlays_for(id);
            }
        }
        self.markdown_image_overlay_ids = overlay_ids_touched;

        // Composite the note's ink layer OVER the rendered markdown, in
        // content coordinates (zoom 1) offset by the scroll so it tracks the
        // text as you scroll.
        for (rect, path, scroll_y) in overlays {
            use crate::editor::neodraw::{
                render_pane_overlay, render_scene, Camera, Vec2,
            };
            let drawing_here = self
                .draw_over_note
                .as_ref()
                .map(|d| d.note == path)
                .unwrap_or(false);
            if drawing_here {
                // Live DrawPane (strokes + tool island), camera locked to
                // the note's scroll so the ink tracks the text 1:1.
                if let Some(d) = self.draw_over_note.as_mut() {
                    d.pane.camera = Camera {
                        pan: Vec2::new(0.0, -scroll_y),
                        zoom: 1.0,
                    };
                    render_pane_overlay(
                        &mut self.sugarloaf,
                        &mut d.pane,
                        rect,
                        &theme,
                        12,
                    );
                }
            } else {
                self.ensure_ink_overlay(&path);
                if let Some((_, Some(scene))) = self.ink_overlay_cache.get(&path) {
                    let cam = Camera {
                        pan: Vec2::new(rect[0], rect[1] - scroll_y),
                        zoom: 1.0,
                    };
                    render_scene(&mut self.sugarloaf, scene, &cam, rect, 0.0, 12);
                }
            }
        }
        markdown_needs_redraw
    }

    fn sync_notebook_image_overlays(
        sugarloaf: &mut neoism_backend::sugarloaf::Sugarloaf<'_>,
        rich_text_id: usize,
        notebook: &neoism_ui::editor::notebook::NotebookPane,
        font_scale: f32,
    ) {
        let images = notebook.rendered_image_outputs();
        if images.is_empty() {
            sugarloaf.clear_image_overlays_for(rich_text_id);
            return;
        }

        let scale = sugarloaf.scale_factor();
        let mut overlays = Vec::new();
        for image in images {
            let Some(block) = notebook.markdown.block_rect_for_source_line(image.line)
            else {
                continue;
            };
            if !sugarloaf.image_data.contains_key(&image.image_id) {
                sugarloaf.image_data.insert(
                    image.image_id,
                    neoism_backend::sugarloaf::GraphicDataEntry::from_graphic_data(
                        neoism_backend::sugarloaf::GraphicData {
                            id: neoism_backend::sugarloaf::GraphicId::new(
                                image.image_id as u64,
                            ),
                            width: image.width as usize,
                            height: image.height as usize,
                            color_type: neoism_backend::sugarloaf::ColorType::Rgba,
                            pixels: image.pixels,
                            is_opaque: image.is_opaque,
                            resize: None,
                            display_width: None,
                            display_height: None,
                            transmit_time: web_time::Instant::now(),
                        },
                    ),
                );
            }

            let (display_w, display_h) = Self::notebook_image_overlay_size(
                image.width as f32,
                image.height as f32,
                block.wrap_width,
                font_scale,
            );
            if display_w <= 0.0 || display_h <= 0.0 {
                continue;
            }
            overlays.push(neoism_backend::sugarloaf::GraphicOverlay {
                image_id: image.image_id,
                x: block.text_x * scale,
                y: (block.rect[1] + block.rect[3] - display_h) * scale,
                width: display_w * scale,
                height: display_h * scale,
                z_index: 1,
                source_rect: neoism_backend::sugarloaf::GraphicOverlay::FULL_SOURCE_RECT,
            });
        }

        let panel_overlays = sugarloaf.image_overlays.entry(rich_text_id).or_default();
        panel_overlays.clear();
        panel_overlays.extend(overlays);
    }

    /// Lay the `cover:` banner image over the band the shared renderer
    /// reserved (`pane.cover_overlay_rect`): aspect-fill center-crop via
    /// `source_rect`, clipped vertically to the pane as it scrolls.
    fn sync_markdown_cover_overlay(
        sugarloaf: &mut neoism_backend::sugarloaf::Sugarloaf<'_>,
        cache: &mut std::collections::HashMap<
            std::path::PathBuf,
            Option<crate::screen::MarkdownCoverImage>,
        >,
        rich_text_id: usize,
        markdown: &neoism_ui::editor::markdown::MarkdownPane,
        pane_rect: [f32; 4],
        font_scale: f32,
    ) {
        let Some(value) = markdown.frontmatter_cover() else {
            return;
        };
        if markdown.cover_overlay_rect.is_none() {
            return;
        }
        // Recompute the band HERE from authoritative frame inputs (pane
        // rect + scroll + font scale, mirroring the shared surface's
        // reservation math) instead of trusting the rect stashed during
        // an earlier render — a stale rect painted the mostly-dark cover
        // texture over live content, which read as "the paragraph turned
        // black".
        let bx = pane_rect[0];
        let bw = pane_rect[2];
        let bh = (170.0 * font_scale).min(pane_rect[3] * 0.35).max(0.0);
        let by = pane_rect[1] - markdown.scroll_y.max(0.0);
        if bw <= 0.0 || bh <= 0.0 {
            return;
        }
        let Some(path) = Self::resolve_markdown_cover_path(&markdown.path, &value) else {
            return;
        };
        if let Some(Some(cover)) = cache.get(&path) {
            // Texture evicted (context rebuild) — re-decode next frame.
            if !sugarloaf.image_data.contains_key(&cover.image_id) {
                cache.remove(&path);
                return;
            }
        }
        if !cache.contains_key(&path) {
            let loaded = Self::load_markdown_cover(sugarloaf, &path);
            cache.insert(path.clone(), loaded);
        }
        let Some(Some(cover)) = cache.get(&path) else {
            return;
        };

        let pane_top = pane_rect[1];
        let pane_bottom = pane_rect[1] + pane_rect[3];
        let visible_top = by.max(pane_top);
        let visible_bottom = (by + bh).min(pane_bottom);
        if visible_bottom <= visible_top {
            return;
        }
        let clip_top_fraction = (visible_top - by) / bh;
        let clip_bottom_fraction = (visible_bottom - by) / bh;

        // Aspect-fill: crop the source to the band's aspect ratio, then
        // apply the scroll clip inside that cropped window.
        let band_aspect = bw / bh;
        let image_aspect = cover.width.max(1) as f32 / cover.height.max(1) as f32;
        let (sx, sy, sw, sh) = if image_aspect > band_aspect {
            let sw = (band_aspect / image_aspect).clamp(0.0, 1.0);
            ((1.0 - sw) * 0.5, 0.0, sw, 1.0)
        } else {
            let sh = (image_aspect / band_aspect).clamp(0.0, 1.0);
            (0.0, (1.0 - sh) * 0.5, 1.0, sh)
        };
        // `source_rect` is CORNERS `[u0, v0, u1, v1]`, not (origin,
        // size) — passing a size here read fine at rest ([0,0,1,1] is
        // identical in both conventions) but inverted the sampler the
        // moment the scroll clip shrank the band: v1 fell below v0 and
        // the image drew squashed and upside-down.
        let source_rect = [
            sx,
            sy + sh * clip_top_fraction,
            sx + sw,
            sy + sh * clip_bottom_fraction,
        ];

        let scale = sugarloaf.scale_factor();
        sugarloaf
            .image_overlays
            .entry(rich_text_id)
            .or_default()
            .push(neoism_backend::sugarloaf::GraphicOverlay {
                image_id: cover.image_id,
                x: bx * scale,
                y: visible_top * scale,
                width: bw * scale,
                height: (visible_bottom - visible_top) * scale,
                // ABOVE text (z >= 0): the BelowText layer renders under
                // the pane's opaque background rect, which blacked the
                // picture out entirely. Content can never overlap the
                // band by construction (pad_top reserves it and the
                // scroll crop shrinks the band in step), so AboveText is
                // safe here.
                z_index: 1,
                source_rect,
            });
    }

    /// Resolve a `cover:` value: absolute paths and `./`-style relatives
    /// (against the note's directory) pass through; bare names look up
    /// the shipped/user covers directory (`<config>/covers/`), trying
    /// common image extensions.
    fn resolve_markdown_cover_path(
        md_path: &std::path::Path,
        value: &str,
    ) -> Option<std::path::PathBuf> {
        let raw = std::path::Path::new(value);
        if raw.is_absolute() {
            return raw.exists().then(|| raw.to_path_buf());
        }
        if value.contains('/') {
            let joined = md_path.parent()?.join(raw);
            return joined.exists().then_some(joined);
        }
        let covers = neoism_backend::config::config_dir_path().join("covers");
        let exact = covers.join(value);
        if exact.exists() {
            return Some(exact);
        }
        for ext in ["png", "jpg", "jpeg", "webp"] {
            let candidate = covers.join(format!("{value}.{ext}"));
            if candidate.exists() {
                return Some(candidate);
            }
        }
        None
    }

    fn load_markdown_cover(
        sugarloaf: &mut neoism_backend::sugarloaf::Sugarloaf<'_>,
        path: &std::path::Path,
    ) -> Option<crate::screen::MarkdownCoverImage> {
        let image = image_rs::open(path)
            .map_err(|error| {
                tracing::warn!(?path, %error, "cover image decode failed");
                error
            })
            .ok()?;
        let rgba = image.to_rgba8();
        let (width, height) = rgba.dimensions();
        if width == 0 || height == 0 {
            return None;
        }
        let image_id = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            path.hash(&mut hasher);
            // High bit keeps cover ids out of the notebook-output id space.
            (hasher.finish() as u32) | (1u32 << 30)
        };
        sugarloaf.image_data.insert(
            image_id,
            neoism_backend::sugarloaf::GraphicDataEntry::from_graphic_data(
                neoism_backend::sugarloaf::GraphicData {
                    id: neoism_backend::sugarloaf::GraphicId::new(image_id as u64),
                    width: width as usize,
                    height: height as usize,
                    color_type: neoism_backend::sugarloaf::ColorType::Rgba,
                    pixels: rgba.into_raw(),
                    is_opaque: true,
                    resize: None,
                    display_width: None,
                    display_height: None,
                    transmit_time: web_time::Instant::now(),
                },
            ),
        );
        Some(crate::screen::MarkdownCoverImage {
            image_id,
            width,
            height,
        })
    }

    fn notebook_image_overlay_size(
        width: f32,
        height: f32,
        available_width: f32,
        font_scale: f32,
    ) -> (f32, f32) {
        if width <= 0.0 || height <= 0.0 {
            return (0.0, 0.0);
        }
        let max_w = available_width.min(640.0 * font_scale).max(1.0);
        let max_h = 360.0 * font_scale;
        let fit = (max_w / width).min(max_h / height).min(1.0);
        ((width * fit).max(1.0), (height * fit).max(1.0))
    }

    /// Load/refresh a note's ink overlay (`"<note> (reMarkable).neodraw"`,
    /// in content coordinates) into the cache, keyed by the `.md` path and
    /// the sidecar's mtime.
    fn ensure_ink_overlay(&mut self, note_path: &std::path::Path) {
        use crate::editor::neodraw::Scene;
        crate::editor::neodraw::migrate_legacy_ink(note_path);
        let sidecar = crate::editor::neodraw::ink_sidecar_path(note_path);
        match std::fs::metadata(&sidecar).and_then(|m| m.modified()) {
            Ok(mtime) => {
                let fresh = self
                    .ink_overlay_cache
                    .get(note_path)
                    .map(|(t, _)| *t == mtime)
                    .unwrap_or(false);
                if !fresh {
                    let scene = std::fs::read_to_string(&sidecar)
                        .ok()
                        .and_then(|json| Scene::from_json(&json).ok())
                        .map(|mut s| {
                            crate::editor::neodraw::strokes_only(&mut s); // ink = strokes, never text
                            s
                        });
                    self.ink_overlay_cache
                        .insert(note_path.to_path_buf(), (mtime, scene));
                }
            }
            Err(_) => {
                self.ink_overlay_cache.remove(note_path);
            }
        }
    }

    /// Show/hide the handwriting/ink overlay on markdown notes. Kept for a
    /// future toggle keybind; not wired to a command yet.
    #[allow(dead_code)]
    pub(crate) fn toggle_ink_overlay(&mut self) -> bool {
        self.show_ink_overlay = !self.show_ink_overlay;
        self.mark_dirty();
        self.show_ink_overlay
    }

    pub(crate) fn scroll_markdown_by(&mut self, delta_pixels: f32) -> bool {
        if self.context_manager.current().markdown.is_none() {
            if self.context_manager.current().notebook.is_none() {
                return false;
            }
        }
        let viewport_height = self.markdown_viewport_height();
        if let Some(markdown) = self.context_manager.current_mut().active_markdown_mut() {
            markdown.scroll_by_content_pixels(delta_pixels, viewport_height);
        }
        self.mark_dirty();
        true
    }

    pub(crate) fn scroll_markdown_page(&mut self, direction: f32, fraction: f32) -> bool {
        if self.context_manager.current().markdown.is_none()
            && self.context_manager.current().notebook.is_none()
        {
            return false;
        }
        self.scroll_markdown_by(self.markdown_viewport_height() * fraction * direction)
    }

    pub(crate) fn scroll_markdown_to_top(&mut self) -> bool {
        if let Some(markdown) = self.context_manager.current_mut().active_markdown_mut() {
            markdown.scroll_to_top();
            self.mark_dirty();
            return true;
        }
        false
    }

    pub(crate) fn scroll_markdown_to_bottom(&mut self) -> bool {
        let viewport_height = self.markdown_viewport_height();
        if let Some(markdown) = self.context_manager.current_mut().active_markdown_mut() {
            markdown.scroll_to_bottom(viewport_height);
            self.mark_dirty();
            return true;
        }
        false
    }

    pub(crate) fn markdown_mouse_logical(&self) -> [f32; 2] {
        let scale = self.sugarloaf.scale_factor();
        [self.mouse.x as f32 / scale, self.mouse.y as f32 / scale]
    }

    #[allow(dead_code)]
    pub(crate) fn markdown_geom_for(
        &self,
        note: &std::path::Path,
    ) -> Option<([f32; 4], f32)> {
        let scale = self.sugarloaf.scale_factor();
        let grid = self.context_manager.current_grid();
        let sm = grid.scaled_margin;
        for (node, item) in grid.contexts().iter() {
            if !grid.is_context_visible(*node) {
                continue;
            }
            if let Some(md) = item.val.markdown.as_ref() {
                if md.path == note {
                    let rect = [
                        (sm.left + item.layout_rect[0]) / scale,
                        (sm.top + item.layout_rect[1]) / scale,
                        item.layout_rect[2] / scale,
                        item.layout_rect[3] / scale,
                    ];
                    return Some((rect, md.scroll_y));
                }
            }
        }
        None
    }

    pub(crate) fn open_markdown_link_target(
        &mut self,
        target: crate::editor::markdown::state::MarkdownLinkTarget,
    ) {
        let path = target.path.clone();
        let raw_target = path.display().to_string();
        if let Some(cell_index) = raw_target
            .strip_prefix("neoism-notebook:/run/")
            .or_else(|| raw_target.strip_prefix("neoism-notebook://run/"))
            .and_then(|value| value.parse::<usize>().ok())
        {
            self.run_notebook_cell(cell_index);
            return;
        }
        let action = crate::editor::markdown::state::markdown_link_open_action(
            &target,
            path.is_dir(),
            crate::editor::markdown::state::is_markdown_path(&path),
            path.exists(),
        );
        match action {
            crate::editor::markdown::state::MarkdownLinkOpenAction::OpenDirectory => {
                self.open_directory_link_in_file_tree(path);
                self.mark_dirty();
                return;
            }
            crate::editor::markdown::state::MarkdownLinkOpenAction::OpenMarkdown {
                create_missing_note,
            } => {
                if create_missing_note && !self.create_missing_markdown_note(&path) {
                    self.mark_dirty();
                    return;
                }
                self.open_path_in_markdown(path);
                if let Some(line) = target.line {
                    if let Some(markdown) =
                        self.context_manager.current_mut().markdown.as_mut()
                    {
                        markdown.jump_to_line(line.max(1));
                        markdown.flash_line(line.max(1));
                    }
                    self.renderer.trail_cursor.reset();
                }
                self.mark_dirty();
                return;
            }
            crate::editor::markdown::state::MarkdownLinkOpenAction::OpenEditor => {}
        }

        self.open_path_in_editor(path.clone());
        if let Some(line) = target.line {
            self.ensure_primary_editor_route();
            if let Some(route) = self.renderer.primary_editor_route {
                let path_str = path.display().to_string();
                let path_lit =
                    neoism_backend::performer::nvim::lua_string_literal(&path_str);
                let cmd = format!(
                    r#"lua pcall(function() vim.cmd.edit({path_lit}); vim.api.nvim_win_set_cursor(0, {{ {}, 0 }}); vim.cmd('normal! zz'); require('rio.search').preview({}) end)"#,
                    line.max(1),
                    line.max(1)
                );
                self.send_editor_command_to_route(route, cmd);
            }
        }
        self.mark_dirty();
    }
}
