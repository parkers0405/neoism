//! The instanced wgpu renderer, with lazy bounded group layers and offscreen readback.

use bytemuck::{Pod, Zeroable};

use crate::pixels::ImageAtlas;
use crate::text::{ATLAS_SIZE, Atlas};

mod backdrop_blur;
mod dxbc;
mod fill;
mod first_use;
pub(crate) mod gpu_events;
pub(crate) mod layer_blur;
mod layers;
mod metallib;
mod outlines;
mod pass;
use pass::{ActivePass, Backdrop};
pub(crate) mod surface_alpha;
pub(crate) mod translucency;
mod upload;
mod variant;
#[cfg(target_os = "macos")]
mod windfoil_metallib;
pub use backdrop_blur::MAX_BACKDROP_BLURS;
pub use first_use::precompiled_misses;
pub use layers::LayerStats;

/// Rounded rect in `fill` with a `params.x`-wide `border` ring drawn over it, as CSS paints a
/// border over its background. `params.w` is the opacity the colours already carry (1 when not
/// faded), so the ring hides the fill by `border.a / params.w`. Non-zero `uv` (with `uv.z >= 0`)
/// gives per-side widths `[top, right, bottom, left]`; `params.x` is then the widest.
pub const KIND_RECT: f32 = 0.0;
pub const KIND_SHADOW: f32 = 1.0;
pub const KIND_GLYPH: f32 = 2.0;
/// Inset shadow inside a rounded rect: `params.x` spread, `params.z` sigma, `uv.xy` offset.
pub const KIND_INSET: f32 = 3.0;
/// Rounded rect with a vertical gradient: `fill` at the top, `uv` (premultiplied) at the bottom.
/// With a border width in `params.x` it is a `KIND_RECT` whose ring runs from `border` to `uv`,
/// over the fill, with the same `params.w`.
pub const KIND_GRADIENT: f32 = 4.0;
/// A stroke segment: `radii` = x0, y0, x1, y1; `params.x` half width, `params.z` 1 for round caps.
pub const KIND_SEGMENT: f32 = 5.0;
/// A filled triangle: `radii` = ax, ay, bx, by; `uv.xy` = cx, cy; `params.x` the antialiased-edge
/// mask (a→b, b→c, c→a as bits 0, 1, 2; other edges are hard so neighbours meet without seams).
pub const KIND_TRIANGLE: f32 = 6.0;
/// An RGBA image (`El::pixels`): `uv.xy` its origin in the image atlas, `uv.zw` its size in
/// texels, drawn scaled into `rect` with the element's corner `radii`; `fill.a` is the opacity.
pub const KIND_IMAGE: f32 = 7.0;
/// Balanced scope records in the painter stream. They occupy instance indices but are never
/// rasterized as ordinary quads. Keep scopes intact when partitioning compositor lists.
pub const KIND_LAYER: f32 = 8.0;
pub const KIND_LAYER_END: f32 = 9.0;
/// A dashed border ring (`El::border_dash`), `params.x` wide inside the rounded rect, drawn
/// alone (the fill is its own instance, showing through the gaps). Its colour runs from `fill`
/// at the top to `border` at the bottom; `uv.xyz` = dash, gap, phase in device pixels.
pub const KIND_DASHED: f32 = 10.0;
/// A radial (`uv.z > 0`: `uv` = `[cx, cy, rx, ry]` in rect-local device pixels) or angled linear
/// (`uv.z == 0`: `uv.xy` = gradient vector per device pixel) stop segment from `fill` at `t0`
/// (`params.x`) to `border` at `t1` (`params.w`), with `+ 2.0` marking an open interior stop edge.
pub const KIND_RADIAL: f32 = 11.0;
/// An effect slot (`El::effect`). It rasterizes no pixels but keeps its place in the painter
/// stream, so `Renderer::draw_range_with` or `Renderer::draw_with` can hand the compositor the pass
/// at exactly this point, inside the enclosing isolated layer if there is one. `rect` is the
/// element, `clip`/`fade` (and the rounded-clip radius in `fade[3]`) its clip, `params.x` `1.0`
/// when the slot requests a backdrop copy (`El::effect_backdrop(true)`), `params.z` the bleed past
/// `rect` in device pixels and `params.w` the slot's scale in device pixels per point (`Env::scale`
/// times any element or ancestor `scale` / `live_scale`). `fill[3]` is always 1: a faded slot is
/// drawn in an isolated layer that applies its opacity. `border[0]` and `border[1]` hold the low
/// and high 16 bits of its `El::effect` tag as exact floats (`EffectSite::mark`), and `uv` its
/// `El::effect_data` payload, bit for bit (`EffectSite::data`).
pub const KIND_EFFECT: f32 = 12.0;
/// A backdrop blur (`El::backdrop_blur`). Like an effect slot it rasterizes nothing itself in
/// the uber shader; the renderer stops there, copies and blurs what is below, and draws the blur
/// inside `rect` with signed corner `radii`, cut by `clip`/`fade`, at opacity `fill[3]`.
/// `params.x` is the blur's standard deviation in device pixels and `params.w` the scale in
/// device pixels per point.
pub const KIND_BACKDROP_BLUR: f32 = 13.0;

/// How a scope combines its contents. The completed scope still uses source-over on its parent
/// unless that parent also requests additive composition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Blend {
    #[default]
    SourceOver,
    /// Premultiplied One/One addition, clamped when the isolated scope is composited.
    PlusLighter,
}

impl Blend {
    pub(crate) fn state(self) -> wgpu::BlendState {
        match self {
            Self::SourceOver => wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
            Self::PlusLighter => {
                let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
                wgpu::BlendState { color: add, alpha: add }
            }
        }
    }
}

/// Composition properties of an isolated subtree. Filters can extend this contract without
/// changing painter-stream scopes or the target pool.
#[derive(Clone, Copy, Debug)]
pub struct LayerSpec {
    pub opacity: f32,
    pub blend: Blend,
    /// Gaussian standard deviation in device pixels, clamped to 64. Zero bypasses filtering.
    pub blur: f32,
    /// Radius of the rounded `clip` rectangle, in device pixels; zero is rectangular and a
    /// negative radius rounds with continuous corners (`El::continuous_corners`). Applied to
    /// the completed layer after filtering. The caller supplies finite clip edges.
    pub clip_radius: f32,
}

impl Default for LayerSpec {
    fn default() -> Self { Self { opacity: 1.0, blend: Blend::SourceOver, blur: 0.0, clip_radius: 0.0 } }
}

/// WGSL for clipping like Rune does: `sd_rounded`, `ramp` and `clip_alpha(p, clip, fade)`. `p` is
/// a window device-pixel position, `clip` an instance's `clip` and `fade` (`vec4<u32>`) its packed
/// soft edges and rounded-clip radius, so a compositor shader cuts its effect with the same
/// rectangle, feathers and circular or continuous corners as the primitives around it. Prepend it
/// to the shader source.
pub const CLIP_WGSL: &str = include_str!("clip.wgsl");

/// The uber shader's WGSL and the hash of its pipeline layout, as the precompiled DX12 bytecode
/// records them; `examples/regen_dxbc.rs` regenerates that bytecode from them.
#[cfg(target_os = "windows")]
#[doc(hidden)]
pub fn uber_shader_identity() -> (String, u64) {
    (variant::windfoil_wgsl().unwrap_or_else(metallib::uber_wgsl), variant::windfoil_abi_hash())
}

