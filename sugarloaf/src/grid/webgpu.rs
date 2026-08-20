// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! wgpu backend for the grid renderer.
//!
//! Data-side mirror of `super::metal`. Phase 1b added the bg pass;
//! Phase 1d here adds the text pass — per-instance vertex buffer of
//! `CellText`, grayscale glyph atlas, instanced quad draws.

use rustc_hash::FxHashMap;

use super::atlas::{AtlasSlot, GlyphKey, RasterizedGlyph};
use super::cell::{CellBg, CellText, GridUniforms};
use super::GridRowSnapshot;
use crate::context::webgpu::WgpuContext;
use crate::renderer::image_cache::atlas::AtlasAllocator;

const FRAMES_IN_FLIGHT: usize = 3;
const CURSOR_ROW_SLOTS: usize = 2;
const ATLAS_SIZE: u32 = 2048;
const ATLAS_MAX_SIZE: u32 = 8192;

fn atlas_size_limit(device: &wgpu::Device) -> u32 {
    device
        .limits()
        .max_texture_dimension_2d
        .min(ATLAS_MAX_SIZE)
        .max(1)
}

fn atlas_start_size(device: &wgpu::Device) -> u32 {
    ATLAS_SIZE.min(atlas_size_limit(device)).max(1)
}

pub struct WgpuGlyphAtlas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    allocator: AtlasAllocator,
    slots: FxHashMap<GlyphKey, AtlasSlot>,
    queue: wgpu::Queue,
    bytes_per_pixel: u32,
    format: wgpu::TextureFormat,
    label: &'static str,
}

impl WgpuGlyphAtlas {
    pub fn new_grayscale(device: &wgpu::Device, queue: wgpu::Queue) -> Self {
        Self::new_with_format(
            device,
            queue,
            wgpu::TextureFormat::R8Unorm,
            1,
            "grid.atlas_grayscale",
        )
    }

    pub fn new_color(device: &wgpu::Device, queue: wgpu::Queue) -> Self {
        Self::new_with_format(
            device,
            queue,
            wgpu::TextureFormat::Rgba8Unorm,
            4,
            "grid.atlas_color",
        )
    }

    fn new_with_format(
        device: &wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
        bytes_per_pixel: u32,
        label: &'static str,
    ) -> Self {
        let size = atlas_start_size(device);
        let texture = create_atlas_texture(device, format, size, label);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            allocator: AtlasAllocator::new(size as u16, size as u16),
            slots: FxHashMap::default(),
            queue,
            bytes_per_pixel,
            format,
            label,
        }
    }

    /// Double the atlas texture, blitting existing texels into the
    /// top-left. Existing `AtlasSlot`s stay valid. Returns `false` when
    /// the device `max_texture_dimension_2d` is already reached (WebGL
    /// is typically 2048). The caller must rebuild any bind group that
    /// sampled the previous view.
    pub fn grow(&mut self, device: &wgpu::Device) -> bool {
        let (old_w, old_h) = self.allocator.dimensions();
        let limit = atlas_size_limit(device);
        if old_w as u32 >= limit {
            return false;
        }
        let new_size = (old_w as u32).saturating_mul(2).min(limit);
        if new_size <= old_w as u32 {
            return false;
        }

        let new_texture = create_atlas_texture(device, self.format, new_size, self.label);
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("grid.atlas_grow"),
            });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &new_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: old_w as u32,
                height: old_h as u32,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        self.texture = new_texture;
        self.view = self
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.allocator.grow_to(new_size as u16, new_size as u16);
        true
    }

    #[inline]
    pub fn lookup(&self, key: GlyphKey) -> Option<AtlasSlot> {
        self.slots.get(&key).copied()
    }

    pub fn insert(
        &mut self,
        key: GlyphKey,
        glyph: RasterizedGlyph<'_>,
    ) -> Option<AtlasSlot> {
        if glyph.width == 0 || glyph.height == 0 {
            let slot = AtlasSlot {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
                bearing_x: glyph.bearing_x,
                bearing_y: glyph.bearing_y,
            };
            self.slots.insert(key, slot);
            return Some(slot);
        }

        let (x, y) = self.allocator.allocate(glyph.width, glyph.height)?;
        let slot = AtlasSlot {
            x,
            y,
            w: glyph.width,
            h: glyph.height,
            bearing_x: glyph.bearing_x,
            bearing_y: glyph.bearing_y,
        };
        self.slots.insert(key, slot);
        write_atlas_texture(
            &self.queue,
            &self.texture,
            x as u32,
            y as u32,
            glyph.width as u32,
            glyph.height as u32,
            self.bytes_per_pixel,
            glyph.bytes,
        );
        Some(slot)
    }

    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.allocator.clear();
        self.slots.clear();
    }

    #[inline]
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }
}

