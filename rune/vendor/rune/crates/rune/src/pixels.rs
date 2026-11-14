//! Pixel buffers an app replaces at will (a CPU-shaded HUD frame, a video frame), drawn by
//! `El::pixels`. Each runtime keeps a CPU-side RGBA atlas the renderer uploads by dirty rows,
//! like the glyph atlas, allocated only once an element shows pixels.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const IMAGE_ATLAS_SIZE: u32 = 1024;

struct Frame {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    generation: u64,
    mips: Option<Arc<crate::image::Decoded>>,
}

/// A replaceable RGBA image, shared between the producer and the view. Clone it freely.
#[derive(Clone)]
pub struct Pixels {
    id: u64,
    frame: Arc<Mutex<Frame>>,
    /// Wakes the windows and overlays whose scene shows this image.
    pub(crate) source: Arc<crate::live::Source>,
    /// The URL or path an `ImageCache` loaded it from.
    url: Option<Arc<str>>,
    /// The candidates `ImageCache::url_set` chose `url` from, as an HTML `srcset`.
    srcset: Option<Arc<str>>,
    /// The `ImageCache` entry these pixels came from, so the cache still finds it while
    /// anything draws them, and a view that asks for the image again on its next build gets it
    /// back instead of fetching it anew.
    keep: Option<Arc<dyn std::any::Any + Send + Sync>>,
}

impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let frame = self.frame.lock().unwrap();
        write!(f, "Pixels({}, {}x{}, gen {})", self.id, frame.width, frame.height, frame.generation)
    }
}

impl Default for Pixels {
    fn default() -> Self {
        Self::new()
    }
}

impl Pixels {
    pub fn new() -> Pixels {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Pixels {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            frame: Arc::new(Mutex::new(Frame { width: 0, height: 0, rgba: Vec::new(), generation: 0, mips: None })),
            source: crate::live::Source::new(0),
            url: None,
            srcset: None,
            keep: None,
        }
    }

    pub(crate) fn from_url(url: Option<Arc<str>>, srcset: Option<Arc<str>>) -> Pixels {
        Pixels { url, srcset, ..Pixels::new() }
    }

    pub(crate) fn keeping(mut self, keep: Arc<dyn std::any::Any + Send + Sync>) -> Pixels {
        self.keep = Some(keep);
        self
    }

    /// The URL ([`ImageCache::url`](crate::ImageCache::url)) or path
    /// ([`ImageCache::file`](crate::ImageCache::file)) the image was loaded from, as given, so
    /// a page export can point an `<img>` at it.
    pub fn source_url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// For an image from [`ImageCache::url_set`](crate::ImageCache::url_set), every size it
    /// chose from, as an HTML `srcset` (`"a-640.avif 640w, a-1280.avif 1280w"`).
    pub fn source_set(&self) -> Option<&str> {
        self.srcset.as_deref()
    }

    /// Replaces the image: `rgba` is `width * height` straight-alpha sRGB pixels, rows top
    /// first. Call it from any thread: it wakes only the windows and overlays showing the
    /// image, and their next frame uploads it. Sets between two frames coalesce into one wake
    /// and one upload of the latest image; a hidden or covered window catches up when it shows
    /// again. This is the streaming path: no decoding or mip generation, and a same-size
    /// frame reuses its atlas slot.
    pub fn set(&self, width: u32, height: u32, rgba: Vec<u8>) {
        assert_eq!(Some(rgba.len()), (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(4)), "rgba must hold width * height pixels");
        {
            let mut frame = self.frame.lock().unwrap();
            frame.width = width;
            frame.height = height;
            frame.rgba = rgba;
            frame.generation += 1;
            frame.mips = None;
        }
        self.source.notify();
    }

    pub(crate) fn set_mips(&self, mips: Arc<crate::image::Decoded>) {
        {
            let mut frame = self.frame.lock().unwrap();
            frame.width = mips.mips[0].width;
            frame.height = mips.mips[0].height;
            frame.rgba.clear();
            frame.mips = Some(mips);
            frame.generation += 1;
        }
        self.source.notify();
    }

    pub fn size(&self) -> (u32, u32) {
        let frame = self.frame.lock().unwrap();
        (frame.width, frame.height)
    }

    pub(crate) fn signature(&self) -> (u64, u64) {
        let frame = self.frame.lock().unwrap();
        (self.id, frame.generation)
    }

    #[cfg(test)]
    pub(crate) fn rgba(&self) -> Vec<u8> {
        self.frame.lock().unwrap().rgba.clone()
    }
}

/// Where an image of `size` texels draws in `rect` ([x, y, w, h], device pixels) and which window
/// of it shows, as fractions [x0, y0, x1, y1]. `None` stretches; `Fit` letterboxes (on whole
/// pixels at unscaled rest, continuous while scaled); `Fill` keeps the rect and crops the
/// overflow. Spare space and overflow split by `place`'s anchor (centred by default); an unscaled
/// place draws one texel per pixel and crops or leaves the rest empty.
#[cfg(test)]
pub(crate) fn fit(
    fit: Option<crate::SurfaceFit>,
    place: crate::native_surface::place::SurfacePlace,
    rect: [f32; 4],
    size: (u32, u32),
) -> ([f32; 4], [f32; 4]) {
    fit_in(fit, place, rect, size, 1.0)
}