/// Where a compositor is asked to draw an effect slot (`Renderer::draw_range_with`).
pub struct EffectSite<'a> {
    /// The slot's position in the painter stream.
    pub index: u32,
    /// The slot's record (`KIND_EFFECT`): rect, radii, clip and fade.
    pub instance: &'a Instance,
    /// The slot's `El::effect` tag, so a compositor can find the resources it keeps for it.
    pub mark: u32,
    /// The slot's `El::effect_data` payload, as the element was last built. A slot playing its
    /// exit transition keeps the payload it had when it was removed.
    pub data: [f32; 4],
    /// Device pixels per point at this slot (`instance.params[3]`: `Env::scale` times any
    /// element or ancestor `scale` / `live_scale`).
    pub scale: f32,
    /// Format of the pass being recorded.
    pub format: wgpu::TextureFormat,
    /// Window device-pixel position of this pass's top-left pixel: zero in the frame target,
    /// the layer's bounds origin inside an isolated layer. Subtract it from window coordinates.
    pub origin: [f32; 2],
    /// Size of this pass's target in device pixels.
    pub size: [u32; 2],
    /// True inside an isolated layer: the pass is that layer's offscreen target.
    pub in_layer: bool,
    /// Whether colours are sRGB-encoded for blending (`Renderer::srgb_blend`). Choose a
    /// pipeline's encoding from this: encode the shader's output when it's true, write linear
    /// values when it's false. Don't use `format.is_srgb()`: an isolated layer's `Rgba16Float`
    /// target is neither, and holds whichever encoding the frame uses.
    pub srgb_blend: bool,
    /// Viewport-sized copy of the pass's target up to this slot when the slot asked for a
    /// backdrop (`El::effect_backdrop(true)`, `instance.params[0] != 0.0`) and the pass was run
    /// with a command encoder (`Renderer::draw_with`, `Renderer::draw_with_backdrop`,
    /// `Renderer::draw_target_range_with`, or `Renderer::encode_layers_with`). Has the same
    /// `format` as the pass (`*Srgb` / `Rgba16Float` in linear frames, non-sRGB / `Rgba16Float`
    /// holding sRGB-encoded premultiplied values when `srgb_blend` is true; pass it directly to
    /// `FxRenderer::blur_backdrop` or `FxRenderer::set_layer_backdrop`). Inside an isolated layer
    /// (`in_layer: true`), holds that layer's offscreen target copied at `origin` in window
    /// coordinates.
    pub backdrop: Option<&'a wgpu::TextureView>,
}

/// Called at each effect slot, with the pass open and everything painted before the slot already
/// recorded, and the scissor set to the slot's clip rectangle (a slot clipped away entirely is
/// skipped). Soft edges and rounded corners on a slot outside a layer are the hook's to apply
/// (`CLIP_WGSL`). It may record draws; the viewport and scissor are reset to the full target after.
pub type EffectHook<'h> = dyn FnMut(&mut wgpu::RenderPass<'_>, &EffectSite<'_>) + 'h;

/// Phase of an effect slot callback in `Renderer::draw_with_backdrop`,
/// `Renderer::encode_layers_with_backdrop`, and `Renderer::draw_target_range_with`.
pub enum EffectPhase<'a, 'p> {
    /// Outside the render pass, right after the target up to a backdrop slot
    /// (`El::effect_backdrop(true)`) was copied into `site.backdrop` and before the pass re-opens
    /// with `LoadOp::Load` (e.g. for `FxRenderer::blur_backdrop`).
    Backdrop(&'a mut wgpu::CommandEncoder),
    /// Inside the open render pass at the slot's painter position, with the scissor set to the
    /// slot's clip rectangle.
    Draw(&'a mut wgpu::RenderPass<'p>),
}

/// Called at each effect slot: first with `EffectPhase::Backdrop(encoder)` while the pass is
/// closed if the slot requested a backdrop (`El::effect_backdrop(true)`), then with
/// `EffectPhase::Draw(pass)` inside the open pass.
pub type EffectBackdropHook<'h> = dyn FnMut(EffectPhase<'_, '_>, &EffectSite<'_>) + 'h;

/// One instance, in device pixels.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Instance {
    pub rect: [f32; 4],
    pub radii: [f32; 4],
    pub fill: [f32; 4],
    pub border: [f32; 4],
    /// border width, kind, shadow sigma, glyph coverage exponent
    pub params: [f32; 4],
    pub uv: [f32; 4],
    /// x0, y0, x1, y1
    pub clip: [f32; 4],
    /// Attribute 7 is Uint32x4; the instance ABI stays 128 bytes (8 × vec4).
    /// [0] = pack2x16float(left, top), [1] = pack2x16float(right, bottom) widths; a negative
    /// width ramps linearly, a positive one on smoothstep.
    /// [2] = pack4x8unorm(clamp(inward origin→clip distance / width, 0, 1)), LTRB.
    /// [3] = signed uniform f32 clip-radius bits or tagged mixed radii (`pack_clip_radii`).
    /// The rounded rectangle is `clip`; fade widths never occupy this word.
    /// Layer records may leave all four words zero. See `pack_fade`.
    pub fade: [u32; 4],
}

const _: () = assert!(std::mem::size_of::<Instance>() == 128);

pub fn pack_fade(width: [f32; 4], origin: [f32; 4], clip: [f32; 4]) -> [u32; 4] {
    // A negative width is a linear ramp of that width (`El::clip_fade_linear`).
    let halves = width.map(|w| half::f16::from_f32(w.clamp(-65504.0, 65504.0)).to_bits() as u32);
    let mut offsets = 0;
    for edge in 0..4 {
        let distance = if edge < 2 { clip[edge] - origin[edge] } else { origin[edge] - clip[edge] };
        let w = width[edge].abs();
        let offset = if w > 0.0 { (distance / w).clamp(0.0, 1.0) } else { 0.0 };
        offsets |= ((offset * 255.0 + 0.5) as u32) << (edge * 8);
    }
    [halves[0] | (halves[1] << 16), halves[2] | (halves[3] << 16), offsets, 0]
}

/// Decode widths and normalized origin offsets for tools inspecting the instance stream. A
/// negative width is a linear ramp.
pub fn unpack_fade(fade: [u32; 4]) -> ([f32; 4], [f32; 4]) {
    (std::array::from_fn(|e| half::f16::from_bits((fade[e / 2] >> ((e % 2) * 16)) as u16).to_f32()),
     std::array::from_fn(|e| ((fade[2] >> (e * 8)) & 255) as f32 / 255.0))
}

pub const NO_CLIP: [f32; 4] = [-1e6, -1e6, 1e6, 1e6];

/// Pack per-corner signed clip radii `[tl, tr, br, bl]` (negative = continuous corners)
/// into `Instance::fade[3]`, clamping each corner to half the shorter `clip` side.
/// Uniform corners stay an exact IEEE-754 `f32`; mixed corners use the `0x6000_0000`
/// high-exponent tag (`|r| >= 2^65` in IEEE-754) with 4 × 7-bit magnitudes.
/// Mixed radii round to 0.5 px through 63.5 px, otherwise 2 px, saturating at 254 px.
/// All nonzero radii must have the same sign (one curve style per clip). Uniform radii
/// saturate just below the tag's exponent range, far beyond device-pixel coordinates.
pub fn pack_clip_radii(clip: [f32; 4], radii: [f32; 4]) -> u32 {
    let half = ((clip[2] - clip[0]).min(clip[3] - clip[1]) * 0.5).max(0.0);
    let clamped = radii.map(|radius| {
        let r = radius.abs().min(half).min(f32::from_bits(0x5fff_ffff));
        if radius < 0.0 && r > 0.0 { -r } else { r }
    });
    if clamped[0] == clamped[1] && clamped[1] == clamped[2] && clamped[2] == clamped[3] {
        return if clamped[0] == 0.0 { 0 } else { clamped[0].to_bits() };
    }
    let continuous = clamped.iter().any(|&r| r < 0.0);
    let max_r = clamped.iter().fold(0.0f32, |m, r| m.max(r.abs()));
    let coarse = max_r > 63.5;
    let inv_step = if coarse { 0.5 } else { 2.0 };
    let mut bits = 0x6000_0000u32
        | if continuous { 0x8000_0000 } else { 0 }
        | if coarse { 0x1000_0000 } else { 0 };
    for (idx, r) in clamped.into_iter().enumerate() {
        let q = (r.abs() * inv_step).round().clamp(0.0, 127.0) as u32;
        bits |= q << (idx * 7);
    }
    bits
}

/// Decode `Instance::fade[3]` into four signed corner radii `[tl, tr, br, bl]`.
pub fn unpack_clip_radii(w: u32) -> [f32; 4] {
    if w == 0 {
        return [0.0; 4];
    }
    if (w & 0x6000_0000) != 0x6000_0000 {
        return [f32::from_bits(w); 4];
    }
    let step = if (w & 0x1000_0000) != 0 { 2.0 } else { 0.5 };
    let sign = if (w & 0x8000_0000) != 0 { -1.0 } else { 1.0 };
    std::array::from_fn(|idx| {
        let q = (w >> (idx * 7)) & 0x7f;
        if q == 0 { 0.0 } else { q as f32 * step * sign }
    })
}

impl Instance {
    /// Rounded clipping shares the packed fade ABI across primitives and layer composites.
    /// A negative radius keeps its sign: continuous corners (`corner_extent`).
    pub(crate) fn round_clip(&mut self, radius: f32) {
        self.round_clip_corners([radius; 4]);
    }

    pub(crate) fn round_clip_corners(&mut self, radii: [f32; 4]) {
        self.fade[3] = pack_clip_radii(self.clip, radii);
    }

