use super::adapter::{VirtualSourceRevision, VirtualSurfaceAdapter, VirtualSurfaceBatch};
use super::protocol::{
    DirtyKind, NodeGeometry, NodeId, NodeRevision, NodeSource, NodeSourceRange,
    VirtualContentId, VirtualContentKind, VirtualContentRef, VirtualNode,
    VirtualNodeKind, VirtualSurfaceCommand, VirtualTextPlan,
};
use super::standard::VirtualSurfaceRoute;

use serde::{Deserialize, Serialize};

/// Text payload accepted by the standalone markdown virtual-surface adapter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VirtualMarkdownInput {
    Replace {
        source_id: String,
        text: String,
        revision: VirtualSourceRevision,
    },
    Append {
        source_id: String,
        text: String,
        revision: VirtualSourceRevision,
    },
    Edit {
        source_id: String,
        old_range: NodeSourceRange,
        new_range: NodeSourceRange,
        revision: VirtualSourceRevision,
        kind: DirtyKind,
    },
}

/// Basic parser stats for proof probes and integration assertions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualMarkdownStats {
    pub nodes: usize,
    pub lines: usize,
    pub headings: usize,
    pub code_blocks: usize,
    pub tables: usize,
    pub paragraphs: usize,
    pub blank_lines: usize,
}

/// Standalone markdown-to-virtual-surface adapter.
///
/// This intentionally does not depend on Neoism's current markdown editor
/// state. The real markdown pane can later provide richer block identities and
/// exact measured heights while keeping this handoff shape.
#[derive(Clone, Debug)]
pub struct VirtualMarkdownAdapter {
    namespace: String,
    next_append_index: u64,
    next_append_byte: u64,
    next_append_line: u64,
    stats: VirtualMarkdownStats,
}

