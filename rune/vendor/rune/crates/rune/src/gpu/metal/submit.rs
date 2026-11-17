//! Submission: values on the shared event, the held frame buffer, staged writes, retirement of
//! what command buffers used, and the callbacks that wait on it.

use std::any::Any;
use std::collections::VecDeque;
use std::sync::Arc;

use objc2_metal::{MTLBlitCommandEncoder, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLDrawable, MTLEvent, MTLSharedEvent};
use objc2_video::rc::autoreleasepool;
use objc2_video::runtime::ProtocolObject;
#[cfg(target_os = "macos")]
use objc2_quartz_core::CAMetalDrawable;
use parking_lot::MutexGuard;
use wgpu::custom::*;

use super::encode::{MetalCommandBuffer, buffer_to_texture};
use super::resource::{BufferRaw, MetalBuffer, MetalTexture};
use super::surface::MetalOutput;
use super::{Obj, Shared};

/// What a command buffer uses, kept alive until it completes.
pub(super) type Keep = Vec<Arc<dyn Any + Send + Sync>>;

/// Frames the CPU may run ahead of the GPU.
const IN_FLIGHT: usize = 2;

/// Emptied keep lists kept for the next encoders, so a steady frame allocates none.
const SPARE_KEEPS: usize = 8;

enum Callback {
    Map(BufferMapCallback),
    Done(BoxSubmittedWorkDoneCallback),
}

#[derive(Default)]
pub(crate) struct State {
    /// The value the last stamped command buffer signals when it completes. Buffers commit in
    /// value order, so the event's value says how many have completed.
    pub(super) last: u64,
    /// A frame's stamped command buffer waiting for its drawable's present. While one is held it
    /// is the last stamped, and every other commit commits it first.
    held: Option<(Obj<dyn MTLCommandBuffer>, u64)>,
    uploads: Option<Uploads>,
    retire: VecDeque<(u64, Keep)>,
    callbacks: VecDeque<(u64, Callback)>,
    frames: VecDeque<u64>,
    pub(super) spare: Vec<Keep>,
}

/// Writes staged since the last submit, committed ahead of it.
struct Uploads {
    cb: Obj<dyn MTLCommandBuffer>,
    blit: Obj<dyn MTLBlitCommandEncoder>,
    keep: Keep,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State").field("last", &self.last).field("held", &self.held.is_some()).finish_non_exhaustive()
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> { self.state.lock() }

    /// A command buffer that doesn't retain what it uses. Made in a pool of its own, so the
    /// buffer is freed when its last owner here drops it rather than when the thread's pool drains.
    pub(super) fn command_buffer(&self) -> Obj<dyn MTLCommandBuffer> {
        autoreleasepool(|_| self.queue.commandBufferWithUnretainedReferences()).expect("a Metal command buffer")
    }

    /// An empty keep list, reusing one a retired command buffer emptied.
    pub(super) fn keep_list(&self) -> Keep {
        self.lock().spare.pop().unwrap_or_else(|| Vec::with_capacity(32))
    }

    /// Empties `keep`, releasing what it held, and saves it for a later encoder.
    pub(super) fn recycle(&self, mut keep: Keep) {
        keep.clear();
        if keep.capacity() == 0 { return }
        let mut st = self.lock();
        if st.spare.len() < SPARE_KEEPS { st.spare.push(keep); }
    }

    /// Hands `cb` the next value and keeps `keep` alive until the event reaches it.
    fn stamp(&self, st: &mut State, cb: &ProtocolObject<dyn MTLCommandBuffer>, keep: Keep) -> u64 {
        st.last += 1;
        cb.encodeSignalEvent_value(ProtocolObject::<dyn MTLEvent>::from_ref(&*self.event), st.last);
        if !keep.is_empty() { st.retire.push_back((st.last, keep)); }
        st.last
    }

    fn flush_held(st: &mut State) {
        if let Some((cb, _)) = st.held.take() { cb.commit(); }
    }

    /// Commits a held buffer whose drawable was dropped or never presented.
    pub(super) fn flush(&self) { Self::flush_held(&mut self.lock()); }

    fn commit_uploads(&self, st: &mut State) {
        let Some(Uploads { cb, blit, keep }) = st.uploads.take() else { return };
        blit.endEncoding();
        self.stamp(st, &cb, keep);
        cb.commit();
    }

    /// Encodes a staged write into the blit buffer the next submit commits first.
    pub(super) fn upload(&self, encode: impl FnOnce(&ProtocolObject<dyn MTLBlitCommandEncoder>), keep: [Arc<dyn Any + Send + Sync>; 2]) {
        let mut st = self.lock();
        let uploads = st.uploads.get_or_insert_with(|| {
            let cb = self.command_buffer();
            let blit = autoreleasepool(|_| cb.blitCommandEncoder()).expect("a Metal blit encoder");
            Uploads { cb, blit, keep: Vec::new() }
        });
        encode(&uploads.blit);
        uploads.keep.extend(keep);
    }

