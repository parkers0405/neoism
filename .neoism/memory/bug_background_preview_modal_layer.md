---
name: "Blank Backgrounds preview image - fixed"
description: "Blank Backgrounds preview was covered by late modal material; explicit UI image pass fixes Vulkan/Metal/WGPU; artwork previews fullopacity and folder selection preserves dimming"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-08"
updated: "2026-10-08"
---

User actual screenshot Backgrounds picker right title/description present but image regionflatblack, no PreviewUnavailable label. File/decodedpixelsvalid and globalbackgroundlive verified. ROOTCAUSE: GraphicOverlay z1 AboveText draws before LATE overlay material; opaque palettepreviewcard covers alreadyrendered image. FIX shared command_palette/background_preview.rs uses GraphicOverlay::LATE_OVERLAY_Z_INDEX; sugarloaf graphics.rs explicitsentinel i32MAX; renderer/mod.rs ImageLayer LateOverlay forwarded Vulkan/Metal/WGPU ordered lateModalMaterial -> previewImage -> overlaylabels. Vulkan pre-reserves overlayinstancebuffer BEFORE anydraw binding for below+above+late; nonoverlapping startindices0,below.len,below+above; fullbackground usesseparateVertexBuffer. Added layer/UV/highID/source-order regressiontests. ValidKittyrawzMAX collision prevented GraphicOverlay::external_z_index maps ONLY max->max-1 at native adapters host/run.rs and screen/mod.rs afterexistingplacement sorting; virtualplaceholderz=-1 unaffected; test verifiesnormalterminal semantics. PreviewnowshowsartFULLopacity independent wallpaperopacity (removed opacity cachekey/pixelalphafade). bg-folder catalogue entries retain current standalone imageopacity when switching instead of reset1; packimageentries keepdeclared packopacity. Backend testdirectorycurrentOpacity .55 added, Appearance docs updated. Final cargo check -p neoism -p neoism-backend -p sugarloaf --features sugarloaf/wgpu --tests AND wasm web check passed; gitdiffcheckclean. Tests compiledonly no execution, no releasebuild/newruntime preview screenshot. Next userdev rebuild/relaunch required preview-layer code (assetselectedglobalbackground alreadylive).