impl VirtualMarkdownAdapter {
    pub fn new(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            next_append_index: 0,
            next_append_byte: 0,
            next_append_line: 0,
            stats: VirtualMarkdownStats::default(),
        }
    }

    pub fn stats(&self) -> VirtualMarkdownStats {
        self.stats
    }

    pub fn build_replace_batch(
        &mut self,
        source_id: &str,
        text: &str,
        revision: VirtualSourceRevision,
    ) -> VirtualSurfaceBatch {
        let (nodes, stats) =
            markdown_nodes(&self.namespace, source_id, text, revision, 0, 0, 0);
        self.next_append_index = nodes.len() as u64;
        self.next_append_byte = text.len() as u64;
        self.next_append_line = stats.lines as u64;
        self.stats = stats;

        let mut batch = VirtualSurfaceBatch::for_route(
            VirtualSurfaceRoute::markdown_file(source_id),
            revision,
        );
        batch.push(VirtualSurfaceCommand::ReplaceAll(nodes));
        batch
    }

    pub fn build_replace_batch_from_lines(
        &mut self,
        source_id: &str,
        lines: &[String],
        revision: VirtualSourceRevision,
    ) -> VirtualSurfaceBatch {
        let (nodes, stats) = markdown_large_semantic_nodes_from_lines(
            &self.namespace,
            source_id,
            lines,
            revision,
            0,
            0,
            0,
            0,
            lines.len(),
            32,
        );
        self.next_append_index = nodes.len() as u64;
        self.next_append_byte = joined_lines_len(lines) as u64;
        self.next_append_line = stats.lines as u64;
        self.stats = stats;

        let mut batch = VirtualSurfaceBatch::for_route(
            VirtualSurfaceRoute::markdown_file(source_id),
            revision,
        );
        batch.push(VirtualSurfaceCommand::ReplaceAll(nodes));
        batch
    }

    pub fn build_line_tail_splice_batch(
        &mut self,
        source_id: &str,
        lines: &[String],
        start_node_index: usize,
        delete_nodes: usize,
        start_line: usize,
        start_source_byte: u64,
        revision: VirtualSourceRevision,
    ) -> VirtualSurfaceBatch {
        let start_line = start_line.min(lines.len().saturating_sub(1));
        let (nodes, stats) = markdown_large_semantic_nodes_from_lines(
            &self.namespace,
            source_id,
            lines,
            revision,
            start_node_index as u64,
            start_source_byte,
            start_line as u64,
            start_line,
            lines.len(),
            32,
        );
        self.next_append_index = self
            .next_append_index
            .max(start_node_index.saturating_add(nodes.len()) as u64);
        self.next_append_byte = joined_lines_len(lines) as u64;
        self.next_append_line = self
            .next_append_line
            .max(start_line as u64 + stats.lines as u64);

        let mut batch = VirtualSurfaceBatch::for_route(
            VirtualSurfaceRoute::markdown_file(source_id),
            revision,
        );
        batch.push(VirtualSurfaceCommand::SpliceNodes {
            start: start_node_index,
            delete: delete_nodes,
            insert: nodes,
        });
        batch
    }

    pub fn build_append_batch(
        &mut self,
        source_id: &str,
        text: &str,
        revision: VirtualSourceRevision,
    ) -> VirtualSurfaceBatch {
        self.build_append_batch_at(
            source_id,
            text,
            self.next_append_byte,
            self.next_append_line,
            revision,
        )
    }

    pub fn build_append_batch_at(
        &mut self,
        source_id: &str,
        text: &str,
        source_start_byte: u64,
        source_start_line: u64,
        revision: VirtualSourceRevision,
    ) -> VirtualSurfaceBatch {
        let base_index = self.next_append_index;
        let (nodes, stats) = markdown_nodes(
            &self.namespace,
            source_id,
            text,
            revision,
            base_index,
            source_start_byte,
            source_start_line,
        );
        self.next_append_index =
            self.next_append_index.saturating_add(nodes.len() as u64);
        self.next_append_byte = self
            .next_append_byte
            .max(source_start_byte.saturating_add(text.len() as u64));
        self.next_append_line = self
            .next_append_line
            .max(source_start_line.saturating_add(stats.lines as u64));
        self.stats.nodes = self.stats.nodes.saturating_add(stats.nodes);
        self.stats.lines = self.stats.lines.saturating_add(stats.lines);
        self.stats.headings = self.stats.headings.saturating_add(stats.headings);
        self.stats.code_blocks = self.stats.code_blocks.saturating_add(stats.code_blocks);
        self.stats.tables = self.stats.tables.saturating_add(stats.tables);
        self.stats.paragraphs = self.stats.paragraphs.saturating_add(stats.paragraphs);
        self.stats.blank_lines = self.stats.blank_lines.saturating_add(stats.blank_lines);

        let mut batch = VirtualSurfaceBatch::for_route(
            VirtualSurfaceRoute::markdown_file(source_id),
            revision,
        );
        batch.push(VirtualSurfaceCommand::UpsertNodes(nodes));
        batch
    }

    pub fn build_tail_update_batch_at(
        &mut self,
        source_id: &str,
        source_text: &str,
        node_index: u64,
        source_start_byte: u64,
        source_start_line: u64,
        kind: VirtualNodeKind,
        revision: VirtualSourceRevision,
    ) -> VirtualSurfaceBatch {
        let source_start = source_start_byte.min(source_text.len() as u64);
        let tail = source_text.get(source_start as usize..).unwrap_or_default();
        let line_count = tail.lines().count().max(1);
        let mut stats = VirtualMarkdownStats::default();
        let height = match kind {
            VirtualNodeKind::Heading => heading_level(tail.lines().next().unwrap_or(""))
                .map(|level| (38.0 - level as f32 * 2.0).max(24.0))
                .unwrap_or(34.0),
            VirtualNodeKind::CodeBlock => (line_count as f32 * 22.0 + 24.0).max(64.0),
            VirtualNodeKind::Table => (line_count as f32 * 30.0 + 28.0).max(72.0),
            _ => {
                tail.lines()
                    .map(|line| large_markdown_line_visual_height(line, &mut stats))
                    .sum::<f32>()
                    + 10.0
            }
        };
        let mut geometry = NodeGeometry::fixed(height);
        geometry.can_split = line_count > 16;

        let mut nodes = Vec::with_capacity(1);
        push_node(
            &mut nodes,
            source_text,
            0,
            &self.namespace,
            source_id,
            revision,
            node_index,
            kind,
            geometry,
            source_start_byte,
            source_text.len() as u64,
            source_start_line,
            line_count as u32,
        );

        self.next_append_index = self.next_append_index.max(node_index.saturating_add(1));
        self.next_append_byte = self.next_append_byte.max(source_text.len() as u64);
        self.next_append_line = self
            .next_append_line
            .max(source_start_line.saturating_add(line_count as u64));

        let mut batch = VirtualSurfaceBatch::for_route(
            VirtualSurfaceRoute::markdown_file(source_id),
            revision,
        );
        batch.push(VirtualSurfaceCommand::UpsertNodes(nodes));
        batch
    }

    pub fn build_line_node_update_batch(
        &mut self,
        source_id: &str,
        lines: &[String],
        node_index: u64,
        line_start: usize,
        line_count: usize,
        source_start_byte: u64,
        source_end_byte: u64,
        kind: VirtualNodeKind,
        revision: VirtualSourceRevision,
    ) -> VirtualSurfaceBatch {
        let line_start = line_start.min(lines.len().saturating_sub(1));
        let line_end = line_start
            .saturating_add(line_count.max(1))
            .min(lines.len())
            .max(line_start.saturating_add(1).min(lines.len()));
        let line_count = line_end.saturating_sub(line_start).max(1);
        let mut stats = VirtualMarkdownStats::default();
        let height = lines[line_start..line_end]
            .iter()
            .map(|line| large_markdown_line_visual_height(line, &mut stats))
            .sum::<f32>()
            + 10.0;
        let mut geometry = NodeGeometry::fixed(height);
        geometry.can_split = line_count > 16;
        let mut nodes = Vec::with_capacity(1);
        push_node_from_lines(
            &mut nodes,
            lines,
            &self.namespace,
            source_id,
            revision,
            node_index,
            kind,
            geometry,
            source_start_byte,
            source_end_byte.max(source_start_byte),
            line_start as u64,
            line_count as u32,
        );

        let mut batch = VirtualSurfaceBatch::for_route(
            VirtualSurfaceRoute::markdown_file(source_id),
            revision,
        );
        batch.push(VirtualSurfaceCommand::UpsertNodes(nodes));
        batch
    }

    pub fn build_existing_line_node_update_batch(
        &mut self,
        source_id: &str,
        lines: &[String],
        node_id: NodeId,
        node_index: u64,
        line_start: usize,
        line_count: usize,
        source_start_byte: u64,
        source_end_byte: u64,
        kind: VirtualNodeKind,
        revision: VirtualSourceRevision,
    ) -> VirtualSurfaceBatch {
        let mut batch = self.build_line_node_update_batch(
            source_id,
            lines,
            node_index,
            line_start,
            line_count,
            source_start_byte,
            source_end_byte,
            kind,
            revision,
        );
        for command in &mut batch.commands {
            if let VirtualSurfaceCommand::UpsertNodes(nodes) = command {
                if let Some(node) = nodes.first_mut() {
                    node.id = node_id;
                    if let Some(text_plan) = node.text_plan.as_mut() {
                        text_plan.content.id = VirtualContentId(node_id.0);
                    }
                    if let Some(content) = node.content.as_mut() {
                        content.id = VirtualContentId(node_id.0);
                    }
                }
            }
        }
        batch
    }

    pub fn build_edit_batch(
        &mut self,
        source_id: &str,
        old_range: NodeSourceRange,
        new_range: NodeSourceRange,
        revision: VirtualSourceRevision,
        kind: DirtyKind,
    ) -> VirtualSurfaceBatch {
        let mut batch = VirtualSurfaceBatch::for_route(
            VirtualSurfaceRoute::markdown_file(source_id),
            revision,
        );
        batch.push_source_edit(markdown_source(source_id), old_range, new_range, kind);
        batch
    }
}