pub(crate) fn fit_in(
    fit: Option<crate::SurfaceFit>,
    place: crate::native_surface::place::SurfacePlace,
    rect: [f32; 4],
    size: (u32, u32),
    snap_weight: f32,
) -> ([f32; 4], [f32; 4]) {
    let whole = [0.0, 0.0, 1.0, 1.0];
    let [x, y, w, h] = rect;
    let (iw, ih) = (size.0 as f32, size.1 as f32);
    if fit.is_none() || iw <= 0.0 || ih <= 0.0 || w <= 0.0 || h <= 0.0 { return (rect, whole); }
    let [kx, ky] = place.share();
    let u = if (w - w.round()).abs() < 1.0 / 512.0 && (h - h.round()).abs() < 1.0 / 512.0 {
        snap_weight.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let blend = |exact: [f32; 4], snapped: [f32; 4]| {
        if u <= 0.0 {
            exact
        } else if u >= 1.0 {
            snapped
        } else {
            std::array::from_fn(|i| exact[i] + (snapped[i] - exact[i]) * u)
        }
    };
    if place.unscaled {
        let (dw, dh) = (iw.min(w), ih.min(h));
        let (sx, sy) = (dw / iw, dh / ih);
        let ((x0, x1), (y0, y1)) = (window(sx, kx), window(sy, ky));
        let exact = [x + (w - dw) * kx, y + (h - dh) * ky, dw, dh];
        let snapped = [x + ((w - dw) * kx).round(), y + ((h - dh) * ky).round(), dw, dh];
        return (blend(exact, snapped), [x0, y0, x1, y1]);
    }
    if fit == Some(crate::SurfaceFit::Fit) {
        let k = (w / iw).min(h / ih);
        let (dw, dh) = ((iw * k).clamp(1.0, w), (ih * k).clamp(1.0, h));
        let exact = [x + (w - dw) * kx, y + (h - dh) * ky, dw, dh];
        if u <= 0.0 {
            return (exact, whole);
        }
        let (sw, sh) = ((iw * k).round().clamp(1.0, w), (ih * k).round().clamp(1.0, h));
        let snapped = [x + ((w - sw) * kx).round(), y + ((h - sh) * ky).round(), sw, sh];
        return (blend(exact, snapped), whole);
    }
    let k = (w / iw).max(h / ih);
    let (sx, sy) = (w / (iw * k), h / (ih * k));
    let ((x0, x1), (y0, y1)) = (window(sx, kx), window(sy, ky));
    (rect, [x0, y0, x1, y1])
}

/// The visible fraction `[a, b]` of an axis when `span` of it shows and `share` of the hidden part lies before.
fn window(span: f32, share: f32) -> (f32, f32) {
    if share == 0.5 { return ((1.0 - span) * 0.5, (1.0 + span) * 0.5); }
    let a = (1.0 - span) * share;
    (a, a + span)
}

struct Slot {
    page: usize,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    generation: u64,
    /// The animation frame copied in.
    frame: usize,
}

/// One stable shelf page, or an exact-size standalone page for a large image.
pub struct ImagePage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    /// Rows `[y0, y1)` changed since the last upload.
    pub dirty: Option<(u32, u32)>,
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    /// Drawn from in the last completed frame.
    shown: bool,
    /// Drawn from in this frame so far; only ever set until the frame ends.
    touched: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub rect: [u32; 4],
    pub page: usize,
    pub premultiplied: bool,
}

/// Streaming buffers keep one slot. Static images keep a slot per sampled mip level.
#[derive(Default)]
pub struct ImageAtlas {
    pub pages: Vec<ImagePage>,
    slots: HashMap<(u64, usize), Slot>,
    /// When each animated image drawn here started playing, on the animation clock: its first
    /// draw, kept while the atlas holds it.
    starts: HashMap<u64, f64>,
    /// Frames copied in so far, so the idle meter sees new pixels in a slot it already drew.
    #[cfg(debug_assertions)]
    pub(crate) copies: u64,
}

impl ImageAtlas {
    /// The atlas origin and size of `pixels`, copying a new frame in when it changed. None while
    /// it's empty or doesn't fit.
    pub fn place(&mut self, pixels: &Pixels, drawn: [f32; 2]) -> Option<Placed> {
        self.place_at(pixels, drawn, None).map(|(placed, _)| placed)
    }

