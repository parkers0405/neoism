//! Per-frame GPU uploads through reused staging memory.
//!
//! On native backends every `Queue::write_buffer` and `Queue::write_texture` allocates a fresh
//! staging buffer and frees it once the GPU has copied out of it. `Staging` writes into mapped
//! chunks instead and records the copies itself: into an encoder at `flush`, or in a submission
//! of its own at `submit`. A chunk comes back for reuse once the GPU is done with it, so a steady
//! animating frame allocates no buffers.
//!
//! Writes land in the order they were made, at the next `flush` or `submit`. A write must be
//! flushed into an encoder ahead of the commands that read it, or submitted before them.
//! On the web, whose `writeBuffer` already stages, writes go straight to the queue. A backend
//! whose buffers the CPU can write in place registers a `Direct` writer, and buffer writes it
//! takes skip the staging.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

/// Writes `data` to `buffer` at `offset` in place, for the commands recorded after it, or
/// returns false to have it staged. Commands recorded before it keep reading what they would
/// have, so the write lands no later than a flush would land it.
pub type Direct = fn(&wgpu::Buffer, u64, &[u8]) -> bool;

static DIRECT: OnceLock<Direct> = OnceLock::new();

/// Registers the process's `Direct` writer. The first one stays.
pub fn set_direct(direct: Direct) {
    let _ = DIRECT.set(direct);
}

/// The size of a new chunk; a larger write gets a chunk sized to its next power of two.
const CHUNK: u64 = 128 * 1024;
/// Returned chunks kept for reuse, by count and in bytes; larger chunks are freed on return.
const KEEP_CHUNKS: usize = 8;
const KEEP_BYTES: u64 = 4 << 20;
const KEEP_CHUNK_MAX: u64 = 1 << 20;
/// Staged bytes past which a write first submits what is pending, so a caller that never
/// flushes can't grow the chunks without bound.
const MAX_PENDING: u64 = 16 << 20;
/// Staged texture rows start at this alignment, and `bytes_per_row` is a multiple of it.
const ROW_ALIGN: u64 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u64;

/// A shared handle to one set of staging chunks and the writes pending in them: the chunks,
/// the count of lost ones, and whether any are pending or mapped (so `flush` and `submit` with
/// nothing to do never lock).
#[derive(Clone)]
pub struct Staging(Arc<Mutex<Inner>>, Arc<AtomicU64>, Arc<AtomicBool>);

/// Counters, for tests and benchmarks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Stats {
    /// Chunks created, each one buffer allocation.
    pub chunks: u64,
    /// Writes staged.
    pub writes: u64,
    /// Bytes staged, row padding included.
    pub bytes: u64,
    /// Pending writes recorded into a caller's encoder.
    pub flushes: u64,
    /// Pending writes submitted on their own.
    pub submits: u64,
    /// Writes made in place by the `Direct` writer, counted in `writes` and `bytes` too.
    pub direct: u64,
}

struct Chunk {
    buffer: wgpu::Buffer,
    used: u64,
    /// The whole chunk's mapping, made at its first write and dropped before it unmaps, so a
    /// write costs wgpu no mapped-range bookkeeping of its own.
    view: Option<wgpu::BufferViewMut>,
}

enum Dst {
    Buffer(wgpu::Buffer, u64),
    /// Full-width texture rows from `y`, `rows` of them, `bytes_per_row` apart in the chunk.
    Rows { texture: wgpu::Texture, y: u32, rows: u32, bytes_per_row: u32 },
}

struct Pending {
    src: wgpu::Buffer,
    offset: u64,
    size: u64,
    dst: Dst,
}

struct Inner {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Mapped chunks holding pending writes.
    active: Vec<Chunk>,
    free: Vec<Chunk>,
    recycle: mpsc::Sender<Chunk>,
    returned: mpsc::Receiver<Chunk>,
    pending: Vec<Pending>,
    pending_bytes: u64,
    stats: Stats,
}

/// Hands a chunk back when the GPU has finished copying out of it. Dropped without that (its
/// encoder was dropped unsubmitted, or the device was lost), it counts a lost chunk.
struct Return {
    chunk: Option<Chunk>,
    recycle: mpsc::Sender<Chunk>,
    lost: Arc<AtomicU64>,
}

impl Drop for Return {
    fn drop(&mut self) {
        if self.chunk.is_some() { self.lost.fetch_add(1, Ordering::Relaxed); }
    }
}

