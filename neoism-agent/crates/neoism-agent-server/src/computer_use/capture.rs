//! Lossless screenshot preparation and opt-in, operation-scoped Linux capture.
//! Encoding/tests never access the desktop; no image contents are logged.
use anyhow::{ensure, Result};
use image_rs::{
    codecs::png::{CompressionType, FilterType, PngEncoder},
    DynamicImage, ImageEncoder,
};
use std::{
    io::{self, Write},
    time::{Duration, Instant},
};

const MAX_PIXELS: u64 = 64_000_000;
const MAX_BYTES: usize = 8 * 1024 * 1024;
#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
impl Region {
    pub fn validate(&self, width: u32, height: u32) -> Result<()> {
        ensure!(
            self.width > 0
                && self.height > 0
                && self.x.checked_add(self.width).is_some_and(|x| x <= width)
                && self.y.checked_add(self.height).is_some_and(|y| y <= height),
            "Crop must be a nonempty rectangle inside the native display image"
        );
        Ok(())
    }
    pub fn point(
        &self,
        x: u32,
        y: u32,
        image_width: u32,
        image_height: u32,
    ) -> (u32, u32) {
        (
            self.x
                + (u64::from(x) * u64::from(self.width) / u64::from(image_width)) as u32,
            self.y
                + (u64::from(y) * u64::from(self.height) / u64::from(image_height))
                    as u32,
        )
    }
}

const MAX_EDGE: u32 = 1600;

#[derive(Debug, Default)]
pub(super) struct EncodeTimings {
    pub resize: Duration,
    /// Includes the bounded fast attempt and, if needed, lossless retry.
    pub png: Duration,
    pub lossless_retry: bool,
}

// Deliberately no Debug: image contents must not enter diagnostics.
pub(super) struct EncodedCapture {
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
    pub timings: EncodeTimings,
}

/// Consume the native image AFTER native-resolution settling. Frame dimensions
/// must come from this result; base64/publication and cancellation remain owned
/// by the caller. Never enlarges, crops, or changes pixels after resizing.
pub(super) fn encode(image: DynamicImage) -> Result<EncodedCapture> {
    ensure!(
        image.width() > 0 && image.height() > 0,
        "Invalid screenshot dimensions"
    );
    ensure!(
        u64::from(image.width()) * u64::from(image.height()) <= MAX_PIXELS,
        "Physical display exceeds capture limit"
    );
    // Native adapters must also bound allocations BEFORE capturing.
    let start = Instant::now();
    let image = if image.width() > MAX_EDGE || image.height() > MAX_EDGE {
        image.thumbnail(MAX_EDGE, MAX_EDGE)
    } else {
        image
    };
    let resize = start.elapsed();
    let start = Instant::now();
    let (bytes, lossless_retry) = bounded_png(&image, MAX_BYTES)?;
    Ok(EncodedCapture {
        width: image.width(),
        height: image.height(),
        bytes,
        timings: EncodeTimings {
            resize,
            png: start.elapsed(),
            lossless_retry,
        },
    })
}

struct BoundedBytes {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}
impl Write for BoundedBytes {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(io::Error::other("Screenshot exceeds encoded size limit"));
        }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn png_attempt(
    image: &DynamicImage,
    compression: CompressionType,
    filter: FilterType,
    limit: usize,
) -> Result<Option<Vec<u8>>> {
    let mut out = BoundedBytes {
        bytes: Vec::new(),
        limit,
        exceeded: false,
    };
    // Use the encoder directly: DynamicImage's convenience method can silently
    // convert unsupported float pixels. Unsupported colors must fail losslessly.
    let result = PngEncoder::new_with_quality(&mut out, compression, filter).write_image(
        image.as_bytes(),
        image.width(),
        image.height(),
        image.color().into(),
    );
    if out.exceeded {
        return Ok(None);
    }
    result?;
    Ok(Some(out.bytes))
}

fn bounded_png(image: &DynamicImage, limit: usize) -> Result<(Vec<u8>, bool)> {
    if let Some(bytes) =
        png_attempt(image, CompressionType::Fast, FilterType::Sub, limit)?
    {
        return Ok((bytes, false));
    }
    // One bounded lossless attempt; no further resizing, quantization, or JPEG.
    if let Some(bytes) =
        png_attempt(image, CompressionType::Default, FilterType::Adaptive, limit)?
    {
        return Ok((bytes, true));
    }
    anyhow::bail!("Screenshot exceeds encoded size limit")
}

/// One permission-checked screenshot operation, including its settling probes.
/// Construct inside the parent's serialized worker and drop before publication;
/// never cache across tool calls. Parent cancellation/target checks still apply.
#[cfg(target_os = "linux")]
pub(super) struct ScopedLinuxCaptureSession {
    connection: libwayshot::WayshotConnection,
    topology: Vec<libwayshot::output::OutputInfo>,
}

