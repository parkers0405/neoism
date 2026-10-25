---
name: "Vulkan presentation lifetime gap remains open"
description: "Open preexisting WSI presentation-completion/semaphore lifetime gap; no unbounded swapchain-retention workaround shipped; needs maintenance1/present-wait and runtime validation"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-23"
updated: "2026-10-23"
---

# Vulkan presentation lifetime requires a separate validated fix

Whole-window performance audit/review found an EXISTING WSI synchronization gap in sugarloaf/src/context/vulkan.rs. Graphics submission fences are not presentation-completion proof. Current original code uses render_finished semaphores per frame slot and collects retired swapchains when all graphics fences signal. A slot's graphics completion does not universally prove presentation consumed its wait semaphore or released old WSI resources.

Reference: https://docs.vulkan.org/guide/latest/swapchain_semaphore_reuse.html . Proper direction: presentation-wait semaphores indexed by acquired swapchain image plus explicit presentation completion (correctly queried/enabled VK_EXT_swapchain_maintenance1 present fences or equivalent present-wait/ID support), with bounded runtime retirement and driver synchronization validation.

An attempted WSI patch retained old generations until shutdown; it was rejected because resizing/recreation could grow memory indefinitely. ALL new WSI changes, including per-image semaphore work and associated tests, were reverted by their author. Do not describe WSI as fixed. Do not restore the unbounded workaround. Device/queue-idle alone is not universal presentation-completion proof; do not silently claim it as a portable fallback. Do not introduce steady-frame idle waits or throttle continuous effects.

Separate safe performance changes remain: redundant glyph cross-slot CPU fence waits removed while same-queue GPU read/copy/read barriers and acquired-slot staging protection remain; image textures/descriptors retire using actual acquired frame slots with graphics-fence completion, including eviction, resize/background replacement/clear and font/reset clearing. These IMAGE resources are not WSI present resources; their graphics fence proof is valid. One-shot image upload waits remain synchronous and batching is deferred.

Source reviews found no regression in those scoped atlas/image changes; GPU synchronization validation and target-driver pixel/pacing tests still need to be run through the user's normal dev loop. Only cargo check was used, no application launch or release build.