    /// Begin an isolated scope. `bounds` is a device-pixel [x,y,w,h] content bound, including
    /// native surfaces. Primitive bounds (including shadow support) expand it during prepare.
    pub fn begin_layer(bounds: [f32; 4], clip: [f32; 4], spec: LayerSpec) -> Self {
        let mut instance = Self { rect: bounds, clip, fill: [0.0, 0.0, 0.0, spec.opacity.clamp(0.0, 1.0)],
            params: [if spec.blend == Blend::PlusLighter { 1.0 } else { 0.0 }, KIND_LAYER,
                layer_blur::sigma(spec.blur), 0.0], ..Self::zeroed() };
        instance.round_clip(spec.clip_radius);
        instance
    }

    /// Turn this scope's composite `angle` radians clockwise about `pivot` (device pixels).
    /// The contents draw upright; the layer's clip applies after the turn. Layer records keep
    /// it in `border[1..4]` as `[angle, pivot.x, pivot.y]`.
    pub fn turned(mut self, angle: f32, pivot: [f32; 2]) -> Self {
        self.border = [0.0, angle, pivot[0], pivot[1]];
        self
    }

    pub fn end_layer() -> Self {
        Self { params: [0.0, KIND_LAYER_END, 0.0, 0.0], ..Self::zeroed() }
    }
}

/// An effect slot's `El::effect` tag, from the two exact 16-bit floats in `border` (`KIND_EFFECT`).
pub(crate) fn effect_mark(i: &Instance) -> u32 {
    (i.border[0] as u32 & 0xFFFF) | (i.border[1] as u32 & 0xFFFF) << 16
}

/// Pack a 64-bit integer into four exact non-negative 16-bit integer floats (`0.0..=65535.0`).
pub(crate) fn pack_u64_f32(v: u64) -> [f32; 4] {
    [
        (v & 0xFFFF) as f32,
        ((v >> 16) & 0xFFFF) as f32,
        ((v >> 32) & 0xFFFF) as f32,
        ((v >> 48) & 0xFFFF) as f32,
    ]
}

/// Unpack a 64-bit integer from [`pack_u64_f32`].
pub(crate) fn unpack_u64_f32(a: [f32; 4]) -> u64 {
    (a[0] as u64 & 0xFFFF)
        | ((a[1] as u64 & 0xFFFF) << 16)
        | ((a[2] as u64 & 0xFFFF) << 32)
        | ((a[3] as u64 & 0xFFFF) << 48)
}