    /// [`Self::place`] at `now` on the animation clock: an animated image shows the frame for
    /// the time since it was first drawn here, and says when (on the same clock) the next one
    /// shows, or `None` for a still or an animation that has stopped. `None` for `now` shows
    /// an animation's first frame.
    pub(crate) fn place_at(&mut self, pixels: &Pixels, drawn: [f32; 2], now: Option<f64>) -> Option<(Placed, Option<f64>)> {
        let frame = pixels.frame.lock().unwrap();
        let (mut w, mut h) = (frame.width, frame.height);
        if w == 0 || h == 0 || w > 8192 || h > 8192 {
            return None;
        }
        let mut level = 0;
        let mut shown = 0;
        let mut next = None;
        let bytes = if let Some(decoded) = &frame.mips {
            let mips = match (&decoded.motion, now) {
                (Some(motion), Some(now)) => {
                    let start = *self.starts.entry(pixels.id).or_insert(now);
                    let (i, change) = motion.at(now - start);
                    (shown, next) = (i, change.map(|t| start + t));
                    motion.frame(&decoded.mips, i)
                }
                _ => &decoded.mips[..],
            };
            // Choose the smallest prefiltered level that still covers both device-pixel axes.
            // No mip construction or resampling happens on the render thread.
            while let Some(next) = mips.get(level + 1) {
                if (next.width as f32) < drawn[0].clamp(1.0, frame.width as f32)
                    || (next.height as f32) < drawn[1].clamp(1.0, frame.height as f32) { break; }
                level += 1;
            }
            let mip = &mips[level];
            (w, h) = (mip.width, mip.height);
            &mip.pixels
        } else { &frame.rgba };
        let key = (pixels.id, level);
        if self.slots.get(&key).is_some_and(|s| s.w != w || s.h != h) {
            self.slots.remove(&key);
        }
        if !self.slots.contains_key(&key) {
            let (page, x, y) = self.allocate(w, h);
            self.slots.insert(key, Slot { page, x, y, w, h, generation: 0, frame: 0 });
        }
        let slot = self.slots.get_mut(&key).unwrap();
        self.pages[slot.page].touched = true;
        if slot.generation != frame.generation || slot.frame != shown {
            slot.generation = frame.generation;
            slot.frame = shown;
            #[cfg(debug_assertions)]
            { self.copies += 1; }
            let page = &mut self.pages[slot.page];
            let row = (page.width * 4) as usize;
            for line in 0..h as usize {
                let dst = (slot.y as usize + line) * row + slot.x as usize * 4;
                let src = line * w as usize * 4;
                page.data[dst..dst + w as usize * 4].copy_from_slice(&bytes[src..src + w as usize * 4]);
            }
            let (y0, y1) = (slot.y, slot.y + h);
            page.dirty = Some(page.dirty.map_or((y0, y1), |(a, b)| (a.min(y0), b.max(y1))));
        }
        Some((Placed { rect: [slot.x, slot.y, w, h], page: slot.page, premultiplied: frame.mips.is_some() }, next))
    }

    /// Forgets images no element showed since the last call, except on pages that still show
    /// one and on the first page: shelves never reuse a slot's space until their page starts
    /// over, so an image shown again there (a view come back to) needs no copy or upload. A
    /// page left with nothing starts over so replaced sizes don't fill the atlas; a full atlas
    /// starts over a page that showed nothing last frame or yet this one before it adds one.
    pub fn end_frame(&mut self) {
        let pages = &self.pages;
        let first_kept = pages.first().is_some_and(|p| p.width <= IMAGE_ATLAS_SIZE && p.height <= IMAGE_ATLAS_SIZE);
        self.slots.retain(|_, s| pages[s.page].touched || (s.page == 0 && first_kept));
        if !self.starts.is_empty() {
            let slots = &self.slots;
            self.starts.retain(|id, _| slots.keys().any(|(slot, _)| slot == id));
        }
        for (i, page) in self.pages.iter_mut().enumerate() {
            page.shown = std::mem::take(&mut page.touched);
            if !self.slots.values().any(|slot| slot.page == i) {
                (page.shelf_x, page.shelf_y, page.shelf_h) = (0, 0, 0);
                page.dirty = None;
                if page.width > IMAGE_ATLAS_SIZE || page.height > IMAGE_ATLAS_SIZE {
                    page.data = Vec::new();
                }
            }
        }
        let keep = self.slots.values().map(|s| s.page + 1).max().unwrap_or(0);
        self.pages.truncate(keep);
    }

    /// Marks every row for upload (a new texture).
    pub fn invalidate(&mut self) {
        for page in &mut self.pages { page.dirty = Some((0, page.height)); }
    }

    fn allocate(&mut self, w: u32, h: u32) -> (usize, u32, u32) {
        for (i, page) in self.pages.iter_mut().enumerate() {
            if page.data.is_empty() { *page = ImagePage::new(w, h); }
            if let Some((x, y)) = page.allocate(w, h) { return (i, x, y); }
        }
        // Full: start over a shelf page that showed nothing last frame or yet this one. A page
        // not drawn from yet this frame may still be, later in the tree.
        let idle = |p: &ImagePage| !p.shown && !p.touched && p.width <= IMAGE_ATLAS_SIZE && p.height <= IMAGE_ATLAS_SIZE
            && p.shelf_y + p.shelf_h > 0;
        if w <= IMAGE_ATLAS_SIZE && h <= IMAGE_ATLAS_SIZE && let Some(i) = self.pages.iter().position(idle) {
            self.slots.retain(|_, s| s.page != i);
            let page = &mut self.pages[i];
            (page.shelf_x, page.shelf_y, page.shelf_h) = (0, 0, 0);
            let (x, y) = page.allocate(w, h).unwrap();
            return (i, x, y);
        }
        let mut page = ImagePage::new(w, h);
        let (x, y) = page.allocate(w, h).unwrap();
        self.pages.push(page);
        (self.pages.len() - 1, x, y)
    }
}

