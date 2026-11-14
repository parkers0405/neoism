//! The device: what it shares with its queue, and the resources it makes.

use std::future::ready;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

use objc2_metal::{MTLCommandQueue, MTLDevice, MTLLanguageVersion, MTLLibrary, MTLSharedEvent};
use parking_lot::Mutex;
use wgpu::custom::*;

use super::encode::{Inert, MetalEncoder, Zeros, zero_texture};
use super::layout::{BindGroup, BindGroupLayout, PipelineLayout};
use super::resource::{Live, MetalBuffer, MetalSampler, MetalTexture};
use super::shader::ShaderModule;
use super::submit::{MetalQueue, State};
use super::{Obj, info, send_sync};

/// The device, its one queue, the event every command buffer signals on completion, and the
/// submission state behind it.
#[derive(Debug)]
pub(crate) struct Shared {
    pub(super) device: Obj<dyn MTLDevice>,
    pub(super) queue: Obj<dyn MTLCommandQueue>,
    pub(super) event: Obj<dyn MTLSharedEvent>,
    pub(super) msl: ((u8, u8), MTLLanguageVersion),
    pub(super) state: Mutex<State>,
    /// Buffers alive, for `Device::get_internal_counters`.
    pub(super) buffers: Arc<AtomicIsize>,
    /// Bind groups and textures alive, likewise.
    bind_groups: Arc<AtomicIsize>,
    textures: Arc<AtomicIsize>,
    /// Encoders that made a command buffer not yet submitted or dropped. While none has, every
    /// recorded command that reads a buffer's storage was submitted.
    pub(super) recording: AtomicUsize,
    /// What new textures are zeroed from, made with the first.
    pub(super) zeros: OnceLock<Zeros>,
    /// Rune's precompiled pipelines, loaded with the first pipeline that looks one up.
    pub(super) builtin: OnceLock<Option<Obj<dyn MTLLibrary>>>,
}
send_sync!(Shared);

#[derive(Debug)]
pub(crate) struct MetalDevice {
    pub(crate) shared: Arc<Shared>,
    features: wgpu::Features,
    limits: wgpu::Limits,
}

pub(super) fn new(device: Obj<dyn MTLDevice>, desc: &wgpu::DeviceDescriptor<'_>) -> (MetalDevice, MetalQueue) {
    let shared = Arc::new(Shared {
        queue: device.newCommandQueue().expect("a Metal command queue"),
        event: device.newSharedEvent().expect("a Metal shared event"),
        msl: msl_version(),
        state: Mutex::new(State::default()),
        buffers: Arc::default(),
        bind_groups: Arc::default(),
        textures: Arc::default(),
        recording: AtomicUsize::new(0),
        zeros: OnceLock::new(),
        builtin: OnceLock::new(),
        device,
    });
    let device = MetalDevice { shared: shared.clone(), features: desc.required_features, limits: desc.required_limits.clone() };
    (device, MetalQueue(shared))
}

/// wgpu-hal's choice of MSL version for this macOS or iOS. Their numbers meet at 26; before, iOS
/// 18 and 17 match macOS 15 and 14.
pub(super) fn msl_version() -> ((u8, u8), MTLLanguageVersion) {
    let mut buf = [0u8; 32];
    let mut len = buf.len();
    // SAFETY: sysctlbyname writes at most `len` bytes into `buf` and the length it wrote into `len`.
    let ok = unsafe { libc::sysctlbyname(c"kern.osproductversion".as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) } == 0;
    let version = if ok { String::from_utf8_lossy(&buf[..len.saturating_sub(1)]).into_owned() } else { String::new() };
    let major: u32 = version.split('.').next().and_then(|m| m.trim().parse().ok()).unwrap_or(0);
    #[cfg(not(target_os = "macos"))]
    let major = match major { 18 => 15, 17 => 14, 26.. => major, _ => 0 };
    match major {
        26.. => ((4, 0), MTLLanguageVersion::Version4_0),
        15 => ((3, 2), MTLLanguageVersion::Version3_2),
        14 => ((3, 1), MTLLanguageVersion::Version3_1),
        _ => ((3, 0), MTLLanguageVersion::Version3_0),
    }
}

