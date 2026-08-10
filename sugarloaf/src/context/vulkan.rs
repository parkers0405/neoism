//! Vulkan backend built directly on `ash`.
//!
//! Mirrors `context::metal::MetalContext` in shape and intent: one struct
//! owning everything needed to present a swapchain image, plus the
//! per-frame synchronisation primitives. No wgpu involvement.
//!
//! Targets Vulkan 1.3 so we can reach for `VK_KHR_dynamic_rendering` (core
//! in 1.3) later without changing device creation. Anyone on a driver
//! older than early-2022 will fail at `create_instance` with
//! `ERROR_INCOMPATIBLE_DRIVER` — same class of failure as an ancient GPU
//! on the Metal path.
//!
//! Surface creation dispatches on `raw-window-handle` variants inline
//! rather than pulling in `ash-window` — that crate is not in Debian and
//! `ash-window` buys us ~30 lines of glue per platform that we'd rather
//! own.

use crate::sugarloaf::{Colorspace, SugarloafWindow, SugarloafWindowSize};
use ash::khr;
use ash::vk;
use ash::{Device, Entry, Instance};
use raw_window_handle::{
    HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
};
use std::ffi::{c_char, CStr};

/// How many frames the CPU is allowed to pipeline ahead of the GPU.
/// Three matches the Metal backend (`MetalLayer::set_maximum_drawable_count(3)`
/// in `context::metal::MetalContext::new`) and Apple's standard sample
/// pattern — CPU / GPU / compositor each work on their own frame in
/// parallel. The cost is two extra `FrameSync` slots and one extra
/// swapchain image's worth of memory.
pub const FRAMES_IN_FLIGHT: usize = 3;

/// Keep the UI thread from parking forever in the Vulkan driver when a
/// compositor/swapchain/fence wait stops making progress.
const FRAME_WAIT_TIMEOUT_NS: u64 = 16_000_000;
const IMAGE_ACQUIRE_TIMEOUT_NS: u64 = 16_000_000;
const VULKAN_DEVICE_ENV: &str = "NEOISM_VULKAN_DEVICE";
const VULKAN_PRESENT_MODE_ENV: &str = "NEOISM_VULKAN_PRESENT_MODE";
const VULKAN_IMAGE_COUNT_ENV: &str = "NEOISM_VULKAN_IMAGE_COUNT";
const VULKAN_FRAME_LOG_ENV: &str = "NEOISM_VULKAN_FRAME_LOG";
const VULKAN_FRAME_SPIKE_US_ENV: &str = "NEOISM_VULKAN_FRAME_SPIKE_US";
const DEFAULT_FRAME_SPIKE_US: u128 = 8_000;

/// Number of live `VulkanContext`s in this process.
///
/// NVIDIA's Vulkan ICD (`libnvidia-glcore`) keeps process-global
/// dispatch state. Destroying one window's `VkInstance` while another
/// window is still rendering nulls out the survivor's swapchain
/// dispatch entry — the next `vkCreateSwapchainKHR` (fired by the
/// resize when the closing window vacates the compositor layout) then
/// jumps through a null function pointer and SIGSEGVs deep inside the
/// driver (`libnvidia-glcore` → `libvulkan` → `create_swapchain`).
///
/// So we defer `destroy_device` / `destroy_surface` / `destroy_instance`
/// until the *last* context drops; earlier drops leak their instance +
/// device (cheap — windows close rarely and the OS reclaims everything
/// at process exit). Mesa/AMD keeps per-instance state and never tripped
/// this, which is why the crash only reproduced on discrete NVIDIA.
static LIVE_VULKAN_CONTEXTS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// One set of synchronisation objects + a command pool & pre-allocated
/// primary buffer, reused each time the same slot comes around. The
/// `in_flight` fence is signalled by the submit that uses this slot so
/// the *next* owner knows the GPU is done with this slot's resources.
struct FrameSync {
    image_available: vk::Semaphore,
    render_finished: vk::Semaphore,
    in_flight: vk::Fence,
    cmd_pool: vk::CommandPool,
    cmd_buffer: vk::CommandBuffer,
}

struct RetiredSwapchain {
    swapchain: vk::SwapchainKHR,
    views: Vec<vk::ImageView>,
}

#[derive(Debug, Clone, Copy)]
struct VulkanFrameLog {
    enabled: bool,
    spike_us: u128,
}

pub struct VulkanContext {
    // Logical fields for the public surface.
    pub size: SugarloafWindowSize,
    pub scale: f32,
    pub supports_f16: bool,
    pub colorspace: Colorspace,
    /// Updated on every `acquire_frame()` — `true` if the driver hinted
    /// that the swapchain is out of date and we should recreate at our
    /// earliest convenience (we already did if ERROR_OUT_OF_DATE_KHR, but
    /// SUBOPTIMAL_KHR says "still usable this frame").
    pub needs_recreate: bool,

    // Per-frame state.
    frame_index: usize,
    frames: [FrameSync; FRAMES_IN_FLIGHT],

    // Swapchain state. Rebuilt by `resize()`.
    swapchain_extent: vk::Extent2D,
    swapchain_color_space: vk::ColorSpaceKHR,
    swapchain_format: vk::Format,
    swapchain_images: Vec<vk::Image>,
    swapchain_views: Vec<vk::ImageView>,
    swapchain: vk::SwapchainKHR,
    retired_swapchains: Vec<RetiredSwapchain>,
    swapchain_loader: khr::swapchain::Device,
    present_mode: vk::PresentModeKHR,
    frame_log: VulkanFrameLog,

    // Core device.
    queue: vk::Queue,
    // Kept around so future phases (atlas uploads, a dedicated transfer
    // pool, pipeline creation) don't have to re-probe the family.
    #[allow(dead_code)]
    queue_family_index: u32,
    device: Device,
    physical_device: vk::PhysicalDevice,

    /// Pipeline cache shared by every `create_graphics_pipelines`
    /// call. Loaded from `~/.cache/rio/sugarloaf-vulkan.cache` (best
    /// effort) at startup and serialised back on `Drop`. Saves
    /// ~10–50ms of pipeline build time on subsequent launches.
    pipeline_cache: vk::PipelineCache,

    // Instance-level state — held last so it outlives everything above in
    // the Drop impl (drop order = declaration order).
    surface: vk::SurfaceKHR,
    surface_loader: khr::surface::Instance,
    /// Debug-utils messenger, present only when validation layers
    /// were requested via `RIO_VULKAN_VALIDATION=1`. Drops before
    /// `instance` (declaration order) so the messenger handle is
    /// destroyed while the instance is still alive.
    _debug_messenger: Option<DebugMessenger>,
    instance: Instance,
    _entry: Entry,
}

/// Owns one `vk::DebugUtilsMessengerEXT` and its loader. Destroyed
/// in `Drop` — the loader needs the parent `Instance` to still be
/// valid, which the field-order convention ensures.
struct DebugMessenger {
    loader: ash::ext::debug_utils::Instance,
    handle: vk::DebugUtilsMessengerEXT,
}

impl Drop for DebugMessenger {
    fn drop(&mut self) {
        unsafe {
            self.loader.destroy_debug_utils_messenger(self.handle, None);
        }
    }
}

/// In-flight handle returned by `acquire_frame()`. The caller records
/// commands into `cmd_buffer` targeting `image` / `image_view`, then
/// hands it back to `present_frame()`.
pub struct VulkanFrame {
    pub image_index: u32,
    pub image: vk::Image,
    pub image_view: vk::ImageView,
    pub cmd_buffer: vk::CommandBuffer,
    pub extent: vk::Extent2D,
    pub format: vk::Format,
    /// Frame-in-flight slot for this frame. Renderers (grid, text,
    /// images) ring their per-frame GPU resources by this index; the
    /// `in_flight` fence wait inside `acquire_frame` proved this slot
    /// is GPU-idle, so writing into slot `N`'s buffers from the CPU
    /// is safe.
    pub slot: usize,
}