impl VirtualSurfaceAdapter for VirtualMarkdownAdapter {
    type Input = VirtualMarkdownInput;
    type Error = std::convert::Infallible;

    fn build_initial(
        &mut self,
        input: Self::Input,
    ) -> Result<VirtualSurfaceBatch, Self::Error> {
        Ok(match input {
            VirtualMarkdownInput::Replace {
                source_id,
                text,
                revision,
            }
            | VirtualMarkdownInput::Append {
                source_id,
                text,
                revision,
            } => self.build_replace_batch(&source_id, &text, revision),
            VirtualMarkdownInput::Edit {
                source_id,
                old_range,
                new_range,
                revision,
                kind,
            } => self.build_edit_batch(&source_id, old_range, new_range, revision, kind),
        })
    }

    fn update(&mut self, input: Self::Input) -> Result<VirtualSurfaceBatch, Self::Error> {
        Ok(match input {
            VirtualMarkdownInput::Replace {
                source_id,
                text,
                revision,
            } => self.build_replace_batch(&source_id, &text, revision),
            VirtualMarkdownInput::Append {
                source_id,
                text,
                revision,
            } => self.build_append_batch(&source_id, &text, revision),
            VirtualMarkdownInput::Edit {
                source_id,
                old_range,
                new_range,
                revision,
                kind,
            } => self.build_edit_batch(&source_id, old_range, new_range, revision, kind),
        })
    }
}

