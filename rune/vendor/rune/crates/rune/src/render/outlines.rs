//! The outline store's texture (`text::OutlineStore`, binding 4 of the Windfoil shader): its
//! changed rows go up with each frame's uploads, and when the store outgrows it a taller one
//! replaces it and every bind group is made again.

use rune_staging::Staging;

use super::*;

/// Rows the texture starts with (256 KB).
pub(super) const MIN_ROWS: u32 = 16;

impl Renderer {
    pub(super) fn outline_texture(device: &wgpu::Device, rows: u32) -> wgpu::Texture {
        Self::texture(device, "glyph outlines", wgpu::TextureFormat::Rgba32Uint, crate::text::outline::WIDTH, rows)
    }

    /// Hands text's new outline records to the GPU, growing the texture first if they don't fit.
    pub(super) fn upload_outlines(&mut self, device: &wgpu::Device, staging: &Staging, atlas: &mut Atlas) {
        let store = &mut atlas.outlines;
        let variant::Uber::Windfoil { outlines, .. } = &mut self.uber else { return };
        if store.rows() > outlines.height() {
            *outlines = Self::outline_texture(device, store.rows().next_power_of_two());
            store.invalidate();
            self.rebind_all(device);
        }
        let variant::Uber::Windfoil { outlines, .. } = &self.uber else { return };
        if let Some((y, rows)) = store.take_dirty() {
            staging.write_texture_rows(outlines, y, rows);
        }
    }

    /// Every bind group again, for a new shader or outline texture.
    pub(super) fn rebind_all(&mut self, device: &wgpu::Device) {
        let outlines = self.uber.outlines();
        self.bind_group = Self::bind_group(device, &self.bind_layout, &self.globals, outlines, &self.atlas, &self.stand_in, None);
        for page in 0..self.atlas_pages.len() {
            let texture = &self.atlas_pages[page].0;
            let bind = Self::bind_group(device, &self.bind_layout, &self.globals, outlines, texture, texture, None);
            self.atlas_pages[page].1 = bind;
        }
        for page in 0..self.images.len() {
            let Some((texture, _)) = &self.images[page] else { continue };
            let bind = Self::bind_group(device, &self.bind_layout, &self.globals, outlines, &self.atlas, texture, None);
            if let Some(slot) = &mut self.images[page] { slot.1 = bind; }
        }
        self.rebind_layers(device);
        if let variant::Uber::Windfoil { outlines, fill, .. } = &mut self.uber {
            fill.rebind(device, fill::FillBindings { layout: &self.bind_layout, globals: &self.globals, outlines, stand_in: &self.stand_in });
        }
    }
}