/// Close a runtime scope, folding empty/single-draw groups back into the direct path. Only the
/// last one or two records move, so video insertions before this scope keep their indices.
pub(crate) fn end_layer(instances: &mut Vec<Instance>, surfaces: &mut [crate::native_surface::SurfaceDraw], marks: &mut Vec<u32>, start: usize, surface_start: usize) {
    let primitives = instances.len() - start - 1;
    let videos = surfaces.len() - surface_start;
    let scope = instances[start];
    let unmasked = if primitives == 1 {
        instances[start + 1].clip == NO_CLIP && instances[start + 1].fade == [0; 4]
    } else if videos == 1 {
        surfaces[surface_start].clip == NO_CLIP && surfaces[surface_start].fade == [0; 4]
    } else { true };
    let transfer_clip = scope.fade[3] == 0 && unmasked;
    // A lone effect slot keeps a faded scope: the compositor's paint can't take the opacity.
    let faded_effect = primitives == 1 && instances[start + 1].params[1] == KIND_EFFECT && scope.fill[3] != 1.0;
    // A layer's `radii` hold its hole (`El::cutout_circle`), which only its composite can cut.
    if !faded_effect && primitives + videos <= 1 && (primitives + videos == 0 || (scope.border[1] == 0.0 && scope.radii == [0.0; 4])) && scope.params[0] == 0.0 && scope.params[2] == 0.0 && scope.params[3] == 0.0
        && (transfer_clip || (scope.clip == NO_CLIP && scope.fade == [0; 4])) {
        let opacity = scope.fill[3];
        if primitives == 1 {
            let i = &mut instances[start + 1];
            if transfer_clip { i.clip = scope.clip; i.fade = scope.fade; }
            i.fill = i.fill.map(|v| v * opacity);
            // A glyph's `border` holds no colour (an outline glyph's names its record).
            if i.params[1] != KIND_GLYPH { i.border = i.border.map(|v| v * opacity); }
            if i.params[1] == KIND_GRADIENT { i.uv = i.uv.map(|v| v * opacity); }
            // A box's `params.w` is the opacity its colours carry (shader.wgsl `border_over`).
            if i.params[1] == KIND_RECT || i.params[1] == KIND_GRADIENT { i.params[3] *= opacity; }
        }
        if videos == 1 {
            if transfer_clip { surfaces[surface_start].clip = scope.clip; surfaces[surface_start].fade = scope.fade; }
            surfaces[surface_start].opacity *= opacity;
            surfaces[surface_start].before_instance -= 1;
        }
        instances.remove(start);
        if marks.len() > start { marks.remove(start); }
    } else {
        instances.push(Instance::end_layer());
        if let Some(&mark) = marks.get(start) { marks.push(mark); }
    }
}

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// Vulkan loaders crash when two threads create instances or devices at once (seen with
/// parallel GPU tests on lavapipe), so Rune creates them one at a time.
static GPU_INIT: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl Gpu {
    /// Panics without a usable adapter or device; windows use `try_new`.
    pub fn new(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> Gpu {
        Self::try_new(instance, surface).unwrap_or_else(|error| panic!("{error}"))
    }

    pub fn try_new(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>) -> Result<Gpu, String> {
        Self::try_with_features(instance, surface, wgpu::Features::empty())
    }

    /// Like `try_new`, also enabling whichever of `wanted` the adapter supports.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn try_with_features(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>, wanted: wgpu::Features) -> Result<Gpu, String> {
        Self::try_for(instance, surface, wanted, false)
    }

    /// Like `try_with_features`, preferring for a `translucent` surface `translucency::prefer`'s adapter.
    #[cfg(not(target_arch = "wasm32"))]
    fn try_for(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>, wanted: wgpu::Features, translucent: bool) -> Result<Gpu, String> {
        Self::try_picked(instance, surface, wanted, translucent, false)
    }

    /// A device for a surface not made yet (`warm_gpu`): on the adapter a surface would get.
    #[cfg(not(any(target_arch = "wasm32", target_os = "macos")))]
    fn try_for_display(instance: wgpu::Instance, wanted: wgpu::Features) -> Result<Gpu, String> {
        Self::try_picked(instance, None, wanted, false, true)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn try_picked(instance: wgpu::Instance, surface: Option<&wgpu::Surface<'_>>, wanted: wgpu::Features, translucent: bool, presented: bool) -> Result<Gpu, String> {
        let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
        let pick = crate::gpu::pick::pick(&instance, surface, presented)?;
        let adapter = match surface {
            Some(s) if translucent && !pick.from_env() => crate::render::translucency::prefer(&instance, s, &pick),
            _ => pick.adapter,
        };
        match Self::device_on(&adapter, wanted) {
            Ok((device, queue)) => {
                crate::native_surface::note_device(&device, surface.is_some() || presented);
                Ok(Gpu { instance, adapter, device, queue })
            }
            // A driver that can't make a device (a broken integrated GPU on a hybrid laptop)
            // leaves the window to the next adapter that presents to its surface.
            Err(error) => {
                let first = adapter.get_info();
                let others = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()))
                    .into_iter()
                    .filter(|other| {
                        let info = other.get_info();
                        (info.name != first.name || info.backend != first.backend)
                            && surface.is_none_or(|s| other.is_surface_supported(s))
                            && (!translucent || surface.is_none_or(|s| crate::render::translucency::mode(s, other).is_some()))
                    });
                for other in others {
                    if let Ok((device, queue)) = Self::device_on(&other, wanted) {
                        eprintln!("rune: {} ({:?}) made no device ({error}); using {}", first.name, first.backend, other.get_info().name);
                        crate::native_surface::note_device(&device, surface.is_some() || presented);
                        return Ok(Gpu { instance, adapter: other, device, queue });
                    }
                }
                Err(error)
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn device_on(adapter: &wgpu::Adapter, wanted: wgpu::Features) -> Result<(wgpu::Device, wgpu::Queue), String> {
        let optional_features = wgpu::Features::TIMESTAMP_QUERY | wanted | crate::gpu::backend::free_features(adapter);
        // Precompiled uber shaders: a metallib on macOS, DXBC on Windows.
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let optional_features = optional_features | wgpu::Features::PASSTHROUGH_SHADERS;
        let required_features = adapter.features() & optional_features;
        // A compositor that asks for immediates (`Compositor::features`) gets all the adapter has.
        let max_immediate_size = if required_features.contains(wgpu::Features::IMMEDIATES) { adapter.limits().max_immediate_size } else { 0 };
        let adapter_limits = adapter.limits();
        let required_limits = if adapter_limits.max_inter_stage_shader_variables < wgpu::Limits::default().max_inter_stage_shader_variables
            || adapter_limits.max_color_attachments < wgpu::Limits::default().max_color_attachments
            || adapter_limits.max_compute_workgroup_storage_size < wgpu::Limits::default().max_compute_workgroup_storage_size
        {
            wgpu::Limits {
                max_immediate_size,
                ..wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter_limits.clone())
                    .using_alignment(adapter_limits)
            }
        } else {
            wgpu::Limits {
                max_immediate_size,
                ..wgpu::Limits::default()
                    .using_resolution(adapter_limits.clone())
                    .using_alignment(adapter_limits)
            }
        };
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("rune"),
            required_features,
            required_limits,
            experimental_features: Default::default(),
            // `Performance` suballocates from 128 MB device and 64 MB host blocks, so a device
            // that holds a few atlases commits about 190 MB more than it uses: process memory on
            // an integrated GPU, and on Windows on a discrete one too. Rune keeps and reuses its
            // buffers and textures, so the smaller blocks only cost a few more heaps.
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: Default::default(),
        }))
        .map_err(|error| format!("no GPU device: {error}"))
    }

    #[cfg(target_arch = "wasm32")]
    pub fn try_with_features(_: wgpu::Instance, _: Option<&wgpu::Surface<'_>>, _: wgpu::Features) -> Result<Gpu, String> {
        Err("synchronous GPU initialization is unsupported on web; use App::run".into())
    }

    #[cfg(target_arch = "wasm32")]
    fn try_for(i: wgpu::Instance, s: Option<&wgpu::Surface<'_>>, w: wgpu::Features, _: bool) -> Result<Gpu, String> { Self::try_with_features(i, s, w) }

    pub fn headless() -> Gpu {
        Self::new(Self::instance(), None)
    }

    /// A headless GPU on wgpu's backends, for tests of what needs wgpu-hal.
    #[cfg(test)]
    pub(crate) fn headless_wgpu() -> Gpu {
        Self::new(Self::wgpu_instance(), None)
    }

    /// Without `VALIDATION_INDIRECT_CALL`: Rune issues no indirect draws or dispatches, and the
    /// validation pipeline wgpu would build for them costs every device a shader compile (about
    /// 40 ms on a Mac's first launch after an install or update). Indirect draws a compositor
    /// records on Rune's device still work; wgpu just doesn't check their argument buffers.
    pub fn instance() -> wgpu::Instance {
        #[cfg(all(target_vendor = "apple", feature = "metal-backend"))]
        if crate::gpu::metal::enabled() { return crate::gpu::metal::instance(); }
        Self::wgpu_instance()
    }

    /// `instance`, or wgpu's when the native Metal backend is only the default and lacks one of
    /// the features a compositor wants (`Compositor::features`).
    pub(crate) fn instance_for(wanted: wgpu::Features) -> wgpu::Instance {
        if crate::gpu::backend::wgpu_for(wanted) { Self::wgpu_instance() } else { Self::instance() }
    }

    /// wgpu's own backends, whatever `RUNE_GPU_BACKEND` says: DX12 alone on Windows: it's the backend that composites transparent surfaces, and a
    /// Vulkan instance loads every GPU's Vulkan driver, where a broken one (Intel's, on a hybrid
    /// laptop) crashed the process before its first window (Hex #662). Vulkan only when no DX12
    /// adapter exists.
    /// The choice is made on the first GPU use in the process, not at launch, so a windowless
    /// app (a tray app before it draws anything) touches no GPU: asking whether a DX12 adapter
    /// exists makes a device on every adapter, over 100 ms.
    #[cfg(target_os = "windows")]
    fn wgpu_instance() -> wgpu::Instance {
        static DX12: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
        let dx12 = *DX12.get_or_init(|| {
            let found = !pollster::block_on(Self::wgpu_instance_on(wgpu::Backends::DX12).enumerate_adapters(wgpu::Backends::DX12)).is_empty();
            if !found { eprintln!("rune: no DX12 adapter; drawing with Vulkan"); }
            found
        });
        Self::wgpu_instance_on(if dx12 { wgpu::Backends::DX12 } else { wgpu::Backends::VULKAN })
    }

    /// wgpu's own backends, whatever `RUNE_GPU_BACKEND` says.
    #[cfg(not(target_os = "windows"))]
    fn wgpu_instance() -> wgpu::Instance {
        let _serial = GPU_INIT.lock().unwrap_or_else(|e| e.into_inner());
        Self::wgpu_instance_on(wgpu::Backends::all())
    }

    /// One instance per set of backends for the whole process: a new one per device was a new
    /// DXGI factory and adapter list each time a window or overlay made a device (hex#1268).
    fn wgpu_instance_on(backends: wgpu::Backends) -> wgpu::Instance {
        static MADE: std::sync::Mutex<Vec<(wgpu::Backends, wgpu::Instance)>> = std::sync::Mutex::new(Vec::new());
        let mut made = MADE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, instance)) = made.iter().find(|(made, _)| *made == backends) { return instance.clone(); }
        let instance = Self::new_wgpu_instance(backends);
        made.push((backends, instance.clone()));
        instance
    }

    fn new_wgpu_instance(backends: wgpu::Backends) -> wgpu::Instance {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = backends;
        descriptor.flags.remove(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL);
        // An HWND's DXGI swapchain is opaque only; DirectComposition composites alpha on every GPU.
        descriptor.backend_options.dx12.presentation_system = wgpu::Dx12SwapchainKind::DxgiFromVisual;
        // FXC ships with Windows (d3dcompiler_47.dll in System32). wgpu's default looks for
        // dxcompiler.dll on PATH, where another app's incompatible copy crashes the first
        // shader compile (Hex #587).
        descriptor.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
        wgpu::Instance::new(descriptor)
    }

    /// The GPU for a window's first surface, which `surface` makes from the instance:
    /// `prewarm_gpu`'s device, with the renderer it made, when its adapter presents to that
    /// surface and has `wanted`'s features; otherwise a new device on the same instance, so the
    /// window never gets a second surface.
    /// A `translucent` surface gets an adapter that composites translucent pixels when any can.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn for_surface<'w>(wanted: wgpu::Features, translucent: bool,
        surface: impl FnOnce(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>) -> Result<(Gpu, wgpu::Surface<'w>, Option<Renderer>), String> {
        Self::for_surface_from(prewarm::take(), wanted, translucent, surface)
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn for_surface_from<'w>(early: Option<(Gpu, Renderer)>, wanted: wgpu::Features, translucent: bool,
        surface: impl FnOnce(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>) -> Result<(Gpu, wgpu::Surface<'w>, Option<Renderer>), String> {
        let made = Self::surface_gpu_from(early, wanted, translucent, surface)?;
        if std::env::var_os("RUNE_GPU_LOG").is_some_and(|v| v != "0") {
            eprintln!("rune: a surface draws with {}", crate::gpu::backend::describe(&made.0.device));
        }
        Ok(made)
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn surface_gpu_from<'w>(early: Option<(Gpu, Renderer)>, wanted: wgpu::Features, translucent: bool,
        surface: impl FnOnce(&wgpu::Instance) -> Result<wgpu::Surface<'w>, String>) -> Result<(Gpu, wgpu::Surface<'w>, Option<Renderer>), String> {
        // A prewarmed native Metal GPU lacking a wanted feature is dropped before it makes a surface.
        let Some((early, renderer)) = early.filter(|(gpu, _)| crate::gpu::backend::serves(&gpu.device, wanted)) else {
            let instance = Self::instance_for(wanted);
            let surface = surface(&instance)?;
            gpu_events::starting();
            return Ok((Self::try_for(instance, Some(&surface), wanted, translucent)?, surface, None));
        };
        let surface = surface(&early.instance)?;
        if crate::render::translucency::serves(&early, &surface, wanted, translucent) {
            return Ok((early, surface, Some(renderer)));
        }
        let instance = early.instance.clone();
        drop((renderer, early));
        gpu_events::starting();
        Ok((Self::try_for(instance, Some(&surface), wanted, translucent)?, surface, None))
    }
}