fn markdown_nodes(
    namespace: &str,
    source_id: &str,
    text: &str,
    revision: VirtualSourceRevision,
    base_index: u64,
    base_byte: u64,
    base_line: u64,
) -> (Vec<VirtualNode>, VirtualMarkdownStats) {
    const PARAGRAPH_CHUNK_LINES: usize = 64;
    const LARGE_MARKDOWN_FAST_BYTES: usize = 2 * 1024 * 1024;
    const LARGE_MARKDOWN_CHUNK_LINES: usize = 32;

    if text.len() > LARGE_MARKDOWN_FAST_BYTES {
        return markdown_large_semantic_nodes(
            namespace,
            source_id,
            text,
            revision,
            base_index,
            base_byte,
            base_line,
            LARGE_MARKDOWN_CHUNK_LINES,
        );
    }

    let mut nodes = Vec::new();
    let mut stats = VirtualMarkdownStats::default();
    let mut lines = MarkdownLineIter::new(text).peekable();

    while let Some(current) = lines.next() {
        stats.lines += 1;
        let current_line = base_line + current.line_index as u64;
        if is_notebook_cell_marker(current.line) {
            push_node(
                &mut nodes,
                text,
                base_byte,
                namespace,
                source_id,
                revision,
                base_index,
                VirtualNodeKind::Custom("notebook-cell".to_string()),
                NodeGeometry::fixed(28.0),
                base_byte + current.start as u64,
                base_byte + current.end as u64,
                current_line,
                1,
            );
            continue;
        }
        if current.line.trim().is_empty() {
            stats.blank_lines += 1;
            push_node(
                &mut nodes,
                text,
                base_byte,
                namespace,
                source_id,
                revision,
                base_index,
                VirtualNodeKind::MarkdownBlock,
                NodeGeometry::fixed(18.0),
                base_byte + current.start as u64,
                base_byte + current.end as u64,
                current_line,
                1,
            );
            continue;
        }

        if let Some(level) = heading_level(current.line) {
            stats.headings += 1;
            push_node(
                &mut nodes,
                text,
                base_byte,
                namespace,
                source_id,
                revision,
                base_index,
                VirtualNodeKind::Heading,
                NodeGeometry::fixed((38.0 - level as f32 * 2.0).max(24.0)),
                base_byte + current.start as u64,
                base_byte + current.end as u64,
                current_line,
                1,
            );
            continue;
        }

        if current.line.trim_start().starts_with("```") {
            let block_start = current.start;
            let mut block_end = current.end;
            let mut block_lines = 1usize;
            for code in lines.by_ref() {
                stats.lines += 1;
                block_end = code.end;
                block_lines += 1;
                if code.line.trim_start().starts_with("```") {
                    break;
                }
            }
            stats.code_blocks += 1;
            let mut geometry =
                NodeGeometry::fixed((block_lines as f32 * 22.0 + 24.0).max(64.0));
            geometry.can_split = block_lines > 32;
            push_node(
                &mut nodes,
                text,
                base_byte,
                namespace,
                source_id,
                revision,
                base_index,
                VirtualNodeKind::CodeBlock,
                geometry,
                base_byte + block_start as u64,
                base_byte + block_end as u64,
                current_line,
                block_lines as u32,
            );
            continue;
        }

        if looks_like_table(current.line) {
            let table_start = current.start;
            let mut table_end = current.end;
            let mut table_lines = 1usize;
            while let Some(next) = lines.peek().copied() {
                if !looks_like_table(next.line) {
                    break;
                }
                let table = lines.next().expect("peeked table line should exist");
                stats.lines += 1;
                table_end = table.end;
                table_lines += 1;
            }
            stats.tables += 1;
            let mut geometry =
                NodeGeometry::fixed((table_lines as f32 * 30.0 + 28.0).max(72.0));
            geometry.can_split = table_lines > 24;
            push_node(
                &mut nodes,
                text,
                base_byte,
                namespace,
                source_id,
                revision,
                base_index,
                VirtualNodeKind::Table,
                geometry,
                base_byte + table_start as u64,
                base_byte + table_end as u64,
                current_line,
                table_lines as u32,
            );
            continue;
        }

        let chunk_start = current.start;
        let mut chunk_end = current.end;
        let mut chunk_lines = 1usize;
        let mut visual_lines = (current.line.len() / 88).max(1) as f32;
        while chunk_lines < PARAGRAPH_CHUNK_LINES {
            let Some(next) = lines.peek().copied() else {
                break;
            };
            if is_markdown_block_boundary(next.line) {
                break;
            }
            let paragraph = lines.next().expect("peeked paragraph line should exist");
            stats.lines += 1;
            chunk_end = paragraph.end;
            chunk_lines += 1;
            visual_lines += (paragraph.line.len() / 88).max(1) as f32;
        }

        stats.paragraphs += chunk_lines;
        let mut geometry = NodeGeometry::fixed(visual_lines * 24.0 + 10.0);
        geometry.can_split = chunk_lines > 32;
        push_node(
            &mut nodes,
            text,
            base_byte,
            namespace,
            source_id,
            revision,
            base_index,
            VirtualNodeKind::MarkdownBlock,
            geometry,
            base_byte + chunk_start as u64,
            base_byte + chunk_end as u64,
            current_line,
            chunk_lines as u32,
        );
    }

    stats.nodes = nodes.len();
    (nodes, stats)
}

