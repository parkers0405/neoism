//! Dragging data out of a window into other apps: an OS drag source.
//!
//! An element marked [`crate::El::drag_out`] starts a system drag session once the pointer
//! travels [`DRAG_THRESHOLD`] points from the press. The session carries a [`DragItem`] (file
//! URLs, PNG data, plain text) and shows a [`DragImage`], by default a snapshot of the element
//! as the window last drew it, starting exactly over the element. [`crate::El::on_drag_start`]
//! and [`crate::El::on_drag_end`] bracket the session, so the app can style the source while
//! it's dragged; [`DragOutcome`] says how the drop ended. Rune windows take no drops from their
//! own app's drags unless [`DragItem::within_app`] says so, so a drag let go over the app ends
//! [`DragOutcome::Cancelled`] and the image slides back.
//!
//! macOS runs the session with `NSDraggingSource`. Other platforms don't start one yet: there
//! `drag_out` is ignored and the element keeps its ordinary press and `on_drag` behaviour.
//! Windows (`DoDragDrop`), Wayland and X11 backends slot in behind the same types.
//!
//! Windows start the session from the mouse-dragged event AppKit is delivering. Overlay panels
//! track the pointer by polling, so there may be no such event; the overlay then builds a
//! dragged event at the current mouse location. Both paths have GUI tests on a real desktop
//! (`tests/macos_drag_out.rs`, and `tests/macos_overlay_drag_out.rs` for a non-activating
//! overlay that takes input only on its hit regions).

use crate::color::{Color, srgb_decode, srgb_encode};
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
mod macos;

/// Whether this platform's windows start OS drag sessions.
pub(crate) const SUPPORTED: bool = cfg!(target_os = "macos");

/// Pointer travel (points, straight-line) from the press before a drag session starts.
/// AppKit views commonly use 4; GTK's default is 8; Windows' `SM_CXDRAG` is 4 pixels.
pub const DRAG_THRESHOLD: f32 = if cfg!(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))) { 8.0 } else { 4.0 };

/// Whether a pointer that went down at `from` and is now at `to` has travelled past
/// `threshold` points. [`crate::El::drag_threshold`] sets it per element.
pub(crate) fn past_threshold(from: (f32, f32), to: (f32, f32), threshold: f32) -> bool {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    dx * dx + dy * dy > threshold * threshold
}

/// What a drag carries to another app: any of file URLs, PNG data and plain text, plus the
/// picture shown under the pointer.
///
/// ```
/// # use rune::drag_out::DragItem;
/// let item = DragItem::new().file("/Applications/Example.app");
/// assert_eq!(item.paths().len(), 1);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct DragItem {
    files: Vec<PathBuf>,
    png: Option<Vec<u8>>,
    text: Option<String>,
    image: Option<DragImage>,
    operations: u8,
    backing: Option<Color>,
    within_app: bool,
}

impl Default for DragItem {
    fn default() -> Self {
        Self { files: Vec::new(), png: None, text: None, image: None, operations: DragOperation::Copy.bit(), backing: None, within_app: false }
    }
}

impl DragItem {
    pub fn new() -> Self { Self::default() }

    /// Add a file (or folder or app bundle) by path; repeat for several. Finder copies it,
    /// and System Settings' privacy lists accept an app bundle.
    pub fn file(mut self, path: impl Into<PathBuf>) -> Self {
        self.files.push(path.into());
        self
    }

    /// PNG-encoded image data, for targets that take an image rather than a file.
    pub fn png(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.png = Some(bytes.into());
        self
    }

    /// Straight-alpha sRGB RGBA8 pixels, encoded as PNG data. Ignored if the sizes disagree.
    pub fn rgba(self, width: u32, height: u32, pixels: &[u8]) -> Self {
        match encode_png(width, height, pixels) {
            Some(png) => self.png(png),
            None => self,
        }
    }

    /// Plain text. Without it, a file's path is offered as text.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// The picture under the pointer. Defaults to a snapshot of the element.
    pub fn image(mut self, image: DragImage) -> Self {
        self.image = Some(image);
        self
    }

    /// Show the drag image composited over `color`, so it's opaque even where the element or
    /// its window is transparent (an overlay panel, a translucent window material). AppKit
    /// still draws its drag window at about 90% opacity, as it does for every app's drag.
    ///
    /// ```
    /// # use rune::{Color, DragItem};
    /// let item = DragItem::new().text("row").opaque(Color::WHITE);
    /// assert_eq!(item.backing(), Some(Color::WHITE));
    /// ```
    pub fn opaque(mut self, color: Color) -> Self {
        self.backing = Some(color.alpha(1.0));
        self
    }

