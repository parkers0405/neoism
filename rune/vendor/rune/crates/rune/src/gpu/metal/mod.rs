//! Rune's native Metal backend, macOS's default (research/34), through wgpu's
//! `custom` backend, so compositors and the frame loop keep calling wgpu types.
//!
//! Shaders stay WGSL: naga translates each pipeline's entry points to MSL with the pipeline
//! layout's slots, as wgpu-hal does, minus its vertex pulling (vertex inputs use a vertex
//! descriptor). The uber shader instead loads `uber_native.metallib`, that MSL precompiled, and
//! Rune's other pipelines take theirs from `builtin_native.metallib` (`builtin`). Buffers are
//! `MTLStorageModeShared`; textures are private.
//!
//! A command encoder records one `MTLCommandBuffer` with unretained references: everything it
//! uses is kept alive until the queue's `MTLSharedEvent` reaches that buffer's value. A submit
//! that drew into a drawable holds its buffer, and `Queue::present` presents the drawable on it
//! and commits it, so a frame is one command buffer.
//!
//! The CPU writes buffers in place (`store`): `Staging` and `Frame::write_buffer` writes always,
//! swapping in a spare storage while the GPU still reads the current one, and
//! `Queue::write_buffer` when no recorded command uses the buffer. Writes to buffers the GPU
//! writes, and `write_texture`, stage through a blit buffer committed ahead of the next submit,
//! as wgpu orders them. Map and work-done callbacks run from `Device::poll` and `Queue::submit`
//! once their work has finished.
//!
//! Rune's layer tweaks (materials, present timing, transactions, occlusion) reach this backend's
//! CAMetalLayer through `gpu::with_layer`. Video planes from CoreVideo wrap as textures
//! (`texture_from_raw`) whose owner the bind groups and command buffers using them keep.
//! Compute passes, timestamps at pass boundaries (`TIMESTAMP_QUERY`), and immediates
//! (`IMMEDIATES`, set as inline vertex and fragment bytes, up to Metal's 4 KB) are as wgpu-hal's.
//! New textures read as zero, as wgpu promises. A pipeline without a layout derives one from
//! its shaders, as wgpu-core does (`derive`). Other queries, render bundles, indirect commands,
//! depth and multisampling are not implemented: they panic naming the way back to wgpu
//! (`unsupported!`).

pub(crate) mod bench;
mod builtin;
mod compute;
#[cfg(test)]
mod compute_tests;
mod derive;
#[cfg(test)]
mod derive_tests;
mod device;
mod encode;
mod layout;
mod pass;
mod pipeline;
mod query;
mod resource;
mod shader;
#[cfg(test)]
mod shader_tests;
mod store;
#[cfg(test)]
mod store_tests;
mod submit;
mod surface;
#[cfg(test)]
mod tests;

use std::future::ready;
use std::pin::Pin;

use objc2_metal::{MTLCopyAllDevices, MTLDevice};
use objc2_video::rc::Retained;
use objc2_video::runtime::ProtocolObject;
use wgpu::custom::*;

pub(crate) use device::{MetalDevice, Shared};

pub(crate) type Obj<P> = Retained<ProtocolObject<P>>;

/// Keeps memory a texture made outside this backend reads valid, as long as the texture, its
/// views, bind groups using them, or a command buffer using those is alive.
pub(crate) type Owner = std::sync::Arc<dyn std::any::Any + Send + Sync>;

/// The Metal device under `device`, when it's this backend's.
// macOS-only callers: video, native surfaces and the precompiled metallib.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn raw_device(device: &wgpu::Device) -> Option<&ProtocolObject<dyn MTLDevice>> {
    device.as_custom::<MetalDevice>().map(|d| &*d.shared.device)
}

/// A texture over `raw`, made on `raw_device`'s device and matching `desc`, which `owner` keeps valid.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn texture_from_raw(raw: Obj<dyn objc2_metal::MTLTexture>, desc: &wgpu::TextureDescriptor<'_>, owner: Owner) -> wgpu::Texture {
    wgpu::Texture::from_custom(resource::MetalTexture::from_raw(raw, desc, owner), desc)
}

/// Whether `module`, made on this backend, can make pipelines from all its entry points: false
/// for a precompiled library Metal refused or that lacks one of them.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn module_ready(module: &wgpu::ShaderModule) -> bool {
    module.as_custom::<shader::ShaderModule>().is_some_and(shader::ShaderModule::ready)
}

