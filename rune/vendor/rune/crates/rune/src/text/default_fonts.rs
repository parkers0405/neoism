//! The fonts text falls back to when the app registers none. Native builds always bundle Inter;
//! on web the `embedded-fonts` feature bundles Inter and IBM Plex Mono.

#[cfg(not(target_arch = "wasm32"))]
pub(super) const DEFAULT_FONTS: &[&[u8]] = &[
    include_bytes!("../../assets/Inter-Regular.ttf"),
    include_bytes!("../../assets/Inter-Medium.ttf"),
    include_bytes!("../../assets/Inter-SemiBold.ttf"),
];
// Inter without TrueType hinting or glyph names (`scripts/web-fonts.sh`): the web rasterizes
// with Rune's own pipeline, which reads neither. Cut to Latin, Greek and Cyrillic text, under
// half the full faces, which the app fetches after its first frame (`full_faces`).
#[cfg(all(target_arch = "wasm32", any(test, feature = "embedded-fonts")))]
pub(super) const DEFAULT_FONTS: &[&[u8]] = &[
    include_bytes!("../../assets/web/first/Inter-Regular.ttf"),
    include_bytes!("../../assets/web/first/Inter-Medium.ttf"),
    include_bytes!("../../assets/web/first/Inter-SemiBold.ttf"),
    include_bytes!("../../assets/IBMPlexMono-Regular.ttf"),
];
#[cfg(all(target_arch = "wasm32", not(any(test, feature = "embedded-fonts"))))]
pub(super) const DEFAULT_FONTS: &[&[u8]] = &[];

#[cfg(test)]
#[path = "default_fonts_tests.rs"]
mod tests;