#[cfg(target_os = "linux")]
impl ScopedLinuxCaptureSession {
    pub(super) fn new() -> Result<Self> {
        let connection = libwayshot::WayshotConnection::new()?;
        let topology = connection.get_all_outputs().to_vec();
        ensure!(!topology.is_empty(), "No Wayland outputs");
        Ok(Self {
            connection,
            topology,
        })
    }

    pub(super) fn displays(&self) -> Vec<super::Display> {
        self.topology
            .iter()
            .map(|o| super::Display {
                id: o.name.clone(),
                x: o.logical_position().x,
                y: o.logical_position().y,
                width: o.logical_size().width,
                height: o.logical_size().height,
            })
            .collect()
    }

    fn refresh_and_validate(&mut self) -> Result<()> {
        // libwayshot 0.8 rebinds wl_output and roundtrips both native geometry
        // (mode + transform) and xdg logical geometry. Proxy IDs change on each
        // refresh, so compare named geometry rather than OutputInfo equality.
        self.connection.refresh_outputs()?;
        let current = self.connection.get_all_outputs();
        ensure!(
            current.len() == self.topology.len()
                && self.topology.iter().all(|old| {
                    let mut matches = current.iter().filter(|new| new.name == old.name);
                    matches.next().is_some_and(|new| {
                        new.logical_region == old.logical_region
                            && new.physical_size == old.physical_size
                            && new.transform == old.transform
                    }) && matches.next().is_none()
                }),
            "Display topology changed during capture; observe again"
        );
        Ok(())
    }

