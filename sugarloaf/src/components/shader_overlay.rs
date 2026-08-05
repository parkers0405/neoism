use std::borrow::Cow;
use std::fmt;
use std::path::{Path, PathBuf};
#[cfg(any(feature = "wgpu", target_os = "macos"))]
use web_time::Instant;

#[cfg(feature = "wgpu")]
use crate::context::webgpu::WgpuContext;

const SHADERTOY_PREFIX: &str = r#"#version 450
layout(location = 0) out vec4 _fragColor;

layout(set = 0, binding = 0) uniform Globals {
    vec3 iResolution;
    float iTime;
    float iTimeDelta;
    float iFrameRate;
    int iFrame;
    vec4 iChannelTime;
    vec4 iChannelResolution[4];
    vec4 iMouse;
    vec4 iDate;
    float iFocus;
    float iTimeFocused;
    float iTimeUnfocused;
};

layout(set = 0, binding = 1) uniform texture2D iChannel0;
layout(set = 0, binding = 2) uniform sampler iSampler;

vec4 sampleChannel0(vec2 uv) {
    return texture(sampler2D(iChannel0, iSampler), uv);
}

"#;

const SHADERTOY_SUFFIX: &str = r#"
void main() {
    mainImage(_fragColor, gl_FragCoord.xy);
}
"#;

pub const BUILTIN_CTV_ROUND: &str = "builtin:ctv_round";
pub const BUILTIN_HYPNO_CRT: &str = "builtin:hypno_crt";
pub const BUILTIN_SHADER_OVERLAYS: &[&str] = &[BUILTIN_CTV_ROUND];

#[cfg(feature = "wgpu")]
const FULLSCREEN_VERTEX: &str = r#"
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 3.0,  1.0),
    );

    let position = positions[vertex_index];
    return vec4<f32>(position, 0.0, 1.0);
}
"#;

#[cfg(any(feature = "wgpu", target_os = "macos"))]
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct GlobalsUniform {
    resolution_time: [f32; 4],
    time_delta_frame_rate_frame: [f32; 4],
    channel_time: [f32; 4],
    channel_resolution: [[f32; 4]; 4],
    mouse: [f32; 4],
    date: [f32; 4],
    focus: [f32; 4],
}

#[cfg(any(feature = "wgpu", target_os = "macos"))]
impl GlobalsUniform {
    pub(crate) fn new(
        width: f32,
        height: f32,
        frame: u32,
        started_at: Instant,
        last_frame_at: &mut Instant,
    ) -> Self {
        let now = Instant::now();
        let elapsed = now.duration_since(started_at).as_secs_f32();
        let delta = now.duration_since(*last_frame_at).as_secs_f32().max(0.0);
        *last_frame_at = now;

        let rate = if delta > 0.0 { 1.0 / delta } else { 0.0 };

        Self {
            resolution_time: [width, height, 1.0, elapsed],
            time_delta_frame_rate_frame: [delta, rate, frame as f32, 0.0],
            channel_time: [elapsed, 0.0, 0.0, 0.0],
            channel_resolution: [[width, height, 1.0, 0.0], [0.0; 4], [0.0; 4], [0.0; 4]],
            mouse: [0.0; 4],
            date: shader_date(now),
            // Focus tracking is wired as always-focused until the window layer
            // exposes focus transitions to Sugarloaf.
            focus: [1.0, elapsed, 0.0, 0.0],
        }
    }
}

#[cfg(any(feature = "wgpu", target_os = "macos"))]
fn shader_date(_now: Instant) -> [f32; 4] {
    // Keep the ABI compatible with Ghostty/Shadertoy. The exact wall-clock date
    // is not currently available without another dependency in Sugarloaf.
    [1970.0, 1.0, 1.0, 0.0]
}

#[derive(Clone, Debug, Default)]
pub struct ShaderOverlayConfig {
    pub shaders: Vec<PathBuf>,
}

impl ShaderOverlayConfig {
    pub fn new<I, P>(shaders: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        Self {
            shaders: shaders.into_iter().map(Into::into).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.shaders.is_empty()
    }
}

#[derive(Debug)]
pub enum ShaderOverlayError {
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        message: String,
    },
    Validate {
        path: PathBuf,
        message: String,
    },
    WriteWgsl {
        path: PathBuf,
        message: String,
    },
    ValidateWgsl {
        path: PathBuf,
        message: String,
    },
    WriteMsl {
        path: PathBuf,
        message: String,
    },
    CompileMetal {
        path: PathBuf,
        message: String,
    },
    UnsupportedBackend {
        backend: &'static str,
    },
}

