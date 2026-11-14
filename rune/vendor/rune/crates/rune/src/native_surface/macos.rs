//! CoreVideo ownership boundary. Decoder output must be complete and immutable before handoff.
use super::*;
use objc2_core_foundation::{CFBoolean, CFDictionary, CFRetained, CFString, CFType};
use objc2_core_video::*;
use objc2_metal::{MTLPixelFormat, MTLTextureType};
use std::ptr::{NonNull, null_mut};

/// A retained, immutable CVPixelBuffer, normally supplied by VideoToolbox's output callback.
#[derive(Clone)]
pub struct PixelBuffer(CFRetained<CVPixelBuffer>);
// CoreVideo retain/release and read-only buffer access are thread safe. Construction requires
// the caller to prohibit mutations, including through aliases, until the last owner is released.
unsafe impl Send for PixelBuffer {}
unsafe impl Sync for PixelBuffer {}
impl PixelBuffer {
    /// Retain a borrowed CVPixelBufferRef (does not consume the caller's retain).
    ///
    /// # Safety
    /// `raw` must be a live CVPixelBufferRef. Decoding/writes must be finished, and its pixels
    /// and attachments must not be modified until every clone and GPU use has been released.
    pub unsafe fn from_raw(raw: *mut std::ffi::c_void) -> Result<Self, String> {
        let ptr = NonNull::new(raw.cast::<CVPixelBuffer>()).ok_or("null CVPixelBuffer")?;
        Ok(Self(unsafe { CFRetained::retain(ptr) }))
    }
    /// Wrap an IOSurface without copying. The new CVPixelBuffer retains the surface.
    ///
    /// # Safety
    /// `raw` must be a live IOSurfaceRef containing complete BGRA/420v/420f pixels. It must
    /// remain immutable through all GPU uses, including through aliases in other processes.
    pub unsafe fn from_iosurface(raw: *mut std::ffi::c_void, color: ColorInfo) -> Result<Self, String> {
        if raw.is_null() {
            return Err("null IOSurface".into());
        }
        let mut out = null_mut();
        check(
            unsafe { CVPixelBufferCreateWithIOSurface(None, &*raw.cast(), None, NonNull::from(&mut out)) },
            "CVPixelBufferCreateWithIOSurface",
        )?;
        let buffer = Self(unsafe { CFRetained::from_raw(NonNull::new(out).ok_or("null pixel buffer")?) });
        buffer.set_color(color);
        buffer.info()?;
        Ok(buffer)
    }
    fn set_color(&self, color: ColorInfo) {
        unsafe {
            self.0.set_attachment(
                kCVImageBufferYCbCrMatrixKey,
                match color.matrix {
                    Matrix::Bt601 => kCVImageBufferYCbCrMatrix_ITU_R_601_4,
                    Matrix::Bt709 => kCVImageBufferYCbCrMatrix_ITU_R_709_2,
                }
                .as_ref(),
                CVAttachmentMode::ShouldPropagate,
            );
            self.0.set_attachment(
                kCVImageBufferTransferFunctionKey,
                match color.transfer {
                    Transfer::Srgb => kCVImageBufferTransferFunction_sRGB,
                    Transfer::Bt709 => kCVImageBufferTransferFunction_ITU_R_709_2,
                }
                .as_ref(),
                CVAttachmentMode::ShouldPropagate,
            );
        }
    }
    #[allow(non_upper_case_globals)] // CoreVideo's public fourcc names.
    pub(crate) fn info(&self) -> Result<([u32; 2], Format, ColorInfo), String> {
        let b = &self.0;
        let format = match CVPixelBufferGetPixelFormatType(b) {
            kCVPixelFormatType_32BGRA => Format::Bgra,
            kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange => Format::Nv12Video,
            kCVPixelFormatType_420YpCbCr8BiPlanarFullRange => Format::Nv12Full,
            other => return Err(format!("unsupported CVPixelBuffer format {other:#x}; use BGRA, 420v or 420f")),
        };
        let size = [CVPixelBufferGetWidth(b) as u32, CVPixelBufferGetHeight(b) as u32];
        if size.contains(&0) || size.iter().any(|v| *v > 16384) {
            return Err("invalid video dimensions".into());
        }
        if format != Format::Bgra
            && (!size[0].is_multiple_of(2) || !size[1].is_multiple_of(2) || CVPixelBufferGetPlaneCount(b) != 2)
        {
            return Err("NV12 requires two planes and even dimensions".into());
        }
        // An absent matrix follows the common SD/HD convention; explicit unsupported colour
        // metadata is rejected instead of silently treating HDR/2020 as SDR/709.
        let matrix = unsafe { b.attachment(kCVImageBufferYCbCrMatrixKey, null_mut()) };
        let matrix = match matrix {
            None => {
                if size[1] <= 576 {
                    Matrix::Bt601
                } else {
                    Matrix::Bt709
                }
            }
            Some(v) if unsafe { cf_equal(&v, kCVImageBufferYCbCrMatrix_ITU_R_601_4) } => Matrix::Bt601,
            Some(v) if unsafe { cf_equal(&v, kCVImageBufferYCbCrMatrix_ITU_R_709_2) } => Matrix::Bt709,
            _ => return Err("unsupported YCbCr matrix; use BT.601 or BT.709".into()),
        };
        let transfer = unsafe { b.attachment(kCVImageBufferTransferFunctionKey, null_mut()) };
        let transfer = match transfer {
            None => {
                if format == Format::Bgra {
                    Transfer::Srgb
                } else {
                    Transfer::Bt709
                }
            }
            Some(v) if unsafe { cf_equal(&v, kCVImageBufferTransferFunction_sRGB) } => Transfer::Srgb,
            Some(v) if unsafe { cf_equal(&v, kCVImageBufferTransferFunction_ITU_R_709_2) } => Transfer::Bt709,
            _ => return Err("unsupported transfer function; use sRGB or BT.709 (HDR is not supported)".into()),
        };
        Ok((size, format, ColorInfo { matrix, transfer }))
    }
    /// Test source only: create a Metal-compatible IOSurface and fill it before publishing.
    /// The production decoder path never locks, reads or copies pixel memory.
    pub fn from_bytes(frame: &ByteFrame) -> Result<Self, String> {
        // Reuse portable validation before touching an unsafe plane pointer.
        let w = frame.width as usize;
        let h = frame.height as usize;
        if w == 0 || h == 0 || w > 16384 || h > 16384 {
            return Err("invalid dimensions".into());
        }
        let nv12 = frame.format != Format::Bgra;
        if nv12 && (!w.is_multiple_of(2) || !h.is_multiple_of(2)) {
            return Err("odd NV12 dimensions".into());
        }
        if frame.planes[0].len() != w * h * if nv12 { 1 } else { 4 }
            || frame.planes[1].len() != if nv12 { w * h / 2 } else { 0 }
        {
            return Err("invalid plane length".into());
        }
        let empty = CFDictionary::<CFString, CFType>::from_slices(&[], &[]);
        let attrs = unsafe {
            CFDictionary::<CFString, CFType>::from_slices(
                &[kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey],
                &[AsRef::<CFType>::as_ref(&*empty), CFBoolean::new(true).as_ref()],
            )
        };
        let format = match frame.format {
            Format::Bgra => kCVPixelFormatType_32BGRA,
            Format::Nv12Video => kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
            Format::Nv12Full => kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
        };
        let mut raw = null_mut();
        let status =
            unsafe { CVPixelBufferCreate(None, w, h, format, Some(attrs.as_opaque()), NonNull::from(&mut raw)) };
        check(status, "CVPixelBufferCreate")?;
        let buffer = Self(unsafe { CFRetained::from_raw(NonNull::new(raw).ok_or("null created pixel buffer")?) });
        unsafe {
            check(CVPixelBufferLockBaseAddress(&buffer.0, CVPixelBufferLockFlags(0)), "lock synthetic buffer")?;
            for plane in 0..if nv12 { 2 } else { 1 } {
                let (ptr, stride) = if nv12 {
                    (
                        CVPixelBufferGetBaseAddressOfPlane(&buffer.0, plane),
                        CVPixelBufferGetBytesPerRowOfPlane(&buffer.0, plane),
                    )
                } else {
                    (CVPixelBufferGetBaseAddress(&buffer.0), CVPixelBufferGetBytesPerRow(&buffer.0))
                };
                let row_bytes = w * if nv12 { 1 } else { 4 };
                let rows = if plane == 1 { h / 2 } else { h };
                for row in 0..rows {
                    std::ptr::copy_nonoverlapping(
                        frame.planes[plane].as_ptr().add(row * row_bytes),
                        ptr.cast::<u8>().add(row * stride),
                        row_bytes,
                    );
                }
            }
            CVPixelBufferUnlockBaseAddress(&buffer.0, CVPixelBufferLockFlags(0));
        }
        buffer.set_color(frame.color);
        Ok(buffer)
    }
}

