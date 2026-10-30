use super::protocol::{
    NodeId, VirtualBounds, VirtualContentRef, VirtualNodeKind, VirtualTextPlan,
};

use serde::{Deserialize, Serialize};

/// Backend-neutral draw command emitted by a virtual surface.
///
/// The initial runtime emits a compact command list for visible nodes. Backends
/// can retain this list, translate it into Sugarloaf text/quad instances, or
/// bake stable chunks into textures without changing the protocol adapters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum VirtualDrawCommand {
    BeginNode {
        node: NodeId,
        kind: VirtualNodeKind,
        bounds: VirtualBounds,
        clip: VirtualBounds,
    },
    Rect {
        node: NodeId,
        bounds: VirtualBounds,
        color: [f32; 4],
    },
    TextRun {
        node: NodeId,
        x: f32,
        y: f32,
        content: Option<VirtualContentRef>,
        text_plan: Option<VirtualTextPlan>,
        text_hash: u64,
        byte_len: u32,
    },
    EndNode {
        node: NodeId,
    },
}

impl VirtualDrawCommand {
    pub(crate) fn byte_size_estimate(&self) -> usize {
        match self {
            Self::BeginNode { .. } => 96,
            Self::Rect { .. } => 64,
            Self::TextRun {
                byte_len,
                content,
                text_plan,
                ..
            } => {
                64 + *byte_len as usize
                    + content.as_ref().map(|_| 96).unwrap_or(0)
                    + text_plan
                        .as_ref()
                        .map(|plan| 64 + plan.spans.len() * 48 + plan.overlays.len() * 48)
                        .unwrap_or(0)
            }
            Self::EndNode { .. } => 16,
        }
    }
}