    fn after(&self, callback: Callback) {
        let mut st = self.lock();
        let at = st.last;
        st.callbacks.push_back((at, callback));
    }

    /// Releases what completed command buffers used and runs the callbacks waiting on them.
    /// One at a time, outside the lock: callbacks may call back into the device. Returns the
    /// value it retired up to.
    pub(super) fn retire(&self) -> u64 {
        let done = self.event.signaledValue();
        loop {
            let next = {
                let mut st = self.lock();
                if st.retire.front().is_some_and(|(at, _)| *at <= done) { st.retire.pop_front() } else { None }
            };
            let Some((_, keep)) = next else { break };
            self.recycle(keep);
        }
        loop {
            let next = {
                let mut st = self.lock();
                if st.callbacks.front().is_some_and(|(at, _)| *at <= done) { st.callbacks.pop_front() } else { None }
            };
            match next {
                Some((_, Callback::Map(f))) => f(Ok(())),
                Some((_, Callback::Done(f))) => f(),
                None => break,
            }
        }
        done
    }

    pub(super) fn poll(&self, poll: wgpu::wgt::PollType<u64>) -> Result<wgpu::PollStatus, wgpu::PollError> {
        let wgpu::wgt::PollType::Wait { submission_index, timeout } = poll else {
            // Empty only as of the value retire ran callbacks up to: work finishing after it
            // still has callbacks to run, so the caller polls again.
            let done = self.retire();
            let st = self.lock();
            let idle = done >= st.last && st.callbacks.is_empty();
            return Ok(if idle { wgpu::PollStatus::QueueEmpty } else { wgpu::PollStatus::Poll });
        };
        let last = {
            let mut st = self.lock();
            Self::flush_held(&mut st);
            st.last
        };
        let target = submission_index.unwrap_or(last);
        if target > last { return Err(wgpu::PollError::WrongSubmissionIndex(target, last)); }
        let ms = timeout.map_or(u64::MAX, |t| u64::try_from(t.as_millis()).unwrap_or(u64::MAX));
        if self.event.signaledValue() < target && !self.event.waitUntilSignaledValue_timeoutMS(target, ms) {
            return Err(wgpu::PollError::Timeout);
        }
        self.retire();
        Ok(if target == last { wgpu::PollStatus::QueueEmpty } else { wgpu::PollStatus::WaitSucceeded })
    }

    /// Waits until at most `IN_FLIGHT - 1` presented frames are still on the GPU.
    pub(super) fn pace(&self) {
        let oldest = {
            let mut st = self.lock();
            if st.frames.len() < IN_FLIGHT { return }
            st.frames.pop_front()
        };
        if let Some(at) = oldest.filter(|at| self.event.signaledValue() < *at) {
            self.event.waitUntilSignaledValue_timeoutMS(at, 1000);
        }
    }

    pub(super) fn map_after_work(&self, callback: BufferMapCallback) { self.after(Callback::Map(callback)); }

    /// Stamps and commits a buffer recorded outside an encoder, as a submit would.
    pub(super) fn commit(&self, cb: &ProtocolObject<dyn MTLCommandBuffer>) {
        let mut st = self.lock();
        Self::flush_held(&mut st);
        self.commit_uploads(&mut st);
        self.stamp(&mut st, cb, Vec::new());
        cb.commit();
    }
}

impl Drop for Shared {
    /// Runs what was recorded and waits for the GPU to finish with it, since command buffers
    /// don't retain what they use.
    fn drop(&mut self) {
        let last = {
            let mut st = self.lock();
            Self::flush_held(&mut st);
            self.commit_uploads(&mut st);
            st.last
        };
        if self.event.signaledValue() < last { self.event.waitUntilSignaledValue_timeoutMS(last, 10_000); }
    }
}

#[derive(Debug)]
pub(crate) struct MetalQueue(pub(super) Arc<Shared>);

