---
name: bug_nvidia_multiwindow_close_crash
description: Closing 1 of 2 windows killed ALL windows on NVIDIA (not AMD) — VkInstance teardown corrupts driver global state; FIXED
metadata: 
  node_type: memory
  type: project
  originSessionId: 7f62545c-19db-444c-86e4-3b6a6574bcc7
  modified: 2026-07-29T04:31:33.349Z
---

Multi-window: super+w one window → ALL neoism windows crash, but ONLY on
discrete NVIDIA (RTX 3090 piss-desktop), NOT on AMD 370 iGPU (Framework).

Root cause (symbolized coredump on piss-desktop): SIGSEGV, `#0 0x0` (call
through null fn ptr) → `libnvidia-glcore.so` → `libvulkan.so` →
`ash create_swapchain` → `sugarloaf::context::vulkan::create_swapchain`
(vulkan.rs) ← `recreate_swapchain` ← `resize`. Each window has its OWN
`Entry`+`Instance`+`Device` (`VulkanContext::new` per window). Closing one
window's `Drop` called `destroy_instance`; NVIDIA's ICD keeps process-global
dispatch state, so destroying one instance while a sibling still renders
NULLS OUT the survivor's swapchain dispatch entry. The survivor's next
`vkCreateSwapchainKHR` (fired by the resize when the closed window vacates
the Hyprland layout) then jumps through null → SIGSEGV inside the driver,
taking down every window. Mesa/AMD keeps per-instance state → never hit it.

Fix (vulkan.rs): process-global `LIVE_VULKAN_CONTEXTS` atomic; `Drop` defers
`destroy_device`/`destroy_surface`/`destroy_instance` until the LAST context
drops (remaining==0). Earlier closes leak instance/device (cheap — windows
close rarely, OS reclaims at exit). Verified live on piss-desktop via
`hyprctl dispatch closewindow`: close-1-of-2 → survivor lives + guard warn
`remaining=1` + no coredump; close-last → clean exit. Commit e2caa91c on
better_workspace2.

**Why:** the OutOfMemory-panic theory was WRONG — it's a driver null-deref,
not a Rust panic. **How to apply:** N independent VkInstances per process is
an NVIDIA foot-gun; the "correct" long-term fix is ONE shared instance+device
+ N surfaces (wgpu/Zed do this), but the deferred-teardown guard fixes the
crash without that refactor. Related: [[bug_f32_epoch_animation_clock]],
[[bug_shader_terminal_stutter]], [[bug_nvim_lsp_orphan_swap]].

To reproduce/test on desktop: derive GUI env from Hyprland socket dirs
(WAYLAND_DISPLAY from /run/user/1000/wayland-*, HYPRLAND_INSTANCE_SIGNATURE
from /run/user/1000/hypr/), launch `neoism` then `neoism --new-window`,
`hyprctl dispatch closewindow address:0x...`, check `pgrep -x neoism` +
`coredumpctl`.