impl VulkanContext {
    pub fn new(sugarloaf_window: SugarloafWindow) -> Self {
        let size = sugarloaf_window.size;
        let scale = sugarloaf_window.scale;
        tracing::info!(
            target: "sugarloaf::context",
            raw_display_handle = ?sugarloaf_window.display,
            raw_window_handle = ?sugarloaf_window.handle,
            width = size.width,
            height = size.height,
            scale,
            "initializing native Vulkan context"
        );

        // Loading the loader itself can fail if libvulkan.so is missing —
        // which is the expected failure on a machine without a Vulkan
        // driver installed. We let the panic propagate: the caller is
        // `Context::new` and the backend selection happened upstream, so
        // there's no graceful degradation path here (the WGPU/CPU
        // backends live behind different enum variants).
        let entry =
            unsafe { Entry::load() }.expect("failed to load Vulkan loader (libvulkan)");

        let validation_requested = validation_requested();
        // Skip the NVIDIA ICD on hybrid laptops while the loader builds the
        // instance, so it doesn't resume the runtime-suspended discrete GPU
        // (~2s D3cold wake) when we only ever render on the iGPU. The guard
        // restores the environment right after instance creation.
        let _icd_guard = restrict_icds_avoiding_dgpu(&vulkan_device_preference());
        let instance = create_instance(&entry, &sugarloaf_window, validation_requested);
        drop(_icd_guard);
        let _debug_messenger = if validation_requested {
            create_debug_messenger(&entry, &instance)
        } else {
            None
        };
        let surface_loader = khr::surface::Instance::new(&entry, &instance);
        let surface = create_surface(&entry, &instance, &sugarloaf_window);
        let (physical_device, queue_family_index) =
            pick_physical_device(&instance, &surface_loader, surface);

        let device = create_device(&instance, physical_device, queue_family_index);
        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };
        let pipeline_cache = create_pipeline_cache(&device);

        let swapchain_loader = khr::swapchain::Device::new(&instance, &device);

        let (
            swapchain,
            swapchain_format,
            swapchain_color_space,
            swapchain_extent,
            swapchain_images,
            swapchain_views,
            present_mode,
        ) = create_swapchain(
            &device,
            &surface_loader,
            &swapchain_loader,
            physical_device,
            surface,
            size.width as u32,
            size.height as u32,
            vk::SwapchainKHR::null(),
        );

        let frames = create_frames(&device, queue_family_index);

        // f16 = Vulkan's VK_KHR_shader_float16_int8 feature. Probe at
        // device creation time in a follow-up; for the MVP we report
        // false, matching the conservative default.
        let supports_f16 = false;

        tracing::info!(
            "Vulkan device created: {}",
            physical_device_name(&instance, physical_device)
        );
        tracing::info!(
            "Swapchain: {:?} {}x{} ({} images)",
            swapchain_format,
            swapchain_extent.width,
            swapchain_extent.height,
            swapchain_images.len()
        );
        log_memory_heap_choice(&instance, physical_device);