impl QueueInterface for MetalQueue {
    fn write_buffer(&self, buffer: &DispatchBuffer, offset: wgpu::BufferAddress, data: &[u8]) {
        if data.is_empty() { return }
        let store = &buffer.as_custom::<MetalBuffer>().expect("a Metal buffer").store;
        // Written now if nothing recorded uses the storage, or into a spare if only submitted
        // work does: an encoder recorded earlier and submitted later must read these bytes too.
        // Staged, it pins the storage, since a later swap would copy the rest without this write.
        // An encoder counts in `recording` before it reads any storage, which it does under the
        // store's lock, the one `may_swap` is asked under.
        let idle = || self.0.recording.load(std::sync::atomic::Ordering::Acquire) == 0;
        if store.write_ahead(&self.0, offset, data, idle) { return }
        let target = store.gpu_write();
        let staging = Arc::new(BufferRaw::with_bytes(&self.0, data));
        self.0.upload(|blit| {
            // SAFETY: both ranges are in bounds; wgpu checked the target's.
            unsafe { blit.copyFromBuffer_sourceOffset_toBuffer_destinationOffset_size(&staging.raw, 0, &target.raw, offset as usize, data.len()) }
        }, [staging.clone(), target.clone()]);
    }
    fn create_staging_buffer(&self, _: wgpu::BufferSize) -> Option<DispatchQueueWriteBuffer> { None }
    fn validate_write_buffer(&self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: wgpu::BufferSize) -> Option<()> { Some(()) }
    fn write_staging_buffer(&self, _: &DispatchBuffer, _: wgpu::BufferAddress, _: &DispatchQueueWriteBuffer) {
        super::unsupported!("Queue::write_buffer_with")
    }
    fn write_texture(&self, dst: wgpu::TexelCopyTextureInfo<'_>, data: &[u8], layout: wgpu::TexelCopyBufferLayout, size: wgpu::Extent3d) {
        let data = &data[layout.offset as usize..];
        if data.is_empty() || size.width == 0 || size.height == 0 || size.depth_or_array_layers == 0 { return }
        let texture = dst.texture.as_custom::<MetalTexture>().expect("a Metal texture").raw.clone();
        let staging = Arc::new(BufferRaw::with_bytes(&self.0, data));
        let layout = wgpu::TexelCopyBufferLayout { offset: 0, ..layout };
        self.0.upload(|blit| buffer_to_texture(blit, &staging.raw, layout, &texture, dst.mip_level, dst.origin, size), [staging.clone(), texture.clone()]);
    }
    fn submit(&self, command_buffers: &mut dyn Iterator<Item = DispatchCommandBuffer>) -> u64 {
        let last = {
            let mut st = self.0.lock();
            Shared::flush_held(&mut st);
            self.0.commit_uploads(&mut st);
            for buffer in command_buffers {
                let Some(recorded) = buffer.as_custom::<MetalCommandBuffer>().and_then(MetalCommandBuffer::take) else { continue };
                Shared::flush_held(&mut st);
                let drawable = recorded.drawable;
                let (cb, keep) = recorded.close();
                let value = self.0.stamp(&mut st, &cb, keep);
                if drawable { st.held = Some((cb, value)); } else { cb.commit(); }
            }
            st.last
        };
        self.0.retire();
        last
    }
    fn get_timestamp_period(&self) -> f32 { 1.0 }
    fn on_submitted_work_done(&self, callback: BoxSubmittedWorkDoneCallback) { self.0.after(Callback::Done(callback)) }
    fn compact_blas(&self, _: &DispatchBlas) -> (Option<u64>, DispatchBlas) { super::unsupported!("ray tracing") }
    fn present(&self, detail: &DispatchSurfaceOutputDetail) {
        let Some(output) = detail.as_custom::<MetalOutput>() else { return };
        let Some(drawable) = &output.drawable else { return };
        let mut st = self.0.lock();
        let (cb, mut value) = match st.held.take() {
            Some(held) => held,
            None => {
                let cb = self.0.command_buffer();
                let value = self.0.stamp(&mut st, &cb, Vec::new());
                (cb, value)
            }
        };
        #[cfg(target_os = "macos")]
        if let Some(encode) = crate::native_frame::get(output.view) {
            let texture = drawable.texture();
            // SAFETY: borrowed live objects, no open encoder. Hook owns its resource retirement.
            unsafe { encode(objc2_video::rc::Retained::as_ptr(&cb).cast_mut().cast(),
                objc2_video::rc::Retained::as_ptr(&texture).cast_mut().cast(), output.scale); }
            // Pace the entire frame, including the extension, not just Rune's earlier work.
            value = self.0.stamp(&mut st, &cb, Vec::new());
        }
        let drawable = ProtocolObject::<dyn MTLDrawable>::from_ref(&**drawable);
        gpu_trace(&cb);
        if output.transaction {
            cb.commit();
            cb.waitUntilScheduled();
            drawable.present();
        } else {
            cb.presentDrawable(drawable);
            cb.commit();
        }
        st.frames.push_back(value);
    }
}

/// `RUNE_TRACE_GPU=1`: when each frame's command buffer was committed and when the GPU ran
/// it, in host seconds (the clock of `presentedTime`), as `[rune-gpu]` on stderr.
fn gpu_trace(cb: &ProtocolObject<dyn MTLCommandBuffer>) {
    static ON: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var_os("RUNE_TRACE_GPU").is_some());
    if !*ON { return }
    #[link(name = "QuartzCore", kind = "framework")]
    unsafe extern "C" { fn CACurrentMediaTime() -> f64; }
    // SAFETY: no preconditions.
    let committed = unsafe { CACurrentMediaTime() };
    let handler = block2_metal::RcBlock::new(move |cb: std::ptr::NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
        // SAFETY: Metal passes the completed buffer for the handler's duration.
        let cb = unsafe { cb.as_ref() };
        eprintln!("[rune-gpu] commit={committed:.6} start={:.6} end={:.6}", cb.GPUStartTime(), cb.GPUEndTime());
    });
    // SAFETY: Metal copies the block.
    unsafe { cb.addCompletedHandler(block2_metal::RcBlock::as_ptr(&handler)); }
}