impl Staging {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Staging {
        let (recycle, returned) = mpsc::channel();
        Staging(Arc::new(Mutex::new(Inner {
            device: device.clone(), queue: queue.clone(), active: Vec::new(), free: Vec::new(), recycle, returned,
            pending: Vec::new(), pending_bytes: 0, stats: Stats::default(),
        })), Arc::default(), Arc::default())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stages a write of `data` to `buffer` at `offset`. Both are multiples of 4.
    pub fn write_buffer(&self, buffer: &wgpu::Buffer, offset: u64, data: &[u8]) {
        if data.is_empty() { return; }
        if let Some(direct) = DIRECT.get() && direct(buffer, offset, data) {
            let mut s = self.lock();
            s.stats.direct += 1;
            s.stats.writes += 1;
            s.stats.bytes += data.len() as u64;
            return;
        }
        let mut s = self.lock();
        if cfg!(target_arch = "wasm32") { return s.queue.write_buffer(buffer, offset, data); }
        let size = data.len() as u64;
        let (src, at) = s.stage(size, wgpu::MAP_ALIGNMENT, self, |view| view.slice(..data.len()).copy_from_slice(data));
        s.pending.push(Pending { src, offset: at, size, dst: Dst::Buffer(buffer.clone(), offset) });
    }

    /// Stages a write of full-width rows of `texture` from row `y`. `rows` holds them tightly
    /// packed; its length is a whole number of rows.
    pub fn write_texture_rows(&self, texture: &wgpu::Texture, y: u32, rows: &[u8]) {
        let row = texture.width() * texture.format().block_copy_size(None).expect("a color format");
        let count = (rows.len() / row as usize) as u32;
        if count == 0 { return; }
        let mut s = self.lock();
        if cfg!(target_arch = "wasm32") {
            return s.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d { x: 0, y, z: 0 }, aspect: wgpu::TextureAspect::All },
                rows,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None },
                wgpu::Extent3d { width: texture.width(), height: count, depth_or_array_layers: 1 },
            );
        }
        let padded = u64::from(row).next_multiple_of(ROW_ALIGN);
        let size = padded * u64::from(count);
        let (src, at) = s.stage(size, ROW_ALIGN, self, |view| {
            if padded == u64::from(row) { return view.copy_from_slice(&rows[..size as usize]); }
            for (r, line) in rows.chunks_exact(row as usize).enumerate() {
                let start = r * padded as usize;
                view.slice(start..start + line.len()).copy_from_slice(line);
            }
        });
        s.pending.push(Pending { src, offset: at, size,
            dst: Dst::Rows { texture: texture.clone(), y, rows: count, bytes_per_row: padded as u32 } });
    }

    /// Records the pending writes into `encoder` here, ahead of what is recorded after them.
    /// `encoder` must be submitted: dropped, the writes never land (`lost` counts it).
    pub fn flush(&self, encoder: &mut wgpu::CommandEncoder) {
        if !self.busy() { return; }
        let mut s = self.lock();
        if s.pending.is_empty() && s.active.is_empty() { return; }
        s.encode(encoder, self);
        s.stats.flushes += 1;
    }

    /// Records the pending copies into `encoder` but leaves their chunks mapped, so later writes
    /// share them: one chunk unmapped and remapped per frame instead of one per write. The next
    /// `flush` unmaps them and must be into the same encoder (or one submitted after it); a
    /// `submit` before then would remap them before `encoder` copies out of them.
    pub fn record(&self, encoder: &mut wgpu::CommandEncoder) {
        if !self.busy() { return; }
        let mut s = self.lock();
        if s.pending.is_empty() { return; }
        s.copy(encoder);
        s.stats.flushes += 1;
    }

    /// Submits the pending writes now, ahead of anything submitted later.
    pub fn submit(&self) {
        if !self.busy() { return; }
        self.lock().submit(self);
    }

    /// Whether writes are pending or chunks still mapped since the last `flush` or `submit`.
    pub fn busy(&self) -> bool {
        self.2.load(Ordering::Acquire)
    }

    /// Whether writes are waiting for a `flush` or `submit`.
    pub fn is_pending(&self) -> bool {
        !self.lock().pending.is_empty()
    }

    /// Chunks whose writes may never have landed because their encoder was dropped unsubmitted.
    /// A caller that skips writes matching what it last wrote resends everything when it grows.
    pub fn lost(&self) -> u64 {
        self.1.load(Ordering::Relaxed)
    }

    pub fn stats(&self) -> Stats {
        self.lock().stats
    }
}