fn markdown_large_semantic_nodes(
    namespace: &str,
    source_id: &str,
    text: &str,
    revision: VirtualSourceRevision,
    base_index: u64,
    base_byte: u64,
    base_line: u64,
    chunk_size: usize,
) -> (Vec<VirtualNode>, VirtualMarkdownStats) {
    let mut nodes = Vec::new();
    let mut stats = VirtualMarkdownStats::default();
    let mut lines = MarkdownLineIter::new(text).peekable();

    while let Some(first) = lines.next() {
        let chunk_start = first.start;
        let mut chunk_end = first.end;
        let chunk_line_start = base_line + first.line_index as u64;
        if is_notebook_cell_marker(first.line) {
            stats.lines += 1;
            push_node(
                &mut nodes,
                text,
                base_byte,
                namespace,
                source_id,
                revision,
                base_index,
                VirtualNodeKind::Custom("notebook-cell".to_string()),
                NodeGeometry::fixed(28.0),
                base_byte + chunk_start as u64,
                base_byte + chunk_end as u64,
                chunk_line_start,
                1,
            );
            continue;
        }
        let mut chunk_lines = 1usize;
        let mut visual_lines = large_markdown_line_visual_height(first.line, &mut stats);
        let first_is_heading = heading_level(first.line).is_some();

        while chunk_lines < chunk_size {
            let Some(next) = lines.peek().copied() else {
                break;
            };
            if is_notebook_cell_anchor(next.line) {
                break;
            }
            if chunk_lines > 1 && heading_level(next.line).is_some() {
                break;
            }
            if !first_is_heading && chunk_lines >= 8 && heading_level(next.line).is_some()
            {
                break;
            }
            let paragraph = lines.next().expect("peeked markdown line should exist");
            chunk_end = paragraph.end;
            chunk_lines += 1;
            visual_lines += large_markdown_line_visual_height(paragraph.line, &mut stats);
        }

        let mut geometry = NodeGeometry::fixed(visual_lines + 10.0);
        geometry.can_split = chunk_lines > 16;
        push_node(
            &mut nodes,
            text,
            base_byte,
            namespace,
            source_id,
            revision,
            base_index,
            VirtualNodeKind::MarkdownBlock,
            geometry,
            base_byte + chunk_start as u64,
            base_byte + chunk_end as u64,
            chunk_line_start,
            chunk_lines as u32,
        );
    }

    stats.nodes = nodes.len();
    (nodes, stats)
}

