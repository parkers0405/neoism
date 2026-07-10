//! Editor-pane performer — drives a `Crosswords` grid from
//! `nvim --embed` msgpack-rpc events, parallel to the PTY-driven
//! `Machine` in `mod.rs`.
//!
//! ## Architecture (Option C — tokio thread bridged to mio loop)
//!
//! Rio's existing `Machine` is sync, mio-based, and byte-oriented. The
//! neovim-rs ecosystem is async, tokio-based, and msgpack-typed. Rather
//! than rewrite either side, `NvimEmbedMachine` runs nvim-rs inside a
//! dedicated single-threaded tokio runtime on its own OS thread and
//! bridges into Rio's existing channel infrastructure:
//!
//!   ┌────────────────────┐   tokio thread     ┌──────────────────┐
//!   │ nvim --embed child │◄──msgpack-rpc────► │   nvim-rs Bridge │
//!   └────────────────────┘                    │  (Handler impl)  │
//!                                             └──────┬───────────┘
//!                                                    │ std::mpsc::Sender
//!                                                    ▼
//!                                  ┌─────────────────────────────────┐
//!                                  │  NvimEmbedMachine (neoism-backend) │
//!                                  │  Phase 2c: events → Crosswords  │
//!                                  └─────────────────────────────────┘
//!
//! The `Crosswords` mutex stays the single source of cell state, so the
//! renderer doesn't care whether the bytes came from a PTY or from
//! `grid_line` redraw events.
//!
//! ## What this implementation covers (Phase 2b)
//!
//! - Spawns nvim, completes the rpc handshake, calls `ui_attach`, and
//!   keeps the io future driven on the runtime.
//! - Forwards `redraw` notifications as raw `rmpv::Value` events through
//!   a std::mpsc channel — the receiver (Phase 2c) will parse these
//!   into typed `RedrawEvent`s ported from `neovide/src/bridge/events.rs`
//!   and apply them to `Crosswords`.
//! - Exposes `input(keys)` and `resize(cols, rows)` as fire-and-forget
//!   commands relayed to the runtime via tokio mpsc.
//! - Drops without blocking the UI: `Shutdown` command + detached runtime cleanup.
//!
//! ## What's deferred to Phase 2c
//!
//! - Parsing `rmpv::Value` redraw events into typed `RedrawEvent`s.
//! - Applying those events to `Crosswords` cell-by-cell.
//! - Wiring `ContextSource::Editor` into `ContextManager::create_context`.
//! - Translating renderer key events into `nvim_input` strings.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::{fs, io};

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use nvim_rs::{Handler, Neovim, UiAttachOptions};
use rmpv::Value;
use tokio::process::Command as TokioCommand;
use tokio::sync::{mpsc as tokio_mpsc, watch as tokio_watch};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::clipboard::ClipboardType;
use crate::event::{EventListener, RioEvent, WindowId};

mod bridge;
mod commands;
mod machine;
mod types;

pub(crate) use bridge::*;
pub use commands::*;
pub use machine::*;
pub use types::*;

#[cfg(test)]
mod tests;