/// The MSL this backend compiles `wgsl`'s `vs` and `fs` to, in MSL `lang`, for a render
/// pipeline whose one bind group has `entries`.
#[cfg(test)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn render_msl(wgsl: &str, entries: &[wgpu::BindGroupLayoutEntry], lang: (u8, u8)) -> [String; 2] {
    let ir = shader::Ir::parse("render_msl", wgsl, wgpu::ShaderRuntimeChecks::checked());
    let group = layout::BindGroupLayout::new(&wgpu::BindGroupLayoutDescriptor { label: None, entries });
    let layout = layout::PipelineLayout::from_groups(&[Some(&group)], 0);
    [(wgpu::naga::ShaderStage::Vertex, "vs", 0), (wgpu::naga::ShaderStage::Fragment, "fs", 1)].map(|(kind, entry, s)| {
        let point = shader::Point { kind, entry: Some(entry), constants: &[] };
        shader::translate(&ir, &point, &layout.raw.stages[s], false, lang).unwrap_or_else(|e| panic!("{e}")).source
    })
}

/// How many pipelines compiled at runtime because the precompiled library lacked them.
pub(crate) fn builtin_misses() -> usize { builtin::misses() }

/// The wgpu features this backend can offer: a compositor that wants others draws with wgpu
/// unless the app or `RUNE_GPU_BACKEND` asked for this backend (`backend::wgpu_for`).
pub(crate) const FEATURES: wgpu::Features = wgpu::Features::TIMESTAMP_QUERY.union(wgpu::Features::IMMEDIATES);

/// The most immediate data a pipeline takes: what `setVertexBytes` and `setFragmentBytes` copy.
const MAX_IMMEDIATE_SIZE: u32 = 4096;

/// Whether `adapter` is this backend's.
pub(crate) fn owns_adapter(adapter: &wgpu::Adapter) -> bool { adapter.as_custom::<MetalAdapter>().is_some() }

/// Whether this backend draws: the default, unless `App::gpu_backend` or `RUNE_GPU_BACKEND`
/// picks wgpu.
pub(crate) fn enabled() -> bool {
    crate::gpu::backend::current() == crate::GpuBackend::Metal
}

pub(crate) fn instance() -> wgpu::Instance {
    rune_staging::set_direct(write_in_place);
    wgpu::Instance::from_custom(MetalInstance)
}

/// Whether `queue` is this backend's.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn owns(queue: &wgpu::Queue) -> bool { queue.as_custom::<submit::MetalQueue>().is_some() }

/// Staging's in-place writer: a Metal buffer the GPU only reads takes writes into its storage.
fn write_in_place(buffer: &wgpu::Buffer, offset: u64, data: &[u8]) -> bool {
    buffer.as_custom::<resource::MetalBuffer>().is_some_and(|b| b.store.write_ahead(&b.shared, offset, data, || true))
}

/// The CAMetalLayer a surface of this backend presents into.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn layer<'s>(surface: &'s wgpu::Surface<'_>) -> Option<&'s objc2_video::runtime::AnyObject> {
    surface.as_custom::<surface::MetalSurface>().map(|s| -> &objc2_video::runtime::AnyObject { s.layer() })
}

macro_rules! send_sync {
    ($($t:ty),*) => { $(
        // SAFETY: Metal devices, queues, resources and events are thread-safe; command buffers,
        // encoders and drawables are used by one thread at a time, as wgpu's handles are.
        unsafe impl Send for $t {}
        unsafe impl Sync for $t {}
    )* };
}
pub(crate) use send_sync;

/// Panics for a wgpu feature this backend doesn't implement, naming the way back to wgpu.
macro_rules! unsupported {
    ($($what:tt)*) => {
        panic!("Rune's Metal backend doesn't support {}; draw with wgpu instead: App::gpu_backend(GpuBackend::Wgpu) or RUNE_GPU_BACKEND=wgpu", format_args!($($what)*))
    };
}
pub(crate) use unsupported;

#[derive(Debug)]
struct MetalInstance;

