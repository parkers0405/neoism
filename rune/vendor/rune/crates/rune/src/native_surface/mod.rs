//! Latest-wins video frames. macOS imports IOSurface-backed CoreVideo buffers without a copy;
//! Windows reads D3D shared textures from another process or API (`windows`).
//! CPU byte frames provide a portable source and a reference for headless verification.

#[cfg(test)]
#[path = "tests.rs"]
mod gpu_tests;
#[cfg(test)]
mod place_tests;
#[cfg(target_os = "macos")]
pub mod macos;
mod pipeline;
pub(crate) mod place;
#[cfg(target_os = "windows")]
pub mod windows;
pub use pipeline::SurfaceRenderer;
use std::sync::{Arc, Mutex};
use crate::time::Instant;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SurfaceFit {
    #[default]
    Fit,
    Fill,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Matrix {
    Bt601,
    Bt709,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer {
    Srgb,
    Bt709,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Bgra,
    Nv12Video,
    Nv12Full,
}
#[derive(Clone, Copy, Debug)]
pub struct ColorInfo {
    pub matrix: Matrix,
    pub transfer: Transfer,
}
impl Default for ColorInfo {
    fn default() -> Self {
        Self { matrix: Matrix::Bt709, transfer: Transfer::Srgb }
    }
}

/// Immutable packed planes. NV12 plane 0 is Y, plane 1 is interleaved CbCr.
pub struct ByteFrame {
    pub width: u32,
    pub height: u32,
    pub format: Format,
    pub color: ColorInfo,
    pub planes: [Vec<u8>; 2],
}

pub(crate) enum Source {
    Bytes(ByteFrame),
    #[cfg(target_os = "macos")]
    PixelBuffer(macos::PixelBuffer),
    #[cfg(target_os = "windows")]
    Shared(windows::SharedTexture),
}
pub(crate) struct Frame {
    pub id: u64,
    pub source: Source,
    /// Buffer size, before `orient`.
    pub size: [u32; 2],
    pub format: Format,
    pub color: ColorInfo,
    pub orient: Orient,
}

/// How a frame's buffer turns into its upright picture: a quarter-turn rotation and/or flip,
/// applied while sampling. The matrix maps centred upright coordinates to centred buffer
/// coordinates: `buffer = [m0 m1; m2 m3] * upright`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Orient(pub [i8; 4]);

impl Default for Orient {
    fn default() -> Self {
        Self::UP
    }
}

impl Orient {
    pub const UP: Self = Self([1, 0, 0, 1]);

    /// From an affine transform taking buffer to upright coordinates (`x' = a x + c y`,
    /// `y' = b x + d y`), such as AVFoundation's `preferredTransform`. Anything but a quarter
    /// turn or flip is drawn upright as is.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn from_transform(a: f64, b: f64, c: f64, d: f64) -> Self {
        let m = [a, b, c, d].map(|v| v.round());
        let unit = m.iter().all(|v| v.abs() <= 1.0) && m.iter().zip([a, b, c, d]).all(|(r, v)| (r - v).abs() < 1e-3);
        let quarter = (m[0] != 0.0 && m[3] != 0.0 && m[1] == 0.0 && m[2] == 0.0) || (m[0] == 0.0 && m[3] == 0.0 && m[1] != 0.0 && m[2] != 0.0);
        // Orthogonal, so the inverse is the transpose: [a b; c d].
        if unit && quarter { Self(m.map(|v| v as i8)) } else { Self::UP }
    }

    /// Whether width and height trade places.
    pub fn swaps(self) -> bool {
        self.0[0] == 0
    }

    /// The upright size of a buffer of `size`.
    pub fn upright(self, [w, h]: [u32; 2]) -> [u32; 2] {
        if self.swaps() { [h, w] } else { [w, h] }
    }
}

/// Submit is a CPU monotonic timestamp. Completion is callback-observed CPU time;
/// GPU start/end are render-pass timestamp-query seconds on the GPU clock, not `Instant`
/// or display time. Their difference is GPU duration; the epoch is backend-specific.
/// GPU times are absent when unsupported, overridden by caller timing, or all timing
/// slots are in flight. CPU submission/completion notifications are still delivered.
#[derive(Clone, Debug)]
pub struct FrameTiming {
    pub frame_id: u64,
    pub submitted_at: Instant,
    pub completed_at: Option<Instant>,
    pub gpu_start_seconds: Option<f64>,
    pub gpu_end_seconds: Option<f64>,
}
pub type FrameObserver = Arc<dyn Fn(FrameTiming) + Send + Sync>;
#[derive(Default)]
struct State {
    latest: Option<Arc<Frame>>,
    waker: Option<crate::Waker>,
    observer: Option<FrameObserver>,
    timing: Option<FrameTiming>,
    submitted: u64,
    completed: u64,
    error: Option<String>,
    mirrored: bool,
    /// The player feeding this surface, pulled once per frame that draws it.
    video: Option<Arc<crate::video::Shared>>,
}
/// Clones share a single bounded slot; pushing never queues frames behind the decoder.
#[derive(Clone, Default)]
pub struct NativeSurface(Arc<Mutex<State>>, place::SurfacePlace);
impl std::fmt::Debug for NativeSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("NativeSurface").field(&self.key()).finish()
    }
}
impl NativeSurface {
    pub fn new() -> Self {
        Self::default()
    }
    /// A handle to the same frames that draws with `place`; the element sets it per draw.
    pub(crate) fn placed(&self, place: place::SurfacePlace) -> Self {
        NativeSurface(self.0.clone(), place)
    }
    pub(crate) fn place(&self) -> place::SurfacePlace {
        self.1
    }
    pub(crate) fn with_video(video: Arc<crate::video::Shared>) -> Self {
        let surface = Self::default();
        surface.0.lock().unwrap().video = Some(video);
        surface
    }
    pub(crate) fn video(&self) -> Option<Arc<crate::video::Shared>> {
        self.0.lock().unwrap().video.clone()
    }
    /// Pull this frame's picture from the surface's video, if it has one.
    pub(crate) fn pull_video(&self, clock: crate::video::Clock, wake: &Arc<crate::live::Wake>) -> Option<crate::video::Pulled> {
        let video = self.video()?;
        Some(video.pull(self, clock, wake))
    }
    pub fn set_waker(&self, waker: crate::Waker) {
        self.0.lock().unwrap().waker = Some(waker);
    }
    pub fn set_observer(&self, observer: FrameObserver) {
        self.0.lock().unwrap().observer = Some(observer);
    }
    pub fn timing(&self) -> Option<FrameTiming> {
        self.0.lock().unwrap().timing.clone()
    }
    /// Counts draws submitted/completed (the same source frame may be drawn more than once).
    pub fn counts(&self) -> (u64, u64) {
        let s = self.0.lock().unwrap();
        (s.submitted, s.completed)
    }
    pub fn last_error(&self) -> Option<String> {
        self.0.lock().unwrap().error.clone()
    }
    pub fn clear(&self) {
        let (old, wake) = {
            let mut s = self.0.lock().unwrap();
            (s.latest.take(), s.waker.clone())
        };
        drop(old);
        if let Some(wake) = wake {
            wake();
        }
    }
    /// Flip the image horizontally at draw time, about its centre, for a front-camera preview.
    /// Pixels are untouched, so stills taken from the same buffers stay unmirrored. Clones
    /// share the flag. A change wakes the app and redraws the current frame; no push is needed.
    ///
    /// ```
    /// let preview = rune::NativeSurface::new();
    /// preview.set_mirrored(true); // front camera
    /// // el().native_surface(&preview, rune::SurfaceFit::Fill) fills and crops, mirrored.
    /// assert!(preview.is_mirrored());
    /// ```
    pub fn set_mirrored(&self, mirrored: bool) {
        let wake = {
            let mut s = self.0.lock().unwrap();
            if s.mirrored == mirrored {
                return;
            }
            s.mirrored = mirrored;
            s.waker.clone()
        };
        if let Some(wake) = wake {
            wake();
        }
    }
    pub fn is_mirrored(&self) -> bool {
        self.0.lock().unwrap().mirrored
    }
    pub fn push_bytes(&self, frame: ByteFrame, frame_id: u64) -> Result<(), String> {
        self.push_bytes_oriented(frame, frame_id, Orient::UP)
    }
    pub(crate) fn push_bytes_oriented(&self, frame: ByteFrame, frame_id: u64, orient: Orient) -> Result<(), String> {
        let [w, h] = [frame.width, frame.height];
        if w == 0 || h == 0 || w > 16384 || h > 16384 {
            return Err("video size must be 1..16384".into());
        }
        let n = w as usize * h as usize;
        let sizes = match frame.format {
            Format::Bgra => [n * 4, 0],
            _ => {
                if !w.is_multiple_of(2) || !h.is_multiple_of(2) {
                    return Err("NV12 requires even dimensions".into());
                }
                [n, n / 2]
            }
        };
        if frame.planes[0].len() != sizes[0] || frame.planes[1].len() != sizes[1] {
            return Err("video plane length does not match dimensions".into());
        }
        self.replace(Frame {
            id: frame_id,
            size: [w, h],
            format: frame.format,
            color: frame.color,
            orient,
            source: Source::Bytes(frame),
        });
        Ok(())
    }
    #[cfg(target_os = "macos")]
    pub fn push(&self, buffer: macos::PixelBuffer, frame_id: u64) -> Result<(), String> {
        self.push_oriented(buffer, frame_id, Orient::UP)
    }
    #[cfg(target_os = "macos")]
    pub(crate) fn push_oriented(&self, buffer: macos::PixelBuffer, frame_id: u64, orient: Orient) -> Result<(), String> {
        let (size, format, color) = buffer.info()?;
        self.replace(Frame { id: frame_id, source: Source::PixelBuffer(buffer), size, format, color, orient });
        Ok(())
    }
    /// Shows the frame in a D3D shared texture (`windows::SharedTexture`), read on Rune's D3D12
    /// device the first time it draws.
    #[cfg(target_os = "windows")]
    pub fn push_shared(&self, texture: windows::SharedTexture, frame_id: u64) -> Result<(), String> {
        let size = texture.size();
        if size.contains(&0) || size.iter().any(|v| *v > 16384) {
            return Err("video size must be 1..16384".into());
        }
        self.replace(Frame {
            id: frame_id,
            size,
            format: Format::Bgra,
            color: ColorInfo::default(),
            orient: Orient::UP,
            source: Source::Shared(texture),
        });
        Ok(())
    }
    fn replace(&self, frame: Frame) {
        let (old, wake) = {
            let mut s = self.0.lock().unwrap();
            (s.latest.replace(Arc::new(frame)), s.waker.clone())
        };
        drop(old); // CoreVideo release never runs under the mailbox lock.
        if let Some(wake) = wake {
            wake();
        }
    }
    #[cfg(test)]
    pub(crate) fn latest(&self) -> Option<Arc<Frame>> {
        self.0.lock().unwrap().latest.clone()
    }
    fn latest_mirrored(&self) -> Option<(Arc<Frame>, bool)> {
        let s = self.0.lock().unwrap();
        s.latest.clone().map(|f| (f, s.mirrored))
    }
    pub(crate) fn key(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
    fn report(&self, timing: FrameTiming) {
        let observer = {
            let mut s = self.0.lock().unwrap();
            if timing.completed_at.is_some() {
                s.completed += 1;
            } else {
                s.submitted += 1;
            }
            if s.timing.as_ref().is_none_or(|old| old.submitted_at <= timing.submitted_at) {
                s.timing = Some(timing.clone());
            }
            s.observer.clone()
        };
        if let Some(f) = observer {
            f(timing);
        }
    }
}

/// A device Rune made, `presented` when it draws a window: on Windows its adapter is the one
/// shared-texture producers render on (`windows::adapter_luid`).
pub(crate) fn note_device(device: &wgpu::Device, presented: bool) {
    #[cfg(target_os = "windows")]
    if presented {
        windows::note_adapter(device);
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (device, presented);
}

/// A node's video draw in painter order. Geometry is already in device pixels.
#[derive(Clone, Debug)]
pub struct SurfaceDraw {
    pub surface: NativeSurface,
    pub fit: SurfaceFit,
    pub before_instance: u32,
    pub rect: [f32; 4],
    pub radii: [f32; 4],
    pub clip: [f32; 4],
    /// Packed as `render::Instance::fade`.
    pub fade: [u32; 4],
    pub opacity: f32,
}

/// Independent scalar colour oracle: encoded YCbCr -> linear RGB.
pub fn nv12_reference(y: u8, cb: u8, cr: u8, format: Format, color: ColorInfo) -> [f32; 3] {
    let (y, u, v) = if format == Format::Nv12Video {
        ((y as f32 - 16.0) / 219.0, (cb as f32 - 128.0) / 224.0, (cr as f32 - 128.0) / 224.0)
    } else {
        (y as f32 / 255.0, (cb as f32 - 128.0) / 255.0, (cr as f32 - 128.0) / 255.0)
    };
    let (kr, kb) = match color.matrix {
        Matrix::Bt601 => (0.299, 0.114),
        Matrix::Bt709 => (0.2126, 0.0722),
    };
    let r = y + 2.0 * (1.0 - kr) * v;
    let b = y + 2.0 * (1.0 - kb) * u;
    let g = (y - kr * r - kb * b) / (1.0 - kr - kb);
    [r, g, b].map(|x| linear(x.clamp(0.0, 1.0), color.transfer))
}
pub fn linear(x: f32, transfer: Transfer) -> f32 {
    match transfer {
        Transfer::Srgb => {
            if x <= 0.04045 {
                x / 12.92
            } else {
                ((x + 0.055) / 1.055).powf(2.4)
            }
        }
        Transfer::Bt709 => {
            if x < 0.081 {
                x / 4.5
            } else {
                ((x + 0.099) / 1.099).powf(1.0 / 0.45)
            }
        }
    }
}

/// Synthetic bars, gradient and one-pixel text-like strokes. No decoded content is copied by import.
pub fn synthetic(width: u32, height: u32, phase: u64) -> ByteFrame {
    assert!(width > 0 && height > 0 && width.is_multiple_of(2) && height.is_multiple_of(2));
    let bars = [
        [235, 128, 128],
        [210, 16, 146],
        [170, 166, 16],
        [145, 54, 34],
        [106, 202, 222],
        [81, 90, 240],
        [41, 240, 110],
        [16, 128, 128],
    ];
    let mut y = vec![0; (width * height) as usize];
    let mut uv = vec![128; (width * height / 2) as usize];
    for row in 0..height {
        for x in 0..width {
            y[(row * width + x) as usize] = if row < height / 3 {
                bars[(x * 8 / width) as usize][0]
            } else if row < 2 * height / 3 {
                (16 + x * 219 / width) as u8
            } else if (x + phase as u32).is_multiple_of(8) || row.is_multiple_of(9) {
                235
            } else {
                16
            };
        }
    }
    for row in 0..height / 6 {
        for x in 0..width / 2 {
            let bar = bars[(x * 16 / width) as usize];
            let i = (row * width + x * 2) as usize;
            uv[i] = bar[1];
            uv[i + 1] = bar[2];
        }
    }
    ByteFrame {
        width,
        height,
        format: Format::Nv12Video,
        color: ColorInfo { matrix: Matrix::Bt709, transfer: Transfer::Bt709 },
        planes: [y, uv],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn latest_wins_and_releases_old_frames() {
        let surface = NativeSurface::new();
        surface.push_bytes(synthetic(16, 16, 0), 1).unwrap();
        let held = surface.latest().unwrap();
        let old = Arc::downgrade(&held);
        let producer = surface.clone();
        std::thread::spawn(move || {
            for id in 2..100 {
                producer.push_bytes(synthetic(16, 16, id), id).unwrap();
            }
        })
        .join()
        .unwrap();
        assert_eq!(surface.latest().unwrap().id, 99);
        assert!(old.upgrade().is_some());
        drop(held);
        assert!(old.upgrade().is_none());
        let newest = Arc::downgrade(&surface.latest().unwrap());
        surface.clear();
        assert!(newest.upgrade().is_none());
    }
    #[test]
    fn range_endpoints_and_neutral_chroma() {
        for matrix in [Matrix::Bt601, Matrix::Bt709] {
            for transfer in [Transfer::Srgb, Transfer::Bt709] {
                let c = ColorInfo { matrix, transfer };
                assert_eq!(nv12_reference(16, 128, 128, Format::Nv12Video, c), [0.0; 3]);
                for v in nv12_reference(235, 128, 128, Format::Nv12Video, c) {
                    assert!((v - 1.0).abs() < 1e-6);
                }
                assert_eq!(nv12_reference(0, 128, 128, Format::Nv12Full, c), [0.0; 3]);
            }
        }
    }
}