fn markdown_large_semantic_nodes_from_lines(
    namespace: &str,
    source_id: &str,
    lines: &[String],
    revision: VirtualSourceRevision,
    base_index: u64,
    base_byte: u64,
    base_line: u64,
    start_line: usize,
    end_line: usize,
    chunk_size: usize,
) -> (Vec<VirtualNode>, VirtualMarkdownStats) {
    let mut nodes = Vec::new();
    let mut stats = VirtualMarkdownStats::default();
    if lines.is_empty() {
        let mut geometry = NodeGeometry::fixed(18.0);
        geometry.can_split = false;
        push_node_from_lines(
            &mut nodes,
            lines,
            namespace,
            source_id,
            revision,
            base_index,
            VirtualNodeKind::MarkdownBlock,
            geometry,
            base_byte,
            base_byte,
            base_line,
            1,
        );
        stats.nodes = 1;
        stats.lines = 1;
        stats.blank_lines = 1;
        return (nodes, stats);
    }

    let start_line = start_line.min(lines.len());
    let end_line = end_line.min(lines.len()).max(start_line);
    let mut line_ix = start_line;
    let mut source_cursor = base_byte;
    while line_ix < end_line {
        let chunk_start_line = line_ix;
        let chunk_start_byte = source_cursor;
        let first = lines[line_ix].as_str();
        if is_notebook_cell_marker(first) {
            stats.lines += 1;
            source_cursor = source_cursor
                .saturating_add(first.len() as u64)
                .saturating_add(if line_ix + 1 < lines.len() { 1 } else { 0 });
            line_ix += 1;
            push_node_from_lines(
                &mut nodes,
                lines,
                namespace,
                source_id,
                revision,
                base_index,
                VirtualNodeKind::Custom("notebook-cell".to_string()),
                NodeGeometry::fixed(28.0),
                chunk_start_byte,
                source_cursor,
                base_line + chunk_start_line.saturating_sub(start_line) as u64,
                1,
            );
            continue;
        }
        let mut chunk_lines = 1usize;
        let mut visual_lines = large_markdown_line_visual_height(first, &mut stats);
        let first_is_heading = heading_level(first).is_some();
        source_cursor = source_cursor
            .saturating_add(first.len() as u64)
            .saturating_add(if line_ix + 1 < lines.len() { 1 } else { 0 });
        line_ix += 1;

        while chunk_lines < chunk_size && line_ix < end_line {
            let next = lines[line_ix].as_str();
            if is_notebook_cell_anchor(next) {
                break;
            }
            if chunk_lines > 1 && heading_level(next).is_some() {
                break;
            }
            if !first_is_heading && chunk_lines >= 8 && heading_level(next).is_some() {
                break;
            }
            visual_lines += large_markdown_line_visual_height(next, &mut stats);
            chunk_lines += 1;
            source_cursor = source_cursor
                .saturating_add(next.len() as u64)
                .saturating_add(if line_ix + 1 < lines.len() { 1 } else { 0 });
            line_ix += 1;
        }

        let mut geometry = NodeGeometry::fixed(visual_lines + 10.0);
        geometry.can_split = chunk_lines > 16;
        push_node_from_lines(
            &mut nodes,
            lines,
            namespace,
            source_id,
            revision,
            base_index,
            VirtualNodeKind::MarkdownBlock,
            geometry,
            chunk_start_byte,
            source_cursor,
            base_line + chunk_start_line.saturating_sub(start_line) as u64,
            chunk_lines as u32,
        );
    }

    stats.nodes = nodes.len();
    (nodes, stats)
}

fn large_markdown_line_visual_height(
    line: &str,
    stats: &mut VirtualMarkdownStats,
) -> f32 {
    stats.lines += 1;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        stats.blank_lines += 1;
        return 18.0;
    }
    if heading_level(line).is_some() {
        stats.headings += 1;
        return 34.0;
    }
    if line.trim_start().starts_with("```") {
        stats.code_blocks += 1;
        return 24.0;
    }
    if looks_like_table(line) {
        stats.tables += 1;
        return 30.0;
    }
    stats.paragraphs += 1;
    ((line.len() / 88).max(1) as f32) * 24.0
}