        // Paired with the decrement in `Drop`. Counts only fully-built
        // contexts — a panic earlier in `new` never runs `Drop`, so the
        // increment lives here at the successful-construction boundary.
        LIVE_VULKAN_CONTEXTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        VulkanContext {
            size,
            scale,
            supports_f16,
            colorspace: Colorspace::Srgb,
            needs_recreate: false,
            frame_index: 0,
            frames,
            swapchain_extent,
            swapchain_color_space,
            swapchain_format,
            swapchain_images,
            swapchain_views,
            swapchain,
            retired_swapchains: Vec::new(),
            swapchain_loader,
            present_mode,
            frame_log: vulkan_frame_log(),
            queue,
            queue_family_index,
            device,
            physical_device,
            pipeline_cache,
            surface,
            surface_loader,
            _debug_messenger,
            instance,
            _entry: entry,
        }
    }

    #[inline]
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
    }

    #[inline]
    pub fn supports_f16(&self) -> bool {
        self.supports_f16
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.size.width = width as f32;
        self.size.height = height as f32;
        self.recreate_swapchain(width, height);
    }

    fn recreate_swapchain(&mut self, width: u32, height: u32) {
        self.collect_retired_swapchains();
        self.swapchain_images.clear();

        let old = self.swapchain;
        let old_views = std::mem::take(&mut self.swapchain_views);
        let (swapchain, format, color_space, extent, images, views, present_mode) =
            create_swapchain(
                &self.device,
                &self.surface_loader,
                &self.swapchain_loader,
                self.physical_device,
                self.surface,
                width,
                height,
                old,
            );

        if old != vk::SwapchainKHR::null() {
            self.retired_swapchains.push(RetiredSwapchain {
                swapchain: old,
                views: old_views,
            });
        }

        self.swapchain = swapchain;
        self.swapchain_format = format;
        self.swapchain_color_space = color_space;
        self.swapchain_extent = extent;
        self.swapchain_images = images;
        self.swapchain_views = views;
        self.present_mode = present_mode;
        self.needs_recreate = false;
        self.collect_retired_swapchains();
    }

    fn frame_fences_are_signaled(&self) -> bool {
        self.frames.iter().all(|frame| unsafe {
            matches!(self.device.get_fence_status(frame.in_flight), Ok(true))
        })
    }

    fn collect_retired_swapchains(&mut self) {
        if self.retired_swapchains.is_empty() || !self.frame_fences_are_signaled() {
            return;
        }

        for retired in self.retired_swapchains.drain(..) {
            for view in retired.views {
                unsafe { self.device.destroy_image_view(view, None) };
            }
            unsafe {
                self.swapchain_loader
                    .destroy_swapchain(retired.swapchain, None)
            };
        }
    }

    /// Return the in-flight fences for every slot EXCEPT `current_slot`.
    /// Atlas (single `vk::Image` shared across all slots) writers must
    /// CPU-wait on these before recording their `TRANSFER_DST` upload
    /// — without it, in-flight slots can still be reading the atlas in
    /// fragment shaders while we queue a write, producing torn glyph
    /// data ghosting on screen for 1-2 frames. See
    /// `VulkanGlyphAtlas::flush_uploads` for the full rationale.
    pub fn other_slot_fences(&self, current_slot: usize) -> Vec<vk::Fence> {
        (0..FRAMES_IN_FLIGHT)
            .filter(|slot| *slot != current_slot)
            .map(|slot| self.frames[slot].in_flight)
            .collect()
    }

    #[inline]
    pub fn surface_format(&self) -> vk::Format {
        self.swapchain_format
    }

    fn next_ready_frame_slot(&self) -> Option<usize> {
        (0..FRAMES_IN_FLIGHT)
            .map(|offset| (self.frame_index + offset) % FRAMES_IN_FLIGHT)
            .find(|&slot| unsafe {
                matches!(
                    self.device.get_fence_status(self.frames[slot].in_flight),
                    Ok(true)
                )
            })
    }

    /// Acquire the next swapchain image and begin the per-frame command
    /// buffer. Returns `None` if the swapchain needed recreation (caller
    /// should skip this frame).
    pub fn acquire_frame(&mut self) -> Option<VulkanFrame> {
        let slot = match self.next_ready_frame_slot() {
            Some(slot) => slot,
            None => {
                let slot = self.frame_index;
                let in_flight = self.frames[slot].in_flight;
                unsafe {
                    match self.device.wait_for_fences(
                        &[in_flight],
                        true,
                        FRAME_WAIT_TIMEOUT_NS,
                    ) {
                        Ok(()) => slot,
                        Err(vk::Result::TIMEOUT) => {
                            tracing::warn!(
                                target: "sugarloaf::vulkan",
                                slot,
                                timeout_ms = FRAME_WAIT_TIMEOUT_NS / 1_000_000,
                                "skipping frame because no Vulkan frame fence became ready"
                            );
                            return None;
                        }
                        Err(e) => panic!("wait_for_fences failed: {e:?}"),
                    }
                }
            }
        };
        self.frame_index = slot;
        self.collect_retired_swapchains();

        let sync = &self.frames[slot];
        let (image_index, suboptimal) = unsafe {
            match self.swapchain_loader.acquire_next_image(
                self.swapchain,
                IMAGE_ACQUIRE_TIMEOUT_NS,
                sync.image_available,
                vk::Fence::null(),
            ) {
                Ok(pair) => pair,
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                    self.recreate_swapchain(
                        self.size.width as u32,
                        self.size.height as u32,
                    );
                    return None;
                }
                Err(vk::Result::TIMEOUT) | Err(vk::Result::NOT_READY) => {
                    tracing::warn!(
                        target: "sugarloaf::vulkan",
                        slot,
                        timeout_ms = IMAGE_ACQUIRE_TIMEOUT_NS / 1_000_000,
                        "skipping frame because Vulkan swapchain image acquire timed out"
                    );
                    return None;
                }
                Err(e) => panic!("acquire_next_image failed: {e:?}"),
            }
        };
        if suboptimal {
            self.needs_recreate = true;
        }

        // Only reset *after* we've committed to submitting — resetting
        // before acquire_next_image would leave us deadlocked if the
        // acquire returned OUT_OF_DATE and we bailed out.
        unsafe {
            self.device
                .reset_fences(&[sync.in_flight])
                .expect("reset_fences");
            self.device
                .reset_command_pool(sync.cmd_pool, vk::CommandPoolResetFlags::empty())
                .expect("reset_command_pool");
            self.device
                .begin_command_buffer(
                    sync.cmd_buffer,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .expect("begin_command_buffer");
        }

        Some(VulkanFrame {
            image_index,
            image: self.swapchain_images[image_index as usize],
            image_view: self.swapchain_views[image_index as usize],
            cmd_buffer: sync.cmd_buffer,
            extent: self.swapchain_extent,
            format: self.swapchain_format,
            slot,
        })
    }

    /// End the command buffer, submit, present, advance frame index.
    pub fn present_frame(&mut self, frame: VulkanFrame) {
        let sync = &self.frames[frame.slot];
        let submit_start = web_time::Instant::now();
        unsafe {
            self.device
                .end_command_buffer(sync.cmd_buffer)
                .expect("end_command_buffer");

            let wait_semaphores = [sync.image_available];
            let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            let signal_semaphores = [sync.render_finished];
            let cmd_buffers = [sync.cmd_buffer];
            let submit = vk::SubmitInfo::default()
                .wait_semaphores(&wait_semaphores)
                .wait_dst_stage_mask(&wait_stages)
                .command_buffers(&cmd_buffers)
                .signal_semaphores(&signal_semaphores);
            self.device
                .queue_submit(self.queue, &[submit], sync.in_flight)
                .expect("queue_submit");

            let swapchains = [self.swapchain];
            let image_indices = [frame.image_index];
            let present_info = vk::PresentInfoKHR::default()
                .wait_semaphores(&signal_semaphores)
                .swapchains(&swapchains)
                .image_indices(&image_indices);
            let present_start = web_time::Instant::now();
            match self
                .swapchain_loader
                .queue_present(self.queue, &present_info)
            {
                Ok(suboptimal) => {
                    if suboptimal {
                        self.needs_recreate = true;
                    }
                }
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                    self.needs_recreate = true;
                }
                Err(e) => panic!("queue_present failed: {e:?}"),
            }
            let present_us = present_start.elapsed().as_micros();
            let submit_total_us = submit_start.elapsed().as_micros();
            if self.frame_log.enabled || present_us >= self.frame_log.spike_us {
                if present_us >= self.frame_log.spike_us {
                    tracing::warn!(
                        target: "sugarloaf::vulkan::frame",
                        slot = frame.slot,
                        image_index = frame.image_index,
                        present_mode = ?self.present_mode,
                        submit_total_us,
                        present_us,
                        spike_threshold_us = self.frame_log.spike_us,
                        "Vulkan queue_submit + queue_present spike"
                    );
                } else {
                    tracing::debug!(
                        target: "sugarloaf::vulkan::frame",
                        slot = frame.slot,
                        image_index = frame.image_index,
                        present_mode = ?self.present_mode,
                        submit_total_us,
                        present_us,
                        spike_threshold_us = self.frame_log.spike_us,
                        "Vulkan queue_submit + queue_present"
                    );
                }
            }
        }

        self.frame_index = (self.frame_index + 1) % FRAMES_IN_FLIGHT;
    }

    /// Expose the underlying device so the renderer can record commands.
    #[inline]
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// Color attachment format the swapchain was created with. Real
    /// pipelines need this at construction time so `VkPipelineRenderingCreateInfo`
    /// can declare a matching color attachment format. Stable across
    /// resize (only `extent` changes there).
    #[inline]
    pub fn swapchain_format(&self) -> vk::Format {
        self.swapchain_format
    }

    /// The instance + physical device that own this context's logical
    /// device. Renderers cache these so they can allocate buffers /
    /// images via the free `allocate_host_visible_buffer_raw` /
    /// `allocate_sampled_image_raw` helpers without needing a live
    /// `&VulkanContext` borrow at every allocation site (chiefly,
    /// `resize` which only has `&mut self`).
    #[inline]
    pub fn instance(&self) -> &Instance {
        &self.instance
    }

    #[inline]
    pub fn physical_device(&self) -> vk::PhysicalDevice {
        self.physical_device
    }

    /// Pipeline cache shared by every renderer's
    /// `create_graphics_pipelines` call. Pass this instead of
    /// `vk::PipelineCache::null()` so cached binaries land on disk
    /// at shutdown and short-circuit subsequent compiles.
    #[inline]
    pub fn pipeline_cache(&self) -> vk::PipelineCache {
        self.pipeline_cache
    }

    /// Run `record` against a transient command buffer, submit it,
    /// wait for completion. Used for one-shot transfer work that
    /// can't piggy-back on the per-frame command buffer (atlas /
    /// image / texture uploads triggered from outside the render
    /// loop, where there's no live `cmd` to append to).
    ///
    /// Allocates a fresh `vk::CommandPool` + `vk::Fence` per call
    /// and tears them down at the end. Cheap (microseconds) compared
    /// to the actual GPU transfer; not a hot path.
    pub fn submit_oneshot<F: FnOnce(vk::CommandBuffer)>(&self, record: F) {
        unsafe {
            let pool_info = vk::CommandPoolCreateInfo::default()
                .queue_family_index(self.queue_family_index)
                .flags(vk::CommandPoolCreateFlags::TRANSIENT);
            let pool = self
                .device
                .create_command_pool(&pool_info, None)
                .expect("create_command_pool(oneshot)");

            let alloc = vk::CommandBufferAllocateInfo::default()
                .command_pool(pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1);
            let cmd = self
                .device
                .allocate_command_buffers(&alloc)
                .expect("allocate_command_buffers(oneshot)")[0];

            let begin = vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            self.device
                .begin_command_buffer(cmd, &begin)
                .expect("begin_command_buffer(oneshot)");

            record(cmd);

            self.device
                .end_command_buffer(cmd)
                .expect("end_command_buffer(oneshot)");

            let fence = self
                .device
                .create_fence(&vk::FenceCreateInfo::default(), None)
                .expect("create_fence(oneshot)");
            let cmds = [cmd];
            let submit = vk::SubmitInfo::default().command_buffers(&cmds);
            self.device
                .queue_submit(self.queue, &[submit], fence)
                .expect("queue_submit(oneshot)");
            self.device
                .wait_for_fences(&[fence], true, u64::MAX)
                .expect("wait_for_fences(oneshot)");

            self.device.destroy_fence(fence, None);
            self.device.destroy_command_pool(pool, None);
        }
    }

    /// Index of the slot the *next* `acquire_frame` will use. Renderers
    /// (grid, text, image overlay) ring their per-frame GPU resources by
    /// this index so that a write into slot N can't race the GPU still
    /// reading from slot N. Stable for the lifetime of `VulkanContext`.
    #[inline]
    pub fn current_frame_slot(&self) -> usize {
        self.frame_index
    }

    /// Allocate a host-visible, host-coherent, persistently-mapped buffer
    /// suitable for per-frame uploads (uniform buffers, vertex/instance
    /// buffers, storage buffers that the CPU writes into and the GPU
    /// reads from this frame). On UMA/integrated GPUs the underlying
    /// memory will also be `DEVICE_LOCAL` (BAR memory) — the driver
    /// picks the best matching type via `memory_type_bits` filtering.
    ///
    /// We do not run a suballocator: each call burns one device memory
    /// allocation. Vulkan guarantees ≥4096 active allocations per
    /// device, and sugarloaf's working set is well under that ceiling
    /// (a couple of atlases + per-frame ring buffers per terminal).
    /// Switch to a slab allocator only if profiling ever shows
    /// allocation churn — current call sites construct once, reuse
    /// thereafter, and only reallocate on grow.
    pub fn allocate_host_visible_buffer(
        &self,
        size: u64,
        usage: vk::BufferUsageFlags,
    ) -> VulkanBuffer {
        // `vkCreateBuffer` rejects zero-sized buffers; bump up to a
        // single byte so callers don't have to special-case empty rings.
        let size = size.max(1);

        let buffer_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe {
            self.device
                .create_buffer(&buffer_info, None)
                .expect("create_buffer")
        };

        let req = unsafe { self.device.get_buffer_memory_requirements(buffer) };
        let mem_type = find_memory_type(
            &self.instance,
            self.physical_device,
            req.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE
                | vk::MemoryPropertyFlags::HOST_COHERENT,
        )
        .expect("no HOST_VISIBLE | HOST_COHERENT memory type — driver bug?");

        let alloc_info = vk::MemoryAllocateInfo::default()
            .allocation_size(req.size)
            .memory_type_index(mem_type);
        let memory = unsafe {
            self.device
                .allocate_memory(&alloc_info, None)
                .expect("allocate_memory")
        };
        unsafe {
            self.device
                .bind_buffer_memory(buffer, memory, 0)
                .expect("bind_buffer_memory");
        }

        // HOST_COHERENT means we never have to flush; mapping stays
        // valid until `vkUnmapMemory`, which we only do at Drop.
        let mapped = unsafe {
            self.device
                .map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
                .expect("map_memory") as *mut u8
        };

        VulkanBuffer {
            device: self.device.clone(),
            buffer,
            memory,
            mapped,
            size,
        }
    }
}

