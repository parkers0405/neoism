// O code that weaves through silicon veins,
// each node a verse, each surface a refrain.
use super::adapter::{VirtualSourceRevision, VirtualSurfaceAdapter, VirtualSurfaceBatch};
use super::content::VirtualTextLineIndex;
use super::protocol::{
    DirtyKind, NodeGeometry, NodeId, NodeRevision, NodeSource, NodeSourceRange,
    VirtualContentId, VirtualContentKind, VirtualContentRef, VirtualNode,
    VirtualNodeKind, VirtualSurfaceCommand, VirtualTextPlan, VirtualTextWrap,
};
use super::standard::VirtualSurfaceRoute;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VirtualCodeInput {
    Replace {
        buffer_id: String,
        text: String,
        revision: VirtualSourceRevision,
    },
    Append {
        buffer_id: String,
        text: String,
        revision: VirtualSourceRevision,
    },
    Edit {
        buffer_id: String,
        old_range: NodeSourceRange,
        new_range: NodeSourceRange,
        revision: VirtualSourceRevision,
        kind: DirtyKind,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualCodeAdapterConfig {
    pub tile_lines: usize,
    pub line_height_px: u16,
    pub glyph_width_px: u16,
    pub index_checkpoint_lines: u32,
}

impl Default for VirtualCodeAdapterConfig {
    fn default() -> Self {
        Self {
            tile_lines: 256,
            line_height_px: 20,
            glyph_width_px: 8,
            index_checkpoint_lines: 512,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualCodeStats {
    pub lines: usize,
    pub tiles: usize,
    pub bytes: usize,
    pub max_line_bytes: usize,
    pub max_tile_bytes: usize,
    pub trailing_newline: bool,
}

#[derive(Clone, Debug)]
pub struct VirtualCodeAdapter {
    namespace: String,
    config: VirtualCodeAdapterConfig,
    next_append_line: u64,
    next_append_byte: u64,
    stats: VirtualCodeStats,
}

impl VirtualCodeAdapter {
    pub fn new(namespace: impl Into<String>) -> Self {
        Self::with_config(namespace, VirtualCodeAdapterConfig::default())
    }

    pub fn with_config(
        namespace: impl Into<String>,
        config: VirtualCodeAdapterConfig,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            config,
            next_append_line: 0,
            next_append_byte: 0,
            stats: VirtualCodeStats::default(),
        }
    }

    pub fn stats(&self) -> VirtualCodeStats {
        self.stats
    }

    pub fn build_line_index(&self, text: &str) -> VirtualTextLineIndex {
        VirtualTextLineIndex::with_checkpoint_lines(
            text,
            self.config.index_checkpoint_lines,
        )
    }

    pub fn build_replace_batch(
        &mut self,
        buffer_id: &str,
        text: &str,
        revision: VirtualSourceRevision,
    ) -> VirtualSurfaceBatch {
        let (nodes, stats) = code_nodes(
            &self.namespace,
            buffer_id,
            text,
            revision,
            0,
            0,
            self.config,
        );
        self.next_append_line = stats.lines as u64;
        self.next_append_byte = text.len() as u64;
        self.stats = stats;

        let mut batch = VirtualSurfaceBatch::for_route(
            VirtualSurfaceRoute::code_buffer(buffer_id),
            revision,
        );
        batch.push(VirtualSurfaceCommand::ReplaceAll(nodes));
        batch
    }

    pub fn build_append_batch(
        &mut self,
        buffer_id: &str,
        text: &str,
        revision: VirtualSourceRevision,
    ) -> VirtualSurfaceBatch {
        let (nodes, stats) = code_nodes(
            &self.namespace,
            buffer_id,
            text,
            revision,
            self.next_append_line,
            self.next_append_byte,
            self.config,
        );
        self.next_append_line = self.next_append_line.saturating_add(stats.lines as u64);
        self.next_append_byte = self.next_append_byte.saturating_add(text.len() as u64);
        self.stats.lines = self.stats.lines.saturating_add(stats.lines);
        self.stats.tiles = self.stats.tiles.saturating_add(stats.tiles);
        self.stats.bytes = self.stats.bytes.saturating_add(stats.bytes);
        self.stats.max_line_bytes = self.stats.max_line_bytes.max(stats.max_line_bytes);
        self.stats.max_tile_bytes = self.stats.max_tile_bytes.max(stats.max_tile_bytes);
        self.stats.trailing_newline = stats.trailing_newline;

        let mut batch = VirtualSurfaceBatch::for_route(
            VirtualSurfaceRoute::code_buffer(buffer_id),
            revision,
        );
        batch.push(VirtualSurfaceCommand::UpsertNodes(nodes));
        batch
    }

    pub fn build_edit_batch(
        &mut self,
        buffer_id: &str,
        old_range: NodeSourceRange,
        new_range: NodeSourceRange,
        revision: VirtualSourceRevision,
        kind: DirtyKind,
    ) -> VirtualSurfaceBatch {
        let mut batch = VirtualSurfaceBatch::for_route(
            VirtualSurfaceRoute::code_buffer(buffer_id),
            revision,
        );
        batch.push_source_edit(
            NodeSource::CodeBuffer {
                buffer: buffer_id.to_string(),
            },
            old_range,
            new_range,
            kind,
        );
        batch
    }
}

impl VirtualSurfaceAdapter for VirtualCodeAdapter {
    type Input = VirtualCodeInput;
    type Error = std::convert::Infallible;

    fn build_initial(
        &mut self,
        input: Self::Input,
    ) -> Result<VirtualSurfaceBatch, Self::Error> {
        Ok(match input {
            VirtualCodeInput::Replace {
                buffer_id,
                text,
                revision,
            }
            | VirtualCodeInput::Append {
                buffer_id,
                text,
                revision,
            } => self.build_replace_batch(&buffer_id, &text, revision),
            VirtualCodeInput::Edit {
                buffer_id,
                old_range,
                new_range,
                revision,
                kind,
            } => self.build_edit_batch(&buffer_id, old_range, new_range, revision, kind),
        })
    }

    fn update(&mut self, input: Self::Input) -> Result<VirtualSurfaceBatch, Self::Error> {
        Ok(match input {
            VirtualCodeInput::Replace {
                buffer_id,
                text,
                revision,
            } => self.build_replace_batch(&buffer_id, &text, revision),
            VirtualCodeInput::Append {
                buffer_id,
                text,
                revision,
            } => self.build_append_batch(&buffer_id, &text, revision),
            VirtualCodeInput::Edit {
                buffer_id,
                old_range,
                new_range,
                revision,
                kind,
            } => self.build_edit_batch(&buffer_id, old_range, new_range, revision, kind),
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct PendingCodeTile {
    start_line: usize,
    start_byte: usize,
    line_count: usize,
    byte_len: usize,
    max_line_bytes: usize,
}

impl PendingCodeTile {
    fn new(start_line: usize, start_byte: usize) -> Self {
        Self {
            start_line,
            start_byte,
            line_count: 0,
            byte_len: 0,
            max_line_bytes: 0,
        }
    }

    fn push_line(&mut self, line: &str) {
        self.line_count += 1;
        self.byte_len += line.len();
        self.max_line_bytes = self.max_line_bytes.max(line.len());
    }

    fn is_empty(self) -> bool {
        self.line_count == 0
    }

    fn end_byte(self) -> usize {
        self.start_byte + self.byte_len
    }
}

fn code_nodes(
    namespace: &str,
    buffer_id: &str,
    text: &str,
    revision: VirtualSourceRevision,
    base_line: u64,
    base_byte: u64,
    config: VirtualCodeAdapterConfig,
) -> (Vec<VirtualNode>, VirtualCodeStats) {
    let tile_lines = config.tile_lines.max(1);
    let mut nodes = Vec::new();
    let mut stats = VirtualCodeStats {
        bytes: text.len(),
        trailing_newline: text.ends_with('\n'),
        ..VirtualCodeStats::default()
    };
    let mut byte_cursor = 0usize;
    let mut line_cursor = 0usize;
    let mut tile = PendingCodeTile::new(0, 0);

    for line in text.split_inclusive('\n') {
        if tile.is_empty() {
            tile = PendingCodeTile::new(line_cursor, byte_cursor);
        }
        tile.push_line(line);
        byte_cursor += line.len();
        line_cursor += 1;

        stats.lines += 1;
        stats.max_line_bytes = stats.max_line_bytes.max(line.len());

        if tile.line_count == tile_lines {
            push_code_tile(
                &mut nodes, text, namespace, buffer_id, revision, base_line, base_byte,
                config, tile,
            );
            stats.tiles += 1;
            stats.max_tile_bytes = stats.max_tile_bytes.max(tile.byte_len);
            tile = PendingCodeTile::new(line_cursor, byte_cursor);
        }
    }

    if !tile.is_empty() {
        push_code_tile(
            &mut nodes, text, namespace, buffer_id, revision, base_line, base_byte,
            config, tile,
        );
        stats.tiles += 1;
        stats.max_tile_bytes = stats.max_tile_bytes.max(tile.byte_len);
    }

    (nodes, stats)
}

fn push_code_tile(
    nodes: &mut Vec<VirtualNode>,
    source_text: &str,
    namespace: &str,
    buffer_id: &str,
    revision: VirtualSourceRevision,
    base_line: u64,
    base_byte: u64,
    config: VirtualCodeAdapterConfig,
    tile: PendingCodeTile,
) {
    let mut geometry =
        NodeGeometry::fixed(tile.line_count as f32 * f32::from(config.line_height_px));
    geometry.can_split = tile.line_count > 64;
    geometry.min_width =
        tile.max_line_bytes as f32 * f32::from(config.glyph_width_px.max(1));
    let kind = if tile.line_count == 1 {
        VirtualNodeKind::CodeLine
    } else {
        VirtualNodeKind::CodeTile
    };
    let absolute_line = base_line + tile.start_line as u64;
    let absolute_start = base_byte + tile.start_byte as u64;
    let absolute_end = base_byte + tile.end_byte() as u64;
    let tile_text = source_text
        .get(
            tile.start_byte.min(source_text.len())
                ..tile.end_byte().min(source_text.len()),
        )
        .map(str::as_bytes)
        .unwrap_or_default();
    let text_hash = stable_hash_parts(&[&[kind_tag(&kind)], tile_text]);
    let content_id = stable_hash_parts(&[
        buffer_id.as_bytes(),
        &absolute_line.to_le_bytes(),
        &absolute_start.to_le_bytes(),
        &absolute_end.to_le_bytes(),
        &text_hash.to_le_bytes(),
    ]);
    let source = NodeSource::CodeBuffer {
        buffer: buffer_id.to_string(),
    };
    let source_range = NodeSourceRange::new(absolute_start, absolute_end);
    let content = VirtualContentRef::new(
        VirtualContentId(content_id),
        VirtualContentKind::Code { language: None },
        source.clone(),
        source_range,
        NodeRevision(revision.0),
        text_hash,
        tile.line_count as u32,
    )
    .with_line_start(absolute_line);
    let text_plan =
        VirtualTextPlan::new(content.clone()).with_wrap(VirtualTextWrap::NoWrap);
    let node = VirtualNode::new(
        stable_node_id(namespace, buffer_id, absolute_line, kind_tag(&kind)),
        kind,
    )
    .with_geometry(geometry)
    .with_revision(text_hash)
    .with_text_hash(text_hash)
    .with_source(source, source_range)
    .with_content(content)
    .with_text_plan(text_plan);
    nodes.push(node);
}

fn stable_node_id(namespace: &str, buffer_id: &str, line: u64, kind: u8) -> NodeId {
    NodeId::new(stable_hash_parts(&[
        namespace.as_bytes(),
        buffer_id.as_bytes(),
        &line.to_le_bytes(),
        &[kind],
    ]))
}

fn kind_tag(kind: &VirtualNodeKind) -> u8 {
    match kind {
        VirtualNodeKind::CodeLine => 1,
        VirtualNodeKind::CodeTile => 2,
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
