//! Render pipelines: each stage compiled against the layout, vertex buffers described at the
//! top buffer slots (vertex buffer `i` at 30 - i), and the color targets' formats and blending.

use std::sync::Arc;

use objc2_metal::{
    MTLBlendFactor, MTLBlendOperation, MTLColorWriteMask, MTLCullMode, MTLDevice, MTLPixelFormat, MTLPrimitiveTopologyClass,
    MTLPrimitiveType, MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLTriangleFillMode, MTLVertexDescriptor, MTLVertexFormat,
    MTLVertexStepFunction, MTLWinding,
};
use wgpu::custom::*;
use wgpu::naga;

use super::derive::{self, Use};
use super::layout::{LayoutRaw, PipelineLayout};
use super::resource::pixel_format;
use super::shader::{Stage, compile};
use super::{Obj, Shared, send_sync};

/// Metal's buffer slots per stage; vertex buffers count down from the last.
const MAX_BUFFERS: usize = 31;

#[derive(Debug)]
pub(crate) struct RenderPipeline {
    pub(super) raw: Arc<PipelineRaw>,
}

#[derive(Debug)]
pub(crate) struct PipelineRaw {
    pub(super) state: Obj<dyn MTLRenderPipelineState>,
    pub(super) layout: Arc<LayoutRaw>,
    pub(super) primitive: MTLPrimitiveType,
    pub(super) cull: MTLCullMode,
    pub(super) winding: MTLWinding,
    pub(super) fill: MTLTriangleFillMode,
    /// Per stage, the bindings whose lengths its sizes buffer holds.
    pub(super) sized: [Vec<naga::ResourceBinding>; 2],
}
send_sync!(PipelineRaw);

pub(super) fn vertex_slot(index: u32) -> usize { MAX_BUFFERS - 1 - index as usize }

fn vertex_format(format: wgpu::VertexFormat) -> MTLVertexFormat {
    use MTLVertexFormat as M;
    use wgpu::VertexFormat as V;
    match format {
        V::Float32 => M::Float,
        V::Float32x2 => M::Float2,
        V::Float32x3 => M::Float3,
        V::Float32x4 => M::Float4,
        V::Uint32 => M::UInt,
        V::Uint32x2 => M::UInt2,
        V::Uint32x3 => M::UInt3,
        V::Uint32x4 => M::UInt4,
        V::Sint32 => M::Int,
        V::Sint32x2 => M::Int2,
        V::Sint32x3 => M::Int3,
        V::Sint32x4 => M::Int4,
        V::Float16x2 => M::Half2,
        V::Float16x4 => M::Half4,
        V::Unorm8x2 => M::UChar2Normalized,
        V::Unorm8x4 => M::UChar4Normalized,
        V::Uint8x2 => M::UChar2,
        V::Uint8x4 => M::UChar4,
        V::Unorm16x2 => M::UShort2Normalized,
        V::Unorm16x4 => M::UShort4Normalized,
        V::Uint16x2 => M::UShort2,
        V::Uint16x4 => M::UShort4,
        V::Snorm16x2 => M::Short2Normalized,
        V::Snorm16x4 => M::Short4Normalized,
        other => super::unsupported!("vertex format {other:?}"),
    }
}

fn blend_factor(factor: wgpu::BlendFactor) -> MTLBlendFactor {
    use MTLBlendFactor as M;
    use wgpu::BlendFactor as B;
    match factor {
        B::Zero => M::Zero,
        B::One => M::One,
        B::Src => M::SourceColor,
        B::OneMinusSrc => M::OneMinusSourceColor,
        B::SrcAlpha => M::SourceAlpha,
        B::OneMinusSrcAlpha => M::OneMinusSourceAlpha,
        B::Dst => M::DestinationColor,
        B::OneMinusDst => M::OneMinusDestinationColor,
        B::DstAlpha => M::DestinationAlpha,
        B::OneMinusDstAlpha => M::OneMinusDestinationAlpha,
        B::SrcAlphaSaturated => M::SourceAlphaSaturated,
        B::Constant => M::BlendColor,
        B::OneMinusConstant => M::OneMinusBlendColor,
        B::Src1 => M::Source1Color,
        B::OneMinusSrc1 => M::OneMinusSource1Color,
        B::Src1Alpha => M::Source1Alpha,
        B::OneMinusSrc1Alpha => M::OneMinusSource1Alpha,
    }
}

