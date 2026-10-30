use super::gpu::{VirtualGpuFramePacket, VirtualGpuPrimitive};

use crate::primitives::{Object, Rect, RichText};

/// Native Sugarloaf primitive plan lowered from a virtual GPU packet.
///
/// This is the bridge shape a real integration can feed into the existing
/// renderer after the higher-level markdown/agent/code producer has submitted
/// virtual nodes. It deliberately does not resolve text content itself; text is
/// still resolved through the content-provider protocol.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VirtualObjectPlan {
    pub objects: Vec<Object>,
    pub skipped_texture_tiles: usize,
}

impl VirtualObjectPlan {
    pub fn from_gpu_packet(packet: &VirtualGpuFramePacket) -> Self {
        let mut plan = Self {
            objects: Vec::with_capacity(packet.instances.len()),
            skipped_texture_tiles: 0,
        };

        for instance in &packet.instances {
            match instance.primitive {
                VirtualGpuPrimitive::SolidQuad => {
                    plan.objects.push(Object::Rect(Rect::new(
                        instance.bounds.x,
                        instance.bounds.y,
                        instance.bounds.width,
                        instance.bounds.height,
                        instance.color,
                    )));
                }
                VirtualGpuPrimitive::TextRun => {
                    let id = instance
                        .content
                        .map(|content| stable_usize(content.0))
                        .unwrap_or_else(|| stable_usize(instance.node.0));
                    plan.objects.push(Object::RichText(
                        RichText::new(id)
                            .with_position(instance.origin[0], instance.origin[1]),
                    ));
                }
                VirtualGpuPrimitive::TextureTile => {
                    plan.skipped_texture_tiles =
                        plan.skipped_texture_tiles.saturating_add(1);
                }
            }
        }

        plan
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

fn stable_usize(value: u64) -> usize {
    if usize::BITS >= 64 {
        value as usize
    } else {
        (value ^ (value >> 32)) as usize
    }
}
