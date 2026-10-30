use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

use super::gpu::{VirtualGpuContentRequest, VirtualGpuFramePacket};
use super::protocol::{
    NodeRevision, NodeSource, NodeSourceRange, VirtualContentId, VirtualContentRef,
};

use serde::{Deserialize, Serialize};

/// Text payload resolved for a source-backed content request.
///
/// Real integrations can fetch this from a file buffer, model stream, agent
/// transcript, rope, mmap, or editor buffer. The renderer only needs the
/// descriptor plus the resolved text for shaping/rasterization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualContentPayload {
    pub content: VirtualContentRef,
    pub text: String,
}

impl VirtualContentPayload {
    pub fn new(content: VirtualContentRef, text: impl Into<String>) -> Self {
        Self {
            content,
            text: text.into(),
        }
    }

    pub fn byte_len(&self) -> usize {
        self.text.len()
    }
}

/// Sparse byte/line checkpoints for very large text sources.
///
/// The index stores one checkpoint every N lines instead of one entry per
/// line. That keeps memory tiny for giant markdown/model/code buffers while
/// still making line seeks bounded by the checkpoint stride.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualTextLineIndex {
    checkpoint_lines: u32,
    byte_len: u64,
    line_count: u64,
    trailing_newline: bool,
    max_line_bytes: u32,
    checkpoints: Vec<VirtualTextLineCheckpoint>,
}

impl VirtualTextLineIndex {
    pub fn new(text: &str) -> Self {
        Self::with_checkpoint_lines(
            text,
            VirtualTextLineIndexConfig::default().checkpoint_lines,
        )
    }

    pub fn with_checkpoint_lines(text: &str, checkpoint_lines: u32) -> Self {
        let checkpoint_lines = checkpoint_lines.max(1);
        let mut index = Self {
            checkpoint_lines,
            byte_len: text.len() as u64,
            line_count: 0,
            trailing_newline: text.ends_with('\n'),
            max_line_bytes: 0,
            checkpoints: vec![VirtualTextLineCheckpoint { line: 0, byte: 0 }],
        };
        let mut byte = 0u64;
        for line in text.split_inclusive('\n') {
            if index.line_count > 0 && index.line_count % u64::from(checkpoint_lines) == 0
            {
                index.checkpoints.push(VirtualTextLineCheckpoint {
                    line: index.line_count,
                    byte,
                });
            }
            index.line_count += 1;
            index.max_line_bytes = index.max_line_bytes.max(line.len() as u32);
            byte = byte.saturating_add(line.len() as u64);
        }
        index
    }

    pub fn stats(&self) -> VirtualTextLineIndexStats {
        VirtualTextLineIndexStats {
            checkpoint_lines: self.checkpoint_lines,
            byte_len: self.byte_len,
            line_count: self.line_count,
            trailing_newline: self.trailing_newline,
            max_line_bytes: self.max_line_bytes,
            checkpoints: self.checkpoints.len(),
        }
    }

    pub fn byte_len(&self) -> u64 {
        self.byte_len
    }

    pub fn line_count(&self) -> u64 {
        self.line_count
    }

    pub fn trailing_newline(&self) -> bool {
        self.trailing_newline
    }

    pub fn max_line_bytes(&self) -> u32 {
        self.max_line_bytes
    }

    pub fn checkpoints(&self) -> &[VirtualTextLineCheckpoint] {
        &self.checkpoints
    }

    pub fn checkpoint_lines(&self) -> u32 {
        self.checkpoint_lines
    }

