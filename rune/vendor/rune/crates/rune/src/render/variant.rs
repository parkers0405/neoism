//! Which uber shader a renderer draws with, as a state machine ([`Uber`]).
//!
//! `Classic` is shader.wgsl alone, the shader every renderer drew before glyphs could draw from
//! their outlines: its WGSL, layout and precompiled libraries are unchanged, so a Mac that keeps
//! the default text path draws exactly what it drew before. `Windfoil` is shader.wgsl with
//! outline.wgsl, whose glyphs can also draw from the outline store (binding 4). Renderers start
//! with Windfoil except on macOS (and with GPU springs, which keep Classic); a Classic renderer
//! builds Windfoil the first time text asks for outlines. A Windfoil that can't be built leaves
//! the renderer Classic for good (`Unavailable`), and text falls back to its other rasterisers.

use super::*;
use crate::machine::Effects;

const OUTLINE_WGSL: &str = include_str!("../outline.wgsl");
/// shader.wgsl's atlas glyph branch, which Windfoil puts the outline branch before.
const GLYPH_BRANCH: &str = "        if i.uv.z > 0.0 || rotated {";
const OUTLINE_BRANCH: &str = "        if i.radii.w > 0.0 {\n            cov = outline_glyph(i, screen);\n        } else if i.uv.z > 0.0 || rotated {";

/// Windfoil's WGSL, or None if shader.wgsl's glyph branch no longer reads as `GLYPH_BRANCH`.
pub(crate) fn windfoil_wgsl() -> Option<String> {
    let classic = metallib::uber_wgsl();
    (classic.matches(GLYPH_BRANCH).count() == 1).then(|| format!("{}\n{OUTLINE_WGSL}", classic.replacen(GLYPH_BRANCH, OUTLINE_BRANCH, 1)))
}

/// Classic's bindings and the outline store's.
pub(crate) const fn windfoil_bind_entries() -> [wgpu::BindGroupLayoutEntry; 5] {
    let [a, b, c, d] = metallib::uber_bind_entries();
    let outlines = wgpu::BindingType::Texture {
        sample_type: wgpu::TextureSampleType::Uint,
        view_dimension: wgpu::TextureViewDimension::D2,
        multisampled: false,
    };
    [a, b, c, d, wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::FRAGMENT, ty: outlines, count: None }]
}

/// Windfoil's pipeline ABI, as `metallib::uber_abi_hash` hashes Classic's.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn windfoil_abi_hash() -> u64 {
    metallib::abi_hash_of(&windfoil_bind_entries())
}

/// Where a Windfoil shader came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// The bytecode Rune ships for it (DXBC on Windows, a metallib on macOS).
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    Precompiled,
    /// Its WGSL, compiled at run time.
    Compiled,
}

pub(super) enum Uber {
    /// shader.wgsl alone, and no outline store.
    Classic,
    /// shader.wgsl and outline.wgsl, with the outline store's texture and the atlas fill pass.
    Windfoil { source: Source, outlines: wgpu::Texture, fill: fill::FillPass },
    /// Windfoil was asked for and couldn't be built: Classic from now on.
    Unavailable,
}

pub(super) enum Event {
    /// Text drew glyphs from outlines.
    OutlinesWanted,
    /// Windfoil's pipeline was built.
    Built { source: Source, outlines: wgpu::Texture, fill: fill::FillPass },
    /// Windfoil's pipeline couldn't be built, and why.
    Refused(String),
}

#[derive(Debug, PartialEq)]
pub(super) enum Effect {
    /// Build Windfoil's pipeline, and send `Built` or `Refused`.
    Build,
    /// Say why text falls back from outlines.
    Fallback(String),
}

impl Uber {
    pub(super) fn step(&mut self, event: Event) -> Effects<Effect> {
        match (&*self, event) {
            (Uber::Classic, Event::OutlinesWanted) => Effects::one(Effect::Build),
            (Uber::Classic, Event::Built { source, outlines, fill }) => {
                *self = Uber::Windfoil { source, outlines, fill };
                Effects::none()
            }
            (Uber::Classic, Event::Refused(why)) => {
                *self = Uber::Unavailable;
                Effects::one(Effect::Fallback(why))
            }
            _ => Effects::none(),
        }
    }