impl DeviceInterface for MetalDevice {
    fn features(&self) -> wgpu::Features { self.features }
    fn limits(&self) -> wgpu::Limits { self.limits.clone() }
    fn adapter_info(&self) -> wgpu::AdapterInfo { info(&self.shared.device) }
    fn create_shader_module(&self, desc: wgpu::ShaderModuleDescriptor<'_>, checks: wgpu::ShaderRuntimeChecks) -> DispatchShaderModule {
        DispatchShaderModule::custom(ShaderModule::new(desc, checks))
    }
    unsafe fn create_shader_module_passthrough(&self, desc: &wgpu::ShaderModuleDescriptorPassthrough<'_>) -> DispatchShaderModule {
        DispatchShaderModule::custom(ShaderModule::passthrough(&self.shared, desc))
    }
    fn create_bind_group_layout(&self, desc: &wgpu::BindGroupLayoutDescriptor<'_>) -> DispatchBindGroupLayout {
        DispatchBindGroupLayout::custom(BindGroupLayout::new(desc))
    }
    fn create_bind_group(&self, desc: &wgpu::BindGroupDescriptor<'_>) -> DispatchBindGroup { DispatchBindGroup::custom(BindGroup::new(desc, Live::new(&self.shared.bind_groups))) }
    fn create_pipeline_layout(&self, desc: &wgpu::PipelineLayoutDescriptor<'_>) -> DispatchPipelineLayout {
        DispatchPipelineLayout::custom(PipelineLayout::new(desc))
    }
    fn create_render_pipeline(&self, desc: &wgpu::RenderPipelineDescriptor<'_>) -> DispatchRenderPipeline {
        DispatchRenderPipeline::custom(super::pipeline::create(&self.shared, desc))
    }
    fn create_mesh_pipeline(&self, _: &wgpu::MeshPipelineDescriptor<'_>) -> DispatchRenderPipeline {
        super::unsupported!("mesh shaders")
    }
    fn create_compute_pipeline(&self, desc: &wgpu::ComputePipelineDescriptor<'_>) -> DispatchComputePipeline {
        DispatchComputePipeline::custom(super::compute::create(&self.shared, desc))
    }
    unsafe fn create_pipeline_cache(&self, _: &wgpu::PipelineCacheDescriptor<'_>) -> DispatchPipelineCache { DispatchPipelineCache::custom(Inert) }
    fn create_buffer(&self, desc: &wgpu::BufferDescriptor<'_>) -> DispatchBuffer {
        DispatchBuffer::custom(MetalBuffer::new(&self.shared, desc.size))
    }
    fn create_texture(&self, desc: &wgpu::TextureDescriptor<'_>) -> DispatchTexture {
        let texture = MetalTexture::new(&self.shared.device, desc, Live::new(&self.shared.textures));
        let raw = texture.raw.clone();
        let zeros = self.shared.zeros.get_or_init(|| Zeros::new(&self.shared));
        self.shared.upload(|blit| zero_texture(blit, zeros, &raw), [raw.clone(), raw.clone()]);
        DispatchTexture::custom(texture)
    }
    fn create_external_texture(&self, _: &wgpu::ExternalTextureDescriptor<'_>, _: &[&wgpu::TextureView]) -> DispatchExternalTexture {
        super::unsupported!("external textures")
    }
    fn create_blas(&self, _: &wgpu::CreateBlasDescriptor<'_>, _: wgpu::BlasGeometrySizeDescriptors) -> (Option<u64>, DispatchBlas) {
        super::unsupported!("ray tracing")
    }
    fn create_tlas(&self, _: &wgpu::CreateTlasDescriptor<'_>) -> DispatchTlas { super::unsupported!("ray tracing") }
    fn create_sampler(&self, desc: &wgpu::SamplerDescriptor<'_>) -> DispatchSampler {
        DispatchSampler::custom(MetalSampler::new(&self.shared.device, desc))
    }
    fn create_query_set(&self, desc: &wgpu::QuerySetDescriptor<'_>) -> DispatchQuerySet {
        DispatchQuerySet::custom(super::query::QuerySet::new(&self.shared.device, desc))
    }
    fn create_command_encoder(&self, _: &wgpu::CommandEncoderDescriptor<'_>) -> DispatchCommandEncoder {
        DispatchCommandEncoder::custom(MetalEncoder::new(self.shared.clone()))
    }
    fn create_render_bundle_encoder(&self, _: &wgpu::RenderBundleEncoderDescriptor<'_>) -> DispatchRenderBundleEncoder {
        super::unsupported!("render bundles")
    }
    fn set_device_lost_callback(&self, _: BoxDeviceLostCallback) {}
    fn on_uncaptured_error(&self, _: Arc<dyn wgpu::UncapturedErrorHandler>) {}
    // The backend validates nothing, so a scope never catches an error.
    fn push_error_scope(&self, _: wgpu::ErrorFilter) -> u32 { 0 }
    fn pop_error_scope(&self, _: u32) -> Pin<Box<dyn PopErrorScopeFuture>> { Box::pin(ready(None)) }
    unsafe fn start_graphics_debugger_capture(&self) {}
    unsafe fn stop_graphics_debugger_capture(&self) {}
    fn poll(&self, poll: wgpu::wgt::PollType<u64>) -> Result<wgpu::PollStatus, wgpu::PollError> { self.shared.poll(poll) }
    fn get_internal_counters(&self) -> wgpu::InternalCounters {
        let counters = wgpu::InternalCounters::default();
        counters.hal.buffers.set(self.shared.buffers.load(Ordering::Relaxed));
        counters.hal.bind_groups.set(self.shared.bind_groups.load(Ordering::Relaxed));
        counters.hal.textures.set(self.shared.textures.load(Ordering::Relaxed));
        counters
    }
    fn generate_allocator_report(&self) -> Option<wgpu::AllocatorReport> { None }
    fn destroy(&self) {}
}