impl Inner {
    /// Reserves `size` bytes (rounded up to 4) at `align` in a mapped chunk and fills them.
    fn stage(&mut self, size: u64, align: u64, staging: &Staging, fill: impl FnOnce(&mut wgpu::WriteOnly<'_, [u8]>)) -> (wgpu::Buffer, u64) {
        let size = size.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
        if self.pending_bytes + size > MAX_PENDING { self.submit(staging); }
        let fits = |c: &Chunk| c.used.next_multiple_of(align) + size <= c.buffer.size();
        let index = match self.active.iter().position(fits) {
            Some(i) => i,
            None => {
                self.receive();
                let chunk = match self.free.iter().position(|c| c.buffer.size() >= size) {
                    Some(i) => self.free.swap_remove(i),
                    None => {
                        self.stats.chunks += 1;
                        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("rune staging"),
                            size: CHUNK.max(size.next_power_of_two()),
                            usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
                            mapped_at_creation: true,
                        });
                        Chunk { buffer, used: 0, view: None }
                    }
                };
                self.active.push(chunk);
                self.active.len() - 1
            }
        };
        let chunk = &mut self.active[index];
        let at = chunk.used.next_multiple_of(align);
        chunk.used = at + size;
        let view = chunk.view.get_or_insert_with(|| chunk.buffer.get_mapped_range_mut(..).expect("a mapped staging chunk"));
        fill(&mut view.slice(at as usize..(at + size) as usize));
        self.pending_bytes += size;
        self.stats.writes += 1;
        self.stats.bytes += size;
        staging.2.store(true, Ordering::Release);
        (chunk.buffer.clone(), at)
    }

    /// Moves chunks the GPU is done with to the free list, within the keep bounds.
    fn receive(&mut self) {
        while let Ok(mut chunk) = self.returned.try_recv() {
            chunk.used = 0;
            let kept: u64 = self.free.iter().map(|c| c.buffer.size()).sum();
            let size = chunk.buffer.size();
            if size <= KEEP_CHUNK_MAX && self.free.len() < KEEP_CHUNKS && kept + size <= KEEP_BYTES {
                self.free.push(chunk);
            }
        }
    }

    fn encode(&mut self, encoder: &mut wgpu::CommandEncoder, staging: &Staging) {
        self.copy(encoder);
        for mut chunk in self.active.drain(..) {
            chunk.view = None;
            chunk.buffer.unmap();
            let buffer = chunk.buffer.clone();
            let mut back = Return { chunk: Some(chunk), recycle: self.recycle.clone(), lost: staging.1.clone() };
            encoder.map_buffer_on_submit(&buffer, wgpu::MapMode::Write, .., move |mapped| {
                if mapped.is_ok() && let Some(chunk) = back.chunk.take() { let _ = back.recycle.send(chunk); }
            });
        }
        staging.2.store(false, Ordering::Release);
    }

    /// Records the pending copies; their chunks stay mapped and active.
    fn copy(&mut self, encoder: &mut wgpu::CommandEncoder) {
        for p in self.pending.drain(..) {
            match p.dst {
                Dst::Buffer(buffer, offset) => encoder.copy_buffer_to_buffer(&p.src, p.offset, &buffer, offset, p.size),
                Dst::Rows { texture, y, rows, bytes_per_row } => encoder.copy_buffer_to_texture(
                    wgpu::TexelCopyBufferInfo { buffer: &p.src, layout: wgpu::TexelCopyBufferLayout {
                        offset: p.offset, bytes_per_row: Some(bytes_per_row), rows_per_image: None } },
                    wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d { x: 0, y, z: 0 }, aspect: wgpu::TextureAspect::All },
                    wgpu::Extent3d { width: texture.width(), height: rows, depth_or_array_layers: 1 },
                ),
            }
        }
        self.pending_bytes = 0;
    }

    fn submit(&mut self, staging: &Staging) {
        if self.pending.is_empty() { return; }
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("rune uploads") });
        self.encode(&mut encoder, staging);
        self.queue.submit([encoder.finish()]);
        self.stats.submits += 1;
    }
}