/// Starts the GPU device and Rune's render pipeline on a thread, so they're ready when the first
/// window opens instead of being made after AppKit starts, which is most of a first launch after
/// an install or update (when Metal compiles the shaders). Call it first thing in `main`; Rune's
/// run functions call it too, but only once they start the event loop.
///
/// A window or overlay opened at launch picks the result up, waiting for the thread if it's still working;
/// if none opens, it's dropped. Calling again does nothing. Only on macOS, where any Metal adapter
/// presents to any window; elsewhere the window's surface decides the adapter, and this does
/// nothing.
///
/// It starts on the backend the app draws with when nothing chose one: Rune's native Metal
/// backend with the `metal-backend` feature (on by default), wgpu without it. An app that opts
/// out with `GpuBackend::Wgpu` should call `prewarm_gpu_on(GpuBackend::Wgpu)` instead, so the
/// thread starts on wgpu rather than on the native backend and starting over.
pub fn prewarm_gpu() {
    prewarm::start();
}

/// Chooses `backend` (as `App::gpu_backend` does), then starts `prewarm_gpu`'s thread on it, so an
/// app that picks a backend other than the default never prewarms on the default and restarts.
/// `App::gpu_backend` with the same backend afterwards changes nothing. On iOS it only chooses: call
/// it before the first `embed::LayerHost` (and `embed::prewarm`). Elsewhere, or without
/// `metal-backend`, it is `prewarm_gpu`.
pub fn prewarm_gpu_on(backend: crate::GpuBackend) {
    choose_then_start(backend, crate::gpu::backend::choose, prewarm::start);
}

/// `choose` before `start`: a thread started first would run on the old backend.
fn choose_then_start(backend: crate::GpuBackend, choose: impl FnOnce(crate::GpuBackend), start: impl FnOnce()) {
    choose(backend);
    start();
}

/// Starts `prewarm_gpu`'s thread over if it's working, for a backend chosen after it started.
#[cfg(all(target_os = "macos", feature = "metal-backend"))]
pub(crate) fn restart_prewarm() {
    prewarm::restart();
}

/// Drops what `prewarm_gpu` made if no window took it.
pub(crate) fn discard_prewarm() {
    #[cfg(not(test))]
    prewarm::discard();
}

#[cfg(target_os = "macos")]
mod prewarm;

#[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
#[path = "render/warm.rs"]
mod prewarm;

#[cfg(target_arch = "wasm32")]
mod prewarm {
    pub(super) fn start() {}
    pub(super) fn take() -> Option<(super::Gpu, super::Renderer)> { None }
    pub(super) fn discard() {}
}

mod warmup;
pub use warmup::{warm_gpu, warm_gpu_with};
pub(crate) use warmup::{park_gpu, surface_gpu};

#[derive(Clone, Copy, PartialEq, Eq)]
enum BatchBind {
    /// Glyph atlas page 0 with image atlas page `usize`.
    Image(usize),
    /// Secondary glyph atlas page `usize` (index in `atlas_pages`, i.e. glyph page - 1).
    Glyph(usize),
}

pub struct Renderer {
    surfaces: Option<crate::native_surface::SurfaceRenderer>,
    pipeline: wgpu::RenderPipeline,
    format: wgpu::TextureFormat,
    shader: dxbc::Shaders,
    layout: wgpu::PipelineLayout,
    vertex_entry: &'static str,
    /// Pipelines for other target formats a compositor draws into (`add_format`).
    extra: Vec<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
    bind_group: wgpu::BindGroup,
    bind_layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    atlas: wgpu::Texture,
    /// Which uber shader draws (`variant::Uber`), with the outline store's texture under Windfoil.
    uber: variant::Uber,
    /// The 1×1 image the base bind group holds until the first image page.
    stand_in: wgpu::Texture,
    /// Secondary glyph atlas pages (`1..`).
    atlas_pages: Vec<(wgpu::Texture, wgpu::BindGroup)>,
    /// Stable image pages; the initial bind group uses a 1×1 stand-in.
    images: Vec<Option<(wgpu::Texture, wgpu::BindGroup)>>,
    image_batches: Vec<(std::ops::Range<u32>, BatchBind)>,
    instances: wgpu::Buffer,
    capacity: usize,
    count: u32,
    springs: Option<crate::gpu::springs::Bindings>,
    spring_instances: usize,
    layers: layers::Layers,
    srgb_blend: bool,
    /// Effect slots of the uploaded instances, by index.
    effects: Vec<(u32, Instance)>,
    /// Pooled viewport-sized backdrop copies, allocated only when a slot sets `effect_backdrop`
    /// or an element sets `backdrop_blur`.
    backdrops: Vec<Backdrop>,
    /// Pipelines and pyramid for backdrop blur records, built on the first one.
    backdrop_blur: Option<backdrop_blur::BackdropBlur>,
    uploads: upload::Uploads,
    /// Unique to this renderer, so to the device it was made on (`Frame::device_key`).
    device_key: crate::compose::DeviceKey,
}

impl Renderer {
    /// `Frame::device_key`: a renderer belongs to one device, and a new device gets a new one.
    pub fn device_key(&self) -> crate::compose::DeviceKey {
        self.device_key
    }

    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Renderer {
        Self::create(device, format, None, cfg!(target_os = "macos"))
    }

    pub fn set_srgb_blend(&mut self, enabled: bool) {
        self.srgb_blend = enabled;
        if let Some(s) = &mut self.surfaces {
            s.set_srgb_blend(enabled);
        }
    }

    /// Opt-in retained geometry. IDs map instances to shared translation/opacity nodes.
    /// Upload geometry once with `prepare`, then use `present_at` without rebuilding instances.
    pub fn with_springs(device: &wgpu::Device, format: wgpu::TextureFormat, nodes: &[crate::gpu::springs::Node], ids: &[u32]) -> Renderer {
        Self::create(device, format, Some((nodes, ids)), true)
    }

    /// A renderer that starts with the Classic uber shader, as on macOS, and builds Windfoil when
    /// text first draws from outlines (render/variant.rs); for checking that path elsewhere.
    #[doc(hidden)]
    pub fn new_classic(device: &wgpu::Device, format: wgpu::TextureFormat) -> Renderer {
        Self::create(device, format, None, true)
    }