    pub fn append_to_text(
        &mut self,
        text: &mut String,
        appended: &str,
    ) -> VirtualTextAppendStats {
        let previous_byte_len = self.byte_len;
        let previous_line_count = self.line_count;
        if appended.is_empty() {
            return VirtualTextAppendStats {
                previous_byte_len,
                byte_len: self.byte_len,
                previous_line_count,
                line_count: self.line_count,
                appended_bytes: 0,
                reindexed_from_line: self.line_count,
                reindexed_bytes: 0,
                checkpoints: self.checkpoints.len(),
            };
        }

        let checkpoint = if self.line_count == 0 {
            VirtualTextLineCheckpoint { line: 0, byte: 0 }
        } else {
            self.checkpoint_for_line(self.line_count.saturating_sub(1))
        };
        self.checkpoints
            .retain(|candidate| candidate.line <= checkpoint.line);

        text.push_str(appended);
        self.byte_len = text.len() as u64;
        self.trailing_newline = text.ends_with('\n');
        self.line_count = checkpoint.line;

        let mut byte = checkpoint.byte;
        for line in text[checkpoint.byte as usize..].split_inclusive('\n') {
            if self.line_count > 0
                && self.line_count % u64::from(self.checkpoint_lines) == 0
                && self.checkpoints.last().is_none_or(|last| last.byte != byte)
            {
                self.checkpoints.push(VirtualTextLineCheckpoint {
                    line: self.line_count,
                    byte,
                });
            }
            self.line_count += 1;
            self.max_line_bytes = self.max_line_bytes.max(line.len() as u32);
            byte = byte.saturating_add(line.len() as u64);
        }

        VirtualTextAppendStats {
            previous_byte_len,
            byte_len: self.byte_len,
            previous_line_count,
            line_count: self.line_count,
            appended_bytes: appended.len(),
            reindexed_from_line: checkpoint.line,
            reindexed_bytes: self.byte_len.saturating_sub(checkpoint.byte),
            checkpoints: self.checkpoints.len(),
        }
    }

    pub fn replace_range_in_text(
        &mut self,
        text: &mut String,
        range: NodeSourceRange,
        replacement: &str,
    ) -> Result<VirtualTextEditStats, VirtualContentStoreError> {
        let previous_byte_len = self.byte_len;
        let previous_line_count = self.line_count;
        let start = range.start as usize;
        let end = range.end as usize;
        if end > text.len() || start > end {
            return Err(VirtualContentStoreError::RangeOutOfBounds {
                content: VirtualContentId(0),
                range,
                byte_len: text.len() as u64,
            });
        }
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            return Err(VirtualContentStoreError::InvalidUtf8Boundary {
                content: VirtualContentId(0),
                range,
            });
        }

        let checkpoint = self.checkpoint_for_byte(range.start);
        self.checkpoints
            .retain(|candidate| candidate.line <= checkpoint.line);

        text.replace_range(start..end, replacement);
        self.byte_len = text.len() as u64;
        self.trailing_newline = text.ends_with('\n');
        self.line_count = checkpoint.line;

        let mut byte = checkpoint.byte;
        for line in text[checkpoint.byte as usize..].split_inclusive('\n') {
            if self.line_count > 0
                && self.line_count % u64::from(self.checkpoint_lines) == 0
                && self.checkpoints.last().is_none_or(|last| last.byte != byte)
            {
                self.checkpoints.push(VirtualTextLineCheckpoint {
                    line: self.line_count,
                    byte,
                });
            }
            self.line_count += 1;
            self.max_line_bytes = self.max_line_bytes.max(line.len() as u32);
            byte = byte.saturating_add(line.len() as u64);
        }