fn blend_operation(operation: wgpu::BlendOperation) -> MTLBlendOperation {
    match operation {
        wgpu::BlendOperation::Add => MTLBlendOperation::Add,
        wgpu::BlendOperation::Subtract => MTLBlendOperation::Subtract,
        wgpu::BlendOperation::ReverseSubtract => MTLBlendOperation::ReverseSubtract,
        wgpu::BlendOperation::Min => MTLBlendOperation::Min,
        wgpu::BlendOperation::Max => MTLBlendOperation::Max,
    }
}

fn write_mask(writes: wgpu::ColorWrites) -> MTLColorWriteMask {
    let mut mask = MTLColorWriteMask::None;
    mask.set(MTLColorWriteMask::Red, writes.contains(wgpu::ColorWrites::RED));
    mask.set(MTLColorWriteMask::Green, writes.contains(wgpu::ColorWrites::GREEN));
    mask.set(MTLColorWriteMask::Blue, writes.contains(wgpu::ColorWrites::BLUE));
    mask.set(MTLColorWriteMask::Alpha, writes.contains(wgpu::ColorWrites::ALPHA));
    mask
}

fn topology(topology: wgpu::PrimitiveTopology) -> (MTLPrimitiveTopologyClass, MTLPrimitiveType) {
    use wgpu::PrimitiveTopology as T;
    match topology {
        T::PointList => (MTLPrimitiveTopologyClass::Point, MTLPrimitiveType::Point),
        T::LineList => (MTLPrimitiveTopologyClass::Line, MTLPrimitiveType::Line),
        T::LineStrip => (MTLPrimitiveTopologyClass::Line, MTLPrimitiveType::LineStrip),
        T::TriangleList => (MTLPrimitiveTopologyClass::Triangle, MTLPrimitiveType::Triangle),
        T::TriangleStrip => (MTLPrimitiveTopologyClass::Triangle, MTLPrimitiveType::TriangleStrip),
    }
}