/// Host-visible, persistently-mapped buffer. Written to via [`as_mut_ptr`]
/// (raw pointer; caller owns the layout / bounds checks). The buffer
/// destroys itself + frees its backing memory on drop.
pub struct VulkanBuffer {
    device: Device,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: *mut u8,
    size: u64,
}

// `vk::Buffer`, `vk::DeviceMemory`, and the mapped pointer are all
// values the driver hands out per-allocation; ash::Device is itself
// Send+Sync. Buffers are never shared across threads in sugarloaf, but
// `Send` lets them sit inside `Sugarloaf` (which is not `!Send`).
unsafe impl Send for VulkanBuffer {}
unsafe impl Sync for VulkanBuffer {}

impl VulkanBuffer {
    #[inline]
    pub fn handle(&self) -> vk::Buffer {
        self.buffer
    }

    #[inline]
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Raw pointer to the start of the mapping. Valid for the lifetime
    /// of this `VulkanBuffer`. Writes through this pointer are visible
    /// to the GPU at submit time — `HOST_COHERENT` removes the need for
    /// `vkFlushMappedMemoryRanges`.
    #[inline]
    pub fn as_mut_ptr(&self) -> *mut u8 {
        self.mapped
    }
}

impl Drop for VulkanBuffer {
    fn drop(&mut self) {
        unsafe {
            // Order: unmap, free memory, destroy buffer.
            // `vkFreeMemory` on a non-mapped allocation is safe; we
            // unmap first only because some validation layers warn
            // about freeing memory that's still mapped.
            self.device.unmap_memory(self.memory);
            self.device.destroy_buffer(self.buffer, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

/// Free-function variant of [`VulkanContext::allocate_host_visible_buffer`]
/// for callers that hold cached `(device, instance, physical_device)`
/// rather than a live `&VulkanContext` borrow. The grid / text /
/// image renderers stash those handles at construction time so they
/// can allocate from inside their own `resize` paths (which only have
/// `&mut self`, not the parent context).
pub fn allocate_host_visible_buffer_raw(
    device: &Device,
    instance: &Instance,
    physical_device: vk::PhysicalDevice,
    size: u64,
    usage: vk::BufferUsageFlags,
) -> VulkanBuffer {
    let size = size.max(1);
    let buffer_info = vk::BufferCreateInfo::default()
        .size(size)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = unsafe {
        device
            .create_buffer(&buffer_info, None)
            .expect("create_buffer")
    };
    let req = unsafe { device.get_buffer_memory_requirements(buffer) };
    let mem_type = find_memory_type(
        instance,
        physical_device,
        req.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    )
    .expect("no HOST_VISIBLE | HOST_COHERENT memory type");
    let alloc_info = vk::MemoryAllocateInfo::default()
        .allocation_size(req.size)
        .memory_type_index(mem_type);
    let memory = unsafe {
        device
            .allocate_memory(&alloc_info, None)
            .expect("allocate_memory")
    };
    unsafe {
        device
            .bind_buffer_memory(buffer, memory, 0)
            .expect("bind_buffer_memory");
    }
    let mapped = unsafe {
        device
            .map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
            .expect("map_memory") as *mut u8
    };
    VulkanBuffer {
        device: device.clone(),
        buffer,
        memory,
        mapped,
        size,
    }
}

/// Walks the device's memory types looking for one that matches both
/// `type_filter` (the bitmask returned by `vkGetBufferMemoryRequirements`)
/// and the requested `flags`. Returns `None` if no matching type
/// exists — that's a Vulkan-spec violation the driver should never
/// produce for the standard `HOST_VISIBLE | HOST_COHERENT` and
/// `DEVICE_LOCAL` combinations, but callers should still treat it as
/// fatal rather than ignore it.
fn find_memory_type(
    instance: &Instance,
    physical_device: vk::PhysicalDevice,
    type_filter: u32,
    flags: vk::MemoryPropertyFlags,
) -> Option<u32> {
    let props =
        unsafe { instance.get_physical_device_memory_properties(physical_device) };
    for i in 0..props.memory_type_count {
        let supported = (type_filter & (1 << i)) != 0;
        let matches_flags = props.memory_types[i as usize]
            .property_flags
            .contains(flags);
        if supported && matches_flags {
            return Some(i);
        }
    }
    None
}

/// One-off boot-time log of the memory types we'd pick for our two
/// hot allocation patterns. On UMA / integrated GPUs (Intel iGPU,
/// AMD APU, common Debian-laptop hardware) we expect the
/// host-visible heap to also report `DEVICE_LOCAL` — that's BAR
/// memory and our persistently-mapped per-frame buffers land in fast
/// GPU-accessible memory with no staging copy. On discrete GPUs the
/// host-visible heap is plain system RAM, slower for the GPU to
/// read; we'd want to switch to staging-buffer uploads for hot
/// per-frame data if profiling shows it matters.
fn log_memory_heap_choice(instance: &Instance, physical_device: vk::PhysicalDevice) {
    // Pretend `type_filter = !0` to ignore per-resource alignment
    // filtering — we just want the canonical pick for each pattern.
    let host_visible = find_memory_type(
        instance,
        physical_device,
        !0,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    );
    let device_local = find_memory_type(
        instance,
        physical_device,
        !0,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    );
    let props =
        unsafe { instance.get_physical_device_memory_properties(physical_device) };
    if let Some(idx) = host_visible {
        let flags = props.memory_types[idx as usize].property_flags;
        let bar = flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL);
        tracing::info!(
            "Vulkan host-visible memory: type {} flags={:?} ({})",
            idx,
            flags,
            if bar {
                "BAR / unified — fast GPU reads"
            } else {
                "system RAM — slower GPU reads, consider staging for hot data"
            }
        );
    }
    if let Some(idx) = device_local {
        tracing::info!(
            "Vulkan device-local memory: type {} flags={:?}",
            idx,
            props.memory_types[idx as usize].property_flags
        );
    }
}

// -----------------------------------------------------------------------
// Image helper (device-local 2D image + view + memory)
// -----------------------------------------------------------------------

impl VulkanContext {
    /// Allocate a device-local 2D image suitable for sampling from a
    /// shader (atlas, kitty graphic, background image). Created in
    /// `UNDEFINED` layout — the caller's first transfer command must
    /// include a barrier transitioning to `TRANSFER_DST_OPTIMAL`
    /// before any `vkCmdCopyBufferToImage`.
    pub fn allocate_sampled_image(
        &self,
        width: u32,
        height: u32,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
    ) -> VulkanImage {
        allocate_sampled_image_raw(
            &self.device,
            &self.instance,
            self.physical_device,
            width,
            height,
            format,
            usage,
        )
    }
}

pub fn allocate_sampled_image_raw(
    device: &Device,
    instance: &Instance,
    physical_device: vk::PhysicalDevice,
    width: u32,
    height: u32,
    format: vk::Format,
    usage: vk::ImageUsageFlags,
) -> VulkanImage {
    let image_info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    let image = unsafe {
        device
            .create_image(&image_info, None)
            .expect("create_image")
    };

    let req = unsafe { device.get_image_memory_requirements(image) };
    let mem_type = find_memory_type(
        instance,
        physical_device,
        req.memory_type_bits,
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    )
    .expect("no DEVICE_LOCAL memory type — driver bug?");

    let alloc_info = vk::MemoryAllocateInfo::default()
        .allocation_size(req.size)
        .memory_type_index(mem_type);
    let memory = unsafe {
        device
            .allocate_memory(&alloc_info, None)
            .expect("allocate_memory(image)")
    };
    unsafe {
        device
            .bind_image_memory(image, memory, 0)
            .expect("bind_image_memory");
    }

    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .components(vk::ComponentMapping::default())
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .base_mip_level(0)
                .level_count(1)
                .base_array_layer(0)
                .layer_count(1),
        );
    let view = unsafe {
        device
            .create_image_view(&view_info, None)
            .expect("create_image_view")
    };

    VulkanImage {
        device: device.clone(),
        image,
        view,
        memory,
        width,
        height,
        format,
    }
}

/// Device-local 2D image with view + backing memory. Drops the view,
/// image, and memory on `Drop`. The image starts in `UNDEFINED` layout
/// — the first command that uses it must barrier-transition to a
/// usable layout (`TRANSFER_DST_OPTIMAL` for the initial upload).
pub struct VulkanImage {
    device: Device,
    image: vk::Image,
    view: vk::ImageView,
    memory: vk::DeviceMemory,
    pub width: u32,
    pub height: u32,
    pub format: vk::Format,
}

unsafe impl Send for VulkanImage {}
unsafe impl Sync for VulkanImage {}

impl VulkanImage {
    #[inline]
    pub fn handle(&self) -> vk::Image {
        self.image
    }

    #[inline]
    pub fn view(&self) -> vk::ImageView {
        self.view
    }
}

impl Drop for VulkanImage {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_image_view(self.view, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.memory, None);
        }
    }
}