pub struct WgpuGridRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,

    cols: u32,
    rows: u32,

    bg_cpu: [Vec<CellBg>; FRAMES_IN_FLIGHT],
    bg_buffers: [wgpu::Buffer; FRAMES_IN_FLIGHT],

    fg_rows: Vec<Vec<CellText>>,
    fg_buffers: [wgpu::Buffer; FRAMES_IN_FLIGHT],
    fg_capacity: [usize; FRAMES_IN_FLIGHT],
    fg_staging: Vec<CellText>,

    /// GPU-resident instance count in `fg_buffers[0]` from the last
    /// flush. Reused on Noop/CursorOnly frames to skip the concat
    /// and `write_buffer` call. Same pattern as `MetalGridRenderer`.
    fg_live_count: u32,
    /// Any row-level write since the last flush.
    fg_dirty: bool,
    /// `bg_cpu` changed since the last `write_buffer`.
    bg_dirty: bool,

    #[allow(dead_code)]
    frame: usize,

    uniform_buffer: wgpu::Buffer,

    // bg pipeline
    bg_bind_group_layout: wgpu::BindGroupLayout,
    bg_bind_group: wgpu::BindGroup,
    bg_pipeline: wgpu::RenderPipeline,

    // text pipeline. Atlas bind-group layout is kept so grow() can
    // rebuild `text_atlas_bg` against the new texture views.
    #[allow(dead_code)]
    text_uniform_bgl: wgpu::BindGroupLayout,
    text_uniform_bg: wgpu::BindGroup,
    text_atlas_bgl: wgpu::BindGroupLayout,
    text_atlas_bg: wgpu::BindGroup,
    text_pipeline: wgpu::RenderPipeline,

    atlas_grayscale: WgpuGlyphAtlas,
    atlas_color: WgpuGlyphAtlas,

    /// Mirror of `MetalGridRenderer::needs_full_rebuild`. Set on
    /// `new` / `resize`, cleared via `mark_full_rebuild_done`.
    needs_full_rebuild: bool,
}