impl ImagePage {
    fn new(w: u32, h: u32) -> Self {
        let (width, height) = if w > IMAGE_ATLAS_SIZE || h > IMAGE_ATLAS_SIZE { (w, h) }
            else { (IMAGE_ATLAS_SIZE, IMAGE_ATLAS_SIZE) };
        Self { width, height, data: vec![0; width as usize * height as usize * 4],
            dirty: None, shelf_x: 0, shelf_y: 0, shelf_h: 0, shown: false, touched: false }
    }

    fn allocate(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if w > self.width || h > self.height { return None; }
        if self.shelf_x + w > self.width {
            self.shelf_y += self.shelf_h;
            self.shelf_x = 0;
            self.shelf_h = 0;
        }
        if self.shelf_y + h > self.height {
            return None;
        }
        let at = (self.shelf_x, self.shelf_y);
        self.shelf_x += w;
        self.shelf_h = self.shelf_h.max(h);
        Some(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_pages_interleave_with_video_and_retained_springs() {
        use crate::{Cx, El, Env, Runtime, el};
        use crate::native_surface::{ByteFrame, ColorInfo, Format, NativeSurface, SurfaceFit};
        use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
        type State = ([Pixels; 3], NativeSurface);
        fn view(s: &State, _: &mut Cx<State>) -> El {
            el().size(8.0, 8.0).pixels(&s.0[0]).child(
                el().size(6.0, 6.0).pin(Some(1.0), Some(1.0), None, None).native_surface(&s.1, SurfaceFit::Fill)
                    .child(el().size(2.0, 2.0).pin(Some(1.0), Some(1.0), None, None).pixels(&s.0[1]))
                    .child(el().size(1.0, 1.0).pin(Some(4.0), Some(4.0), None, None).pixels(&s.0[2]))
            )
        }
        let pixels = [Pixels::new(), Pixels::new(), Pixels::new()];
        pixels[0].set(8, 8, [255,0,0,255].repeat(64));
        pixels[1].set(2, 2, [0,255,0,255].repeat(4));
        pixels[2].set(1024, 1024, [255;4].repeat(1024 * 1024));
        let surface = NativeSurface::new();
        surface.push_bytes(ByteFrame { width: 6, height: 6, format: Format::Bgra,
            color: ColorInfo::default(), planes: [[255,0,0,255].repeat(36), vec![]] }, 1).unwrap();
        let mut rt = Runtime::new((pixels, surface), view, (8.0, 8.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert_eq!(rt.images.pages.len(), 2);
        let gpu = Gpu::headless();
        let zero = crate::gpu::springs::Track::constant(0.0);
        let node = crate::gpu::springs::Node { x: zero, y: zero, opacity: crate::gpu::springs::Track::constant(1.0) };
        let mut renderer = Renderer::with_springs(&gpu.device, OFFSCREEN_FORMAT, &[node], &vec![0; rt.instances.len()]);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (8,8));
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (8,8));
        let target = Offscreen::new(&gpu.device, 8, 8);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut encoder, &target.view, [0.0;4]);
        target.copy(&mut encoder);
        renderer.submit(&gpu.queue, encoder);
        let out = target.read(&gpu.device);
        for (x, y, want) in [(0,0,[255,0,0,255]), (1,1,[0,0,255,255]), (2,2,[0,255,0,255]), (5,5,[255;4])] {
            assert_eq!(&out[(y*8+x)*4..(y*8+x+1)*4], &want, "at {x},{y}");
        }
    }

    /// An emoji in a text element reaches the screen in colour, from the image atlas.
    #[test]
    fn gpu_emoji_in_text_draws_in_colour() {
        use crate::{Color, Cx, El, Env, Runtime, TextStyle, el, text};
        use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
        fn view(_: &(), _: &mut Cx<()>) -> El {
            el().size(80.0, 24.0).fill(Color::BLACK).child(text("Hi 🎉").font(TextStyle::new(16.0, 400.0)).color(Color::WHITE))
        }
        let mut rt = Runtime::new((), view, (80.0, 24.0), Env::new(true, 2.0));
        rt.frame(0.0);
        if rt.images.pages.is_empty() {
            eprintln!("skipping: no colour emoji font installed");
            return;
        }
        let (w, h) = (160, 48);
        let gpu = Gpu::headless();
        let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (w, h));
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        let target = Offscreen::new(&gpu.device, w, h);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
        target.copy(&mut enc);
        gpu.queue.submit([enc.finish()]);
        let out = target.read(&gpu.device);
        if let Some(path) = std::env::var_os("RUNE_EMOJI_PNG") {
            crate::render::write_png(std::path::Path::new(&path), w, h, &out).unwrap();
        }
        let coloured = out.chunks(4).filter(|p| p[0].abs_diff(p[1]) > 60 || p[1].abs_diff(p[2]) > 60).count();
        assert!(coloured > 100, "only {coloured} coloured pixels");
    }