impl Drop for VulkanContext {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();

            // Best-effort: serialize the pipeline cache to disk
            // before destroying it. Failure (no XDG_CACHE_HOME, no
            // write perms, etc) is logged but not fatal.
            save_pipeline_cache(&self.device, self.pipeline_cache);
            self.device
                .destroy_pipeline_cache(self.pipeline_cache, None);

            for frame in &self.frames {
                self.device.destroy_semaphore(frame.image_available, None);
                self.device.destroy_semaphore(frame.render_finished, None);
                self.device.destroy_fence(frame.in_flight, None);
                self.device.destroy_command_pool(frame.cmd_pool, None);
            }

            for retired in self.retired_swapchains.drain(..) {
                for view in retired.views {
                    self.device.destroy_image_view(view, None);
                }
                self.swapchain_loader
                    .destroy_swapchain(retired.swapchain, None);
            }

            for &view in &self.swapchain_views {
                self.device.destroy_image_view(view, None);
            }
            self.swapchain_loader
                .destroy_swapchain(self.swapchain, None);

            // Only tear down the device / surface / instance when this is
            // the final live context in the process. Destroying an
            // instance while a sibling window is still rendering corrupts
            // the NVIDIA driver's process-global dispatch table and
            // SIGSEGVs the survivor's next `vkCreateSwapchainKHR`. See
            // `LIVE_VULKAN_CONTEXTS`. `fetch_sub` returns the value
            // *before* the subtraction, so `remaining` is the count that
            // stays alive after this drop.
            let remaining = LIVE_VULKAN_CONTEXTS
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst)
                .saturating_sub(1);
            if remaining == 0 {
                self.device.destroy_device(None);
                self.surface_loader.destroy_surface(self.surface, None);
                self.instance.destroy_instance(None);
            } else {
                tracing::warn!(
                    target: "sugarloaf::context::vulkan",
                    remaining,
                    "deferring VkInstance/VkDevice teardown until the last window closes \
                     (NVIDIA multi-instance swapchain crash guard); leaking this \
                     window's instance/device until process exit"
                );
            }
        }
    }
}

// =======================================================================
// Pipeline cache (load on `new`, save on `Drop`)
// =======================================================================

/// Path to the on-disk pipeline cache. Returns `None` if neither
/// `XDG_CACHE_HOME` nor `HOME` is set.
fn pipeline_cache_path() -> Option<std::path::PathBuf> {
    let dir = if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        std::path::PathBuf::from(xdg)
    } else if let Some(home) = std::env::var_os("HOME") {
        let mut p = std::path::PathBuf::from(home);
        p.push(".cache");
        p
    } else {
        return None;
    };
    Some(dir.join("rio").join("sugarloaf-vulkan.cache"))
}

fn create_pipeline_cache(device: &Device) -> vk::PipelineCache {
    let initial_data: Vec<u8> = pipeline_cache_path()
        .and_then(|p| std::fs::read(&p).ok())
        .unwrap_or_default();
    if !initial_data.is_empty() {
        tracing::info!("loaded Vulkan pipeline cache: {} bytes", initial_data.len());
    }
    let info = vk::PipelineCacheCreateInfo::default().initial_data(&initial_data);
    unsafe {
        device
            .create_pipeline_cache(&info, None)
            .expect("create_pipeline_cache")
    }
}

fn save_pipeline_cache(device: &Device, cache: vk::PipelineCache) {
    let Some(path) = pipeline_cache_path() else {
        return;
    };
    let data = match unsafe { device.get_pipeline_cache_data(cache) } {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("get_pipeline_cache_data failed: {:?}", e);
            return;
        }
    };
    if data.is_empty() {
        return;
    }
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!("pipeline cache mkdir {:?} failed: {}", parent, e);
            return;
        }
    }
    if let Err(e) = std::fs::write(&path, &data) {
        tracing::warn!("pipeline cache write {:?} failed: {}", path, e);
    } else {
        tracing::info!(
            "saved Vulkan pipeline cache: {} bytes → {:?}",
            data.len(),
            path
        );
    }
}

// -------------------------------------------------------------------------
// Internal helpers (free functions so `new()` stays readable).
// -------------------------------------------------------------------------