impl fmt::Display for ShaderOverlayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(f, "failed to read shader {}: {source}", path.display())
            }
            Self::Parse { path, message } => {
                write!(f, "failed to parse shader {}: {message}", path.display())
            }
            Self::Validate { path, message } => {
                write!(f, "failed to validate shader {}: {message}", path.display())
            }
            Self::WriteWgsl { path, message } => {
                write!(
                    f,
                    "failed to write WGSL for shader {}: {message}",
                    path.display()
                )
            }
            Self::ValidateWgsl { path, message } => {
                write!(
                    f,
                    "failed to validate WGSL for shader {}: {message}",
                    path.display()
                )
            }
            Self::WriteMsl { path, message } => {
                write!(
                    f,
                    "failed to write MSL for shader {}: {message}",
                    path.display()
                )
            }
            Self::CompileMetal { path, message } => {
                write!(
                    f,
                    "failed to compile Metal shader {}: {message}",
                    path.display()
                )
            }
            Self::UnsupportedBackend { backend } => {
                write!(f, "shader overlays are not implemented for {backend}")
            }
        }
    }
}

impl std::error::Error for ShaderOverlayError {}

#[cfg(feature = "wgpu")]
pub struct ShaderOverlayBrush {
    passes: Vec<ShaderOverlayPass>,
    sampler: wgpu::Sampler,
    globals_buffer: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
    intermediate_textures: Vec<wgpu::Texture>,
    started_at: Instant,
    last_frame_at: Instant,
    frame: u32,
}

#[cfg(feature = "wgpu")]
impl ShaderOverlayBrush {
    pub fn load(
        ctx: &WgpuContext,
        config: ShaderOverlayConfig,
    ) -> Result<Option<Self>, ShaderOverlayError> {
        if config.is_empty() {
            return Ok(None);
        }

        let sampler = ctx.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Shader Overlay Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let globals_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shader Overlay Globals"),
            size: std::mem::size_of::<GlobalsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group_layout =
            ctx.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("Shader Overlay Bind Group Layout"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float {
                                    filterable: true,
                                },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 2,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(
                                wgpu::SamplerBindingType::Filtering,
                            ),
                            count: None,
                        },
                    ],
                });

        let pipeline_layout =
            ctx.device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("Shader Overlay Pipeline Layout"),
                    bind_group_layouts: &[&bind_group_layout],
                    immediate_size: 0,
                });

        let vertex_module =
            ctx.device
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("Shader Overlay Fullscreen Vertex"),
                    source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(FULLSCREEN_VERTEX)),
                });

        let mut passes = Vec::with_capacity(config.shaders.len());
        for path in config.shaders {
            passes.push(ShaderOverlayPass::load(
                ctx,
                &pipeline_layout,
                &vertex_module,
                &path,
            )?);
        }

        let mut brush = Self {
            passes,
            sampler,
            globals_buffer,
            bind_group_layout,
            intermediate_textures: Vec::new(),
            started_at: Instant::now(),
            last_frame_at: Instant::now(),
            frame: 0,
        };
        brush.resize_intermediates(ctx);
        Ok(Some(brush))
    }

    pub fn render(
        &mut self,
        ctx: &WgpuContext,
        encoder: &mut wgpu::CommandEncoder,
        src_texture: &wgpu::Texture,
        dst_texture: &wgpu::Texture,
    ) {
        if self.passes.is_empty() {
            return;
        }

        self.resize_intermediates(ctx);
        self.frame = self.frame.wrapping_add(1);
        let globals = GlobalsUniform::new(
            ctx.size.width as f32,
            ctx.size.height as f32,
            self.frame,
            self.started_at,
            &mut self.last_frame_at,
        );
        ctx.queue
            .write_buffer(&self.globals_buffer, 0, bytemuck::bytes_of(&globals));

        let mut src_view =
            src_texture.create_view(&wgpu::TextureViewDescriptor::default());
        for index in 0..self.passes.len() {
            let is_last = index + 1 == self.passes.len();
            let dst_view = if is_last {
                dst_texture.create_view(&wgpu::TextureViewDescriptor::default())
            } else {
                self.intermediate_textures[index]
                    .create_view(&wgpu::TextureViewDescriptor::default())
            };

            let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Shader Overlay Bind Group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.globals_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&src_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });

            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Shader Overlay Pass"),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &dst_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.passes[index].pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.draw(0..3, 0..1);
            }

            src_view = dst_view;
        }
    }

    fn resize_intermediates(&mut self, ctx: &WgpuContext) {
        let needed = self.passes.len().saturating_sub(1);
        let size = wgpu::Extent3d {
            width: ctx.size.width.max(1.0) as u32,
            height: ctx.size.height.max(1.0) as u32,
            depth_or_array_layers: 1,
        };

        let valid = self.intermediate_textures.len() == needed
            && self
                .intermediate_textures
                .iter()
                .all(|texture| texture.size() == size && texture.format() == ctx.format);
        if valid {
            return;
        }

        self.intermediate_textures = (0..needed)
            .map(|index| {
                ctx.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(&format!("Shader Overlay Intermediate {index}")),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: ctx.format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[ctx.format],
                })
            })
            .collect();
    }
}

