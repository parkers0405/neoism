pub mod core;
// `filters` is the librashader integration — wgpu-only by upstream
// design (`librashader-runtime-wgpu`). Gated together with the rest
// of the wgpu code so the dep tree drops cleanly on Linux/macOS
// builds that don't enable the `wgpu` feature.
//
// Also gated off on wasm32: the builtin filter loaders write shader
// presets to `/tmp/<name>/` via `std::fs`, and `librashader-pack`
// hasn't been verified on the wasm target. Splitting filters behind
// a dedicated feature is the strategic follow-up the audit flagged.
#[cfg(all(feature = "wgpu", not(target_arch = "wasm32")))]
pub mod filters;

pub mod shader_overlay;
#[cfg(target_os = "macos")]
pub(crate) mod shader_overlay_metal;