fn create_instance(
    entry: &Entry,
    window: &SugarloafWindow,
    enable_validation: bool,
) -> Instance {
    let app_name = c"sugarloaf";
    let app_info = vk::ApplicationInfo::default()
        .application_name(app_name)
        .application_version(0)
        .engine_name(app_name)
        .engine_version(0)
        .api_version(vk::API_VERSION_1_3);

    // KHR_surface + the right platform surface extension for the window
    // handle we were given. Adding extensions the driver doesn't
    // advertise makes `create_instance` fail, so we match the window
    // type exactly instead of asking for all three.
    let mut extensions: Vec<*const c_char> = vec![khr::surface::NAME.as_ptr()];
    match window.display_handle().unwrap().as_raw() {
        RawDisplayHandle::Xlib(_) => extensions.push(khr::xlib_surface::NAME.as_ptr()),
        RawDisplayHandle::Xcb(_) => extensions.push(khr::xcb_surface::NAME.as_ptr()),
        RawDisplayHandle::Wayland(_) => {
            extensions.push(khr::wayland_surface::NAME.as_ptr())
        }
        other => panic!("Vulkan backend: unsupported display handle {:?}", other),
    }

    // Validation: append `VK_EXT_debug_utils` so we can install a
    // messenger callback after instance creation. The layer
    // (`VK_LAYER_KHRONOS_validation`) is enabled separately via
    // `enabled_layer_names` below.
    let validation_layer_name = c"VK_LAYER_KHRONOS_validation";
    let layer_ptrs: Vec<*const c_char> = if enable_validation {
        if validation_layer_available(entry, validation_layer_name) {
            extensions.push(ash::ext::debug_utils::NAME.as_ptr());
            vec![validation_layer_name.as_ptr()]
        } else {
            tracing::warn!(
                "RIO_VULKAN_VALIDATION set but VK_LAYER_KHRONOS_validation \
                 not available — install `vulkan-validationlayers` (Debian) \
                 / `vulkan-validation-layers` (Arch) to enable it"
            );
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let create_info = vk::InstanceCreateInfo::default()
        .application_info(&app_info)
        .enabled_extension_names(&extensions)
        .enabled_layer_names(&layer_ptrs);

    unsafe { entry.create_instance(&create_info, None) }
        .expect("vkCreateInstance failed — is a Vulkan 1.3 driver installed?")
}

/// True if the user opted into validation via `RIO_VULKAN_VALIDATION=1`.
/// We always read the env var (debug + release) so users can flip it
/// on for one run without recompiling.
fn validation_requested() -> bool {
    std::env::var_os("RIO_VULKAN_VALIDATION")
        .map(|v| v != "0" && !v.is_empty())
        .unwrap_or(false)
}

fn validation_layer_available(entry: &Entry, target: &CStr) -> bool {
    match unsafe { entry.enumerate_instance_layer_properties() } {
        Ok(layers) => layers.iter().any(|l| {
            let name = unsafe { CStr::from_ptr(l.layer_name.as_ptr()) };
            name == target
        }),
        Err(_) => false,
    }
}

fn create_debug_messenger(entry: &Entry, instance: &Instance) -> Option<DebugMessenger> {
    let loader = ash::ext::debug_utils::Instance::new(entry, instance);

    let info = vk::DebugUtilsMessengerCreateInfoEXT::default()
        .message_severity(
            vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING
                | vk::DebugUtilsMessageSeverityFlagsEXT::INFO,
        )
        .message_type(
            vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
        )
        .pfn_user_callback(Some(debug_callback));

    let handle = unsafe { loader.create_debug_utils_messenger(&info, None) }
        .expect("create_debug_utils_messenger");
    tracing::info!("Vulkan validation layers active");
    Some(DebugMessenger { loader, handle })
}

unsafe extern "system" fn debug_callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    msg_type: vk::DebugUtilsMessageTypeFlagsEXT,
    callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user_data: *mut std::ffi::c_void,
) -> vk::Bool32 {
    let data = unsafe { &*callback_data };
    let message = if data.p_message.is_null() {
        std::borrow::Cow::Borrowed("<null>")
    } else {
        unsafe { CStr::from_ptr(data.p_message) }.to_string_lossy()
    };
    let kind = if msg_type.contains(vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION) {
        "validation"
    } else if msg_type.contains(vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE) {
        "perf"
    } else {
        "general"
    };
    if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        tracing::error!("vk[{}] {}", kind, message);
    } else if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING) {
        tracing::warn!("vk[{}] {}", kind, message);
    } else if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::INFO) {
        tracing::info!("vk[{}] {}", kind, message);
    } else {
        tracing::debug!("vk[{}] {}", kind, message);
    }
    vk::FALSE
}

fn create_surface(
    entry: &Entry,
    instance: &Instance,
    window: &SugarloafWindow,
) -> vk::SurfaceKHR {
    let display = window.display_handle().unwrap().as_raw();
    let window_handle = window.window_handle().unwrap().as_raw();

    unsafe {
        match (display, window_handle) {
            (RawDisplayHandle::Xlib(d), RawWindowHandle::Xlib(w)) => {
                let loader = khr::xlib_surface::Instance::new(entry, instance);
                let info = vk::XlibSurfaceCreateInfoKHR::default()
                    .dpy(
                        d.display
                            .expect("Xlib display pointer missing")
                            .as_ptr()
                            .cast(),
                    )
                    .window(w.window);
                loader
                    .create_xlib_surface(&info, None)
                    .expect("create_xlib_surface")
            }
            (RawDisplayHandle::Xcb(d), RawWindowHandle::Xcb(w)) => {
                let loader = khr::xcb_surface::Instance::new(entry, instance);
                let info = vk::XcbSurfaceCreateInfoKHR::default()
                    .connection(
                        d.connection
                            .expect("Xcb connection pointer missing")
                            .as_ptr()
                            .cast(),
                    )
                    .window(w.window.get());
                loader
                    .create_xcb_surface(&info, None)
                    .expect("create_xcb_surface")
            }
            (RawDisplayHandle::Wayland(d), RawWindowHandle::Wayland(w)) => {
                let loader = khr::wayland_surface::Instance::new(entry, instance);
                let info = vk::WaylandSurfaceCreateInfoKHR::default()
                    .display(d.display.as_ptr().cast())
                    .surface(w.surface.as_ptr().cast());
                loader
                    .create_wayland_surface(&info, None)
                    .expect("create_wayland_surface")
            }
            (d, w) => panic!(
                "Vulkan backend: mismatched or unsupported handles: display={d:?} window={w:?}"
            ),
        }
    }
}

#[derive(Debug)]
enum VulkanDevicePreference {
    Auto,
    Integrated,
    Discrete,
    Cpu,
    Name(String),
}

fn vulkan_device_preference() -> VulkanDevicePreference {
    let Ok(raw) = std::env::var(VULKAN_DEVICE_ENV) else {
        return VulkanDevicePreference::Auto;
    };
    let value = raw.trim().to_ascii_lowercase();
    match value.as_str() {
        "" | "auto" => VulkanDevicePreference::Auto,
        "integrated" | "igpu" | "apu" => VulkanDevicePreference::Integrated,
        "discrete" | "dgpu" => VulkanDevicePreference::Discrete,
        "cpu" | "software" => VulkanDevicePreference::Cpu,
        other => VulkanDevicePreference::Name(other.to_owned()),
    }
}

/// RAII guard restoring `VK_ICD_FILENAMES` to its prior value on drop.
struct IcdEnvGuard {
    prev: Option<std::ffi::OsString>,
    applied: bool,
}

impl Drop for IcdEnvGuard {
    fn drop(&mut self) {
        if !self.applied {
            return;
        }
        match self.prev.take() {
            Some(value) => std::env::set_var("VK_ICD_FILENAMES", value),
            None => std::env::remove_var("VK_ICD_FILENAMES"),
        }
    }
}

/// Keep `vkCreateInstance` from waking a runtime-suspended discrete GPU.
///
/// The Vulkan loader `dlopen`s EVERY installed ICD to enumerate adapters,
/// including the NVIDIA driver — whose init resumes the dGPU from D3cold
/// (~2s) on a hybrid laptop, even though we render exclusively on the
/// integrated GPU. When our scoring is going to land on the iGPU anyway,
/// point the loader at just the non-NVIDIA ICDs for the duration of
/// instance creation, then restore the environment via the returned guard
/// so child PTYs / GPU apps the user launches keep full driver access.
///
/// Conservative + overridable: respects a user-set `VK_ICD_FILENAMES` /
/// `VK_DRIVER_FILES` / `VK_LOADER_DRIVERS_DISABLE`; only acts for an
/// explicit `Integrated` preference or `Auto` on Wayland (where the
/// compositor scans out through the iGPU); and only when an NVIDIA ICD is
/// present AND at least one non-NVIDIA ICD remains, so the loader is never
/// left with zero drivers.
fn restrict_icds_avoiding_dgpu(preference: &VulkanDevicePreference) -> IcdEnvGuard {
    let noop = IcdEnvGuard {
        prev: None,
        applied: false,
    };

    if std::env::var_os("VK_ICD_FILENAMES").is_some()
        || std::env::var_os("VK_DRIVER_FILES").is_some()
        || std::env::var_os("VK_LOADER_DRIVERS_DISABLE").is_some()
    {
        return noop;
    }

    let want_integrated = match preference {
        VulkanDevicePreference::Integrated => true,
        VulkanDevicePreference::Auto => std::env::var_os("WAYLAND_DISPLAY").is_some(),
        _ => false,
    };
    if !want_integrated {
        return noop;
    }

    let mut dirs: Vec<std::path::PathBuf> = vec![
        "/usr/share/vulkan/icd.d".into(),
        "/usr/local/share/vulkan/icd.d".into(),
        "/etc/vulkan/icd.d".into(),
    ];
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(std::path::PathBuf::from(xdg).join("vulkan/icd.d"));
    }

    let mut keep: Vec<String> = Vec::new();
    let mut saw_nvidia = false;
    for dir in &dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let body = std::fs::read_to_string(&path)
                .unwrap_or_default()
                .to_ascii_lowercase();
            if name.contains("nvidia") || body.contains("nvidia") {
                saw_nvidia = true;
            } else {
                keep.push(path.to_string_lossy().into_owned());
            }
        }
    }

    if !saw_nvidia || keep.is_empty() {
        return noop;
    }

    let prev = std::env::var_os("VK_ICD_FILENAMES");
    std::env::set_var("VK_ICD_FILENAMES", keep.join(":"));
    tracing::info!(
        target: "sugarloaf::context::vulkan",
        kept = %keep.join(":"),
        "restricting Vulkan ICDs to non-NVIDIA for instance creation to avoid waking the discrete GPU"
    );
    IcdEnvGuard {
        prev,
        applied: true,
    }
}