impl WgpuGridRenderer {
    pub fn new(ctx: &WgpuContext<'_>, cols: u32, rows: u32) -> Self {
        let device = ctx.device.clone();
        let queue = ctx.queue.clone();

        let bg_len = (cols as usize) * (rows as usize);
        let bg_cpu = std::array::from_fn(|_| vec![CellBg::TRANSPARENT; bg_len]);
        let bg_buffers = std::array::from_fn(|_| alloc_bg_buffer(&device, cols, rows));

        let initial_fg_capacity = bg_len.max(1);
        let fg_buffers =
            std::array::from_fn(|_| alloc_fg_buffer(&device, initial_fg_capacity));

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("grid.uniforms"),
            size: std::mem::size_of::<GridUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // bg pipeline — uniforms + storage buffer.
        let bg_bind_group_layout = create_bg_bind_group_layout(&device);
        let bg_bind_group = create_bg_bind_group(
            &device,
            &bg_bind_group_layout,
            &uniform_buffer,
            &bg_buffers[0],
        );

        // text pipeline — uniforms in group(0), atlas textures in group(1).
        let atlas_grayscale = WgpuGlyphAtlas::new_grayscale(&device, queue.clone());
        let atlas_color = WgpuGlyphAtlas::new_color(&device, queue.clone());
        let text_uniform_bgl = create_text_uniform_bgl(&device);
        let text_uniform_bg =
            create_text_uniform_bg(&device, &text_uniform_bgl, &uniform_buffer);
        let text_atlas_bgl = create_text_atlas_bgl(&device);
        let text_atlas_bg = create_text_atlas_bg(
            &device,
            &text_atlas_bgl,
            atlas_grayscale.view(),
            atlas_color.view(),
        );

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("grid.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/grid.wgsl").into()),
        });

        let bg_pipeline =
            build_bg_pipeline(&device, ctx.format, &bg_bind_group_layout, &shader);
        let text_pipeline = build_text_pipeline(
            &device,
            ctx.format,
            &[&text_uniform_bgl, &text_atlas_bgl],
            &shader,
        );

        Self {
            device,
            queue,
            cols,
            rows,
            bg_cpu,
            bg_buffers,
            fg_rows: init_fg_rows(rows),
            fg_buffers,
            fg_capacity: [initial_fg_capacity; FRAMES_IN_FLIGHT],
            fg_staging: Vec::new(),
            fg_live_count: 0,
            fg_dirty: true,
            bg_dirty: true,
            frame: 0,
            uniform_buffer,
            bg_bind_group_layout,
            bg_bind_group,
            bg_pipeline,
            text_uniform_bgl,
            text_uniform_bg,
            text_atlas_bgl,
            text_atlas_bg,
            text_pipeline,
            atlas_grayscale,
            atlas_color,
            needs_full_rebuild: true,
        }
    }

    #[inline]
    pub fn needs_full_rebuild(&self) -> bool {
        self.needs_full_rebuild
    }

    #[inline]
    pub fn mark_full_rebuild_done(&mut self) {
        self.needs_full_rebuild = false;
    }

    pub fn clear_glyph_atlas(&mut self) {
        self.atlas_grayscale.clear();
        self.atlas_color.clear();
        self.needs_full_rebuild = true;
        self.fg_dirty = true;
    }

    pub fn resize(&mut self, cols: u32, rows: u32) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        let bg_len = (cols as usize) * (rows as usize);
        self.bg_cpu = std::array::from_fn(|_| vec![CellBg::TRANSPARENT; bg_len]);
        self.bg_buffers =
            std::array::from_fn(|_| alloc_bg_buffer(&self.device, cols, rows));
        self.fg_rows = init_fg_rows(rows);
        let initial_fg_capacity = bg_len.max(1);
        self.fg_buffers =
            std::array::from_fn(|_| alloc_fg_buffer(&self.device, initial_fg_capacity));
        self.fg_capacity = [initial_fg_capacity; FRAMES_IN_FLIGHT];
        self.needs_full_rebuild = true;
        self.fg_dirty = true;
        self.bg_dirty = true;
        self.fg_live_count = 0;
        self.bg_bind_group = create_bg_bind_group(
            &self.device,
            &self.bg_bind_group_layout,
            &self.uniform_buffer,
            &self.bg_buffers[0],
        );
    }

    pub fn write_row(&mut self, row: u32, bg: &[CellBg], fg: &[CellText]) {
        let idx = (row as usize) + 1;
        if let Some(slot) = self.fg_rows.get_mut(idx) {
            slot.clear();
            slot.extend_from_slice(fg);
            self.fg_dirty = true;
        }

        if row >= self.rows {
            return;
        }
        let row_start = (row as usize) * (self.cols as usize);
        let row_len = (self.cols as usize).min(bg.len());
        let cpu = &mut self.bg_cpu[0];
        cpu[row_start..row_start + row_len].copy_from_slice(&bg[..row_len]);
        for slot in &mut cpu[row_start + row_len..row_start + self.cols as usize] {
            *slot = CellBg::TRANSPARENT;
        }
        self.bg_dirty = true;
    }

    pub fn clear_row(&mut self, row: u32) {
        let idx = (row as usize) + 1;
        if let Some(slot) = self.fg_rows.get_mut(idx) {
            if !slot.is_empty() {
                self.fg_dirty = true;
            }
            slot.clear();
        }
        if row >= self.rows {
            return;
        }
        let row_start = (row as usize) * (self.cols as usize);
        let cpu = &mut self.bg_cpu[0];
        for slot in &mut cpu[row_start..row_start + self.cols as usize] {
            *slot = CellBg::TRANSPARENT;
        }
        self.bg_dirty = true;
    }

    pub fn copy_row(&mut self, src: u32, dst: u32) {
        if src == dst || src >= self.rows || dst >= self.rows {
            return;
        }

        let src_idx = src as usize + 1;
        let dst_idx = dst as usize + 1;
        if src_idx < self.fg_rows.len() && dst_idx < self.fg_rows.len() {
            let mut row = self.fg_rows[src_idx].clone();
            let dst_row = dst.min(u16::MAX as u32) as u16;
            for glyph in &mut row {
                glyph.grid_pos[1] = dst_row;
            }
            self.fg_rows[dst_idx] = row;
            self.fg_dirty = true;
        }

        let cols = self.cols as usize;
        let src_start = src as usize * cols;
        let dst_start = dst as usize * cols;
        self.bg_cpu[0].copy_within(src_start..src_start + cols, dst_start);
        self.bg_dirty = true;
    }

    pub fn snapshot_row_rect(
        &self,
        row: u32,
        cols: std::ops::Range<u32>,
    ) -> Option<GridRowSnapshot> {
        if row >= self.rows {
            return None;
        }
        let col_start = cols.start.min(self.cols);
        let col_end = cols.end.min(self.cols);
        if col_start >= col_end {
            return None;
        }

        let cols_total = self.cols as usize;
        let bg_start = row as usize * cols_total + col_start as usize;
        let bg_end = row as usize * cols_total + col_end as usize;
        let fg = self
            .fg_rows
            .get(row as usize + 1)
            .map(|row| {
                row.iter()
                    .copied()
                    .filter(|glyph| {
                        let col = glyph.grid_pos[0] as u32;
                        col >= col_start && col < col_end
                    })
                    .collect()
            })
            .unwrap_or_default();

        Some(GridRowSnapshot {
            col_start,
            cols: self.bg_cpu[0][bg_start..bg_end].to_vec(),
            fg,
        })
    }

    pub fn write_row_snapshot(&mut self, row: u32, snapshot: &GridRowSnapshot) {
        if row >= self.rows || snapshot.cols.is_empty() {
            return;
        }
        let col_start = snapshot.col_start.min(self.cols) as usize;
        let col_end = (col_start + snapshot.cols.len()).min(self.cols as usize);
        if col_start >= col_end {
            return;
        }

        let cols_total = self.cols as usize;
        let bg_start = row as usize * cols_total + col_start;
        let bg_end = row as usize * cols_total + col_end;
        self.bg_cpu[0][bg_start..bg_end]
            .copy_from_slice(&snapshot.cols[..col_end - col_start]);
        self.bg_dirty = true;

        if let Some(fg_row) = self.fg_rows.get_mut(row as usize + 1) {
            fg_row.retain(|glyph| {
                let col = glyph.grid_pos[0] as usize;
                col < col_start || col >= col_end
            });
            let dst_row = row.min(u16::MAX as u32) as u16;
            fg_row.extend(snapshot.fg.iter().copied().map(|mut glyph| {
                glyph.grid_pos[1] = dst_row;
                glyph.pixel_offset_y = 0;
                glyph
            }));
            self.fg_dirty = true;
        }
    }

    pub fn clear_pixel_offsets(&mut self) {
        let mut bg_changed = false;
        for cell in &mut self.bg_cpu[0] {
            if cell.pixel_offset_y != 0 {
                cell.pixel_offset_y = 0;
                bg_changed = true;
            }
        }
        if bg_changed {
            self.bg_dirty = true;
        }

        let mut fg_changed = false;
        let last = self.fg_rows.len().saturating_sub(1);
        for row in self.fg_rows.iter_mut().take(last).skip(1) {
            for glyph in row {
                if glyph.pixel_offset_y != 0 {
                    glyph.pixel_offset_y = 0;
                    fg_changed = true;
                }
            }
        }
        if fg_changed {
            self.fg_dirty = true;
        }
    }

    pub fn set_pixel_offset_y_for_rows(
        &mut self,
        rows: std::ops::Range<u32>,
        pixel_offset_y: i32,
    ) {
        let start = rows.start.min(self.rows) as usize;
        let end = rows.end.min(self.rows) as usize;
        if start >= end {
            return;
        }
        let cols = self.cols as usize;
        let mut bg_changed = false;
        for row in start..end {
            let row_start = row * cols;
            for cell in &mut self.bg_cpu[0][row_start..row_start + cols] {
                if cell.pixel_offset_y != pixel_offset_y {
                    cell.pixel_offset_y = pixel_offset_y;
                    bg_changed = true;
                }
            }
            if let Some(fg_row) = self.fg_rows.get_mut(row + 1) {
                for glyph in fg_row {
                    if glyph.pixel_offset_y != pixel_offset_y {
                        glyph.pixel_offset_y = pixel_offset_y;
                        self.fg_dirty = true;
                    }
                }
            }
        }
        if bg_changed {
            self.bg_dirty = true;
        }
    }

    pub fn set_block_cursor(&mut self, cells: &[CellText]) {
        if let Some(slot) = self.fg_rows.first_mut() {
            if slot.is_empty() && cells.is_empty() {
                return;
            }
            slot.clear();
            slot.extend_from_slice(cells);
            self.fg_dirty = true;
        }
    }

    pub fn set_non_block_cursor(&mut self, cells: &[CellText]) {
        let idx = self.fg_rows.len().saturating_sub(1);
        if let Some(slot) = self.fg_rows.get_mut(idx) {
            if slot.is_empty() && cells.is_empty() {
                return;
            }
            slot.clear();
            slot.extend_from_slice(cells);
            self.fg_dirty = true;
        }
    }

    pub fn clear_cursor(&mut self) {
        let mut changed = false;
        if let Some(slot) = self.fg_rows.first_mut() {
            if !slot.is_empty() {
                slot.clear();
                changed = true;
            }
        }
        let last = self.fg_rows.len().saturating_sub(1);
        if last > 0 {
            if let Some(slot) = self.fg_rows.get_mut(last) {
                if !slot.is_empty() {
                    slot.clear();
                    changed = true;
                }
            }
        }
        if changed {
            self.fg_dirty = true;
        }
    }

    pub fn lookup_glyph(&self, key: GlyphKey) -> Option<AtlasSlot> {
        self.atlas_grayscale.lookup(key)
    }

    pub fn lookup_glyph_color(&self, key: GlyphKey) -> Option<AtlasSlot> {
        self.atlas_color.lookup(key)
    }

    pub fn insert_glyph(
        &mut self,
        key: GlyphKey,
        glyph: RasterizedGlyph<'_>,
    ) -> Option<AtlasSlot> {
        if let Some(slot) = self.atlas_grayscale.insert(key, glyph) {
            return Some(slot);
        }
        if self.atlas_grayscale.grow(&self.device) {
            self.rebuild_text_atlas_bg();
            return self.atlas_grayscale.insert(key, glyph);
        }
        self.atlas_grayscale.clear();
        self.atlas_grayscale.insert(key, glyph)
    }

    pub fn insert_glyph_color(
        &mut self,
        key: GlyphKey,
        glyph: RasterizedGlyph<'_>,
    ) -> Option<AtlasSlot> {
        if let Some(slot) = self.atlas_color.insert(key, glyph) {
            return Some(slot);
        }
        if self.atlas_color.grow(&self.device) {
            self.rebuild_text_atlas_bg();
            return self.atlas_color.insert(key, glyph);
        }
        self.atlas_color.clear();
        self.atlas_color.insert(key, glyph)
    }

    fn rebuild_text_atlas_bg(&mut self) {
        self.text_atlas_bg = create_text_atlas_bg(
            &self.device,
            &self.text_atlas_bgl,
            self.atlas_grayscale.view(),
            self.atlas_color.view(),
        );
    }

    /// Record bg pass + text pass against the caller's `render_pass`.
    pub fn render<'pass>(
        &'pass mut self,
        render_pass: &mut wgpu::RenderPass<'pass>,
        uniforms: &GridUniforms,
    ) {
        // Uniforms always upload (cheap, and cursor/min_contrast can
        // change without a row write).
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(uniforms));

        // Skip re-uploading bg cells when no row changed — the GPU
        // copy is already correct from the previous frame.
        if self.bg_dirty {
            self.queue.write_buffer(
                &self.bg_buffers[0],
                0,
                bytemuck::cast_slice(&self.bg_cpu[0]),
            );
            self.bg_dirty = false;
        }

        // ---------- bg pass ----------
        render_pass.set_pipeline(&self.bg_pipeline);
        render_pass.set_bind_group(0, &self.bg_bind_group, &[]);
        render_pass.draw(0..3, 0..1);

        // ---------- text pass ----------
        if self.fg_dirty {
            self.fg_staging.clear();
            for row in &self.fg_rows {
                self.fg_staging.extend_from_slice(row);
            }

            if self.fg_staging.len() > self.fg_capacity[0] {
                let new_cap = self.fg_staging.len().next_power_of_two();
                self.fg_buffers[0] = alloc_fg_buffer(&self.device, new_cap);
                self.fg_capacity[0] = new_cap;
            }
            self.queue.write_buffer(
                &self.fg_buffers[0],
                0,
                bytemuck::cast_slice(&self.fg_staging),
            );
            self.fg_live_count = self.fg_staging.len() as u32;
            self.fg_dirty = false;
        }

        let instance_count = self.fg_live_count as usize;
        if instance_count == 0 {
            return;
        }

        render_pass.set_pipeline(&self.text_pipeline);
        render_pass.set_bind_group(0, &self.text_uniform_bg, &[]);
        render_pass.set_bind_group(1, &self.text_atlas_bg, &[]);
        render_pass.set_vertex_buffer(0, self.fg_buffers[0].slice(..));
        // 4 vertices per instance → triangle strip quad.
        render_pass.draw(0..4, 0..instance_count as u32);
    }
}