impl InstanceInterface for MetalInstance {
    fn new(_: wgpu::InstanceDescriptor) -> Self { MetalInstance }
    unsafe fn create_surface(&self, target: wgpu::SurfaceTargetUnsafe) -> Result<DispatchSurface, wgpu::CreateSurfaceError> {
        Ok(unsafe { surface::create(target) })
    }
    fn request_adapter(&self, options: &wgpu::RequestAdapterOptions<'_, '_>) -> Pin<Box<dyn RequestAdapterFuture>> {
        let device = preferred(devices(), options.power_preference).expect("Rune's Metal backend needs a Metal device");
        Box::pin(ready(Ok(DispatchAdapter::custom(MetalAdapter { device }))))
    }
    fn poll_all_devices(&self, _: bool) -> bool { true }
    fn wgsl_language_features(&self) -> wgpu::WgslLanguageFeatures { wgpu::WgslLanguageFeatures::empty() }
    fn enumerate_adapters(&self, _: wgpu::Backends) -> Pin<Box<dyn EnumerateAdapterFuture>> {
        Box::pin(ready(devices().into_iter().map(|device| DispatchAdapter::custom(MetalAdapter { device })).collect()))
    }
}

#[derive(Debug)]
struct MetalAdapter {
    device: Obj<dyn MTLDevice>,
}
send_sync!(MetalAdapter);

/// The Mac's GPUs, as wgpu-hal lists them. Not `MTLCreateSystemDefaultDevice`: to find the main
/// display's GPU it connects to the window server, which costs a fresh process about 8 ms more
/// (14 ms instead of 5.5 on an M-series Mac), on every headless run and on a launch's prewarm.
fn devices() -> Vec<Obj<dyn MTLDevice>> {
    MTLCopyAllDevices().to_vec()
}

/// The first integrated GPU, or for `HighPerformance` the first discrete one, else the first: the
/// order wgpu ranks adapters in.
fn preferred(devices: Vec<Obj<dyn MTLDevice>>, power: wgpu::PowerPreference) -> Option<Obj<dyn MTLDevice>> {
    let integrated = power != wgpu::PowerPreference::HighPerformance;
    let at = devices.iter().position(|d| d.hasUnifiedMemory() == integrated).unwrap_or(0);
    devices.into_iter().nth(at)
}

fn info(device: &ProtocolObject<dyn MTLDevice>) -> wgpu::AdapterInfo {
    let kind = if device.hasUnifiedMemory() { wgpu::DeviceType::IntegratedGpu } else { wgpu::DeviceType::DiscreteGpu };
    let mut info = wgpu::AdapterInfo::new(kind, wgpu::Backend::Metal);
    info.name = device.name().to_string();
    info.driver = "rune metal".into();
    info
}

/// What a Metal GPU of the Apple or Mac 2 families can do, in wgpu's terms.
fn limits() -> wgpu::Limits {
    wgpu::Limits {
        max_texture_dimension_1d: 16384, max_texture_dimension_2d: 16384, max_buffer_size: 1 << 30, max_immediate_size: MAX_IMMEDIATE_SIZE,
        ..wgpu::Limits::default()
    }
}

impl AdapterInterface for MetalAdapter {
    fn request_device(&self, desc: &wgpu::DeviceDescriptor<'_>) -> Pin<Box<dyn RequestDeviceFuture>> {
        let (device, queue) = device::new(self.device.clone(), desc);
        Box::pin(ready(Ok((DispatchDevice::custom(device), DispatchQueue::custom(queue)))))
    }
    fn is_surface_supported(&self, _: &DispatchSurface) -> bool { true }
    fn features(&self) -> wgpu::Features {
        if query::supported(&self.device) { FEATURES } else { FEATURES - wgpu::Features::TIMESTAMP_QUERY }
    }
    fn limits(&self) -> wgpu::Limits { limits() }
    fn downlevel_capabilities(&self) -> wgpu::DownlevelCapabilities { Default::default() }
    fn get_info(&self) -> wgpu::AdapterInfo { info(&self.device) }
    fn get_texture_format_features(&self, format: wgpu::TextureFormat) -> wgpu::TextureFormatFeatures {
        use wgpu::TextureFormatFeatureFlags as F;
        let float = format.sample_type(None, None).is_some_and(|t| matches!(t, wgpu::TextureSampleType::Float { .. }));
        let flags = if float { F::FILTERABLE | F::BLENDABLE } else { F::empty() };
        wgpu::TextureFormatFeatures { allowed_usages: wgpu::TextureUsages::all(), flags }
    }
    fn get_presentation_timestamp(&self) -> wgpu::PresentationTimestamp { wgpu::PresentationTimestamp(0) }
    fn cooperative_matrix_properties(&self) -> Vec<wgpu::wgt::CooperativeMatrixProperties> { Vec::new() }
}
