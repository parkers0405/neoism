---
name: "Framework 16 graphics and firmware audit"
description: "Verified Framework 16 firmware, Mesa/Vulkan backend, GPU selection, memory topology, and display/compositor factors affecting Neoism performance."
type: "reference"
scope: "project"
origin: "2026-08-30 read-only system audit"
created: "2026-08-31"
updated: "2026-08-30"
---

# Framework 16 Neoism graphics/firmware audit (2026-08-30)

Read-only live audit found:
- Framework Laptop 16 Ryzen AI 9 HX 370, BIOS 04.01 / EC tulip-4.0.1 current. fwupd reports system firmware current; only unrelated UEFI dbx security update offered.
- Omarchy 4.0.0.r1902, kernel 7.1.11, Hyprland 0.56.2, Mesa 26.2.1.
- Neoism uses native ash Vulkan on RADV Radeon 890M (`renderD129`), MAILBOX, not llvmpipe/wgpu/NVIDIA. This is intentional Auto policy that filters NVIDIA and prefers integrated GPU on Wayland to avoid cross-GPU presentation jitter.
- Installed RTX 5070 Laptop GPU is not opened by Neoism; internal display is AMD-driven.
- Panel runtime is 2560x1600 at 144 Hz, scale 2.0, VRR; Neoism surface about 2520x1508 physical. Compositor opacity/blur/shadows and no direct scanout increase iGPU load.
- Only one 32 GiB DDR5-5600 DIMM is installed; empty second slot limits APU/iGPU memory bandwidth.
- No GPU hangs, llvmpipe, mixed Mesa, explicit-sync failures, thermal throttling, active memory pressure, or obsolete BIOS/EC.

Machine-specific performance ranking: Radeon 890M vs RTX 3090 is strongest difference; single-DIMM iGPU bandwidth second; high-resolution 144 Hz compositing third. Safe A/B on planned launch: `NEOISM_VULKAN_DEVICE=discrete`, frame logs (`NEOISM_SCROLL_LOG=1 NEOISM_VULKAN_FRAME_LOG=1 NEOISM_VULKAN_FRAME_SPIKE_US=8000`), FIFO vs MAILBOX. Do not make discrete permanent without measuring because internal panel is AMD-driven. Second matching SODIMM is likely largest hardware improvement.