    /// Offer only `operation` to the target, instead of the default [`DragOperation::Copy`].
    ///
    /// ```
    /// # use rune::{DragItem, DragOperation};
    /// let item = DragItem::new().text("row").operation(DragOperation::Link);
    /// assert!(item.offers(DragOperation::Link) && !item.offers(DragOperation::Copy));
    /// ```
    pub fn operation(mut self, operation: DragOperation) -> Self {
        self.operations = operation.bit();
        self
    }

    /// Offer `operation` as well as those already offered; the target picks one.
    pub fn allow(mut self, operation: DragOperation) -> Self {
        self.operations |= operation.bit();
        self
    }

    /// Let the target move files instead of copying them (Finder moves within one volume).
    /// By default a drag only offers a copy, whatever modifier keys are held.
    pub fn allow_move(self) -> Self { self.allow(DragOperation::Move) }

    /// Offer the drag to this app's own windows too, so their `on_drop_files` targets receive
    /// its files as a drop from Finder would (an image dragged from a timeline into the
    /// composer). By default the app's windows take no drops from its own drags. A drag let go
    /// over a window with no target that accepts it ends [`DragOutcome::Copied`] without sliding
    /// back, as the window takes any file drag.
    ///
    /// ```
    /// # use rune::DragItem;
    /// assert!(DragItem::new().file("/a.png").within_app().drops_within_app());
    /// assert!(!DragItem::new().drops_within_app());
    /// ```
    pub fn within_app(mut self) -> Self {
        self.within_app = true;
        self
    }

    pub fn paths(&self) -> &[PathBuf] { &self.files }
    pub fn png_data(&self) -> Option<&[u8]> { self.png.as_deref() }
    pub fn plain_text(&self) -> Option<&str> { self.text.as_deref() }
    pub fn drag_image(&self) -> Option<&DragImage> { self.image.as_ref() }
    pub fn allows_move(&self) -> bool { self.offers(DragOperation::Move) }
    /// Whether the app's own windows take this drag's drop ([`DragItem::within_app`]).
    pub fn drops_within_app(&self) -> bool { self.within_app }
    /// Whether the drag offers `operation` to the target.
    pub fn offers(&self, operation: DragOperation) -> bool { self.operations & operation.bit() != 0 }
    /// The colour the drag image is shown over, from [`DragItem::opaque`].
    pub fn backing(&self) -> Option<Color> { self.backing }
    /// Carries nothing a target could receive.
    pub fn is_empty(&self) -> bool { self.files.is_empty() && self.png.is_none() && self.text.is_none() }
}

/// What a drag lets the target do with the data. A drag offers [`DragOperation::Copy`] unless
/// [`DragItem::operation`] or [`DragItem::allow`] say otherwise, and modifier keys don't change
/// the offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DragOperation {
    /// The target copies the data.
    Copy,
    /// The target moves the files.
    Move,
    /// The target makes a link or alias to the files.
    Link,
}

impl DragOperation {
    fn bit(self) -> u8 {
        match self { Self::Copy => 1, Self::Move => 2, Self::Link => 4 }
    }
}

/// The picture a drag shows under the pointer, as PNG data.
#[derive(Clone, Debug, PartialEq)]
pub struct DragImage {
    png: Vec<u8>,
    /// Display size in points; `None` draws one pixel per point at the window's scale.
    size: Option<(f32, f32)>,
    placement: Placement,
}

/// Where a drag image starts, in window points.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Placement {
    /// Centred under the press.
    Pointer,
    /// Over the element's top-left, at the element's full (unclipped) size unless sized.
    Element,
    /// Top-left at this point, at the image's own size (the snapshot's pixel-aligned crop).
    At(f32, f32),
}

impl DragImage {
    pub fn png(bytes: impl Into<Vec<u8>>) -> Self { Self { png: bytes.into(), size: None, placement: Placement::Pointer } }

    /// Straight-alpha sRGB RGBA8 pixels. `None` if `pixels` isn't `width * height * 4` bytes.
    pub fn rgba(width: u32, height: u32, pixels: &[u8]) -> Option<Self> {
        encode_png(width, height, pixels).map(Self::png)
    }