#[cfg(feature = "wgpu")]
struct ShaderOverlayPass {
    pipeline: wgpu::RenderPipeline,
}

#[cfg(feature = "wgpu")]
impl ShaderOverlayPass {
    fn load(
        ctx: &WgpuContext,
        pipeline_layout: &wgpu::PipelineLayout,
        vertex_module: &wgpu::ShaderModule,
        path: &Path,
    ) -> Result<Self, ShaderOverlayError> {
        let source = shader_source(path)?;
        let wgsl = compile_shadertoy_glsl(path, &source)?;
        let fragment_module =
            ctx.device
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(&format!("Shader Overlay Fragment {}", path.display())),
                    source: wgpu::ShaderSource::Wgsl(Cow::Owned(wgsl)),
                });

        let pipeline =
            ctx.device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(&format!("Shader Overlay Pipeline {}", path.display())),
                    layout: Some(pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: vertex_module,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &fragment_module,
                        entry_point: Some("main"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: ctx.format,
                            blend: Some(wgpu::BlendState::REPLACE),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        strip_index_format: None,
                        front_face: wgpu::FrontFace::Ccw,
                        cull_mode: None,
                        polygon_mode: wgpu::PolygonMode::Fill,
                        unclipped_depth: false,
                        conservative: false,
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                });

        Ok(Self { pipeline })
    }
}

pub(crate) fn shader_source(
    path: &Path,
) -> Result<Cow<'static, str>, ShaderOverlayError> {
    match path.to_str() {
        Some(BUILTIN_CTV_ROUND) => Ok(Cow::Borrowed(include_str!(
            "../../examples/shaders/ctv_round.glsl"
        ))),
        Some(BUILTIN_HYPNO_CRT) => Ok(Cow::Borrowed(include_str!(
            "../../examples/shaders/hypno_crt.glsl"
        ))),
        _ => std::fs::read_to_string(path)
            .map(Cow::Owned)
            .map_err(|source| ShaderOverlayError::Read {
                path: path.to_path_buf(),
                source,
            }),
    }
}

pub(crate) fn shader_overlay_glsl_source(source: &str) -> String {
    let source = strip_version(source);
    format!("{SHADERTOY_PREFIX}\n#line 1\n{source}\n{SHADERTOY_SUFFIX}")
}

#[cfg(feature = "wgpu")]
fn compile_shadertoy_glsl(
    path: &Path,
    source: &str,
) -> Result<String, ShaderOverlayError> {
    let source = shader_overlay_glsl_source(source);
    let mut frontend = naga::front::glsl::Frontend::default();
    let options = naga::front::glsl::Options::from(naga::ShaderStage::Fragment);
    let module =
        frontend
            .parse(&options, &source)
            .map_err(|err| ShaderOverlayError::Parse {
                path: path.to_path_buf(),
                message: err.to_string(),
            })?;

    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    );
    let info =
        validator
            .validate(&module)
            .map_err(|err| ShaderOverlayError::Validate {
                path: path.to_path_buf(),
                message: err.to_string(),
            })?;

    let wgsl = naga::back::wgsl::write_string(
        &module,
        &info,
        naga::back::wgsl::WriterFlags::empty(),
    )
    .map_err(|err| ShaderOverlayError::WriteWgsl {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;

    let wgsl_module = naga::front::wgsl::parse_str(&wgsl).map_err(|err| {
        ShaderOverlayError::ValidateWgsl {
            path: path.to_path_buf(),
            message: err.to_string(),
        }
    })?;
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    );
    validator
        .validate(&wgsl_module)
        .map_err(|err| ShaderOverlayError::ValidateWgsl {
            path: path.to_path_buf(),
            message: err.to_string(),
        })?;

    Ok(wgsl)
}