fn vulkan_frame_log() -> VulkanFrameLog {
    VulkanFrameLog {
        enabled: env_flag(VULKAN_FRAME_LOG_ENV),
        spike_us: std::env::var(VULKAN_FRAME_SPIKE_US_ENV)
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(DEFAULT_FRAME_SPIKE_US),
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| {
        let value = value.to_string_lossy();
        !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_mode_override_accepts_common_aliases() {
        let supported = [vk::PresentModeKHR::FIFO, vk::PresentModeKHR::MAILBOX];

        assert_eq!(
            parse_present_mode_override("mailbox", &supported),
            Some(vk::PresentModeKHR::MAILBOX)
        );
        assert_eq!(
            parse_present_mode_override("vsync", &supported),
            Some(vk::PresentModeKHR::FIFO)
        );
        assert_eq!(parse_present_mode_override("auto", &supported), None);
    }

    #[test]
    fn present_mode_override_rejects_unsupported_modes() {
        let supported = [vk::PresentModeKHR::FIFO];

        assert_eq!(parse_present_mode_override("mailbox", &supported), None);
        assert_eq!(parse_present_mode_override("not-a-mode", &supported), None);
    }

    #[test]
    fn desired_swapchain_image_count_clamps_to_surface_caps() {
        let mut caps = vk::SurfaceCapabilitiesKHR::default();
        caps.min_image_count = 2;
        caps.max_image_count = 2;
        assert_eq!(desired_swapchain_image_count_from(None, &caps), 2);
        assert_eq!(desired_swapchain_image_count_from(Some("99"), &caps), 2);

        caps.max_image_count = 0;
        assert_eq!(desired_swapchain_image_count_from(Some("1"), &caps), 2);
        assert_eq!(desired_swapchain_image_count_from(Some("4"), &caps), 4);
    }

    #[test]
    fn native_renderer_enables_every_vulkan13_feature_it_uses() {
        let features = required_vulkan13_features();
        assert_eq!(features.dynamic_rendering, vk::TRUE);
        assert_eq!(features.synchronization2, vk::TRUE);
    }
}

fn requested_present_mode(
    supported_modes: &[vk::PresentModeKHR],
) -> Option<vk::PresentModeKHR> {
    let Ok(raw) = std::env::var(VULKAN_PRESENT_MODE_ENV) else {
        return None;
    };
    parse_present_mode_override(&raw, supported_modes)
}

fn parse_present_mode_override(
    raw: &str,
    supported_modes: &[vk::PresentModeKHR],
) -> Option<vk::PresentModeKHR> {
    let normalized = raw.trim().to_ascii_lowercase().replace('-', "_");
    let requested = match normalized.as_str() {
        "fifo" | "vsync" => vk::PresentModeKHR::FIFO,
        "mailbox" | "low_latency" => vk::PresentModeKHR::MAILBOX,
        "immediate" | "no_vsync" => vk::PresentModeKHR::IMMEDIATE,
        "fifo_relaxed" | "relaxed" => vk::PresentModeKHR::FIFO_RELAXED,
        "" | "auto" => return None,
        other => {
            tracing::warn!(
                target: "sugarloaf::context::vulkan",
                env = VULKAN_PRESENT_MODE_ENV,
                value = other,
                "ignoring unknown Vulkan present mode override"
            );
            return None;
        }
    };

    if supported_modes.contains(&requested) {
        Some(requested)
    } else {
        tracing::warn!(
            target: "sugarloaf::context::vulkan",
            env = VULKAN_PRESENT_MODE_ENV,
            requested = ?requested,
            ?supported_modes,
            "Vulkan present mode override unsupported by surface; using automatic choice"
        );
        None
    }
}

fn desired_swapchain_image_count(caps: &vk::SurfaceCapabilitiesKHR) -> u32 {
    desired_swapchain_image_count_from(
        std::env::var(VULKAN_IMAGE_COUNT_ENV).ok().as_deref(),
        caps,
    )
}

fn desired_swapchain_image_count_from(
    raw: Option<&str>,
    caps: &vk::SurfaceCapabilitiesKHR,
) -> u32 {
    let mut desired = raw
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(3)
        .max(1);

    desired = desired.max(caps.min_image_count);
    if caps.max_image_count != 0 && desired > caps.max_image_count {
        desired = caps.max_image_count;
    }
    desired
}

fn score_physical_device(
    preference: &VulkanDevicePreference,
    device_type: vk::PhysicalDeviceType,
    device_name: &str,
) -> i32 {
    let type_rank = match device_type {
        vk::PhysicalDeviceType::DISCRETE_GPU => 1000,
        vk::PhysicalDeviceType::INTEGRATED_GPU => 800,
        vk::PhysicalDeviceType::VIRTUAL_GPU => 100,
        vk::PhysicalDeviceType::CPU => 10,
        _ => 1,
    };

    match preference {
        // On Wayland hybrid laptops, the compositor commonly scans out
        // through the integrated GPU. Prefer it in Auto so we avoid the
        // dGPU -> iGPU PRIME present hop that shows up as uneven frame
        // pacing; keep the old discrete preference for non-Wayland.
        VulkanDevicePreference::Auto => {
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                match device_type {
                    vk::PhysicalDeviceType::INTEGRATED_GPU => 2000,
                    vk::PhysicalDeviceType::DISCRETE_GPU => 1500,
                    _ => type_rank,
                }
            } else {
                type_rank
            }
        }
        VulkanDevicePreference::Integrated => match device_type {
            vk::PhysicalDeviceType::INTEGRATED_GPU => 3000,
            _ => type_rank,
        },
        VulkanDevicePreference::Discrete => match device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU => 3000,
            _ => type_rank,
        },
        VulkanDevicePreference::Cpu => match device_type {
            vk::PhysicalDeviceType::CPU => 3000,
            _ => type_rank,
        },
        VulkanDevicePreference::Name(needle) => {
            if device_name.to_ascii_lowercase().contains(needle) {
                4000 + type_rank
            } else {
                type_rank
            }
        }
    }
}

/// Pick a physical device + queue family. Require a queue family that
/// supports both graphics and present on our surface. Device preference
/// is controlled by `NEOISM_VULKAN_DEVICE`; Auto prefers integrated GPU
/// on Wayland to avoid hybrid-laptop cross-GPU present jitter.
fn pick_physical_device(
    instance: &Instance,
    surface_loader: &khr::surface::Instance,
    surface: vk::SurfaceKHR,
) -> (vk::PhysicalDevice, u32) {
    let devices = unsafe { instance.enumerate_physical_devices() }
        .expect("enumerate_physical_devices");
    let preference = vulkan_device_preference();

    let mut best: Option<(vk::PhysicalDevice, u32, i32, String)> = None;
    for device in devices {
        let props = unsafe { instance.get_physical_device_properties(device) };
        let device_name = physical_device_name(instance, device);
        let qf_props =
            unsafe { instance.get_physical_device_queue_family_properties(device) };

        for (index, qf) in qf_props.iter().enumerate() {
            let index = index as u32;
            if !qf.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
                continue;
            }
            let present_ok = unsafe {
                surface_loader
                    .get_physical_device_surface_support(device, index, surface)
                    .unwrap_or(false)
            };
            if !present_ok {
                continue;
            }
            let score =
                score_physical_device(&preference, props.device_type, &device_name);
            tracing::info!(
                target: "sugarloaf::context::vulkan",
                device_name,
                device_type = ?props.device_type,
                queue_family = index,
                score,
                preference = ?preference,
                override_env = std::env::var(VULKAN_DEVICE_ENV).ok().as_deref(),
                "Vulkan candidate device"
            );
            if best.as_ref().map(|(_, _, s, _)| score > *s).unwrap_or(true) {
                best = Some((device, index, score, device_name.clone()));
            }
        }
    }

    let (device, queue_family, score, device_name) =
        best.expect("no Vulkan device with graphics + present support on this surface");
    tracing::info!(
        target: "sugarloaf::context::vulkan",
        selected_device = device_name,
        queue_family,
        score,
        preference = ?preference,
        override_env = std::env::var(VULKAN_DEVICE_ENV).ok().as_deref(),
        "selected Vulkan physical device"
    );
    (device, queue_family)
}

