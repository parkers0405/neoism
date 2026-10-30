use super::protocol::{
    DirtyKind, NodeSource, NodeSourceRange, SurfaceRevision, VirtualSourceEdit,
    VirtualSurfaceCommand, VirtualSurfaceError,
};
use super::standard::{VirtualBatchApplyReport, VirtualSurfaceRoute};
use super::surface::VirtualSurface;

use serde::{Deserialize, Serialize};

/// Source-side revision carried with a batch. Markdown parsers, agent
/// transcripts, and future code buffers can keep their native revision type and
/// collapse it to this monotonic number at the Sugarloaf boundary.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
)]
pub struct VirtualSourceRevision(pub u64);

/// Caller-produced operation batch. This is the reusable handoff shape for
/// markdown files, model-generated markdown, agent messages, logs, diffs, and
/// future editor buffers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VirtualSurfaceBatch {
    pub route: VirtualSurfaceRoute,
    pub source_revision: VirtualSourceRevision,
    pub expected_surface_revision: Option<SurfaceRevision>,
    pub preserve_anchor_viewport_y: Option<f32>,
    pub commands: Vec<VirtualSurfaceCommand>,
}

impl VirtualSurfaceBatch {
    pub fn new(source_revision: VirtualSourceRevision) -> Self {
        Self {
            route: VirtualSurfaceRoute::default(),
            source_revision,
            expected_surface_revision: None,
            preserve_anchor_viewport_y: None,
            commands: Vec::new(),
        }
    }

    pub fn for_route(
        route: VirtualSurfaceRoute,
        source_revision: VirtualSourceRevision,
    ) -> Self {
        Self {
            route,
            source_revision,
            expected_surface_revision: None,
            preserve_anchor_viewport_y: None,
            commands: Vec::new(),
        }
    }

    pub fn expecting_surface_revision(mut self, revision: SurfaceRevision) -> Self {
        self.expected_surface_revision = Some(revision);
        self
    }

    pub fn preserving_anchor(mut self, viewport_y: f32) -> Self {
        self.preserve_anchor_viewport_y = Some(viewport_y.max(0.0));
        self
    }

    pub fn push(&mut self, command: VirtualSurfaceCommand) {
        self.commands.push(command);
    }

    pub fn push_source_edit(
        &mut self,
        source: NodeSource,
        old_range: NodeSourceRange,
        new_range: NodeSourceRange,
        kind: DirtyKind,
    ) {
        self.push(VirtualSurfaceCommand::ApplySourceEdit(
            VirtualSourceEdit::new(source, old_range, new_range, kind),
        ));
    }

    pub fn extend(&mut self, commands: impl IntoIterator<Item = VirtualSurfaceCommand>) {
        self.commands.extend(commands);
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn apply_to(
        self,
        surface: &mut VirtualSurface,
    ) -> Result<VirtualBatchApplyReport, VirtualSurfaceError> {
        if let Some(expected) = self.expected_surface_revision {
            if surface.revision() != expected {
                return Err(VirtualSurfaceError::RevisionMismatch {
                    expected,
                    actual: surface.revision(),
                });
            }
        }

        let previous_revision = surface.revision();
        let command_count = self.commands.len();
        let anchor = self
            .preserve_anchor_viewport_y
            .and_then(|viewport_y| surface.capture_scroll_anchor(viewport_y));
        for command in self.commands {
            surface.apply(command)?;
        }
        let mut anchor_preserved = false;
        if let Some(anchor) = anchor {
            surface.restore_scroll_anchor(anchor)?;
            anchor_preserved = true;
        }
        Ok(VirtualBatchApplyReport {
            route: self.route,
            previous_revision,
            current_revision: surface.revision(),
            commands: command_count,
            anchor_preserved,
        })
    }
}

/// Adapter contract for source-specific producers. Implementations live outside
/// Sugarloaf; the renderer only receives semantic virtual-surface commands.
pub trait VirtualSurfaceAdapter {
    type Input;
    type Error;

    fn build_initial(
        &mut self,
        input: Self::Input,
    ) -> Result<VirtualSurfaceBatch, Self::Error>;

    fn update(&mut self, input: Self::Input) -> Result<VirtualSurfaceBatch, Self::Error>;
}
