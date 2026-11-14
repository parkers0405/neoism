//! A buffer's shared storage, written straight from the CPU. While a recorded or running
//! command buffer still uses the storage, a write swaps in a spare brought up to date, so what
//! was recorded reads the old bytes and what is recorded next reads the new: a small ring per
//! written buffer, as deep as the frames in flight.

use std::collections::VecDeque;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;

use super::Shared;
use super::resource::BufferRaw;

/// Spares a written buffer keeps: one per frame the GPU may still be reading.
const SPARES: usize = 2;
/// Written ranges remembered, to bring a spare up to date by copying only what it misses.
const LOG: usize = 8;

/// Swaps made in the process. A pass that sees it change rebinds its buffers' current storage.
pub(super) static SWAPS: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct Store {
    slots: Mutex<Slots>,
    /// The GPU writes this buffer (a copy into it, a writable storage binding, a staged queue
    /// write), so its storage is never swapped: a copy of the rest would miss what is in flight.
    pinned: AtomicBool,
}

#[derive(Debug)]
struct Slots {
    current: Arc<BufferRaw>,
    /// Writes made so far, which numbers them.
    writes: u64,
    /// The latest writes' numbers and ranges.
    log: VecDeque<(u64, Range<u64>)>,
    /// Earlier storage, and how many writes it holds.
    spares: VecDeque<(Arc<BufferRaw>, u64)>,
    swaps: u64,
    /// Bytes swaps copied forward, for tests.
    #[cfg(test)]
    copied: u64,
}

/// Whether anything besides the store holds `raw`: a command buffer's keep list, recorded or
/// running, holds what it uses until it completes.
fn idle(raw: &Arc<BufferRaw>) -> bool { Arc::strong_count(raw) == 1 }

impl Slots {
    fn write(&mut self, offset: u64, data: &[u8]) {
        self.current.write(offset, data);
        self.writes += 1;
        self.log.push_back((self.writes, offset..offset + data.len() as u64));
        if self.log.len() > LOG { self.log.pop_front(); }
    }

    /// The span written since a storage holding `held` writes: everything if the log has lost
    /// some of them.
    fn missed(&self, held: u64) -> Range<u64> {
        if self.log.front().is_none_or(|(at, _)| *at > held + 1) && held < self.writes { return 0..self.current.size }
        self.log.iter().filter(|(at, _)| *at > held).fold(0..0, |hull, (_, r)| {
            if hull.is_empty() { r.clone() } else { hull.start.min(r.start)..hull.end.max(r.end) }
        })
    }
}

impl Store {
    pub(super) fn new(raw: BufferRaw) -> Self {
        let slots = Slots {
            current: Arc::new(raw), writes: 0, log: VecDeque::new(), spares: VecDeque::new(), swaps: 0,
            #[cfg(test)]
            copied: 0,
        };
        Store { slots: Mutex::new(slots), pinned: AtomicBool::new(false) }
    }

    /// The storage commands recorded now read and write.
    pub(super) fn current(&self) -> Arc<BufferRaw> { self.slots.lock().current.clone() }

    /// Notes that the GPU writes this buffer, which pins its storage.
    pub(super) fn gpu_write(&self) -> Arc<BufferRaw> {
        self.pinned.store(true, Ordering::Relaxed);
        self.current()
    }

    pub(super) fn pinned(&self) -> bool { self.pinned.load(Ordering::Relaxed) }

    /// Writes `data` at `offset` now if no recorded or running command uses the storage, which
    /// is then exactly as if the write came ahead of every command recorded or submitted later.
    pub(super) fn write_idle(&self, shared: &Shared, offset: u64, data: &[u8]) -> bool {
        for retired in [false, true] {
            // Buffers that completed since the last submit still hold their keep lists.
            if retired { shared.retire(); }
            let mut slots = self.slots.lock();
            if idle(&slots.current) {
                slots.write(offset, data);
                return true;
            }
        }
        false
    }

    /// Writes `data` at `offset` for the commands recorded from now on; those recorded before
    /// keep reading the old bytes. False if the buffer is pinned, or if `may_swap`, asked under
    /// the store's lock, says the old storage must take the write too, so it must be staged.
    pub(super) fn write_ahead(&self, shared: &Shared, offset: u64, data: &[u8], may_swap: impl FnOnce() -> bool) -> bool {
        if self.pinned() { return false }
        if self.write_idle(shared, offset, data) { return true }
        let mut slots = self.slots.lock();
        if !may_swap() { return false }
        let size = slots.current.size;
        let (next, held) = match slots.spares.iter().position(|(raw, _)| idle(raw)) {
            Some(i) => slots.spares.remove(i).expect("a spare"),
            None => (Arc::new(BufferRaw::new(shared, size)), u64::MAX),
        };
        let missed = if held == u64::MAX { 0..size } else { slots.missed(held) };
        let end = offset + data.len() as u64;
        // Only what the write leaves: a write over everything it missed copies nothing.
        let (before, after) = (missed.start..missed.end.min(offset), missed.start.max(end)..missed.end);
        #[cfg(test)] { slots.copied += before.end.saturating_sub(before.start) + after.end.saturating_sub(after.start); }
        next.copy_from(&slots.current, before);
        next.copy_from(&slots.current, after);
        let held = slots.writes;
        let old = std::mem::replace(&mut slots.current, next);
        slots.spares.push_back((old, held));
        if slots.spares.len() > SPARES { slots.spares.pop_front(); }
        slots.write(offset, data);
        slots.swaps += 1;
        SWAPS.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Spares held and swaps made, for tests.
    #[cfg(test)]
    pub(super) fn swaps(&self) -> (usize, u64) {
        let slots = self.slots.lock();
        (slots.spares.len(), slots.swaps)
    }

    /// Bytes swaps copied forward, for tests.
    #[cfg(test)]
    pub(super) fn copied(&self) -> u64 { self.slots.lock().copied }
}