fn vertex_descriptor(buffers: &[Option<wgpu::VertexBufferLayout<'_>>]) -> Option<objc2_video::rc::Retained<MTLVertexDescriptor>> {
    if buffers.iter().all(Option::is_none) { return None }
    let d = MTLVertexDescriptor::new();
    for (i, vb) in buffers.iter().enumerate() {
        let Some(vb) = vb else { continue };
        let slot = vertex_slot(i as u32);
        // SAFETY: slots and locations are within Metal's 31 buffers and attributes.
        unsafe {
            let layout = d.layouts().objectAtIndexedSubscript(slot);
            if vb.array_stride == 0 {
                let stride = vb.attributes.iter().map(|a| a.offset + a.format.size()).max().unwrap_or(0);
                layout.setStride(stride.next_multiple_of(4) as usize);
                layout.setStepFunction(MTLVertexStepFunction::Constant);
                layout.setStepRate(0);
            } else {
                layout.setStride(vb.array_stride as usize);
                layout.setStepFunction(match vb.step_mode {
                    wgpu::VertexStepMode::Vertex => MTLVertexStepFunction::PerVertex,
                    wgpu::VertexStepMode::Instance => MTLVertexStepFunction::PerInstance,
                });
            }
            for a in vb.attributes {
                let attribute = d.attributes().objectAtIndexedSubscript(a.shader_location as usize);
                attribute.setFormat(vertex_format(a.format));
                attribute.setBufferIndex(slot);
                attribute.setOffset(a.offset as usize);
            }
        }
    }
    Some(d)
}

pub(super) fn create(shared: &Shared, desc: &wgpu::RenderPipelineDescriptor<'_>) -> RenderPipeline {
    let label = desc.label.unwrap_or("pipeline");
    let layout = match desc.layout {
        Some(layout) => layout.as_custom::<PipelineLayout>().expect("a Metal pipeline layout").raw.clone(),
        None => {
            let vertex = Use { module: desc.vertex.module, entry: desc.vertex.entry_point, kind: naga::ShaderStage::Vertex };
            let fragment = desc.fragment.as_ref().map(|f| Use { module: f.module, entry: f.entry_point, kind: naga::ShaderStage::Fragment });
            derive::layout(label, &[Some(vertex), fragment].into_iter().flatten().collect::<Vec<_>>()).raw
        }
    };
    if desc.depth_stencil.is_some() { super::unsupported!("depth and stencil") }
    let (class, primitive) = topology(desc.primitive.topology);
    let points = class == MTLPrimitiveTopologyClass::Point;
    let d = MTLRenderPipelineDescriptor::new();
    let vertex = Stage { module: desc.vertex.module, entry: desc.vertex.entry_point, constants: desc.vertex.compilation_options.constants,
        kind: naga::ShaderStage::Vertex };
    let vs = compile(shared, &vertex, &layout.stages[0], points).unwrap_or_else(|e| panic!("{label}: {e}"));
    d.setVertexFunction(Some(&vs.function));
    let mut sized = [vs.sized, Vec::new()];
    if let Some(fragment) = &desc.fragment {
        let stage = Stage { module: fragment.module, entry: fragment.entry_point, constants: fragment.compilation_options.constants,
            kind: naga::ShaderStage::Fragment };
        let fs = compile(shared, &stage, &layout.stages[1], points).unwrap_or_else(|e| panic!("{label}: {e}"));
        d.setFragmentFunction(Some(&fs.function));
        sized[1] = fs.sized;
        if fragment.targets.len() > 4 { super::unsupported!("more than four color targets") }
        for (i, target) in fragment.targets.iter().enumerate() {
            // SAFETY: at most four attachments, checked above.
            let attachment = unsafe { d.colorAttachments().objectAtIndexedSubscript(i) };
            let Some(target) = target else { attachment.setPixelFormat(MTLPixelFormat::Invalid); continue };
            attachment.setPixelFormat(pixel_format(target.format));
            attachment.setWriteMask(write_mask(target.write_mask));
            if let Some(blend) = target.blend {
                attachment.setBlendingEnabled(true);
                attachment.setRgbBlendOperation(blend_operation(blend.color.operation));
                attachment.setSourceRGBBlendFactor(blend_factor(blend.color.src_factor));
                attachment.setDestinationRGBBlendFactor(blend_factor(blend.color.dst_factor));
                attachment.setAlphaBlendOperation(blend_operation(blend.alpha.operation));
                attachment.setSourceAlphaBlendFactor(blend_factor(blend.alpha.src_factor));
                attachment.setDestinationAlphaBlendFactor(blend_factor(blend.alpha.dst_factor));
            }
        }
    }
    d.setVertexDescriptor(vertex_descriptor(desc.vertex.buffers).as_deref());
    // SAFETY: the class matches the primitive type every draw with this pipeline uses.
    unsafe { d.setInputPrimitiveTopology(class) };
    d.setRasterSampleCount(desc.multisample.count as usize);
    d.setAlphaToCoverageEnabled(desc.multisample.alpha_to_coverage_enabled);
    let state = shared.device.newRenderPipelineStateWithDescriptor_error(&d).unwrap_or_else(|e| panic!("{label}: {}", e.localizedDescription()));
    let raw = PipelineRaw {
        state,
        layout,
        primitive,
        cull: match desc.primitive.cull_mode {
            None => MTLCullMode::None,
            Some(wgpu::Face::Front) => MTLCullMode::Front,
            Some(wgpu::Face::Back) => MTLCullMode::Back,
        },
        winding: match desc.primitive.front_face {
            wgpu::FrontFace::Cw => MTLWinding::Clockwise,
            wgpu::FrontFace::Ccw => MTLWinding::CounterClockwise,
        },
        fill: match desc.primitive.polygon_mode {
            wgpu::PolygonMode::Line => MTLTriangleFillMode::Lines,
            _ => MTLTriangleFillMode::Fill,
        },
        sized,
    };
    RenderPipeline { raw: Arc::new(raw) }
}

impl RenderPipelineInterface for RenderPipeline {
    fn get_bind_group_layout(&self, index: u32) -> DispatchBindGroupLayout {
        self.raw.layout.group(index)
    }
}