// ---------- buffer / layout / pipeline helpers ----------

fn create_atlas_texture(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    size: u32,
    label: &'static str,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// Pad each source row so `bytes_per_row` is legal on WebGL and WebGPU.
///
/// WebGL2 `texSubImage2D` still honors the default `UNPACK_ALIGNMENT`
/// of 4 unless the implementation sets it to 1 (wgpu-hal does that
/// only on native GLES). A 6- or 7-wide R8 glyph therefore uploads
/// as empty / garbled texels while the slot and advance stay valid —
/// letters vanish, spacing remains. WebGPU additionally requires
/// `bytesPerRow` to be a multiple of 256 whenever it is specified.
/// Native `Queue::write_texture` will re-pad internally; doing it
/// here keeps every backend on one code path.
fn write_atlas_texture(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    bytes_per_pixel: u32,
    src: &[u8],
) {
    if width == 0 || height == 0 {
        return;
    }
    let unpadded = width.saturating_mul(bytes_per_pixel);
    let padded = unpadded
        .div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        .saturating_mul(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        .max(unpadded);
    let owned;
    let bytes: &[u8] = if padded == unpadded {
        src
    } else {
        let mut buf = vec![0u8; padded as usize * height as usize];
        for row in 0..height as usize {
            let src_off = row * unpadded as usize;
            let dst_off = row * padded as usize;
            buf[dst_off..dst_off + unpadded as usize]
                .copy_from_slice(&src[src_off..src_off + unpadded as usize]);
        }
        owned = buf;
        &owned
    };

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(padded),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

fn alloc_bg_buffer(device: &wgpu::Device, cols: u32, rows: u32) -> wgpu::Buffer {
    let size = (cols as u64)
        .saturating_mul(rows as u64)
        .saturating_mul(std::mem::size_of::<CellBg>() as u64)
        .max(std::mem::size_of::<CellBg>() as u64);
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("grid.bg_cells"),
        size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn alloc_fg_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    let size = (capacity as u64)
        .saturating_mul(std::mem::size_of::<CellText>() as u64)
        .max(std::mem::size_of::<CellText>() as u64);
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("grid.fg_cells"),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn init_fg_rows(rows: u32) -> Vec<Vec<CellText>> {
    (0..(rows as usize + CURSOR_ROW_SLOTS))
        .map(|_| Vec::new())
        .collect()
}

fn create_bg_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("grid.bg_bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(std::mem::size_of::<
                        GridUniforms,
                    >()
                        as u64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    })
}

fn create_bg_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform_buffer: &wgpu::Buffer,
    bg_buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("grid.bg_bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: bg_buffer.as_entire_binding(),
            },
        ],
    })
}

