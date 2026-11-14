//! Atlas masks the GPU fills (`FontSmoothing::Windfoil`): the renderer draws each one's outline
//! coverage straight into its page (render/fill.rs), so a mask at rest and the same glyph drawn
//! from its outline in motion come from one coverage function.
//!
//! Each atlas entry's residency is a state machine ([`Residency`]). A GPU-filled mask's texels
//! live only on the GPU, so the page's CPU copy keeps zeros there, and when the renderer uploads
//! CPU rows over it (an icon placed beside it, a new texture) it is filled again.

use super::*;
use crate::machine::Effects;

/// One mask the renderer fills: its outline record and hint, scale (px per font unit), dilation
/// radius (px) and pen relative to the mask's top-left texel (px, y down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fill {
    pub record: u32,
    pub hint: u32,
    pub scale: f32,
    pub radius: f32,
    pub pen: [f32; 2],
}

/// Where an atlas entry's texels are.
#[derive(Clone, Copy)]
pub(super) enum Residency {
    /// It draws nothing (a space).
    Blank,
    /// In the page's CPU copy, which goes up with its rows.
    Uploaded(AtlasGlyph),
    /// Its fill hasn't run since its texels last changed; it's in its page's queue.
    Pending(AtlasGlyph, Fill),
    /// Filled on the GPU.
    Filled(AtlasGlyph, Fill),
}

#[derive(Clone, Copy)]
pub(super) enum ResidencyEvent {
    /// The renderer ran the fills queued for its page.
    FillRan,
    /// The renderer uploaded rows `y0..y1` of `page` from the CPU copy.
    RowsUploaded { page: u16, rows: (u32, u32) },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ResidencyEffect {
    /// Queue the entry's fill on its page.
    Queue,
}

impl Residency {
    pub(super) fn step(&mut self, event: ResidencyEvent) -> Effects<ResidencyEffect> {
        match (*self, event) {
            (Residency::Pending(g, f), ResidencyEvent::FillRan) => {
                *self = Residency::Filled(g, f);
                Effects::none()
            }
            // The rows hold the CPU copy's zeros over its texels (and their one-texel border).
            (Residency::Filled(g, f), ResidencyEvent::RowsUploaded { page, rows: (y0, y1) })
                if g.page == page && g.y < y1 + 1 && g.y + g.h + 1 > y0 => {
                *self = Residency::Pending(g, f);
                Effects::one(ResidencyEffect::Queue)
            }
            _ => Effects::none(),
        }
    }

    /// The mask a quad samples, if it draws.
    pub(super) fn mask(&self) -> Option<AtlasGlyph> {
        match *self {
            Residency::Blank => None,
            Residency::Uploaded(g) | Residency::Pending(g, _) | Residency::Filled(g, _) => Some(g),
        }
    }

    /// Whether its texels come from an outline record.
    fn outline_filled(&self) -> bool {
        matches!(self, Residency::Pending(..) | Residency::Filled(..))
    }
}

/// One fill for the renderer: the mask's place on its page and its outline.
#[derive(Clone, Copy, Debug)]
pub struct QueuedFill {
    /// Texels `[x, y, w, h]`, the one-texel border not included.
    pub rect: [u32; 4],
    pub fill: Fill,
}

impl Atlas {
    /// Room for a `w`×`h` mask the GPU fills, with zeros in the CPU copy under it so a later row
    /// upload doesn't bring back an older mask; queued for the renderer.
    pub(super) fn reserve(&mut self, key: GlyphKey, w: u32, h: u32, left: i32, top: i32, fill: Fill) -> Option<AtlasGlyph> {
        let (page, ax, ay) = self.alloc(w, h)?;
        let data = if page == 0 { &mut self.data } else { &mut self.pages[page - 1].data };
        for row in ay - 1..ay + h + 1 {
            let line = row as usize * ATLAS_SIZE as usize;
            data[line + ax as usize - 1..line + (ax + w + 1) as usize].fill(0);
        }
        self.rasterized += 1;
        self.touch(page as u16, w * h);
        let g = AtlasGlyph { page: page as u16, x: ax, y: ay, w, h, left, top };
        self.map.insert(key, Residency::Pending(g, fill));
        self.queue(page, key);
        Some(g)
    }

    fn queue(&mut self, page: usize, key: GlyphKey) {
        if self.fills.len() <= page {
            self.fills.resize_with(page + 1, Vec::new);
        }
        self.fills[page].push(key);
    }

    /// Steps every entry on `page` with `event`, queueing those it asks to.
    pub(crate) fn rows_uploaded(&mut self, page: usize, rows: (u32, u32)) {
        let event = ResidencyEvent::RowsUploaded { page: page as u16, rows };
        let mut queued = Vec::new();
        for (key, entry) in self.map.iter_mut() {
            if entry.step(event).into_iter().any(|e| e == ResidencyEffect::Queue) { queued.push(*key); }
        }
        for key in queued { self.queue(page, key); }
    }

    /// Each page's queued fills, stepped as run: the renderer draws them before the frame.
    pub(crate) fn take_fills(&mut self) -> Vec<(usize, Vec<QueuedFill>)> {
        let mut out = Vec::new();
        for (page, keys) in std::mem::take(&mut self.fills).into_iter().enumerate() {
            let mut fills = Vec::with_capacity(keys.len());
            for key in keys {
                // A key whose page went since may live on another page now, queued there.
                let Some(entry) = self.map.get_mut(&key) else { continue };
                if let Residency::Pending(g, fill) = *entry && g.page as usize == page {
                    fills.push(QueuedFill { rect: [g.x, g.y, g.w, g.h], fill });
                    let _ = entry.step(ResidencyEvent::FillRan);
                }
            }
            if !fills.is_empty() { out.push((page, fills)); }
        }
        out
    }

    /// Drops every mask an outline record filled: the outline store started again, so their
    /// records are gone. Their texels stay until their shelves are reused.
    pub(super) fn forget_outline_masks(&mut self) {
        self.map.retain(|_, e| !e.outline_filled());
        self.fills.clear();
        self.clock += 1;
    }
}