    fn create(device: &wgpu::Device, format: wgpu::TextureFormat, properties: Option<(&[crate::gpu::springs::Node], &[u32])>, classic: bool) -> Renderer {
        let spring_shader = if properties.is_some() {
            let source = format!("{CLIP_WGSL}\n{}\n{}", include_str!("shader.wgsl"), crate::gpu::springs::SHADER);
            Some(device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("uber"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            }))
        } else {
            None
        };
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(device.limits().min_uniform_buffer_offset_alignment).max(32) + 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let stand_in = Self::image_texture(device, 1, 1);
        let spring_layout = properties.map(|_| crate::gpu::springs::Bindings::layout(device));
        let vertex_entry = if properties.is_some() { "vs_springs" } else { "vs" };
        // The shader machine starts Classic; everywhere but macOS (whose default text keeps the
        // Classic shader it always drew) and GPU springs, it asks for Windfoil at once.
        let (uber, windfoil) = if classic {
            (variant::Uber::Classic, None)
        } else {
            Self::start_windfoil(device, &variant::BuildContext { format, vertex_entry, globals: &globals, stand_in: &stand_in })
        };
        let atlas = Self::atlas_texture(device, windfoil.is_some());
        let (bgl, layout, shader, pipeline) = match windfoil {
            Some(built) => (built.bind_layout, built.layout, built.shader, built.pipeline),
            None => {
                let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: None,
                    entries: &metallib::uber_bind_entries(),
                });
                let layouts = [Some(&bgl), spring_layout.as_ref()];
                let layout = device.create_pipeline_layout(&metallib::uber_pipeline_layout(
                    &layouts, properties.is_some(),
                ));
                let make_pipeline = |shader: &dxbc::Shaders| Self::pipeline(device, &layout, shader, format, vertex_entry, Blend::SourceOver);
                let (shader, pipeline) = match spring_shader {
                    Some(shader) => { let shader = dxbc::Shaders::one(shader); let pipeline = make_pipeline(&shader); (shader, pipeline) }
                    None => metallib::uber_shader_and_pipeline(device, make_pipeline),
                };
                (bgl, layout, shader, pipeline)
            }
        };
        let bind_group = Self::bind_group(device, &bgl, &globals, uber.outlines(), &atlas, &stand_in, None);
        let capacity = 256;
        let instances = Self::instance_buffer(device, capacity);
        let spring_instances = properties.map_or(0, |(_, ids)| ids.len());
        let springs = properties.map(|(nodes, ids)| crate::gpu::springs::Bindings::new(device, spring_layout.as_ref().unwrap(), nodes, ids));
        Renderer {
            surfaces: None,
            pipeline,
            format,
            shader,
            layout,
            vertex_entry,
            extra: Vec::new(),
            bind_group,
            bind_layout: bgl,
            globals,
            atlas,
            uber,
            stand_in,
            atlas_pages: Vec::new(),
            images: Vec::new(),
            image_batches: Vec::new(),
            instances,
            capacity,
            count: 0,
            springs,
            spring_instances,
            layers: Default::default(),
            srgb_blend: false,
            effects: Vec::new(),
            backdrops: Vec::new(),
            backdrop_blur: None,
            uploads: Default::default(),
            device_key: crate::compose::DeviceKey::next(),
        }
    }

    fn pipeline(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        shader: &dxbc::Shaders,
        format: wgpu::TextureFormat,
        vertex_entry: &str,
        blend: Blend,
    ) -> wgpu::RenderPipeline {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("uber"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: &shader.vertex,
                entry_point: Some(vertex_entry),
                compilation_options: Default::default(),
                buffers: &[Some(metallib::uber_vertex_layout())],
            },
            primitive: metallib::uber_primitive(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader.fragment,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(blend.state()),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }

    fn image_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
        Self::texture(device, "image atlas", wgpu::TextureFormat::Rgba8UnormSrgb, width, height)
    }

    /// A sampled texture the CPU fills with `write_rows`.
    fn texture(device: &wgpu::Device, label: &str, format: wgpu::TextureFormat, width: u32, height: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    fn bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        globals: &wgpu::Buffer,
        outlines: Option<&wgpu::Texture>,
        atlas: &wgpu::Texture,
        images: &wgpu::Texture,
        target: Option<&wgpu::Buffer>,
    ) -> wgpu::BindGroup {
        let atlas_view = atlas.create_view(&Default::default());
        let image_view = images.create_view(&Default::default());
        let outline_view = outlines.map(|t| t.create_view(&Default::default()));
        let mut entries = vec![
            wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&atlas_view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&image_view) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: target.unwrap_or(globals),
                offset: if target.is_some() { 0 } else { u64::from(device.limits().min_uniform_buffer_offset_alignment).max(32) },
                size: std::num::NonZeroU64::new(16),
            }) },
        ];
        // The Windfoil shader's outline store (render/variant.rs); Classic has no binding 4.
        if let Some(view) = &outline_view {
            entries.push(wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(view) });
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout, entries: &entries })
    }

    /// Builds the pipeline for another target format (e.g. an `Rgba16Float` layer), once.
    pub fn add_format(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if format != self.format && !self.extra.iter().any(|(f, _)| *f == format) {
            let p = Self::pipeline(device, &self.layout, &self.shader, format, self.vertex_entry, Blend::SourceOver);
            self.extra.push((format, p));
        }
        if self.layer_stats().targets > 0 { self.add_layer_format(device, format); }
        let [vw, vh] = self.target_frame(None).1;
        if !self.backdrops.is_empty() && vw > 0 && vh > 0 {
            self.ensure_backdrop(device, format, (vw, vh));
        }
        if let Some(s) = &mut self.surfaces { s.add_format(device, format); }
    }

    /// Switches the frame pass's target format (`!format.is_srgb()` turns `srgb_blend` on),
    /// keeping the glyph and image atlases, offscreen layer targets and `extra` pipelines.
    pub fn set_format(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if self.format == format { return; }
        let next = match self.extra.iter().position(|(f, _)| *f == format) {
            Some(i) => self.extra.swap_remove(i).1,
            None => Self::pipeline(device, &self.layout, &self.shader, format, self.vertex_entry, Blend::SourceOver),
        };
        let old = std::mem::replace(&mut self.pipeline, next);
        self.extra.push((self.format, old));
        self.format = format;
        self.srgb_blend = !format.is_srgb();
        if self.layer_stats().targets > 0 { self.add_layer_format(device, format); }
        let [vw, vh] = self.target_frame(None).1;
        if !self.backdrops.is_empty() && vw > 0 && vh > 0 {
            self.ensure_backdrop(device, format, (vw, vh));
        }
        if let Some(s) = &mut self.surfaces {
            s.set_srgb_blend(self.srgb_blend);
            s.add_format(device, format);
        }
    }

    /// Draws uploaded instances `range` into a pass whose target has `format` (the renderer's
    /// own, or one added with `add_format`). Call `encode_layers` before opening the pass.
    /// Scopes are atomic: list/range boundaries must not bisect a layer.
    pub fn draw_range(&self, pass: &mut wgpu::RenderPass<'_>, format: wgpu::TextureFormat, range: std::ops::Range<u32>) {
        if range.is_empty() && range.end != self.count { return; }
        self.submit_uploads();
        self.validate_layer_range(&range);
        let mut active = ActivePass::borrowed(pass);
        self.draw_layer_range(&mut active, format, range, None, &mut |_, _| {});
    }

    /// `draw_range` that stops at each effect slot (`El::effect`) in `range` and calls `hook`
    /// there, in painter order. Effects inside an isolated layer are reached through
    /// `encode_layers_with`, which draws that layer's pass; scopes nest as in `draw_range`.
    /// Because `pass` is already open, `EffectSite::backdrop` is `None`; use `draw_with`,
    /// `draw_with_backdrop`, or `draw_target_range_with` when slots request a backdrop copy.
    pub fn draw_range_with(&self, pass: &mut wgpu::RenderPass<'_>, format: wgpu::TextureFormat, range: std::ops::Range<u32>, hook: &mut EffectHook<'_>) {
        if range.is_empty() && range.end != self.count { return; }
        self.submit_uploads();
        self.validate_layer_range(&range);
        let mut active = ActivePass::borrowed(pass);
        self.draw_layer_range(&mut active, format, range, None, &mut |phase, site| {
            if let EffectPhase::Draw(pass) = phase { hook(pass, site); }
        });
    }

    /// Encodes isolated layers and draws the frame into `view` (cleared to `clear`), calling
    /// `hook` at each effect slot (`El::effect`) in painter order. At slots with
    /// `El::effect_backdrop(true)`, ends the current pass, copies the target up to that slot into
    /// a pooled backdrop texture, re-opens the pass with `LoadOp::Load`, and hands the backdrop
    /// view in `EffectSite::backdrop`.
    pub fn draw_with(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, clear: [f32; 4], hook: &mut EffectHook<'_>) {
        self.draw_with_backdrop(encoder, view, clear, &mut |phase, site| {
            if let EffectPhase::Draw(pass) = phase { hook(pass, site); }
        });
    }

    /// Like `draw_with`, also calling `hook(EffectPhase::Backdrop(encoder), site)` while the
    /// render pass is closed at each backdrop slot (`El::effect_backdrop(true)`) so the compositor
    /// can blur or pre-process `site.backdrop` (e.g. `FxRenderer::blur_backdrop`) before the pass
    /// re-opens for `EffectPhase::Draw(pass)`.
    pub fn draw_with_backdrop(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        clear: [f32; 4],
        hook: &mut EffectBackdropHook<'_>,
    ) {
        self.encode_layers_with_backdrop(encoder, hook);
        let clear = wgpu::Color { r: clear[0] as f64, g: clear[1] as f64, b: clear[2] as f64, a: clear[3] as f64 };
        self.draw_target_range_with(encoder, view, self.format, wgpu::LoadOp::Clear(clear), 0..self.count, hook);
    }

    /// Draws `range` into `view` (opening the pass with `load`), splitting the pass on `encoder`
    /// only at backdrop slots (`El::effect_backdrop(true)`) in `range` to copy `view` into
    /// `EffectSite::backdrop` and run `EffectPhase::Backdrop(encoder)` before re-opening with
    /// `LoadOp::Load`. Call `encode_layers_with` / `encode_layers_with_backdrop` before drawing
    /// the first range.
    /// `view` must be a single-sample, full-size, mip-zero 2D target; backdrop slots require
    /// its texture to have `COPY_SRC` usage. These owned passes have no depth/stencil or resolve
    /// attachment. Use `draw_range_with` for a caller-owned pass instead (no backdrop copies).
    pub fn draw_target_range_with(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        load: wgpu::LoadOp<wgpu::Color>,
        range: std::ops::Range<u32>,
        hook: &mut EffectBackdropHook<'_>,
    ) {
        if range.is_empty() && range.end != self.count { return; }
        self.validate_layer_range(&range);
        self.flush_uploads(encoder);
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        }).forget_lifetime();
        let mut active = ActivePass::owned(pass, encoder, view);
        self.draw_layer_range(&mut active, format, range, None, hook);
    }

    /// `draw_instances`, split after each effect slot in `range` so `hook` runs at its place.
    fn draw_with_effects(&self, pass: &mut ActivePass<'_, '_>, format: wgpu::TextureFormat, range: std::ops::Range<u32>, target: Option<usize>, hook: &mut EffectBackdropHook<'_>) {
        let first = self.effects.partition_point(|(at, _)| *at < range.start);
        let mut cursor = range.start;
        for (at, instance) in self.effects[first..].iter().take_while(|(at, _)| *at < range.end) {
            self.draw_instances(pass, format, cursor..*at + 1, target);
            cursor = *at + 1;
            if instance.params[1] == KIND_BACKDROP_BLUR {
                self.draw_backdrop_blur(pass, format, *at, instance, target);
                continue;
            }
            let (origin, size) = self.target_frame(target);
            // Scissor to the slot's clip rectangle, so a compositor that ignores `clip` still
            // stays inside its scroll view or clipped ancestor.
            let c = instance.clip;
            let edge = |v: f32, o: f32, max: u32| (v - o).clamp(0.0, max as f32) as u32;
            let (x0, y0) = (edge(c[0].floor(), origin[0], size[0]), edge(c[1].floor(), origin[1], size[1]));
            let (x1, y1) = (edge(c[2].ceil(), origin[0], size[0]), edge(c[3].ceil(), origin[1], size[1]));
            if x1 <= x0 || y1 <= y0 { continue; }
            let backdrop = if instance.params[0] != 0.0 {
                pass.split_for_backdrop(self, *at, instance, format, origin, size, target.is_some(), hook)
            } else {
                None
            };
            pass.set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
            hook(EffectPhase::Draw(pass), &EffectSite { index: *at, instance, mark: effect_mark(instance), data: instance.uv, scale: instance.params[3],
                format, origin, size, in_layer: target.is_some(), srgb_blend: self.srgb_blend, backdrop });
            pass.set_viewport(0.0, 0.0, size[0] as f32, size[1] as f32, 0.0, 1.0);
            pass.set_scissor(pass.scissor(), size);
        }
        self.draw_instances(pass, format, cursor..range.end, target);
    }

    fn draw_instances(&self, pass: &mut wgpu::RenderPass<'_>, format: wgpu::TextureFormat, range: std::ops::Range<u32>, target: Option<usize>) {
        if range.is_empty() {
            return;
        }
        let pipeline = if self.layer_blend(target) == Blend::PlusLighter {
            &self.layers.additive.as_ref().expect("prepared dissolve").0
        } else if format == self.format {
            &self.pipeline
        } else {
            &self.extra.iter().find(|(f, _)| *f == format).expect("Renderer::add_format first").1
        };
        pass.set_pipeline(pipeline);
        if let Some(properties) = &self.springs { pass.set_bind_group(1, &properties.group, &[]); }
        pass.set_vertex_buffer(0, self.instances.slice(..));
        let start = self.image_batches.partition_point(|(batch, _)| batch.end <= range.start);
        for (batch, bind_kind) in self.image_batches[start..].iter().take_while(|(batch, _)| batch.start < range.end) {
            let bind = match (*bind_kind, target) {
                (BatchBind::Image(page), Some(index)) => {
                    let t = self.layers.targets[index].as_ref().unwrap();
                    t.images.get(page).and_then(Option::as_ref).unwrap_or(&t.base)
                }
                (BatchBind::Image(page), None) => {
                    self.images.get(page).and_then(Option::as_ref).map_or(&self.bind_group, |(_, bind)| bind)
                }
                (BatchBind::Glyph(page), Some(index)) => {
                    let t = self.layers.targets[index].as_ref().unwrap();
                    t.glyphs.get(page).unwrap_or(&t.base)
                }
                (BatchBind::Glyph(page), None) => {
                    self.atlas_pages.get(page).map_or(&self.bind_group, |(_, bind)| bind)
                }
            };
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..4, batch.start.max(range.start)..batch.end.min(range.end));
        }
    }

    /// The format of the target the renderer was built for (`draw_range` with this format).
    pub fn format(&self) -> wgpu::TextureFormat { self.format }

    /// True when colours are sRGB-encoded for blending: the target is a non-sRGB view and every
    /// pass, offscreen layers included, holds encoded premultiplied values. False means linear
    /// premultiplied values, encoded by the `*Srgb` target on write.
    pub fn srgb_blend(&self) -> bool { self.srgb_blend }

    pub fn submit(&self, queue: &wgpu::Queue, mut encoder: wgpu::CommandEncoder) -> wgpu::SubmissionIndex {
        self.before_submit(&mut encoder);
        if let Some(s) = &self.surfaces { s.submit(queue,encoder) } else { queue.submit([encoder.finish()]) }
    }

    /// Whether `submit` is `queue.submit` after `before_submit` and nothing more: no native video
    /// surface waits on the frame's submission, so another thread may make it.
    #[cfg(target_os = "macos")]
    pub(crate) fn submits_plainly(&self) -> bool {
        self.surfaces.as_ref().is_none_or(|s| s.draws_nothing())
    }

    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, clear: [f32; 4]) {
        self.draw_timed(encoder, view, clear, None);
    }

    /// Constant-time scheduler check. Present the terminal frame, then stop the display link.
    /// Deadlines are recomputed only on commits, so no per-node CPU sampling occurs on ticks.
    pub fn springs_animating(&self, time: f64) -> bool {
        self.springs.as_ref().is_some_and(|p| time < p.settle_at)
    }

    /// Draws over what `view` already holds instead of clearing it (headless overlays).
    pub(crate) fn draw_over(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
        self.draw_load(encoder, view, wgpu::LoadOp::Load, None);
    }

    pub fn draw_timed(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, clear: [f32; 4], timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>) {
        let clear = wgpu::Color { r: clear[0] as f64, g: clear[1] as f64, b: clear[2] as f64, a: clear[3] as f64 };
        self.draw_load(encoder, view, wgpu::LoadOp::Clear(clear), timestamps);
    }

    fn draw_load(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>, timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>) {
        let timestamps = timestamps.or_else(|| self.surfaces.as_ref().and_then(|s| s.timestamp_writes()));
        self.encode_layers_timed(encoder, timestamps.as_ref(), &mut |_, _| {});
        let timestamps = timestamps.map(|mut t| {
            if self.layer_stats().extra_passes > 0 { t.beginning_of_pass_write_index = None; }
            t
        });
        let blurs = self.has_backdrop_blur();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: timestamps.clone(),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        if !blurs {
            self.draw_range(&mut pass, self.format, 0..self.count);
            return;
        }
        // Backdrop blurs split the frame pass, which needs it owned along with the encoder.
        let pass = pass.forget_lifetime();
        let mut active = ActivePass::owned(pass, encoder, view).with_timestamps(timestamps);
        self.draw_layer_range(&mut active, self.format, 0..self.count, None, &mut |_, _| {});
    }
}