fn create_text_uniform_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("grid.text_uniform_bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: std::num::NonZeroU64::new(std::mem::size_of::<
                    GridUniforms,
                >() as u64),
            },
            count: None,
        }],
    })
}

fn create_text_uniform_bg(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform_buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("grid.text_uniform_bg"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buffer.as_entire_binding(),
        }],
    })
}

fn create_text_atlas_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("grid.text_atlas_bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ],
    })
}

fn create_text_atlas_bg(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    grayscale: &wgpu::TextureView,
    color: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("grid.text_atlas_bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(grayscale),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(color),
            },
        ],
    })
}

fn premultiplied_blend() -> wgpu::BlendState {
    // Premultiplied-over, matching Text fragment returns
    // premultiplied RGBA (`in.color * mask_a` for grayscale, atlas
    // sample for color), so source RGB must be `One`.
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
    }
}

fn build_bg_pipeline(
    device: &wgpu::Device,
    color_format: wgpu::TextureFormat,
    bg_bgl: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("grid.bg_pl"),
        bind_group_layouts: &[bg_bgl],
        immediate_size: 0,
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("grid.bg"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("grid_bg_vertex"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("grid_bg_fragment"),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: Some(premultiplied_blend()),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn build_text_pipeline(
    device: &wgpu::Device,
    color_format: wgpu::TextureFormat,
    bgls: &[&wgpu::BindGroupLayout],
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("grid.text_pl"),
        bind_group_layouts: bgls,
        immediate_size: 0,
    });

    // Per-instance vertex buffer layout — mirrors `CellText`.
    let attrs = &[
        // @location(0) glyph_pos: vec2<u32> @ offset 0
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint32x2,
            offset: 0,
            shader_location: 0,
        },
        // @location(1) glyph_size: vec2<u32> @ offset 8
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint32x2,
            offset: 8,
            shader_location: 1,
        },
        // @location(2) bearings: vec2<i32> @ offset 16 — stored as i16x2,
        // widened to i32 in the shader via `Sint16x2`.
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Sint16x2,
            offset: 16,
            shader_location: 2,
        },
        // @location(3) grid_pos: vec2<u32> @ offset 20 — stored as u16x2.
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint16x2,
            offset: 20,
            shader_location: 3,
        },
        // @location(4) color: vec4<f32> @ offset 24 — UNorm8x4 → 0..1.
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Unorm8x4,
            offset: 24,
            shader_location: 4,
        },
        // @location(5) atlas: u32 @ offset 28 — u8 widened.
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint8,
            offset: 28,
            shader_location: 5,
        },
        // @location(6) bools: u32 @ offset 29 — u8 widened.
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint8,
            offset: 29,
            shader_location: 6,
        },
        // @location(7) pixel_offset_y: i32 @ offset 32.
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Sint32,
            offset: 32,
            shader_location: 7,
        },
    ];
    let vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<CellText>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: attrs,
    };

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("grid.text"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("grid_text_vertex"),
            buffers: &[vbuf_layout],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("grid_text_fragment"),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: Some(premultiplied_blend()),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}
