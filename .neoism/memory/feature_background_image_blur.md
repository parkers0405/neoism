---
name: "Background image blur + Lua config"
description: "Image blur is OPT-IN default0 (explicit3 personal config); JSON/Lua/Settings, async startup cache, fixed Margin/color serde for Lua patches."
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-19"
updated: "2026-10-19"
---

# Per-image blur and opacity in JSON and Lua

Current default: ImageProperties.blur is 0.0 (DISABLED/OPT-IN), not 3.0. User explicitly reversed the initially requested global default3 after measured startup slowdown. Valid range finite0..100, approximate Gaussian sigma in source-image pixels. Explicit3 is still supported; zero is byte-preserving. Image opacity stays0..1 default1, independent from whole-window opacity and boolean OS/compositor ui.window.blur. Settings Image blur has default0, step0.5, unitpx, platformoverride descriptors. Catalog/picker/normalization/persistence preserve blur; typed serde validates JSON and Lua patches.

Personal config at user request selects original bg/49610.jpg, opacity0.35, explicitblur3 (not a forced globaldefault), replacing already-preblurred bg/49610-blur-3.png toavoid doubleblur. Oldfile retained. Comparison ImageMagick sigma3 againstpreblurPNG RMSE~0.00113, originaldifference~0.06187. OS compositorblur remainsfalse, init.lua unchanged. User's explicit3 waspreserved whenglobaldefaultbecame0.

Startup andhotreload are now ASYNCHRONOUS and cached: see feature_background_async_cache.md. set_background_image remains synchronous fortransactional picker/pack validation/rollback. CPU helper imageops::fast_blur uses3passesO(n), premultiplies/unpremultiplies nonconstantalpha beforeexistingopacitystep toavoid hiddenRGB halos. No perframe orresizereprocessing. Blur participates inpixelcacheidentity; fit/positiononly reusepixels.

JSON ui.window.background-image {path,opacity,blur,fit,...}. Lua neoism.setup({ui={window={background_image={path=...,opacity=0.35,blur=3,fit='cover'}}}}), underscorekeysnormalize recursively tohyphens. Not neoism.ui.style blur (stylehasnoblurfield). Docs Appearance.md anddocs/lua.md Wallpaper section explicitly show3asopt-in and0asdefault.

NonemptyLua configpatches previously failedwholeConfig roundtrip. Fixed Margin customDeserialize toaccept named-sideobjects emittedbySerialize alongsideCSSarrays (config/layout.rs); color-array deserializers nowacceptRGBAarrays/optionalnull emittedbySerialize alongsidehexstrings (config/colors/mod.rs). No fieldsdropped. WholeConfig roundtrip, Luaimagepatchvalidation/atomicity, actualLua runtimekeynormalization tests pass. Oldbackgroundserialization/Luapatchtest nowpasses.

Verification latest: debugbinarybuiltandlaunched atuserexplicitrequest; 38background/cache/default/picker tests passed (21Sugarloaf,13backend,4sharedUI), wasmwebcheckpassed. ActualLua runtimefeaturetestpassedpreviousstep (mustuse --features runtime orrunszero); standaloneSugarloaf Linux testsneed --features neoism-window/x11. No releasebuild, commit, orpush.