    /// Draw it this many points wide and high.
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.size = Some((width, height));
        self
    }

    /// Start the drag with the image over the element, filling its bounds (scaled to fit), as
    /// the default snapshot does, instead of centred under the pointer. Use it for a picture
    /// rendered at the element's size, so the drag appears to lift the element itself. With
    /// [`DragImage::size`] it keeps that size, from the element's top-left. A row scrolled
    /// partly out of view still gets its whole size, not the visible strip.
    pub fn over_element(mut self) -> Self {
        self.placement = Placement::Element;
        self
    }

    pub fn png_data(&self) -> &[u8] { &self.png }
    pub fn points(&self) -> Option<(f32, f32)> { self.size }

    /// Where the drag image starts, x, y, w, h in window points: `bounds` is the element's
    /// unclipped rect, and `natural` the image's size in points when [`DragImage::size`]
    /// wasn't set.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn frame(&self, bounds: [f32; 4], press: (f32, f32), natural: (f32, f32)) -> [f32; 4] {
        let (w, h) = self.size.unwrap_or(natural);
        match self.placement {
            Placement::Pointer => [press.0 - w / 2.0, press.1 - h / 2.0, w, h],
            Placement::Element => {
                let (w, h) = self.size.unwrap_or((bounds[2], bounds[3]));
                [bounds[0], bounds[1], w.max(1.0), h.max(1.0)]
            }
            Placement::At(x, y) => [x, y, w, h],
        }
    }

    /// The image composited over an opaque `backing`, blended in linear light as the window
    /// blends. `None` if the PNG doesn't decode.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn over(&self, backing: Color) -> Option<Self> {
        let mut rgba = image::load_from_memory_with_format(&self.png, image::ImageFormat::Png).ok()?.to_rgba8();
        let bg = [backing.r, backing.g, backing.b];
        for p in rgba.pixels_mut() {
            let a = p[3] as f32 / 255.0;
            for c in 0..3 {
                let v = srgb_decode(p[c] as f32 / 255.0) * a + bg[c] * (1.0 - a);
                p[c] = (srgb_encode(v) * 255.0).round().clamp(0.0, 255.0) as u8;
            }
            p[3] = 255;
        }
        let png = encode_png(rgba.width(), rgba.height(), rgba.as_raw())?;
        Some(Self { png, ..self.clone() })
    }
}

/// How a drag out of the window ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DragOutcome {
    /// The target copied the data.
    Copied,
    /// The target moved the files (only offered after [`DragItem::allow_move`] or
    /// [`DragOperation::Move`]), or the drop
    /// asked the source to delete them (the Trash).
    Moved,
    /// The target made a link or alias to the files.
    Linked,
    /// Dropped nowhere that accepted it, or cancelled with Escape.
    Cancelled,
}

/// A drag the runtime asked the host to start, inside the pointer event that crossed the
/// threshold.
pub(crate) struct DragOutStart {
    pub path: String,
    pub item: DragItem,
    /// The element's visible drawn rect in window points: x, y, w, h.
    pub rect: [f32; 4],
    /// The element's whole drawn rect, before scroll clipping.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub bounds: [f32; 4],
    /// Where the pointer went down, in window points.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub press: (f32, f32),
}

pub(crate) type DragEnds = std::rc::Rc<std::cell::RefCell<Vec<(String, DragOutcome)>>>;

pub(crate) fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || rgba.len() != (width as usize) * (height as usize) * 4 { return None; }
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let mut w = enc.write_header().ok()?;
    w.write_image_data(rgba).ok()?;
    w.finish().ok()?;
    Some(out)
}

/// Draws the last frame's instances again offscreen and reads back `rect` (window points), so
/// the drag shows what the user grabbed. `None` for formats it can't read back.
pub(crate) fn snapshot(gpu: &crate::render::Gpu, renderer: &crate::render::Renderer, size: (u32, u32), scale: f32, clear: [f32; 4], rect: [f32; 4]) -> Option<DragImage> {
    let format = renderer.format();
    let bgra = match format {
        wgpu::TextureFormat::Bgra8UnormSrgb | wgpu::TextureFormat::Bgra8Unorm => true,
        wgpu::TextureFormat::Rgba8UnormSrgb | wgpu::TextureFormat::Rgba8Unorm => false,
        _ => return None,
    };
    let px = crop(rect, scale, size)?;
    let [x, y, w, h] = px;
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("drag image"),
        size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let padded_row = (w * 4).div_ceil(256) * 256;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("drag image readback"),
        size: u64::from(padded_row * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut enc, &view, clear);
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded_row), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    gpu.queue.submit([enc.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    gpu.device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    {
        let data = slice.get_mapped_range().ok()?;
        for row in 0..h {
            let start = (row * padded_row) as usize;
            rgba.extend_from_slice(&data[start..start + (w * 4) as usize]);
        }
    }
    buffer.unmap();
    to_straight_rgba(&mut rgba, bgra);
    let mut image = DragImage::rgba(w, h, &rgba)?.size(w as f32 / scale, h as f32 / scale);
    // The crop rounds out to whole pixels, so the image sits exactly over what was drawn.
    image.placement = Placement::At(x as f32 / scale, y as f32 / scale);
    Some(image)
}

/// `rect` (points) in device pixels, clamped to the frame: x, y, w, h. `None` when empty.
fn crop(rect: [f32; 4], scale: f32, size: (u32, u32)) -> Option<[u32; 4]> {
    let x0 = (rect[0] * scale).floor().clamp(0.0, size.0 as f32) as u32;
    let y0 = (rect[1] * scale).floor().clamp(0.0, size.1 as f32) as u32;
    let x1 = ((rect[0] + rect[2]) * scale).ceil().clamp(0.0, size.0 as f32) as u32;
    let y1 = ((rect[1] + rect[3]) * scale).ceil().clamp(0.0, size.1 as f32) as u32;
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1 - x0, y1 - y0])
}