        Ok(VirtualTextEditStats {
            previous_byte_len,
            byte_len: self.byte_len,
            previous_line_count,
            line_count: self.line_count,
            replaced_range: range,
            removed_bytes: range.len(),
            inserted_bytes: replacement.len(),
            reindexed_from_line: checkpoint.line,
            reindexed_bytes: self.byte_len.saturating_sub(checkpoint.byte),
            checkpoints: self.checkpoints.len(),
        })
    }

    pub fn line_for_byte(&self, text: &str, byte_offset: u64) -> u64 {
        let offset = byte_offset.min(self.byte_len);
        let checkpoint = self.checkpoint_for_byte(offset);
        let mut line = checkpoint.line;
        let mut byte = checkpoint.byte as usize;
        let target = offset as usize;
        while byte < target && byte < text.len() {
            let remaining = &text[byte..];
            let Some(next_newline) = remaining.find('\n') else {
                break;
            };
            let next_byte = byte.saturating_add(next_newline).saturating_add(1);
            if next_byte > target {
                break;
            }
            byte = next_byte;
            line = line.saturating_add(1);
        }
        line.min(self.line_count.saturating_sub(1))
    }

    pub fn line_range(&self, text: &str, line: u64) -> Option<NodeSourceRange> {
        if line >= self.line_count {
            return None;
        }
        let checkpoint = self.checkpoint_for_line(line);
        let mut current_line = checkpoint.line;
        let mut start = checkpoint.byte as usize;
        while current_line < line && start < text.len() {
            let remaining = &text[start..];
            let next = remaining
                .find('\n')
                .map(|offset| start.saturating_add(offset).saturating_add(1))
                .unwrap_or(text.len());
            start = next;
            current_line = current_line.saturating_add(1);
        }
        let end = text[start..]
            .find('\n')
            .map(|offset| start.saturating_add(offset).saturating_add(1))
            .unwrap_or(text.len());
        Some(NodeSourceRange::new(start as u64, end as u64))
    }

    pub fn byte_range_for_lines(
        &self,
        text: &str,
        lines: Range<u64>,
    ) -> Option<NodeSourceRange> {
        if lines.start >= lines.end || lines.start >= self.line_count {
            return None;
        }
        let start = self.line_range(text, lines.start)?.start;
        let end_line = lines.end.min(self.line_count);
        let end = if end_line >= self.line_count {
            self.byte_len
        } else {
            self.line_range(text, end_line)?.start
        };
        Some(NodeSourceRange::new(start, end))
    }

    fn checkpoint_for_line(&self, line: u64) -> VirtualTextLineCheckpoint {
        let partition = self
            .checkpoints
            .partition_point(|checkpoint| checkpoint.line <= line);
        self.checkpoints[partition.saturating_sub(1)]
    }

    fn checkpoint_for_byte(&self, byte: u64) -> VirtualTextLineCheckpoint {
        let partition = self
            .checkpoints
            .partition_point(|checkpoint| checkpoint.byte <= byte);
        self.checkpoints[partition.saturating_sub(1)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualTextLineIndexConfig {
    pub checkpoint_lines: u32,
}

impl Default for VirtualTextLineIndexConfig {
    fn default() -> Self {
        Self {
            checkpoint_lines: 512,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualTextLineIndexStats {
    pub checkpoint_lines: u32,
    pub byte_len: u64,
    pub line_count: u64,
    pub trailing_newline: bool,
    pub max_line_bytes: u32,
    pub checkpoints: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualTextAppendStats {
    pub previous_byte_len: u64,
    pub byte_len: u64,
    pub previous_line_count: u64,
    pub line_count: u64,
    pub appended_bytes: usize,
    pub reindexed_from_line: u64,
    pub reindexed_bytes: u64,
    pub checkpoints: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualTextEditStats {
    pub previous_byte_len: u64,
    pub byte_len: u64,
    pub previous_line_count: u64,
    pub line_count: u64,
    pub replaced_range: NodeSourceRange,
    pub removed_bytes: u64,
    pub inserted_bytes: usize,
    pub reindexed_from_line: u64,
    pub reindexed_bytes: u64,
    pub checkpoints: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualTextLineCheckpoint {
    pub line: u64,
    pub byte: u64,
}

/// Backend/source contract for resolving content refs carried by GPU packets.
pub trait VirtualContentProvider {
    type Error;

    fn resolve_content(
        &mut self,
        request: &VirtualGpuContentRequest,
    ) -> Result<VirtualContentPayload, Self::Error>;
}

/// Small deterministic content store for probes, tests, and simple local
/// integrations. Production file/model/code buffers can implement the trait
/// directly without copying their whole backing store into this map.
#[derive(Clone, Debug, Default)]
pub struct VirtualInMemoryContentStore {
    entries: BTreeMap<VirtualContentId, VirtualContentPayload>,
}

impl VirtualInMemoryContentStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &mut self,
        content: VirtualContentRef,
        text: impl Into<String>,
    ) -> Option<VirtualContentPayload> {
        self.entries
            .insert(content.id, VirtualContentPayload::new(content, text))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn resolve_packet(
        &mut self,
        packet: &VirtualGpuFramePacket,
    ) -> Result<VirtualContentResolutionStats, VirtualContentStoreError> {
        resolve_requests(self, &packet.content_requests)
    }

    pub fn resolve_prefetch_packet(
        &mut self,
        packet: &VirtualGpuFramePacket,
    ) -> Result<VirtualContentResolutionStats, VirtualContentStoreError> {
        resolve_requests(self, &packet.prefetch_content_requests)
    }
}

impl VirtualContentProvider for VirtualInMemoryContentStore {
    type Error = VirtualContentStoreError;

    fn resolve_content(
        &mut self,
        request: &VirtualGpuContentRequest,
    ) -> Result<VirtualContentPayload, Self::Error> {
        let Some(payload) = self.entries.get(&request.content.id) else {
            return Err(VirtualContentStoreError::MissingContent {
                content: request.content.id,
            });
        };
        if payload.content.revision != request.content.revision {
            return Err(VirtualContentStoreError::RevisionMismatch {
                content: request.content.id,
                expected: request.content.revision,
                actual: payload.content.revision,
            });
        }
        if payload.content.hash != request.content.hash {
            return Err(VirtualContentStoreError::HashMismatch {
                content: request.content.id,
                expected: request.content.hash,
                actual: payload.content.hash,
            });
        }
        Ok(payload.clone())
    }
}

/// Source-backed text store for huge files, model streams, agent transcripts,
/// and future code buffers.
///
/// Unlike [`VirtualInMemoryContentStore`], this keeps one backing string per
/// semantic source/revision and resolves visible content refs by source range.
/// That mirrors the production path: render packets request only visible
/// content, while the owner keeps the full text in a rope, mmap, or stream
/// buffer.
#[derive(Clone, Debug, Default)]
pub struct VirtualSourceTextStore {
    sources: HashMap<VirtualSourceTextKey, VirtualSourceTextEntry>,
}

impl VirtualSourceTextStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &mut self,
        source: NodeSource,
        revision: NodeRevision,
        text: impl Into<String>,
    ) -> Option<VirtualSourceTextEntry> {
        self.insert_with_index_config(
            source,
            revision,
            text,
            VirtualTextLineIndexConfig::default(),
        )
    }

    pub fn insert_with_index_config(
        &mut self,
        source: NodeSource,
        revision: NodeRevision,
        text: impl Into<String>,
        config: VirtualTextLineIndexConfig,
    ) -> Option<VirtualSourceTextEntry> {
        let text = text.into();
        let line_index =
            VirtualTextLineIndex::with_checkpoint_lines(&text, config.checkpoint_lines);
        self.sources.insert(
            VirtualSourceTextKey { source, revision },
            VirtualSourceTextEntry { text, line_index },
        )
    }

    pub fn len(&self) -> usize {
        self.sources.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    pub fn line_index(
        &self,
        source: &NodeSource,
        revision: NodeRevision,
    ) -> Option<&VirtualTextLineIndex> {
        self.sources
            .get(&VirtualSourceTextKey {
                source: source.clone(),
                revision,
            })
            .map(|entry| &entry.line_index)
    }

    pub fn append(
        &mut self,
        source: NodeSource,
        revision: NodeRevision,
        text: &str,
    ) -> Result<VirtualTextAppendStats, VirtualContentStoreError> {
        self.append_revision(source, revision, revision, text)
    }

    pub fn append_revision(
        &mut self,
        source: NodeSource,
        from_revision: NodeRevision,
        to_revision: NodeRevision,
        text: &str,
    ) -> Result<VirtualTextAppendStats, VirtualContentStoreError> {
        if from_revision == to_revision {
            return self.append_in_place(source, from_revision, text);
        }
        let from_key = VirtualSourceTextKey {
            source: source.clone(),
            revision: from_revision,
        };
        let Some(mut entry) = self.sources.remove(&from_key) else {
            return Err(VirtualContentStoreError::MissingSource {
                source,
                revision: from_revision,
            });
        };
        let stats = entry.line_index.append_to_text(&mut entry.text, text);
        self.sources.insert(
            VirtualSourceTextKey {
                source,
                revision: to_revision,
            },
            entry,
        );
        Ok(stats)
    }

    fn append_in_place(
        &mut self,
        source: NodeSource,
        revision: NodeRevision,
        text: &str,
    ) -> Result<VirtualTextAppendStats, VirtualContentStoreError> {
        let key = VirtualSourceTextKey { source, revision };
        let Some(entry) = self.sources.get_mut(&key) else {
            return Err(VirtualContentStoreError::MissingSource {
                source: key.source,
                revision: key.revision,
            });
        };
        Ok(entry.line_index.append_to_text(&mut entry.text, text))
    }

    pub fn replace_range(
        &mut self,
        source: NodeSource,
        revision: NodeRevision,
        range: NodeSourceRange,
        replacement: &str,
    ) -> Result<VirtualTextEditStats, VirtualContentStoreError> {
        self.replace_range_revision(source, revision, revision, range, replacement)
    }

    pub fn replace_range_revision(
        &mut self,
        source: NodeSource,
        from_revision: NodeRevision,
        to_revision: NodeRevision,
        range: NodeSourceRange,
        replacement: &str,
    ) -> Result<VirtualTextEditStats, VirtualContentStoreError> {
        if from_revision == to_revision {
            return self.replace_range_in_place(
                source,
                from_revision,
                range,
                replacement,
            );
        }
        let from_key = VirtualSourceTextKey {
            source: source.clone(),
            revision: from_revision,
        };
        let Some(mut entry) = self.sources.get(&from_key).cloned() else {
            return Err(VirtualContentStoreError::MissingSource {
                source,
                revision: from_revision,
            });
        };
        let stats = entry.line_index.replace_range_in_text(
            &mut entry.text,
            range,
            replacement,
        )?;
        self.sources.remove(&from_key);
        self.sources.insert(
            VirtualSourceTextKey {
                source,
                revision: to_revision,
            },
            entry,
        );
        Ok(stats)
    }

    fn replace_range_in_place(
        &mut self,
        source: NodeSource,
        revision: NodeRevision,
        range: NodeSourceRange,
        replacement: &str,
    ) -> Result<VirtualTextEditStats, VirtualContentStoreError> {
        let key = VirtualSourceTextKey { source, revision };
        let Some(entry) = self.sources.get_mut(&key) else {
            return Err(VirtualContentStoreError::MissingSource {
                source: key.source,
                revision: key.revision,
            });
        };
        entry
            .line_index
            .replace_range_in_text(&mut entry.text, range, replacement)
    }

    pub fn entry(
        &self,
        source: &NodeSource,
        revision: NodeRevision,
    ) -> Option<&VirtualSourceTextEntry> {
        self.sources.get(&VirtualSourceTextKey {
            source: source.clone(),
            revision,
        })
    }

    pub fn resolve_packet(
        &mut self,
        packet: &VirtualGpuFramePacket,
    ) -> Result<VirtualContentResolutionStats, VirtualContentStoreError> {
        resolve_requests(self, &packet.content_requests)
    }

    pub fn resolve_prefetch_packet(
        &mut self,
        packet: &VirtualGpuFramePacket,
    ) -> Result<VirtualContentResolutionStats, VirtualContentStoreError> {
        resolve_requests(self, &packet.prefetch_content_requests)
    }
}

impl VirtualContentProvider for VirtualSourceTextStore {
    type Error = VirtualContentStoreError;

    fn resolve_content(
        &mut self,
        request: &VirtualGpuContentRequest,
    ) -> Result<VirtualContentPayload, Self::Error> {
        let key = VirtualSourceTextKey {
            source: request.content.source.clone(),
            revision: request.content.revision,
        };
        let Some(entry) = self.sources.get(&key) else {
            return Err(VirtualContentStoreError::MissingSource {
                source: request.content.source.clone(),
                revision: request.content.revision,
            });
        };
        let start = request.content.range.start as usize;
        let end = request.content.range.end as usize;
        if end > entry.text.len() || start > end {
            return Err(VirtualContentStoreError::RangeOutOfBounds {
                content: request.content.id,
                range: request.content.range,
                byte_len: entry.text.len() as u64,
            });
        }
        let Some(text) = entry.text.get(start..end) else {
            return Err(VirtualContentStoreError::InvalidUtf8Boundary {
                content: request.content.id,
                range: request.content.range,
            });
        };
        Ok(VirtualContentPayload::new(
            request.content.clone(),
            text.to_string(),
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualSourceTextEntry {
    pub text: String,
    pub line_index: VirtualTextLineIndex,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct VirtualSourceTextKey {
    source: NodeSource,
    revision: NodeRevision,
}

fn resolve_requests<P>(
    provider: &mut P,
    requests: &[VirtualGpuContentRequest],
) -> Result<VirtualContentResolutionStats, P::Error>
where
    P: VirtualContentProvider,
{
    let mut stats = VirtualContentResolutionStats::default();
    for request in requests {
        let payload = provider.resolve_content(request)?;
        stats.requests += 1;
        stats.bytes = stats.bytes.saturating_add(payload.byte_len());
        stats.lines = stats
            .lines
            .saturating_add(payload.text.lines().count().max(1));
    }
    Ok(stats)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VirtualContentResolutionStats {
    pub requests: usize,
    pub bytes: usize,
    pub lines: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VirtualContentStoreError {
    MissingContent {
        content: VirtualContentId,
    },
    RevisionMismatch {
        content: VirtualContentId,
        expected: NodeRevision,
        actual: NodeRevision,
    },
    HashMismatch {
        content: VirtualContentId,
        expected: u64,
        actual: u64,
    },
    MissingSource {
        source: NodeSource,
        revision: NodeRevision,
    },
    RangeOutOfBounds {
        content: VirtualContentId,
        range: NodeSourceRange,
        byte_len: u64,
    },
    InvalidUtf8Boundary {
        content: VirtualContentId,
        range: NodeSourceRange,
    },
}