/// How far a corner of radius `radius` reaches along each edge. Radii are signed: a negative
/// radius draws Apple's continuous corner, whose curve starts 1.528 times the radius from the
/// corner (`CONTINUOUS_K` in shader.wgsl).
pub(crate) fn corner_extent(radius: f32) -> f32 {
    if radius < 0.0 { -radius * CONTINUOUS_K } else { radius }
}

pub(crate) const CONTINUOUS_K: f32 = 1.528_664_8;

/// An offscreen sRGB target with CPU readback.
pub struct Offscreen {
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    buffer: wgpu::Buffer,
    pub width: u32,
    pub height: u32,
    padded_row: u32,
}

pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

impl Offscreen {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Offscreen {
        Self::with_format(device, width, height, OFFSCREEN_FORMAT)
    }

    pub fn with_format(device: &wgpu::Device, width: u32, height: u32, format: wgpu::TextureFormat) -> Offscreen {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let padded_row = (width * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (padded_row * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Offscreen { texture, view, buffer, width, height, padded_row }
    }

    pub fn copy(&self, encoder: &mut wgpu::CommandEncoder) {
        self.copy_to(encoder, &self.buffer);
    }

    /// Bytes between rows in a readback buffer (`width * 4`, rounded up to wgpu's alignment).
    pub(crate) fn padded_row(&self) -> u32 { self.padded_row }

    /// Copies the target into `buffer`, which holds `padded_row() * height` bytes.
    pub(crate) fn copy_to(&self, encoder: &mut wgpu::CommandEncoder, buffer: &wgpu::Buffer) {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
        );
    }

    /// Tightly packed sRGB RGBA8 after `copy` was submitted.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn read(&self, device: &wgpu::Device) -> Vec<u8> {
        let slice = self.buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map readback"));
        device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let out = {
            let data = slice.get_mapped_range().expect("mapped range");
            crate::tooling::record::unpad(&data, self.width, self.height, self.padded_row)
        };
        self.buffer.unmap();
        out
    }