    /// The outline store's texture, which Windfoil's bind groups hold.
    pub(super) fn outlines(&self) -> Option<&wgpu::Texture> {
        match self {
            Uber::Windfoil { outlines, .. } => Some(outlines),
            Uber::Classic | Uber::Unavailable => None,
        }
    }

    /// What text can expect of outlines from this renderer.
    pub(super) fn support(&self) -> crate::text::outline::Support {
        match self {
            Uber::Classic => crate::text::outline::Support::Untried,
            Uber::Windfoil { .. } => crate::text::outline::Support::Drawn,
            Uber::Unavailable => crate::text::outline::Support::Unavailable,
        }
    }
}

/// What building Windfoil needs from its renderer.
pub(super) struct BuildContext<'a> {
    pub format: wgpu::TextureFormat,
    pub vertex_entry: &'static str,
    pub globals: &'a wgpu::Buffer,
    pub stand_in: &'a wgpu::Texture,
}

/// A built Windfoil: its layouts, shader and frame pipeline.
pub(super) struct Built {
    pub bind_layout: wgpu::BindGroupLayout,
    pub layout: wgpu::PipelineLayout,
    pub shader: dxbc::Shaders,
    pub pipeline: wgpu::RenderPipeline,
    pub source: Source,
}

impl Renderer {
    /// Which uber shader draws, for diagnostics: `classic`, `windfoil (precompiled)`,
    /// `windfoil (compiled)` or `unavailable` (Windfoil couldn't be built).
    #[doc(hidden)]
    pub fn uber_shader(&self) -> &'static str {
        match self.uber {
            Uber::Classic => "classic",
            Uber::Windfoil { source: Source::Precompiled, .. } => "windfoil (precompiled)",
            Uber::Windfoil { source: Source::Compiled, .. } => "windfoil (compiled)",
            Uber::Unavailable => "unavailable",
        }
    }

    /// Tells `atlas` what this renderer does with outlines (`text::outline::Support`) before it
    /// first draws, so the first frame's glyph masks are queued for the GPU fill rather than
    /// filled on the CPU. `prepare` tells it again every frame.
    pub fn attach_text(&self, atlas: &mut Atlas) {
        atlas.outlines.set_support(self.uber.support());
    }

    /// Steps the shader machine `state` on `event`, carrying out each effect (building Windfoil
    /// feeds `Built` or `Refused` back in) until none is left; with the Windfoil pipeline it
    /// built on the way, if it built one.
    fn run(device: &wgpu::Device, cx: &BuildContext<'_>, state: &mut Uber, event: Event) -> Option<Built> {
        let mut built = None;
        let mut queue: Vec<Effect> = state.step(event).into_iter().collect();
        while let Some(effect) = queue.pop() {
            match effect {
                Effect::Build => {
                    let next = match Self::build_windfoil(device, cx.format, cx.vertex_entry) {
                        Ok(b) => {
                            let outlines = Self::outline_texture(device, outlines::MIN_ROWS);
                            let fill = fill::FillPass::new(device, &b.layout, &b.shader, fill::FillBindings {
                                layout: &b.bind_layout, globals: cx.globals, outlines: &outlines, stand_in: cx.stand_in });
                            let event = Event::Built { source: b.source, outlines, fill };
                            built = Some(b);
                            event
                        }
                        Err(why) => Event::Refused(why),
                    };
                    queue.extend(state.step(next));
                }
                Effect::Fallback(why) => eprintln!("rune: glyphs can't draw from outlines here ({why}); text draws from the atlas alone"),
            }
        }
        built
    }
    /// A new renderer's shader machine, asked for Windfoil at once.
    pub(super) fn start_windfoil(device: &wgpu::Device, cx: &BuildContext<'_>) -> (Uber, Option<Built>) {
        let mut state = Uber::Classic;
        let built = Self::run(device, cx, &mut state, Event::OutlinesWanted);
        (state, built)
    }

    /// Steps this renderer's shader machine on `event`, taking any Windfoil pipeline it builds;
    /// whether it took one (the caller then makes its atlas pages ones the GPU can fill).
    pub(super) fn select(&mut self, device: &wgpu::Device, event: Event) -> bool {
        let cx = BuildContext { format: self.format, vertex_entry: self.vertex_entry, globals: &self.globals, stand_in: &self.stand_in };
        if let Some(built) = Self::run(device, &cx, &mut self.uber, event) {
            self.adopt(device, built);
            self.rebind_all(device);
            return true;
        }
        false
    }

    /// Windfoil's layouts, shader and pipeline for `format`, from the shipped bytecode where it
    /// matches, else from WGSL; an error where neither builds.
    pub(super) fn build_windfoil(device: &wgpu::Device, format: wgpu::TextureFormat, vertex_entry: &'static str) -> Result<Built, String> {
        if vertex_entry != "vs" {
            return Err("GPU springs draw the classic shader".into());
        }
        let wgsl = windfoil_wgsl().ok_or("shader.wgsl's glyph branch moved, so outline.wgsl can't join it")?;
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &windfoil_bind_entries() });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&bind_layout)], immediate_size: 0 });
        let make_pipeline = |shader: &dxbc::Shaders| Self::pipeline(device, &layout, shader, format, vertex_entry, Blend::SourceOver);
        #[cfg(target_os = "windows")]
        if let Some((shader, pipeline)) = dxbc::uber(device, &wgsl, windfoil_abi_hash(), make_pipeline) {
            return Ok(Built { bind_layout, layout, shader, pipeline, source: Source::Precompiled });
        }
        #[cfg(target_os = "macos")]
        if let Some((shader, pipeline)) = windfoil_metallib::load(device, &wgsl, windfoil_abi_hash(), make_pipeline) {
            return Ok(Built { bind_layout, layout, shader: dxbc::Shaders::one(shader), pipeline, source: Source::Precompiled });
        }
        let (shader, pipeline) = compile(device, &wgsl, make_pipeline)?;
        Ok(Built { bind_layout, layout, shader, pipeline, source: Source::Compiled })
    }

    /// Takes `built` as the renderer's shader: every pipeline again from it. Bind groups follow
    /// once the outline texture exists (`rebind_all`).
    fn adopt(&mut self, device: &wgpu::Device, built: Built) {
        self.bind_layout = built.bind_layout;
        self.layout = built.layout;
        self.shader = built.shader;
        self.pipeline = built.pipeline;
        for index in 0..self.extra.len() {
            let format = self.extra[index].0;
            self.extra[index].1 = Self::pipeline(device, &self.layout, &self.shader, format, self.vertex_entry, Blend::SourceOver);
        }
        self.rebuild_layer_pipelines(device);
        // Built on the old bind layout; made again at the next backdrop blur.
        self.backdrop_blur = None;
    }
}

/// `wgsl`'s module and pipeline, compiled now, or the error the device reported.
fn compile(device: &wgpu::Device, wgsl: &str, make_pipeline: impl FnOnce(&dxbc::Shaders) -> wgpu::RenderPipeline) -> Result<(dxbc::Shaders, wgpu::RenderPipeline), String> {
    #[cfg(not(target_arch = "wasm32"))]
    let scopes = (device.push_error_scope(wgpu::ErrorFilter::Validation), device.push_error_scope(wgpu::ErrorFilter::Internal));
    let shader = dxbc::Shaders::one(device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("uber"), source: wgpu::ShaderSource::Wgsl(wgsl.into()) }));
    let pipeline = make_pipeline(&shader);
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(error) = pollster::block_on(scopes.1.pop()).or(pollster::block_on(scopes.0.pop())) {
        return Err(error.to_string());
    }
    Ok((shader, pipeline))
}