    #[test]
    fn an_image_over_its_fill_fades_into_a_soft_edge_like_a_flat_fill() {
        use crate::{Cx, El, Env, Runtime, ScrollSpec, col, el, row, image::ImageCache};
        use crate::color::hex;
        use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
        // A dark image on a lighter placeholder with a border, as a chat thumbnail, beside a
        // flat fill of the image's colour, under a scroll's 16 pt top fade.
        fn view(s: &Pixels, _: &mut Cx<Pixels>) -> El {
            let thumb = el().size(40.0, 60.0).radius(8.0).fill(hex("#5a5a5a").unwrap())
                .border(0.5, hex("#ffffff33").unwrap()).pixels_fit(s, crate::SurfaceFit::Fill);
            el().size(100.0, 60.0).fill(hex("#141414").unwrap()).child(
                col().w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).clip_fade(0.0, 16.0, 0.0, 8.0)
                    .child(row().gap(10.0).child(thumb).child(el().size(40.0, 60.0).fill(hex("#282828").unwrap())))
                    .child(el().size(4.0, 200.0)))
        }
        let cache = ImageCache::new(Arc::new(|| {}));
        let image = cache.rgba(80, 120, [40u8, 40, 40, 255].repeat(80 * 120));
        image.wait().unwrap();
        let gpu = Gpu::headless();
        let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        let target = Offscreen::new(&gpu.device, 100, 60);
        let mut rt = Runtime::new(image.pixels().clone(), view, (100.0, 60.0), Env::new(true, 1.0));
        rt.frame(0.0);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (100, 60));
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
        target.copy(&mut enc);
        gpu.queue.submit([enc.finish()]);
        let out = target.read(&gpu.device);
        let px = |x: usize, y: usize| out[(y * 100 + x) * 4];
        let (image, flat): (Vec<u8>, Vec<u8>) = (0..24).map(|y| (px(20, y), px(70, y))).unzip();
        assert!(flat[0] < flat[20], "the band fades: {flat:?}");
        assert!(image.iter().zip(&flat).all(|(a, b)| a.abs_diff(*b) <= 1), "the image fades like the fill\nimage {image:?}\nflat  {flat:?}");
    }

    #[test]
    fn gpu_static_mips_alpha_clip_and_page_order_match_pixels() {
        use crate::{Cx, El, Env, Runtime, el, image::ImageCache};
        use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
        type State = (Pixels, Pixels, bool);
        fn view(s: &State, _: &mut Cx<State>) -> El {
            let base = el().size(8.0, 8.0).pixels(&s.0);
            if s.2 {
                base.child(el().size(4.0, 4.0).pin(Some(2.0), Some(2.0), None, None).pixels(&s.1))
            } else { base.radius(4.0).clip_fade(2.0, 0.0, 0.0, 0.0) }
        }
        let cache = ImageCache::new(Arc::new(|| {}));
        let checker: Vec<_> = (0..64 * 64).flat_map(|i| {
            let v = if (i % 64 + i / 64) % 2 == 0 { 0 } else { 255 };
            [v, v, v, 255]
        }).collect();
        let image = cache.rgba(64, 64, checker);
        image.wait().unwrap();
        let gpu = Gpu::headless();
        let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        let target = Offscreen::new(&gpu.device, 8, 8);
        let mut rt = Runtime::new((image.pixels().clone(), Pixels::new(), false), view, (8.0, 8.0), Env::new(true, 1.0));
        let mut draw = |rt: &mut Runtime<State>| {
            rt.frame(0.0);
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (8, 8));
            renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            renderer.draw(&mut enc, &target.view, [0.0, 0.0, 1.0, 1.0]);
            target.copy(&mut enc);
            gpu.queue.submit([enc.finish()]);
            target.read(&gpu.device)
        };
        let pixel = |out: &[u8], x: usize, y: usize| <[u8; 4]>::try_from(&out[(y * 8 + x) * 4..(y * 8 + x + 1) * 4]).unwrap();
        let near = |got: [u8; 4], want: [u8; 4]| assert!(got.iter().zip(want).all(|(a,b)| a.abs_diff(b) <= 2), "{got:?} != {want:?}");
        let out = draw(&mut rt);
        near(pixel(&out, 4, 4), [188, 188, 188, 255]);
        near(pixel(&out, 0, 0), [0, 0, 255, 255]);
        assert!(pixel(&out, 0, 4)[0] < pixel(&out, 3, 4)[0], "edge fade clips the image");
        assert!(rt.images.slots.contains_key(&(image.pixels().id, 3)), "64px source uses its 8px mip");
        let alpha: Vec<_> = (0..64 * 64).flat_map(|i| if i % 2 == 0 { [255,0,0,128] } else { [0,255,0,0] }).collect();
        let transparent = cache.rgba(64, 64, alpha);
        transparent.wait().unwrap();
        rt.state.0 = transparent.pixels().clone();
        rt.invalidate();
        let out = draw(&mut rt);
        near(pixel(&out, 4, 4), [137, 0, 224, 255]);
        let big = Pixels::new();
        big.set(2048, 1, [255,0,0,255].repeat(2048));
        let full = Pixels::new();
        full.set(1024, 1024, [0,255,0,255].repeat(1024 * 1024));
        rt.state = (big, full, true);
        rt.invalidate();
        rt.settle_all();
        let out = draw(&mut rt);
        near(pixel(&out, 0, 0), [255, 0, 0, 255]);
        near(pixel(&out, 4, 4), [0, 255, 0, 255]);
        assert!(rt.images.pages.len() >= 2, "large images and full pages share painter order");
        rt.state.0.set(2, 1, vec![255,0,0,255, 0,0,255,255]);
        let out = draw(&mut rt);
        near(pixel(&out, 0, 0), [255, 0, 0, 255]);
        near(pixel(&out, 7, 0), [0, 0, 255, 255]);
        assert!(rt.images.pages.iter().any(|p| p.data.is_empty()), "the old large page is released between live pages");
    }

    #[test]
    #[ignore = "manual staging benchmark; reports measurements, not a timing threshold"]
    fn image_staging_costs() {
        use std::{hint::black_box, time::Instant};
        let stream = Pixels::new();
        stream.set(224, 128, vec![0; 224 * 128 * 4]);
        let mut atlas = ImageAtlas::default();
        atlas.place(&stream, [224.0, 128.0]);
        atlas.end_frame();
        atlas.pages[0].dirty = None;
        let buffers: Vec<_> = (0..120).map(|_| vec![128; 224 * 128 * 4]).collect();
        let start = Instant::now();
        let mut upload_bytes = 0u64;
        for bytes in buffers {
            stream.set(224, 128, bytes);
            black_box(atlas.place(&stream, [224.0, 128.0]));
            atlas.end_frame();
            let page = &mut atlas.pages[0];
            let (lo, hi) = page.dirty.take().unwrap();
            upload_bytes += u64::from((hi - lo) * page.width * 4);
        }
        eprintln!("stream 224x128: {:.1} us/stage, {} upload bytes/frame, {} page", start.elapsed().as_secs_f64() * 1e6 / 120.0, upload_bytes / 120, atlas.pages.len());
        let cache = crate::ImageCache::new(Arc::new(|| {}));
        let icons: Vec<_> = (0..100).map(|i| cache.rgba(128, 128, [i, 50, 100, 255].repeat(128 * 128))).collect();
        for icon in &icons { icon.wait().unwrap(); }
        let mut atlas = ImageAtlas::default();
        for icon in &icons { atlas.place(icon.pixels(), [32.0; 2]); }
        atlas.end_frame();
        for page in &mut atlas.pages { page.dirty = None; }
        let start = Instant::now();
        for _ in 0..120 {
            for icon in &icons { black_box(atlas.place(icon.pixels(), [32.0; 2])); }
            atlas.end_frame();
        }
        eprintln!("100 static 128->32 icons: {:.1} us/frame, {} pages, {} dirty pages", start.elapsed().as_secs_f64() * 1e6 / 120.0, atlas.pages.len(), atlas.pages.iter().filter(|p| p.dirty.is_some()).count());
    }

    #[test]
    fn gpu_draws_pixels_texel_for_texel_and_follows_new_frames() {
        use crate::el::{El, el};
        use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
        use crate::runtime::{Cx, Env, Runtime};
        fn view(px: &Pixels, _: &mut Cx<Pixels>) -> El {
            el().size(4.0, 4.0).pixels(px)
        }
        let px = Pixels::new();
        let mut rgba = Vec::new();
        for i in 0..16u8 {
            rgba.extend_from_slice(&[i * 16, 255 - i * 16, 40, 255]);
        }
        px.set(4, 4, rgba.clone());
        let mut rt = Runtime::new(px.clone(), view, (4.0, 4.0), Env::new(true, 1.0));
        let gpu = Gpu::new(Gpu::instance(), None);
        let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        let target = Offscreen::new(&gpu.device, 4, 4);
        let mut draw = |rt: &mut Runtime<Pixels>| {
            rt.frame(1.0 / 60.0);
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (4, 4));
            renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
            target.copy(&mut enc);
            gpu.queue.submit([enc.finish()]);
            target.read(&gpu.device)
        };
        let out = draw(&mut rt);
        for (got, want) in out.chunks_exact(4).zip(rgba.chunks_exact(4)) {
            assert!(got.iter().zip(want).all(|(a, b)| a.abs_diff(*b) <= 1), "drew {got:?}, image {want:?}");
        }
        px.set(4, 4, [10, 20, 30, 255].repeat(16));
        let out = draw(&mut rt);
        assert!(out.chunks_exact(4).all(|p| p.iter().zip([10u8, 20, 30, 255]).all(|(a, b)| a.abs_diff(b) <= 1)), "a new frame shows without re-running the view: {:?}", &out[..4]);
    }

    #[test]
    fn fit_letterboxes_on_whole_pixels_and_fill_crops_the_centre() {
        use crate::SurfaceFit::{Fill, Fit};
        const C: crate::native_surface::place::SurfacePlace = crate::native_surface::place::SurfacePlace::CENTRED;
        let rect = [10.0, 20.0, 40.0, 40.0];
        assert_eq!(fit(None, C, rect, (80, 20)), (rect, [0.0, 0.0, 1.0, 1.0]));
        assert_eq!(fit(Some(Fit), C, rect, (80, 20)), ([10.0, 35.0, 40.0, 10.0], [0.0, 0.0, 1.0, 1.0]));
        assert_eq!(fit(Some(Fit), C, rect, (20, 80)).0, [25.0, 20.0, 10.0, 40.0]);
        assert_eq!(fit(Some(Fill), C, rect, (80, 20)), (rect, [0.375, 0.0, 0.625, 1.0]));
        assert_eq!(fit(Some(Fill), C, rect, (40, 40)), (rect, [0.0, 0.0, 1.0, 1.0]));
        assert_eq!(fit(Some(Fit), C, rect, (0, 0)).0, rect, "an empty image keeps the rect");
    }

    #[test]
    fn gpu_pixels_fit_letterboxes_and_fill_crops() {
        use crate::el::{El, el};
        use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
        use crate::runtime::{Cx, Env, Runtime};
        type State = (Pixels, crate::SurfaceFit);
        fn view(s: &State, _: &mut Cx<State>) -> El { el().size(4.0, 4.0).pixels_fit(&s.0, s.1) }
        // Four columns, red green blue white, two rows tall.
        let px = Pixels::new();
        px.set(4, 2, [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255; 4]].concat().repeat(2));
        let gpu = Gpu::headless();
        let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        let target = Offscreen::new(&gpu.device, 4, 4);
        let mut draw = |fit| {
            let mut rt = Runtime::new((px.clone(), fit), view, (4.0, 4.0), Env::new(true, 1.0));
            rt.frame(0.0);
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (4, 4));
            renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            renderer.draw(&mut enc, &target.view, [0.0, 0.0, 0.0, 1.0]);
            target.copy(&mut enc);
            gpu.queue.submit([enc.finish()]);
            target.read(&gpu.device)
        };
        let at = |out: &[u8], x: usize, y: usize| <[u8; 4]>::try_from(&out[(y * 4 + x) * 4..(y * 4 + x + 1) * 4]).unwrap();
        let out = draw(crate::SurfaceFit::Fit);
        for x in 0..4 {
            assert_eq!(at(&out, x, 0), [0, 0, 0, 255], "letterbox above column {x}");
            assert_eq!(at(&out, x, 3), [0, 0, 0, 255], "letterbox below column {x}");
        }
        assert_eq!([at(&out, 0, 1), at(&out, 1, 2), at(&out, 2, 1), at(&out, 3, 2)],
            [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255; 4]], "the image draws texel for texel");
        let out = draw(crate::SurfaceFit::Fill);
        for y in 0..4 {
            assert_eq!(at(&out, 0, y), [0, 255, 0, 255], "row {y} starts on green");
            assert_eq!(at(&out, 3, y), [0, 0, 255, 255], "row {y} ends on blue");
        }
        assert!(out.as_chunks::<4>().0.iter().all(|p| p[0] < 200), "red and white columns are cropped");
    }

    #[test]
    fn frames_copy_in_once_per_generation_and_resize_moves_the_slot() {
        let mut atlas = ImageAtlas::default();
        let px = Pixels::new();
        assert_eq!(atlas.place(&px, [2.0, 1.0]), None, "empty images draw nothing");
        px.set(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 128]);
        assert_eq!(atlas.place(&px, [2.0, 1.0]).unwrap().rect, [0, 0, 2, 1]);
        assert_eq!(atlas.pages[0].dirty.take(), Some((0, 1)));
        assert_eq!(&atlas.pages[0].data[..8], &[255, 0, 0, 255, 0, 255, 0, 128]);
        atlas.place(&px, [2.0, 1.0]);
        assert_eq!(atlas.pages[0].dirty, None, "an unchanged frame uploads nothing");
        px.set(3, 2, vec![7; 24]);
        let [x, y, w, h] = atlas.place(&px, [3.0, 2.0]).unwrap().rect;
        assert_eq!((w, h), (3, 2));
        assert_eq!(atlas.pages[0].data[((y * IMAGE_ATLAS_SIZE + x) * 4) as usize], 7);
        let other = Pixels::new();
        other.set(1, 1, vec![1; 4]);
        atlas.place(&other, [1.0, 1.0]);
        atlas.end_frame();
        atlas.place(&px, [3.0, 2.0]);
        atlas.end_frame();
        assert!(atlas.slots.contains_key(&(px.id, 0)) && atlas.slots.contains_key(&(other.id, 0)), "the first page keeps unshown images");
    }

    #[test]
    fn an_image_shown_again_after_a_while_is_not_copied_again() {
        let mut atlas = ImageAtlas::default();
        let (list, grid) = (Pixels::new(), Pixels::new());
        list.set(32, 32, vec![1; 32 * 32 * 4]);
        grid.set(48, 48, vec![2; 48 * 48 * 4]);
        let shown = atlas.place(&list, [32.0; 2]).unwrap();
        atlas.end_frame();
        for _ in 0..4 {
            atlas.place(&grid, [48.0; 2]);
            atlas.end_frame();
        }
        atlas.pages[0].dirty = None;
        assert_eq!(atlas.place(&list, [32.0; 2]), Some(shown));
        assert_eq!(atlas.pages[0].dirty, None, "no copy, no upload");
    }

    #[test]
    fn a_full_atlas_starts_over_pages_that_show_nothing_before_it_grows() {
        let mut atlas = ImageAtlas::default();
        let old: Vec<Pixels> = (0..4).map(|_| { let p = Pixels::new(); p.set(512, 512, vec![1; 512 * 512 * 4]); p }).collect();
        for p in &old { atlas.place(p, [512.0; 2]); }
        atlas.end_frame();
        atlas.end_frame();
        assert_eq!(atlas.pages.len(), 1);
        let new = Pixels::new();
        new.set(512, 512, vec![2; 512 * 512 * 4]);
        assert_eq!(atlas.place(&new, [512.0; 2]).unwrap().rect, [0, 0, 512, 512]);
        assert_eq!(atlas.pages.len(), 1);
        assert!(!atlas.slots.contains_key(&(old[0].id, 0)), "the page's old images are forgotten");
    }

    /// A new image earlier in the tree than a body that fills the first page: the body isn't
    /// drawn yet this frame when the new image needs room.
    #[test]
    fn a_full_page_shown_last_frame_is_not_started_over_before_its_images_are_drawn() {
        let mut atlas = ImageAtlas::default();
        let body: Vec<Pixels> = (0..4).map(|i| { let p = Pixels::new(); p.set(512, 512, vec![i; 512 * 512 * 4]); p }).collect();
        let placed: Vec<_> = body.iter().map(|p| atlas.place(p, [512.0; 2]).unwrap()).collect();
        atlas.end_frame();
        atlas.pages[0].dirty = None;
        let new = Pixels::new();
        new.set(64, 64, vec![9; 64 * 64 * 4]);
        assert_eq!(atlas.place(&new, [64.0; 2]).unwrap().page, 1);
        assert_eq!(body.iter().map(|p| atlas.place(p, [512.0; 2]).unwrap()).collect::<Vec<_>>(), placed);
        assert_eq!(atlas.pages[0].dirty, None, "the body copies in nothing");
    }

    /// An image drawn twice in a frame, resized by its producer in between: the first draw's
    /// pixels stay put while this frame still samples them.
    #[test]
    fn an_image_resized_between_two_draws_keeps_the_first_draws_pixels() {
        let mut atlas = ImageAtlas::default();
        let px = Pixels::new();
        px.set(512, 512, vec![1; 512 * 512 * 4]);
        let fill: Vec<Pixels> = (0..3).map(|_| { let p = Pixels::new(); p.set(512, 512, vec![2; 512 * 512 * 4]); p }).collect();
        atlas.place(&px, [512.0; 2]);
        for p in &fill { atlas.place(p, [512.0; 2]); }
        atlas.end_frame();
        let first = atlas.place(&px, [512.0; 2]).unwrap();
        px.set(600, 600, vec![3; 600 * 600 * 4]);
        let second = atlas.place(&px, [600.0; 2]).unwrap();
        assert_ne!(second.page, first.page);
        let [x, y, ..] = first.rect;
        assert_eq!(atlas.pages[first.page].data[((y * IMAGE_ATLAS_SIZE + x) * 4) as usize], 1);
    }

    #[test]
    fn pages_expand_reclaim_and_keep_streaming_slots_stable() {
        let mut atlas = ImageAtlas::default();
        let stream = Pixels::new();
        stream.set(224, 128, vec![1; 224 * 128 * 4]);
        let first = atlas.place(&stream, [112.0, 64.0]).unwrap();
        assert!(!first.premultiplied);
        let large = Pixels::new();
        large.set(2048, 1, vec![2; 2048 * 4]);
        let second = atlas.place(&large, [2048.0, 1.0]).unwrap();
        assert_ne!(first.page, second.page);
        assert_eq!((atlas.pages[second.page].width, atlas.pages[second.page].height), (2048, 1));
        let full = Pixels::new();
        full.set(1024, 1024, vec![3; 1024 * 1024 * 4]);
        let third = atlas.place(&full, [1024.0; 2]).unwrap();
        assert_eq!(third.page, 2);
        atlas.end_frame();
        stream.set(224, 128, vec![4; 224 * 128 * 4]);
        assert_eq!(atlas.place(&stream, [112.0, 64.0]).unwrap(), first);
        atlas.place(&full, [1024.0; 2]);
        atlas.end_frame();
        assert_eq!(atlas.pages.len(), 3);
        assert!(atlas.pages[second.page].data.is_empty(), "unused middle standalone page releases storage");
        assert_eq!(atlas.place(&stream, [112.0, 64.0]).unwrap(), first);
        atlas.end_frame();
        assert_eq!(atlas.pages.len(), 1, "unused trailing pages release their storage");
        atlas.end_frame();
        assert_eq!(atlas.pages.len(), 1, "the first page keeps its images");
    }
}