fn is_markdown_block_boundary(line: &str) -> bool {
    is_notebook_cell_anchor(line)
        || line.trim().is_empty()
        || heading_level(line).is_some()
        || line.trim_start().starts_with("```")
        || looks_like_table(line)
        || looks_like_list_marker(line)
}

fn is_notebook_cell_anchor(line: &str) -> bool {
    is_notebook_cell_marker(line)
        || (line.trim_start().starts_with("```")
            && line
                .split_whitespace()
                .any(|part| part.starts_with("neoism_notebook_cell=")))
}

fn is_notebook_cell_marker(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("%%neoism_notebook_cell ") else {
        return false;
    };
    rest.split_whitespace()
        .next()
        .is_some_and(|index| index.parse::<usize>().is_ok())
}

fn looks_like_list_marker(line: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(first) = trimmed.chars().next() else {
        return false;
    };
    match first {
        '-' | '*' | '+' => trimmed
            .get(first.len_utf8()..)
            .is_some_and(|rest| rest.starts_with(' ') || rest.starts_with('\t')),
        ch if ch.is_ascii_digit() => {
            let digits = trimmed
                .bytes()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            trimmed
                .get(digits..)
                .is_some_and(|rest| rest.starts_with(". ") || rest.starts_with(") "))
        }
        _ => false,
    }
}

#[derive(Clone, Copy, Debug)]
struct MarkdownSourceLine<'a> {
    line: &'a str,
    start: usize,
    end: usize,
    line_index: usize,
}

struct MarkdownLineIter<'a> {
    text: &'a str,
    offset: usize,
    line_index: usize,
}

impl<'a> MarkdownLineIter<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            offset: 0,
            line_index: 0,
        }
    }
}

impl<'a> Iterator for MarkdownLineIter<'a> {
    type Item = MarkdownSourceLine<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.text.len() {
            return None;
        }
        let start = self.offset;
        let remaining = &self.text[start..];
        let len = remaining
            .find('\n')
            .map(|newline| newline + 1)
            .unwrap_or(remaining.len());
        let end = start + len;
        self.offset = end;
        let raw = &self.text[start..end];
        let line_index = self.line_index;
        self.line_index += 1;
        Some(MarkdownSourceLine {
            line: raw.trim_end_matches(['\r', '\n']),
            start,
            end,
            line_index,
        })
    }
}

fn push_node(
    nodes: &mut Vec<VirtualNode>,
    source_text: &str,
    source_base_byte: u64,
    namespace: &str,
    source_id: &str,
    revision: VirtualSourceRevision,
    base_index: u64,
    kind: VirtualNodeKind,
    geometry: NodeGeometry,
    source_start: u64,
    source_end: u64,
    line_start: u64,
    line_count: u32,
) {
    let index = nodes.len() as u64;
    let id = stable_node_id(namespace, source_id, base_index + index, &kind);
    let source = markdown_source(source_id);
    let relative_start = source_start.saturating_sub(source_base_byte) as usize;
    let relative_end = source_end.saturating_sub(source_base_byte) as usize;
    let content_bytes = source_text
        .get(relative_start.min(source_text.len())..relative_end.min(source_text.len()))
        .map(str::as_bytes)
        .unwrap_or_default();
    let text_hash = stable_hash_parts(&[&[kind_tag(&kind)], content_bytes]);
    let content_id = stable_hash_parts(&[
        source_id.as_bytes(),
        &source_start.to_le_bytes(),
        &source_end.to_le_bytes(),
        &index.to_le_bytes(),
        &text_hash.to_le_bytes(),
    ]);
    let source_range = NodeSourceRange::new(source_start, source_end);
    let content = VirtualContentRef::new(
        VirtualContentId(content_id),
        content_kind_for_node(&kind),
        source.clone(),
        source_range,
        NodeRevision(revision.0),
        text_hash,
        line_count,
    )
    .with_line_start(line_start);
    nodes.push({
        let text_plan = VirtualTextPlan::new(content.clone());
        VirtualNode::new(id, kind)
            .with_geometry(geometry)
            .with_revision(text_hash)
            .with_text_hash(text_hash)
            .with_source(source, source_range)
            .with_content(content)
            .with_text_plan(text_plan)
    });
}