    #[cfg(target_arch = "wasm32")]
    pub fn read(&self, _: &wgpu::Device) -> Vec<u8> {
        panic!("blocking GPU readback is unsupported on web")
    }
}

pub fn write_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let mut w = enc.write_header().map_err(std::io::Error::other)?;
    w.write_image_data(rgba).map_err(std::io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod backdrop_tests;


#[cfg(test)]
mod prewarm_order_tests {
    #[test]
    fn prewarm_gpu_on_chooses_before_it_starts() {
        let log = std::cell::RefCell::new(Vec::new());
        super::choose_then_start(crate::GpuBackend::Metal, |b| log.borrow_mut().push(format!("choose {b:?}")), || log.borrow_mut().push("start".into()));
        assert_eq!(*log.borrow(), ["choose Metal", "start"]);
    }
}

/// `prewarm_gpu` and how a window picks it up, on a bare Metal layer (no window server needed).
#[cfg(all(test, target_os = "macos"))]
mod prewarm_tests {
    use super::{Gpu, Renderer};
    use objc2_video::{rc::Retained, runtime::{AnyClass, AnyObject}, msg_send};

    fn layer() -> Retained<AnyObject> {
        unsafe { msg_send![AnyClass::get(c"CAMetalLayer").expect("QuartzCore"), new] }
    }

    fn surface<'w>(instance: &wgpu::Instance, layer: &Retained<AnyObject>) -> Result<wgpu::Surface<'w>, String> {
        let target = wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(Retained::as_ptr(layer) as *mut std::ffi::c_void);
        // SAFETY: the layer outlives the surface in each test.
        unsafe { instance.create_surface_unsafe(target) }.map_err(|e| e.to_string())
    }

    /// The window gets the thread's device and its compiled pipeline, once, in the Mac window format.
    #[test]
    fn a_window_takes_the_prewarmed_gpu_and_renderer_once() {
        crate::prewarm_gpu();
        crate::prewarm_gpu();
        let layer = layer();
        let (gpu, _surface, renderer) = match Gpu::for_surface(wgpu::Features::empty(), false, |i| surface(i, &layer)) {
            Ok(taken) => taken,
            Err(error) => return eprintln!("skipped: {error}"),
        };
        let Some(renderer) = renderer else { return eprintln!("skipped: no GPU for the prewarm thread") };
        // Rune's Metal backend loads its metallib without advertising passthrough.
        #[cfg(feature = "metal-backend")]
        let native = crate::gpu::metal::raw_device(&gpu.device).is_some();
        #[cfg(not(feature = "metal-backend"))]
        let native = false;
        assert!(native || gpu.device.features().contains(wgpu::Features::PASSTHROUGH_SHADERS));
        assert_eq!(renderer.format, wgpu::TextureFormat::Bgra8Unorm);
        drop((renderer, gpu));
        let (_, _surface, again) = Gpu::for_surface(wgpu::Features::empty(), false, |i| surface(i, &layer)).expect("a second GPU");
        assert!(again.is_none(), "the prewarmed GPU goes to one window");

        // A slot that already took or discarded a GPU ignores a later `start`, so a late
        // `prewarm_gpu()` call cannot leave an unclaimed GPU in memory.
        let taken = super::prewarm::Slot::new();
        assert!(taken.take().is_none());
        taken.start();
        assert!(taken.take().is_none());

        let discarded = super::prewarm::Slot::new();
        discarded.start();
        discarded.discard();
        assert!(discarded.take().is_none());
        discarded.start();
        assert!(discarded.take().is_none());

        // `restart` replaces a working thread and leaves an idle or finished slot alone.
        let restarted = super::prewarm::Slot::new();
        restarted.restart();
        restarted.start();
        restarted.restart();
        assert!(restarted.take().is_some(), "the new thread's GPU");
        restarted.restart();
        assert!(restarted.take().is_none(), "a taken slot stays taken");
    }

    /// A prewarmed device without a feature the window wants (and the adapter has) is replaced on
    /// the same instance, with the surface made once: a second surface would add a second layer.
    #[test]
    fn a_prewarmed_gpu_missing_wanted_features_is_redone_on_its_instance() {
        let Ok(early) = Gpu::try_new(Gpu::instance(), None) else { return eprintln!("skipped: no GPU") };
        let plain = wgpu::Features::FLOAT32_FILTERABLE | wgpu::Features::DEPTH32FLOAT_STENCIL8 | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
        let missing = (early.adapter.features() - early.device.features()) & plain;
        let Some(wanted) = missing.iter().next() else { return eprintln!("skipped: the adapter has none of the test's features") };
        let renderer = Renderer::new(&early.device, wgpu::TextureFormat::Bgra8UnormSrgb);
        let layer = layer();
        let mut made = 0;
        let (gpu, _surface, renderer) = Gpu::for_surface_from(Some((early, renderer)), wanted, false, |i| { made += 1; surface(i, &layer) })
            .expect("a GPU with the wanted feature");
        assert_eq!(made, 1);
        assert!(renderer.is_none(), "the prewarmed renderer belongs to the dropped device");
        assert!(gpu.device.features().contains(wanted));

        let Ok(early) = Gpu::try_new(Gpu::instance(), None) else { return };
        let renderer = Renderer::new(&early.device, wgpu::TextureFormat::Bgra8UnormSrgb);
        let (_, _surface, kept) = Gpu::for_surface_from(Some((early, renderer)), wgpu::Features::empty(), false, |i| surface(i, &layer)).expect("a GPU");
        assert!(kept.is_some(), "a suitable prewarmed device keeps its renderer");
    }

    /// A compositor wanting a feature the native Metal backend lacks gets wgpu's device, when
    /// Metal is only the default, with the prewarmed native GPU dropped before any surface.
    #[cfg(feature = "metal-backend")]
    #[test]
    fn a_compositor_wanting_what_the_native_backend_lacks_draws_with_wgpu() {
        if !crate::gpu::metal::enabled() || std::env::var_os("RUNE_GPU_BACKEND").is_some() {
            return eprintln!("skipped: the native backend isn't the default here");
        }
        let early = Gpu::headless();
        assert!(crate::gpu::metal::raw_device(&early.device).is_some());
        let renderer = Renderer::new(&early.device, wgpu::TextureFormat::Bgra8UnormSrgb);
        let layer = layer();
        let mut made = 0;
        let wanted = wgpu::Features::POLYGON_MODE_LINE;
        let (gpu, _surface, renderer) = Gpu::for_surface_from(Some((early, renderer)), wanted, false, |i| { made += 1; surface(i, &layer) })
            .expect("a wgpu GPU");
        assert_eq!(made, 1);
        assert!(renderer.is_none());
        assert!(crate::gpu::metal::raw_device(&gpu.device).is_none(), "wgpu's device");
        assert!(gpu.device.features().contains(wanted));
        let described = crate::gpu::backend::describe(&gpu.device);
        assert!(described.starts_with("wgpu Metal on ") && described.contains("which Rune's Metal backend lacks"), "{described}");
        assert!(crate::gpu::backend::describe(&Gpu::headless().device).ends_with("(the default)"));
        assert!(!crate::gpu::backend::serves(&Gpu::headless().device, wanted));
        assert!(crate::gpu::backend::serves(&Gpu::headless().device, wgpu::Features::TIMESTAMP_QUERY));
    }

    /// A compositor wanting immediates keeps the native backend and the prewarmed device, which
    /// has them whether or not anything asked, with the adapter's immediate size.
    #[cfg(feature = "metal-backend")]
    #[test]
    fn a_compositor_wanting_immediates_keeps_the_native_backend_and_the_prewarmed_gpu() {
        if !crate::gpu::metal::enabled() || std::env::var_os("RUNE_GPU_BACKEND").is_some() {
            return eprintln!("skipped: the native backend isn't the default here");
        }
        let early = Gpu::headless();
        let renderer = Renderer::new(&early.device, wgpu::TextureFormat::Bgra8UnormSrgb);
        let layer = layer();
        let wanted = wgpu::Features::IMMEDIATES;
        let (gpu, _surface, kept) = Gpu::for_surface_from(Some((early, renderer)), wanted, false, |i| surface(i, &layer)).expect("a GPU");
        assert!(kept.is_some(), "the prewarmed renderer");
        assert!(crate::gpu::metal::raw_device(&gpu.device).is_some(), "the native backend's device");
        assert!(gpu.device.features().contains(wanted));
        assert_eq!(gpu.device.limits().max_immediate_size, 4096);
        let described = crate::gpu::backend::describe(&gpu.device);
        assert!(described.starts_with("Rune's Metal backend on ") && described.ends_with("(the default)"), "{described}");
    }
}