    /// Returns native-resolution pixels for settling, not the encoded thumbnail.
    pub(super) fn capture(&mut self, display: &super::Display) -> Result<DynamicImage> {
        use anyhow::Context;
        self.refresh_and_validate()?;
        let output = self
            .connection
            .get_all_outputs()
            .iter()
            .find(|o| o.name == display.id)
            .context("Display disappeared")?;
        ensure!(
            output.logical_position().x == display.x
                && output.logical_position().y == display.y
                && output.logical_size().width == display.width
                && output.logical_size().height == display.height,
            "Display geometry changed before capture; retry capabilities"
        );
        ensure!(
            display.width > 0 && display.height > 0,
            "Invalid output dimensions"
        );
        ensure!(
            u64::from(output.physical_size.width)
                * u64::from(output.physical_size.height)
                <= MAX_PIXELS,
            "Physical display exceeds capture limit"
        );
        // Preserve platform::capture's compositor path and allocation guards.
        // screenshot_single_output would skip rotation/flips and is NOT safe.
        let scale =
            (f64::from(output.physical_size.height) / f64::from(display.height)).max(1.0);
        ensure!(
            f64::from(display.width) * f64::from(display.height) * scale * scale
                <= MAX_PIXELS as f64,
            "Composited display exceeds capture limit"
        );
        let image = self
            .connection
            .screenshot_outputs(std::slice::from_ref(output), false)?;
        self.refresh_and_validate()?;
        Ok(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image_rs::{ImageBuffer, Rgba};
    use std::io::Cursor;

    fn synthetic(w: u32, h: u32, random: bool) -> DynamicImage {
        let mut state = 0x12345678u32;
        DynamicImage::ImageRgba8(ImageBuffer::from_fn(w, h, |x, y| {
            if random {
                let mut next = || {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    state as u8
                };
                Rgba([next(), next(), next(), next()])
            } else {
                // Repeated text-like strokes, gutters and dark/light bands. No font/GUI dependency.
                let ink = x % 11 < 7 && y % 19 > 4 && y % 19 < 15 && (x + y) % 7 < 3;
                let c = if ink {
                    215
                } else {
                    24 + ((y / 80) % 2) as u8 * 8
                };
                Rgba([c, c, c, 255])
            }
        }))
    }

    fn assert_pixels(image: DynamicImage) {
        let expected = if image.width() > MAX_EDGE || image.height() > MAX_EDGE {
            image.thumbnail(MAX_EDGE, MAX_EDGE)
        } else {
            image.clone()
        };
        let encoded = encode(image).unwrap();
        let decoded = image_rs::load_from_memory(&encoded.bytes).unwrap();
        assert_eq!(
            (encoded.width, encoded.height),
            (expected.width(), expected.height())
        );
        assert_eq!(
            (decoded.width(), decoded.height()),
            (encoded.width, encoded.height)
        );
        assert_eq!(decoded.color(), expected.color());
        assert_eq!(decoded.as_bytes(), expected.as_bytes());
        assert!(encoded.bytes.len() <= MAX_BYTES);
    }

    #[test]
    fn lossless_and_never_upscales() {
        for (w, h) in [
            (1, 1),
            (128, 72),
            (1280, 720),
            (1920, 1200),
            (1, 1800),
            (1800, 1),
        ] {
            assert_pixels(synthetic(w, h, true));
        }
        assert_pixels(DynamicImage::new_rgb8(27, 13));
        assert_pixels(DynamicImage::new_luma16(27, 13));
        assert_pixels(DynamicImage::ImageRgba16(ImageBuffer::from_fn(
            27,
            13,
            |x, y| {
                Rgba([
                    x as u16 * 1931,
                    y as u16 * 4093,
                    (x + y) as u16 * 1237,
                    32769,
                ])
            },
        )));
    }

    #[test]
    fn invalid_dimensions_and_native_pixel_limit() {
        assert!(encode(DynamicImage::new_rgb8(0, 1)).is_err());
        assert!(encode(DynamicImage::new_rgb32f(2, 2)).is_err());
        assert!(encode(DynamicImage::new_rgba32f(2, 2)).is_err());
        assert!(encode(DynamicImage::new_luma8(8001, 8000)).is_err());
    }

    #[test]
    fn bounded_lossless_retry_and_limit_error() {
        let image = synthetic(640, 400, false);
        let fast = png_attempt(&image, CompressionType::Fast, FilterType::Sub, MAX_BYTES)
            .unwrap()
            .unwrap();
        let balanced = png_attempt(
            &image,
            CompressionType::Default,
            FilterType::Adaptive,
            MAX_BYTES,
        )
        .unwrap()
        .unwrap();
        assert!(balanced.len() < fast.len());
        let (bytes, retried) = bounded_png(&image, balanced.len()).unwrap();
        assert!(retried);
        assert_eq!(
            image_rs::load_from_memory(&bytes).unwrap().as_bytes(),
            image.as_bytes()
        );
        assert!(bounded_png(&image, 20)
            .unwrap_err()
            .to_string()
            .contains("encoded size limit"));
    }

    #[test]
    fn incompressible_frame_reports_real_eight_mib_limit() {
        let error = encode(synthetic(1600, 1600, true))
            .err()
            .expect("must exceed 8 MiB");
        assert!(error.to_string().contains("encoded size limit"));
    }

    /// Explicit opt-in; synthetic data only. Prints elapsed CPU-work wall times
    /// and sizes, never pixels/PNG/base64. Run optimized (not a release build).
    #[test]
    #[ignore = "opt-in synthetic CPU benchmark; no capture or desktop input"]
    fn benchmark_synthetic_encoding() {
        use base64::Engine;
        const RUNS: u32 = 5;
        for (w, h) in [(1280, 720), (1920, 1200)] {
            for random in [false, true] {
                let source = synthetic(w, h, random);
                // Fair filter comparison on identical correctly sized pixels.
                let resized = if w > MAX_EDGE || h > MAX_EDGE {
                    source.thumbnail(MAX_EDGE, MAX_EDGE)
                } else {
                    source.clone()
                };
                for filter in [
                    FilterType::Adaptive,
                    FilterType::Sub,
                    FilterType::Up,
                    FilterType::NoFilter,
                ] {
                    let mut total = Duration::ZERO;
                    let mut bytes = 0;
                    for _ in 0..RUNS {
                        let start = Instant::now();
                        bytes = png_attempt(
                            &resized,
                            CompressionType::Fast,
                            filter,
                            MAX_BYTES,
                        )
                        .unwrap()
                        .unwrap()
                        .len();
                        total += start.elapsed();
                    }
                    eprintln!(
                        "{w}x{h} random={random} Fast+{filter:?} png_us={} bytes={bytes}",
                        total.as_micros() / u128::from(RUNS)
                    );
                }
                for old in [true, false] {
                    let mut total = Duration::ZERO;
                    let mut resize_time = Duration::ZERO;
                    let mut png_time = Duration::ZERO;
                    let mut b64_time = Duration::ZERO;
                    let mut size = 0;
                    let mut dims = (0, 0);
                    for _ in 0..RUNS {
                        let image = source.clone(); // Capture/allocation not included.
                        let start = Instant::now();
                        let encoded = if old {
                            let r = Instant::now();
                            let image = image.thumbnail(MAX_EDGE, MAX_EDGE);
                            let resize = r.elapsed();
                            let p = Instant::now();
                            let mut out = Cursor::new(Vec::new());
                            image
                                .write_to(&mut out, image_rs::ImageFormat::Png)
                                .unwrap();
                            EncodedCapture {
                                width: image.width(),
                                height: image.height(),
                                bytes: out.into_inner(),
                                timings: EncodeTimings {
                                    resize,
                                    png: p.elapsed(),
                                    lossless_retry: false,
                                },
                            }
                        } else {
                            encode(image).unwrap()
                        };
                        resize_time += encoded.timings.resize;
                        png_time += encoded.timings.png;
                        size = encoded.bytes.len();
                        dims = (encoded.width, encoded.height);
                        let b = Instant::now();
                        std::hint::black_box(
                            base64::engine::general_purpose::STANDARD
                                .encode(&encoded.bytes),
                        );
                        b64_time += b.elapsed();
                        total += start.elapsed();
                    }
                    eprintln!("{w}x{h} random={random} old={old} dims={dims:?} bytes={size} resize_us={} png_us={} base64_us={} total_us={}",
                        resize_time.as_micros()/u128::from(RUNS), png_time.as_micros()/u128::from(RUNS),
                        b64_time.as_micros()/u128::from(RUNS), total.as_micros()/u128::from(RUNS));
                }
            }
        }
    }
}