#[cfg(target_os = "macos")]
pub(crate) fn compile_shadertoy_msl(
    path: &Path,
    source: &str,
) -> Result<(String, String), ShaderOverlayError> {
    use naga::back::msl::{BindSamplerTarget, BindTarget, EntryPointResources};

    let source = shader_overlay_glsl_source(source);
    let mut frontend = naga::front::glsl::Frontend::default();
    let options = naga::front::glsl::Options::from(naga::ShaderStage::Fragment);
    let module =
        frontend
            .parse(&options, &source)
            .map_err(|err| ShaderOverlayError::Parse {
                path: path.to_path_buf(),
                message: err.to_string(),
            })?;
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    );
    let info =
        validator
            .validate(&module)
            .map_err(|err| ShaderOverlayError::Validate {
                path: path.to_path_buf(),
                message: err.to_string(),
            })?;

    let mut resources = EntryPointResources::default();
    resources.resources.insert(
        naga::ResourceBinding {
            group: 0,
            binding: 0,
        },
        BindTarget {
            buffer: Some(0),
            ..Default::default()
        },
    );
    resources.resources.insert(
        naga::ResourceBinding {
            group: 0,
            binding: 1,
        },
        BindTarget {
            texture: Some(0),
            ..Default::default()
        },
    );
    resources.resources.insert(
        naga::ResourceBinding {
            group: 0,
            binding: 2,
        },
        BindTarget {
            sampler: Some(BindSamplerTarget::Resource(0)),
            ..Default::default()
        },
    );
    let mut msl_options = naga::back::msl::Options::default();
    msl_options.lang_version = (2, 1);
    msl_options.fake_missing_bindings = false;
    msl_options
        .per_entry_point_map
        .insert("main".to_string(), resources);
    let pipeline_options = naga::back::msl::PipelineOptions {
        entry_point: None,
        allow_and_force_point_size: false,
        vertex_pulling_transform: false,
        vertex_buffer_mappings: Vec::new(),
    };
    let (msl, translation) =
        naga::back::msl::write_string(&module, &info, &msl_options, &pipeline_options)
            .map_err(|err| ShaderOverlayError::WriteMsl {
                path: path.to_path_buf(),
                message: err.to_string(),
            })?;
    let entry_point = translation
        .entry_point_names
        .into_iter()
        .next()
        .ok_or_else(|| ShaderOverlayError::WriteMsl {
            path: path.to_path_buf(),
            message: "shader has no entry point".to_string(),
        })?
        .map_err(|err| ShaderOverlayError::WriteMsl {
            path: path.to_path_buf(),
            message: err.to_string(),
        })?;
    Ok((msl, entry_point))
}

fn strip_version(source: &str) -> Cow<'_, str> {
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        }

        if trimmed.starts_with("#version") {
            let mut stripped = String::with_capacity(source.len());
            for line in source.lines() {
                if !line.trim_start().starts_with("#version") {
                    stripped.push_str(line);
                    stripped.push('\n');
                }
            }
            return Cow::Owned(stripped);
        }

        break;
    }

    Cow::Borrowed(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_bundled_shaders() {
        for (path, source) in [
            (
                "hypno_crt.glsl",
                include_str!("../../examples/shaders/hypno_crt.glsl"),
            ),
            (
                "ctv_round.glsl",
                include_str!("../../examples/shaders/ctv_round.glsl"),
            ),
        ] {
            let wgsl = compile_shadertoy_glsl(Path::new(path), source)
                .expect("bundled shader should compile");

            assert!(wgsl.contains("@fragment"));
            assert!(wgsl.contains("fn main"));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn translates_bundled_shaders_to_msl() {
        for (path, source) in [
            (
                "hypno_crt.glsl",
                include_str!("../../examples/shaders/hypno_crt.glsl"),
            ),
            (
                "ctv_round.glsl",
                include_str!("../../examples/shaders/ctv_round.glsl"),
            ),
        ] {
            let (msl, entry_point) = compile_shadertoy_msl(Path::new(path), source)
                .expect("bundled shader should translate to MSL");

            assert!(msl.contains("fragment"));
            assert!(!entry_point.is_empty());
        }
    }
}
