use std::mem;
use std::path::Path;

use metal::{
    CompileOptions, DeviceRef, MTLLoadAction, MTLPixelFormat, MTLPrimitiveType,
    MTLSamplerAddressMode, MTLSamplerMinMagFilter, MTLStorageMode, MTLStoreAction,
    MTLTextureUsage, RenderPassDescriptor, RenderPipelineDescriptor, RenderPipelineState,
    SamplerDescriptor, SamplerState, Texture, TextureDescriptor, TextureRef,
};
use web_time::Instant;

use super::shader_overlay::{
    compile_shadertoy_msl, shader_source, GlobalsUniform, ShaderOverlayConfig,
    ShaderOverlayError,
};

const FULLSCREEN_VERTEX_MSL: &str = r#"
#include <metal_stdlib>
using namespace metal;

vertex float4 overlay_vertex(uint vertex_id [[vertex_id]]) {
    const float2 positions[] = {
        float2(-1.0, -3.0),
        float2(-1.0,  1.0),
        float2( 3.0,  1.0),
    };
    return float4(positions[vertex_id], 0.0, 1.0);
}
"#;

struct MetalOverlayPass {
    pipeline: RenderPipelineState,
}

pub(crate) struct MetalShaderOverlay {
    passes: Vec<MetalOverlayPass>,
    sampler: SamplerState,
    scene: Option<Texture>,
    ping_pong: [Option<Texture>; 2],
    retired_targets: Vec<Vec<Texture>>,
    extent: (u64, u64),
    frame: u32,
    started_at: Instant,
    last_frame_at: Instant,
}

impl std::fmt::Debug for MetalShaderOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetalShaderOverlay")
            .field("passes", &self.passes.len())
            .field("extent", &self.extent)
            .field("frame", &self.frame)
            .finish_non_exhaustive()
    }
}

impl MetalShaderOverlay {
    pub(crate) fn load(
        device: &DeviceRef,
        config: ShaderOverlayConfig,
    ) -> Result<Option<Self>, ShaderOverlayError> {
        if config.is_empty() {
            return Ok(None);
        }

        let vertex_library = device
            .new_library_with_source(FULLSCREEN_VERTEX_MSL, &CompileOptions::new())
            .map_err(|message| metal_error("<fullscreen-vertex>", message))?;
        let vertex = vertex_library
            .get_function("overlay_vertex", None)
            .map_err(|message| metal_error("<fullscreen-vertex>", message))?;
        let mut passes = Vec::with_capacity(config.shaders.len());

        for path in &config.shaders {
            let source = shader_source(path)?;
            let (msl, entry_point) = compile_shadertoy_msl(path, &source)?;
            let library = device
                .new_library_with_source(&msl, &CompileOptions::new())
                .map_err(|message| ShaderOverlayError::CompileMetal {
                    path: path.clone(),
                    message,
                })?;
            let fragment =
                library
                    .get_function(&entry_point, None)
                    .map_err(|message| ShaderOverlayError::CompileMetal {
                        path: path.clone(),
                        message,
                    })?;
            let descriptor = RenderPipelineDescriptor::new();
            descriptor.set_label("Sugarloaf shader overlay");
            descriptor.set_vertex_function(Some(&vertex));
            descriptor.set_fragment_function(Some(&fragment));
            descriptor
                .color_attachments()
                .object_at(0)
                .expect("overlay color attachment")
                .set_pixel_format(MTLPixelFormat::BGRA8Unorm);
            let pipeline =
                device
                    .new_render_pipeline_state(&descriptor)
                    .map_err(|message| ShaderOverlayError::CompileMetal {
                        path: path.clone(),
                        message,
                    })?;
            passes.push(MetalOverlayPass { pipeline });
        }

        let sampler_descriptor = SamplerDescriptor::new();
        sampler_descriptor.set_min_filter(MTLSamplerMinMagFilter::Linear);
        sampler_descriptor.set_mag_filter(MTLSamplerMinMagFilter::Linear);
        sampler_descriptor.set_s_address_mode(MTLSamplerAddressMode::ClampToEdge);
        sampler_descriptor.set_t_address_mode(MTLSamplerAddressMode::ClampToEdge);
        let now = Instant::now();
        Ok(Some(Self {
            passes,
            sampler: device.new_sampler(&sampler_descriptor),
            scene: None,
            ping_pong: [None, None],
            retired_targets: Vec::new(),
            extent: (0, 0),
            frame: 0,
            started_at: now,
            last_frame_at: now,
        }))
    }