/// Frame pixels are premultiplied, in the surface's channel order.
fn to_straight_rgba(pixels: &mut [u8], bgra: bool) {
    for p in pixels.chunks_exact_mut(4) {
        if bgra { p.swap(0, 2); }
        let a = p[3];
        if a != 0 && a != 255 {
            for c in &mut p[..3] { *c = ((*c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8; }
        }
    }
}

/// Starts the OS drag session for `start` from inside the pointer event that crossed the
/// threshold. `done` runs once with how it ended (also when it couldn't start: `Cancelled`).
#[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
pub(crate) fn begin(window: &winit::window::Window, start: DragOutStart, snapshot: Option<DragImage>, done: Box<dyn FnOnce(DragOutcome)>) {
    #[cfg(target_os = "macos")]
    { let picture = picture(&start.item, snapshot); macos::begin(window, start, picture, done); }
    #[cfg(not(target_os = "macos"))]
    done(DragOutcome::Cancelled);
}

/// The item's own image, else the snapshot, over the item's backing colour if it has one.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn picture(item: &DragItem, snapshot: Option<DragImage>) -> Option<DragImage> {
    let picture = item.drag_image().cloned().or(snapshot)?;
    match item.backing() {
        Some(backing) => picture.over(backing).or(Some(picture)),
        None => Some(picture),
    }
}

/// The path offered as text when a file drag has no text of its own.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn path_text(path: &Path) -> String { path.to_string_lossy().into_owned() }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_is_straight_line_travel_beyond_the_platform_distance() {
        let t = DRAG_THRESHOLD;
        assert!(!past_threshold((10.0, 10.0), (10.0, 10.0), t));
        assert!(!past_threshold((10.0, 10.0), (10.0 + t, 10.0), t), "exactly the threshold doesn't start");
        assert!(past_threshold((10.0, 10.0), (10.0 + t + 0.1, 10.0), t));
        let d = t * 0.75;
        assert!(past_threshold((0.0, 0.0), (d, d), t), "diagonal travel counts its length");
    }

    #[test]
    fn a_drag_item_collects_files_image_and_text() {
        let item = DragItem::new().file("/a.png").file("/b.txt").text("hello").png(vec![1, 2, 3]);
        assert_eq!(item.paths(), [PathBuf::from("/a.png"), PathBuf::from("/b.txt")]);
        assert_eq!(item.plain_text(), Some("hello"));
        assert_eq!(item.png_data(), Some(&[1u8, 2, 3][..]));
        assert!(!item.allows_move() && item.drag_image().is_none() && !item.is_empty());
        assert!(DragItem::new().is_empty());
        assert!(DragItem::new().allow_move().allows_move());
    }

    #[test]
    fn a_drag_offers_copy_unless_told_otherwise() {
        let item = DragItem::new();
        assert!(item.offers(DragOperation::Copy) && !item.offers(DragOperation::Move) && !item.offers(DragOperation::Link));
        let item = DragItem::new().allow_move();
        assert!(item.offers(DragOperation::Copy) && item.offers(DragOperation::Move));
        let item = DragItem::new().operation(DragOperation::Move);
        assert!(!item.offers(DragOperation::Copy) && item.offers(DragOperation::Move) && item.allows_move());
        let item = DragItem::new().operation(DragOperation::Link).allow(DragOperation::Copy);
        assert!(item.offers(DragOperation::Copy) && item.offers(DragOperation::Link) && !item.offers(DragOperation::Move));
    }

    #[test]
    fn drag_image_frames_start_over_the_element_or_under_the_pointer() {
        let rect = [20.0, 40.0, 200.0, 32.0];
        let press = (50.0, 60.0);
        let image = DragImage::rgba(1, 1, &[0; 4]).unwrap();
        assert_eq!(image.frame(rect, press, (64.0, 64.0)), [18.0, 28.0, 64.0, 64.0], "a supplied picture is centred under the press");
        assert_eq!(image.clone().size(10.0, 20.0).frame(rect, press, (64.0, 64.0)), [45.0, 50.0, 10.0, 20.0]);
        assert_eq!(image.clone().over_element().frame(rect, press, (64.0, 64.0)), rect, "over_element fills the element's bounds");
        assert_eq!(image.clone().size(10.0, 12.0).over_element().frame(rect, press, (64.0, 64.0)), [20.0, 40.0, 10.0, 12.0], "an explicit size wins");
        assert_eq!(image.over_element().frame([5.0, 5.0, 0.0, 0.0], press, (1.0, 1.0)), [5.0, 5.0, 1.0, 1.0], "never an empty frame");
    }

    #[test]
    fn the_snapshot_frame_is_its_pixel_aligned_crop() {
        let mut image = DragImage::rgba(41, 21, &[0; 41 * 21 * 4]).unwrap().size(20.5, 10.5);
        image.placement = Placement::At(10.0, 5.0);
        assert_eq!(image.frame([10.2, 5.3, 20.0, 10.0], (0.0, 0.0), (41.0, 21.0)), [10.0, 5.0, 20.5, 10.5]);
    }

    #[test]
    fn an_opaque_backing_fills_transparent_pixels_and_keeps_opaque_ones() {
        let image = DragImage::rgba(3, 1, &[255, 0, 0, 255, 0, 0, 0, 0, 255, 255, 255, 128]).unwrap().size(3.0, 1.0).over_element();
        let flat = image.over(Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 }).unwrap();
        assert_eq!((flat.points(), flat.placement), (Some((3.0, 1.0)), Placement::Element), "size and placement survive");
        let rgba = image::load_from_memory(flat.png_data()).unwrap().to_rgba8();
        assert_eq!(rgba.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(rgba.get_pixel(1, 0).0, [0, 0, 255, 255]);
        let half = rgba.get_pixel(2, 0).0;
        assert_eq!(half[3], 255);
        assert!(half[0] == half[1] && half[0] > 150 && half[2] == 255, "half white over blue, mixed in linear light: {half:?}");
    }

    #[test]
    fn the_picture_is_the_items_image_else_the_snapshot_over_the_backing() {
        let snap = DragImage::rgba(1, 1, &[0, 0, 0, 0]).unwrap();
        let own = DragImage::rgba(1, 1, &[1, 2, 3, 255]).unwrap();
        assert_eq!(picture(&DragItem::new(), Some(snap.clone())), Some(snap.clone()));
        assert_eq!(picture(&DragItem::new().image(own.clone()), Some(snap.clone())), Some(own));
        assert_eq!(picture(&DragItem::new(), None), None);
        let flat = picture(&DragItem::new().opaque(Color::WHITE), Some(snap)).unwrap();
        assert_eq!(image::load_from_memory(flat.png_data()).unwrap().to_rgba8().get_pixel(0, 0).0, [255, 255, 255, 255]);
    }

    #[test]
    fn rgba_encodes_png_and_rejects_a_wrong_length() {
        let item = DragItem::new().rgba(1, 1, &[255, 0, 0, 255]);
        let png = item.png_data().unwrap();
        assert_eq!(&png[1..4], b"PNG");
        assert!(DragItem::new().rgba(2, 2, &[0; 4]).png_data().is_none());
        assert!(DragImage::rgba(0, 0, &[]).is_none());
        assert_eq!(DragImage::rgba(1, 1, &[0; 4]).unwrap().size(8.0, 8.0).points(), Some((8.0, 8.0)));
    }

    #[test]
    fn snapshot_crop_clamps_to_the_frame_in_device_pixels() {
        assert_eq!(crop([10.0, 5.0, 20.0, 10.0], 2.0, (400, 300)), Some([20, 10, 40, 20]));
        assert_eq!(crop([-5.0, -5.0, 10.0, 10.0], 2.0, (400, 300)), Some([0, 0, 10, 10]));
        assert_eq!(crop([190.0, 0.0, 50.0, 10.0], 2.0, (400, 300)), Some([380, 0, 20, 20]));
        assert_eq!(crop([500.0, 0.0, 10.0, 10.0], 2.0, (400, 300)), None);
    }

    #[test]
    fn readback_unpremultiplies_and_swaps_bgra() {
        let mut p = [0, 0, 128, 128, 10, 20, 30, 255];
        to_straight_rgba(&mut p, true);
        assert_eq!(p, [255, 0, 0, 128, 30, 20, 10, 255]);
    }
}
