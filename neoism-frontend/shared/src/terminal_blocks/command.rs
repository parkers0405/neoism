use web_time::Instant;

#[derive(Debug, Clone)]
pub struct TerminalCommandBlock {
    pub command: String,
    pub status: TerminalCommandBlockStatus,
    pub saw_command_start: bool,
    pub submitted_at: Instant,
    pub finished_at: Option<Instant>,
    pub cwd: Option<String>,
    pub output_start_row: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalCommandBlockStatus {
    Running,
    Finished {
        exit_code: Option<i32>,
    },
    /// Input delivery was not acknowledged. The command may or may not have
    /// reached the remote shell, so this must never be rendered as success.
    Interrupted,
}

/// Public snapshot of one command block — what the renderer overlay
/// needs to paint a Warp-style block card without locking the input
/// buffer. Decoupled from the internal `TerminalCommandBlock` so we
/// can tune the public surface without touching every call site.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CommandBlockSnapshot {
    pub command: String,
    pub cwd: Option<String>,
    pub status: BlockStatusKind,
    pub favorite: bool,
    pub output_start_row: Option<usize>,
    pub duration_ms: f32,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockStatusKind {
    Running,
    Ok,
    Error(i32),
    Interrupted,
}

// Reserved for the block-snapshot consumer (`command_block_snapshots`
// builds CommandBlockSnapshot.duration_ms with this).
#[allow(dead_code)]
pub fn duration_ms(block: &TerminalCommandBlock) -> f32 {
    let end = block.finished_at.unwrap_or_else(Instant::now);
    end.saturating_duration_since(block.submitted_at)
        .as_secs_f32()
        * 1000.0
}