    pub(crate) fn scene_texture(
        &mut self,
        device: &DeviceRef,
        drawable: &TextureRef,
    ) -> Texture {
        self.ensure_targets(device, drawable.width(), drawable.height());
        self.scene
            .as_ref()
            .expect("overlay scene texture")
            .to_owned()
    }

    pub(crate) fn encode(
        &mut self,
        command_buffer: &metal::CommandBufferRef,
        drawable: &TextureRef,
    ) {
        self.frame = self.frame.wrapping_add(1);
        let globals = GlobalsUniform::new(
            drawable.width() as f32,
            drawable.height() as f32,
            self.frame,
            self.started_at,
            &mut self.last_frame_at,
        );
        let scene = self.scene.as_deref().expect("overlay scene texture");

        for (index, pass) in self.passes.iter().enumerate() {
            let input = if index == 0 {
                scene
            } else {
                self.ping_pong[(index - 1) % 2]
                    .as_deref()
                    .expect("overlay intermediate texture")
            };
            let output = if index + 1 == self.passes.len() {
                drawable
            } else {
                self.ping_pong[index % 2]
                    .as_deref()
                    .expect("overlay intermediate texture")
            };

            let descriptor = RenderPassDescriptor::new();
            let attachment = descriptor
                .color_attachments()
                .object_at(0)
                .expect("overlay color attachment");
            attachment.set_texture(Some(output));
            attachment.set_load_action(MTLLoadAction::DontCare);
            attachment.set_store_action(MTLStoreAction::Store);
            let encoder = command_buffer.new_render_command_encoder(descriptor);
            encoder.set_label("Sugarloaf shader overlay pass");
            encoder.set_render_pipeline_state(&pass.pipeline);
            encoder.set_fragment_bytes(
                0,
                mem::size_of::<GlobalsUniform>() as u64,
                &globals as *const GlobalsUniform as *const std::ffi::c_void,
            );
            encoder.set_fragment_texture(0, Some(input));
            encoder.set_fragment_sampler_state(0, Some(&self.sampler));
            encoder.draw_primitives(MTLPrimitiveType::Triangle, 0, 3);
            encoder.end_encoding();
        }
    }

    fn ensure_targets(&mut self, device: &DeviceRef, width: u64, height: u64) {
        let extent = (width.max(1), height.max(1));
        if self.extent == extent && self.scene.is_some() {
            return;
        }
        // Keep a few old target generations alive while already-committed
        // command buffers can still reference them during rapid resizes.
        let mut retired = Vec::with_capacity(3);
        retired.extend(self.scene.take());
        retired.extend(self.ping_pong.iter_mut().filter_map(Option::take));
        if !retired.is_empty() {
            self.retired_targets.push(retired);
            if self.retired_targets.len() > 3 {
                self.retired_targets.remove(0);
            }
        }
        self.extent = extent;
        self.scene = Some(make_target(device, extent, "Sugarloaf overlay scene"));
        self.ping_pong = if self.passes.len() > 1 {
            [
                Some(make_target(device, extent, "Sugarloaf overlay ping")),
                Some(make_target(device, extent, "Sugarloaf overlay pong")),
            ]
        } else {
            [None, None]
        };
    }
}

fn make_target(device: &DeviceRef, extent: (u64, u64), label: &str) -> Texture {
    let descriptor = TextureDescriptor::new();
    descriptor.set_width(extent.0);
    descriptor.set_height(extent.1);
    descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
    descriptor.set_storage_mode(MTLStorageMode::Private);
    descriptor.set_usage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
    let texture = device.new_texture(&descriptor);
    texture.set_label(label);
    texture
}

fn metal_error(path: &str, message: String) -> ShaderOverlayError {
    ShaderOverlayError::CompileMetal {
        path: Path::new(path).to_path_buf(),
        message,
    }
}