pub(crate) struct Importer {
    cache: CFRetained<CVMetalTextureCache>,
    device: wgpu::Device,
    /// Whether `device` is Rune's native Metal backend's rather than wgpu-hal's.
    #[cfg_attr(not(feature = "metal-backend"), allow(dead_code))]
    native: bool,
    #[cfg(test)]
    pub(super) live_planes: Arc<std::sync::atomic::AtomicUsize>,
}
// CVMetalTexture retains the IOSurface mapping. wgpu's drop guard, or on the native Metal
// backend the texture's owner, keeps it for as long as any bind group or submitted command uses
// that plane, rather than merely retaining MTLTexture.
struct PlaneOwner {
    _texture: CFRetained<CVMetalTexture>,
    _buffer: PixelBuffer,
    #[cfg(test)]
    live_planes: Arc<std::sync::atomic::AtomicUsize>,
}
unsafe impl Send for PlaneOwner {}
unsafe impl Sync for PlaneOwner {}
impl Drop for PlaneOwner {
    fn drop(&mut self) {
        #[cfg(test)]
        self.live_planes.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}
impl Importer {
    pub fn new(device: &wgpu::Device) -> Result<Self, String> {
        let mut raw = null_mut();
        let mut create = |metal: &objc2_video::runtime::ProtocolObject<dyn objc2_metal::MTLDevice>| {
            check(unsafe { CVMetalTextureCache::create(None, None, metal, None, NonNull::from(&mut raw)) }, "CVMetalTextureCacheCreate")
        };
        let native = if let Some(metal) = unsafe { device.as_hal::<wgpu::hal::api::Metal>() } {
            create(metal.raw_device())?;
            false
        } else {
            #[cfg(feature = "metal-backend")]
            let Some(metal) = crate::gpu::metal::raw_device(device) else {
                return Err("NativeSurface needs a Metal device".into());
            };
            #[cfg(not(feature = "metal-backend"))]
            return Err("NativeSurface needs wgpu's Metal backend".into());
            #[cfg(feature = "metal-backend")]
            {
                create(metal)?;
                true
            }
        };
        Ok(Self {
            cache: unsafe { CFRetained::from_raw(NonNull::new(raw).ok_or("null texture cache")?) },
            device: device.clone(),
            native,
            #[cfg(test)]
            live_planes: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        })
    }
    pub fn flush(&self) {
        self.cache.flush(0);
    }
    pub fn import(&self, buffer: &PixelBuffer, format: Format, size: [u32; 2]) -> Result<[wgpu::Texture; 2], String> {
        let plane = |index: usize, width: u32, height: u32, metal_format, format| -> Result<wgpu::Texture, String> {
            let mut raw = null_mut();
            check(
                unsafe {
                    CVMetalTextureCache::create_texture_from_image(
                        None,
                        &self.cache,
                        &buffer.0,
                        None,
                        metal_format,
                        width as usize,
                        height as usize,
                        index,
                        NonNull::from(&mut raw),
                    )
                },
                "CVMetalTextureCacheCreateTextureFromImage (buffer must be IOSurface-backed)",
            )?;
            let cv = unsafe { CFRetained::from_raw(NonNull::new(raw).ok_or("null CVMetalTexture")?) };
            let texture = CVMetalTextureGetTexture(&cv).ok_or("null MTLTexture")?;
            let owner = PlaneOwner {
                _texture: cv,
                _buffer: buffer.clone(),
                #[cfg(test)]
                live_planes: self.live_planes.clone(),
            };
            #[cfg(test)]
            self.live_planes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let desc = pipeline::descriptor(width, height, format);
            #[cfg(feature = "metal-backend")]
            if self.native {
                return Ok(crate::gpu::metal::texture_from_raw(texture, &desc, std::sync::Arc::new(owner)));
            }
            let hal = unsafe {
                wgpu::hal::metal::Device::texture_from_raw(
                    texture,
                    format,
                    MTLTextureType::Type2D,
                    1,
                    1,
                    wgpu::hal::CopyExtent { width, height, depth: 1 },
                    Some(Box::new(move || drop(owner))),
                )
            };
            // The raw texture comes from this device's CV cache and exactly matches this descriptor.
            Ok(unsafe {
                self.device.create_texture_from_hal::<wgpu::hal::api::Metal>(hal, &desc, wgpu::TextureUses::RESOURCE)
            })
        };
        if format == Format::Bgra {
            let a = plane(0, size[0], size[1], MTLPixelFormat::BGRA8Unorm, wgpu::TextureFormat::Bgra8Unorm)?;
            Ok([a.clone(), a])
        } else {
            Ok([
                plane(0, size[0], size[1], MTLPixelFormat::R8Unorm, wgpu::TextureFormat::R8Unorm)?,
                plane(1, size[0] / 2, size[1] / 2, MTLPixelFormat::RG8Unorm, wgpu::TextureFormat::Rg8Unorm)?,
            ])
        }
    }
}
fn check(status: i32, operation: &str) -> Result<(), String> {
    if status == 0 { Ok(()) } else { Err(format!("{operation}: CoreVideo status {status}")) }
}
fn cf_equal(value: &CFType, string: &CFString) -> bool {
    value == AsRef::<CFType>::as_ref(string)
}
