use super::adapter::{VirtualSourceRevision, VirtualSurfaceAdapter, VirtualSurfaceBatch};
use super::protocol::{
    DirtyKind, NodeGeometry, NodeId, NodeRevision, NodeSource, NodeSourceRange,
    VirtualContentId, VirtualContentKind, VirtualContentRef, VirtualNode,
    VirtualNodeKind, VirtualSurfaceCommand, VirtualTextPlan,
};
use super::standard::VirtualSurfaceRoute;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VirtualAgentRole {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualAgentMessage {
    pub id: String,
    pub role: VirtualAgentRole,
    pub markdown: String,
    pub tool_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualAgentMessageUpdate {
    pub index: usize,
    pub message: VirtualAgentMessage,
    pub old_range: NodeSourceRange,
    pub new_range: NodeSourceRange,
    pub kind: DirtyKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VirtualAgentInput {
    Replace {
        session_id: String,
        messages: Vec<VirtualAgentMessage>,
        revision: VirtualSourceRevision,
    },
    Append {
        session_id: String,
        messages: Vec<VirtualAgentMessage>,
        revision: VirtualSourceRevision,
    },
    Edit {
        session_id: String,
        message_id: String,
        old_range: NodeSourceRange,
        new_range: NodeSourceRange,
        revision: VirtualSourceRevision,
        kind: DirtyKind,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualAgentStats {
    pub messages: usize,
    pub tool_cards: usize,
    pub splittable_messages: usize,
}

#[derive(Clone, Debug)]
pub struct VirtualAgentAdapter {
    namespace: String,
    next_append_index: u64,
    stats: VirtualAgentStats,
}

impl VirtualAgentAdapter {
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            next_append_index: 0,
            stats: VirtualAgentStats::default(),
        }
    }

    pub fn stats(&self) -> VirtualAgentStats {
        self.stats
    }

    pub fn build_replace_batch(
        &mut self,
        session_id: &str,
        messages: &[VirtualAgentMessage],
        revision: VirtualSourceRevision,
    ) -> VirtualSurfaceBatch {
        let (nodes, stats) =
            agent_nodes(&self.namespace, session_id, messages, revision, 0);
        self.next_append_index = nodes.len() as u64;
        self.stats = stats;
        let mut batch = VirtualSurfaceBatch::for_route(
            VirtualSurfaceRoute::agent(session_id),
            revision,
        );
        batch.push(VirtualSurfaceCommand::ReplaceAll(nodes));
        batch
    }

    pub fn build_append_batch(
        &mut self,
        session_id: &str,
        messages: &[VirtualAgentMessage],
        revision: VirtualSourceRevision,
    ) -> VirtualSurfaceBatch {
        let (nodes, stats) = agent_nodes(
            &self.namespace,
            session_id,
            messages,
            revision,
            self.next_append_index,
        );
        self.next_append_index =
            self.next_append_index.saturating_add(nodes.len() as u64);
        self.stats.messages = self.stats.messages.saturating_add(stats.messages);
        self.stats.tool_cards = self.stats.tool_cards.saturating_add(stats.tool_cards);
        self.stats.splittable_messages = self
            .stats
            .splittable_messages
            .saturating_add(stats.splittable_messages);

        let mut batch = VirtualSurfaceBatch::for_route(
            VirtualSurfaceRoute::agent(session_id),
            revision,
        );
        batch.push(VirtualSurfaceCommand::UpsertNodes(nodes));
        batch
    }

    pub fn build_edit_batch(
        &mut self,
        session_id: &str,
        message_id: &str,
        old_range: NodeSourceRange,
        new_range: NodeSourceRange,
        revision: VirtualSourceRevision,
        kind: DirtyKind,
    ) -> VirtualSurfaceBatch {
        let mut batch = VirtualSurfaceBatch::for_route(
            VirtualSurfaceRoute::agent(session_id),
            revision,
        );
        batch.push_source_edit(
            agent_source(session_id, message_id),
            old_range,
            new_range,
            kind,
        );
        batch
    }

    pub fn build_update_message_batch(
        &mut self,
        session_id: &str,
        update: VirtualAgentMessageUpdate,
        revision: VirtualSourceRevision,
    ) -> VirtualSurfaceBatch {
        let mut batch = VirtualSurfaceBatch::for_route(
            VirtualSurfaceRoute::agent(session_id),
            revision,
        );
        batch.push_source_edit(
            agent_source(session_id, &update.message.id),
            update.old_range,
            update.new_range,
            update.kind,
        );
        let (nodes, _) = agent_nodes(
            &self.namespace,
            session_id,
            std::slice::from_ref(&update.message),
            revision,
            update.index as u64,
        );
        batch.push(VirtualSurfaceCommand::UpsertNodes(nodes));
        batch
    }
}

impl VirtualSurfaceAdapter for VirtualAgentAdapter {
    type Input = VirtualAgentInput;
    type Error = std::convert::Infallible;

    fn build_initial(
        &mut self,
        input: Self::Input,
    ) -> Result<VirtualSurfaceBatch, Self::Error> {
        Ok(match input {
            VirtualAgentInput::Replace {
                session_id,
                messages,
                revision,
            }
            | VirtualAgentInput::Append {
                session_id,
                messages,
                revision,
            } => self.build_replace_batch(&session_id, &messages, revision),
            VirtualAgentInput::Edit {
                session_id,
                message_id,
                old_range,
                new_range,
                revision,
                kind,
            } => self.build_edit_batch(
                &session_id,
                &message_id,
                old_range,
                new_range,
                revision,
                kind,
            ),
        })
    }

    fn update(&mut self, input: Self::Input) -> Result<VirtualSurfaceBatch, Self::Error> {
        Ok(match input {
            VirtualAgentInput::Replace {
                session_id,
                messages,
                revision,
            } => self.build_replace_batch(&session_id, &messages, revision),
            VirtualAgentInput::Append {
                session_id,
                messages,
                revision,
            } => self.build_append_batch(&session_id, &messages, revision),
            VirtualAgentInput::Edit {
                session_id,
                message_id,
                old_range,
                new_range,
                revision,
                kind,
            } => self.build_edit_batch(
                &session_id,
                &message_id,
                old_range,
                new_range,
                revision,
                kind,
            ),
        })
    }
}

fn agent_nodes(
    namespace: &str,
    session_id: &str,
    messages: &[VirtualAgentMessage],
    revision: VirtualSourceRevision,
    base_index: u64,
) -> (Vec<VirtualNode>, VirtualAgentStats) {
    let mut nodes = Vec::with_capacity(messages.len());
    let mut stats = VirtualAgentStats::default();
    for (offset, message) in messages.iter().enumerate() {
        let index = base_index + offset as u64;
        let line_count = message.markdown.lines().count().max(1);
        let kind =
            if message.role == VirtualAgentRole::Tool || message.tool_name.is_some() {
                stats.tool_cards += 1;
                VirtualNodeKind::ToolCard
            } else {
                VirtualNodeKind::AgentMessage
            };
        let mut geometry =
            NodeGeometry::fixed(agent_message_height(line_count, kind.clone()));
        geometry.can_split = line_count > 48;
        if geometry.can_split {
            stats.splittable_messages += 1;
        }
        let source = agent_source(session_id, &message.id);
        let text_hash =
            stable_hash_parts(&[message.id.as_bytes(), message.markdown.as_bytes()]);
        let source_range = NodeSourceRange::new(0, message.markdown.len() as u64);
        let content = VirtualContentRef::new(
            VirtualContentId(text_hash),
            VirtualContentKind::Markdown,
            source.clone(),
            source_range,
            NodeRevision(revision.0),
            text_hash,
            line_count as u32,
        );
        let text_plan = VirtualTextPlan::new(content.clone());
        let node = VirtualNode::new(
            stable_node_id(
                namespace,
                session_id,
                &message.id,
                index,
                role_tag(message.role),
            ),
            kind,
        )
        .with_geometry(geometry)
        .with_revision(text_hash)
        .with_text_hash(text_hash)
        .with_source(source, source_range)
        .with_content(content)
        .with_text_plan(text_plan);
        nodes.push(node);
        stats.messages += 1;
    }
    (nodes, stats)
}

fn agent_source(session_id: &str, message_id: &str) -> NodeSource {
    NodeSource::AgentMessage {
        session: session_id.to_string(),
        message: message_id.to_string(),
    }
}

fn agent_message_height(line_count: usize, kind: VirtualNodeKind) -> f32 {
    let base = match kind {
        VirtualNodeKind::ToolCard => 78.0,
        _ => 64.0,
    };
    base + line_count as f32 * 20.0
}

fn role_tag(role: VirtualAgentRole) -> u8 {
    match role {
        VirtualAgentRole::User => 1,
        VirtualAgentRole::Assistant => 2,
        VirtualAgentRole::System => 3,
        VirtualAgentRole::Tool => 4,
    }
}

fn stable_node_id(
    namespace: &str,
    session_id: &str,
    message_id: &str,
    index: u64,
    role: u8,
) -> NodeId {
    NodeId::new(stable_hash_parts(&[
        namespace.as_bytes(),
        session_id.as_bytes(),
        message_id.as_bytes(),
        &index.to_le_bytes(),
        &[role],
    ]))
}

fn stable_hash_parts(parts: &[&[u8]]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for part in parts {
        for byte in *part {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    if hash == NodeId::ROOT.0 {
        1
    } else {
        hash
    }
}