fn physical_device_name(instance: &Instance, device: vk::PhysicalDevice) -> String {
    let props = unsafe { instance.get_physical_device_properties(device) };
    // `device_name` is a C string embedded in a fixed-size array.
    let raw = props.device_name.as_ptr();
    unsafe { CStr::from_ptr(raw) }
        .to_string_lossy()
        .into_owned()
}

fn create_device(
    instance: &Instance,
    physical_device: vk::PhysicalDevice,
    queue_family_index: u32,
) -> Device {
    let queue_priorities = [1.0f32];
    let queue_info = vk::DeviceQueueCreateInfo::default()
        .queue_family_index(queue_family_index)
        .queue_priorities(&queue_priorities);

    let device_extensions = [khr::swapchain::NAME.as_ptr()];

    let mut vk13_features = required_vulkan13_features();

    let queue_infos = [queue_info];
    let create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queue_infos)
        .enabled_extension_names(&device_extensions)
        .push_next(&mut vk13_features);

    unsafe { instance.create_device(physical_device, &create_info, None) }
        .expect("vkCreateDevice")
}

/// Vulkan 1.3 features used unconditionally by the native renderer.
///
/// `dynamic_rendering` is required by `vkCmdBeginRendering`.  Equally
/// importantly, `synchronization2` is required by every
/// `vkCmdPipelineBarrier2` call used to transition swapchain and atlas
/// images.  Merely requesting a Vulkan 1.3 instance does not enable either
/// device feature.  Mesa happened to tolerate the missing synchronization2
/// enable, while NVIDIA could leave the image transitions ineffective and
/// present an all-black frame.
fn required_vulkan13_features() -> vk::PhysicalDeviceVulkan13Features<'static> {
    vk::PhysicalDeviceVulkan13Features::default()
        .dynamic_rendering(true)
        .synchronization2(true)
}

/// Build a swapchain and its image views. `old` is passed as
/// `old_swapchain` so the driver can recycle images during resize.
#[allow(clippy::too_many_arguments)]
fn create_swapchain(
    device: &Device,
    surface_loader: &khr::surface::Instance,
    swapchain_loader: &khr::swapchain::Device,
    physical_device: vk::PhysicalDevice,
    surface: vk::SurfaceKHR,
    requested_width: u32,
    requested_height: u32,
    old: vk::SwapchainKHR,
) -> (
    vk::SwapchainKHR,
    vk::Format,
    vk::ColorSpaceKHR,
    vk::Extent2D,
    Vec<vk::Image>,
    Vec<vk::ImageView>,
    vk::PresentModeKHR,
) {
    let caps = unsafe {
        surface_loader
            .get_physical_device_surface_capabilities(physical_device, surface)
            .expect("get_physical_device_surface_capabilities")
    };
    let formats = unsafe {
        surface_loader
            .get_physical_device_surface_formats(physical_device, surface)
            .expect("get_physical_device_surface_formats")
    };
    // Prefer BGRA8_UNORM (linear) so blending stays in gamma space — the
    // same choice Metal makes (`MTLPixelFormat::BGRA8Unorm` + DisplayP3
    // tag). Fragment shaders will emit sRGB-encoded output. If the
    // driver doesn't offer BGRA8_UNORM, fall back to whatever it gives
    // us — formats[0] is guaranteed present per the spec.
    let chosen_format = formats
        .iter()
        .find(|f| {
            f.format == vk::Format::B8G8R8A8_UNORM
                && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
        })
        .copied()
        .unwrap_or(formats[0]);

    // Present mode: keep MAILBOX as the automatic choice when available
    // because it improves high-refresh Wayland/Hyprland pacing on Mesa,
    // but expose a runtime override so users can compare FIFO vs MAILBOX
    // on their compositor/driver without rebuilding.
    let supported_modes = unsafe {
        surface_loader
            .get_physical_device_surface_present_modes(physical_device, surface)
            .unwrap_or_else(|_| vec![vk::PresentModeKHR::FIFO])
    };
    let auto_present_mode = if supported_modes.contains(&vk::PresentModeKHR::MAILBOX) {
        vk::PresentModeKHR::MAILBOX
    } else {
        vk::PresentModeKHR::FIFO
    };
    let present_mode =
        requested_present_mode(&supported_modes).unwrap_or(auto_present_mode);
    tracing::info!(
        target: "sugarloaf::context::vulkan",
        ?present_mode,
        ?auto_present_mode,
        ?supported_modes,
        override_env = std::env::var(VULKAN_PRESENT_MODE_ENV).ok().as_deref(),
        "Vulkan swapchain present mode"
    );

    let extent = if caps.current_extent.width != u32::MAX {
        caps.current_extent
    } else {
        vk::Extent2D {
            width: requested_width
                .clamp(caps.min_image_extent.width, caps.max_image_extent.width),
            height: requested_height
                .clamp(caps.min_image_extent.height, caps.max_image_extent.height),
        }
    };

    // Aim for 3 images where the driver allows it (triple buffering),
    // clamped to the advertised range. `max_image_count == 0` means "no
    // upper limit". Override is intentionally image-count only; frames in
    // flight stay fixed to avoid broader synchronization churn.
    let image_count = desired_swapchain_image_count(&caps);
    tracing::info!(
        target: "sugarloaf::context::vulkan",
        image_count,
        min_image_count = caps.min_image_count,
        max_image_count = caps.max_image_count,
        override_env = std::env::var(VULKAN_IMAGE_COUNT_ENV).ok().as_deref(),
        "Vulkan swapchain image count"
    );

    let create_info = vk::SwapchainCreateInfoKHR::default()
        .surface(surface)
        .min_image_count(image_count)
        .image_format(chosen_format.format)
        .image_color_space(chosen_format.color_space)
        .image_extent(extent)
        .image_array_layers(1)
        .image_usage(
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST,
        )
        .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
        .pre_transform(caps.current_transform)
        .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
        .present_mode(present_mode)
        .clipped(true)
        .old_swapchain(old);

    let swapchain = unsafe { swapchain_loader.create_swapchain(&create_info, None) }
        .expect("create_swapchain");

    let images = unsafe { swapchain_loader.get_swapchain_images(swapchain) }
        .expect("get_swapchain_images");

    let views = images
        .iter()
        .map(|&image| {
            let info = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(chosen_format.format)
                .components(vk::ComponentMapping::default())
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .base_mip_level(0)
                        .level_count(1)
                        .base_array_layer(0)
                        .layer_count(1),
                );
            unsafe { device.create_image_view(&info, None) }.expect("create_image_view")
        })
        .collect();

    (
        swapchain,
        chosen_format.format,
        chosen_format.color_space,
        extent,
        images,
        views,
        present_mode,
    )
}

fn create_frames(
    device: &Device,
    queue_family_index: u32,
) -> [FrameSync; FRAMES_IN_FLIGHT] {
    std::array::from_fn(|_| {
        let semaphore_info = vk::SemaphoreCreateInfo::default();
        let fence_info =
            vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(queue_family_index)
            .flags(vk::CommandPoolCreateFlags::TRANSIENT);

        unsafe {
            let image_available = device
                .create_semaphore(&semaphore_info, None)
                .expect("create_semaphore");
            let render_finished = device
                .create_semaphore(&semaphore_info, None)
                .expect("create_semaphore");
            let in_flight = device
                .create_fence(&fence_info, None)
                .expect("create_fence");
            let cmd_pool = device
                .create_command_pool(&pool_info, None)
                .expect("create_command_pool");
            let alloc_info = vk::CommandBufferAllocateInfo::default()
                .command_pool(cmd_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1);
            let cmd_buffer = device
                .allocate_command_buffers(&alloc_info)
                .expect("allocate_command_buffers")[0];

            FrameSync {
                image_available,
                render_finished,
                in_flight,
                cmd_pool,
                cmd_buffer,
            }
        }
    })
}