fn push_node_from_lines(
    nodes: &mut Vec<VirtualNode>,
    lines: &[String],
    namespace: &str,
    source_id: &str,
    revision: VirtualSourceRevision,
    base_index: u64,
    kind: VirtualNodeKind,
    geometry: NodeGeometry,
    source_start: u64,
    source_end: u64,
    line_start: u64,
    line_count: u32,
) {
    let index = nodes.len() as u64;
    let id = stable_node_id(namespace, source_id, base_index + index, &kind);
    let source = markdown_source(source_id);
    let text_hash = stable_hash_line_range(
        &kind,
        lines,
        line_start as usize,
        line_start as usize + line_count as usize,
    );
    let content_id = stable_hash_parts(&[
        source_id.as_bytes(),
        &source_start.to_le_bytes(),
        &source_end.to_le_bytes(),
        &index.to_le_bytes(),
        &text_hash.to_le_bytes(),
    ]);
    let source_range = NodeSourceRange::new(source_start, source_end);
    let content = VirtualContentRef::new(
        VirtualContentId(content_id),
        content_kind_for_node(&kind),
        source.clone(),
        source_range,
        NodeRevision(revision.0),
        text_hash,
        line_count,
    )
    .with_line_start(line_start);
    let text_plan = VirtualTextPlan::new(content.clone());
    nodes.push(
        VirtualNode::new(id, kind)
            .with_geometry(geometry)
            .with_revision(text_hash)
            .with_text_hash(text_hash)
            .with_source(source, source_range)
            .with_content(content)
            .with_text_plan(text_plan),
    );
}

fn joined_lines_len(lines: &[String]) -> usize {
    if lines.is_empty() {
        return 0;
    }
    lines.iter().map(String::len).sum::<usize>() + lines.len().saturating_sub(1)
}

fn markdown_source(source_id: &str) -> NodeSource {
    if source_id.contains('/') || source_id.contains('.') {
        NodeSource::File {
            path: source_id.to_string(),
        }
    } else {
        NodeSource::Synthetic {
            namespace: source_id.to_string(),
        }
    }
}

fn content_kind_for_node(kind: &VirtualNodeKind) -> VirtualContentKind {
    match kind {
        VirtualNodeKind::CodeBlock => VirtualContentKind::Code { language: None },
        VirtualNodeKind::Table => VirtualContentKind::Table,
        VirtualNodeKind::Heading | VirtualNodeKind::MarkdownBlock => {
            VirtualContentKind::Markdown
        }
        _ => VirtualContentKind::PlainText,
    }
}

fn heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let level = trimmed.chars().take_while(|ch| *ch == '#').count();
    if (1..=6).contains(&level) && trimmed.as_bytes().get(level) == Some(&b' ') {
        Some(level)
    } else {
        None
    }
}

fn looks_like_table(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.contains('|') && trimmed.matches('|').count() >= 2
}

fn stable_node_id(
    namespace: &str,
    source_id: &str,
    index: u64,
    kind: &VirtualNodeKind,
) -> NodeId {
    NodeId::new(stable_hash_parts(&[
        namespace.as_bytes(),
        source_id.as_bytes(),
        &index.to_le_bytes(),
        &[kind_tag(kind)],
    ]))
}

fn kind_tag(kind: &VirtualNodeKind) -> u8 {
    match kind {
        VirtualNodeKind::Heading => 1,
        VirtualNodeKind::CodeBlock => 2,
        VirtualNodeKind::Table => 3,
        VirtualNodeKind::MarkdownBlock => 4,
        VirtualNodeKind::Custom(name) if name == "notebook-cell" => 5,
        _ => 255,
    }
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

fn stable_hash_line_range(
    kind: &VirtualNodeKind,
    lines: &[String],
    start_line: usize,
    end_line: usize,
) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in [kind_tag(kind)] {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash ^= 0xff;
    hash = hash.wrapping_mul(0x100000001b3);

    let end_line = end_line.min(lines.len());
    for (ix, line) in lines
        .iter()
        .enumerate()
        .take(end_line)
        .skip(start_line.min(end_line))
    {
        for byte in line.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        if ix + 1 < end_line {
            hash ^= u64::from(b'\n');
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
