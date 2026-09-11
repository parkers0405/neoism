# Home wordmark integration

Import `Wordmark` from `./components/Identity` and render `<Wordmark />` immediately above the home composer. Remove the old home `Logo` usage and slogan/subtitle in the parent. `Avatar` and `Logo` remain available and unchanged for other consumers.

The component imports its own scoped `Wordmark.css`; do not copy animation rules into the global stylesheet. The parent owns centering and spacing. Native `view/home.rs` anchors the wordmark **44px above the input card** (18px minimum when constrained), with a 34–84px-high wordmark and a 20px top inset. The default component sizing follows that height range; a parent class can override width for its available pane dimensions.

Six SVG groups retain all 18 original SVG paths/transforms/opacities. Motion uses native exponential smoothing (14/s, dt capped at 100ms), 18% hover scale and height-relative lift, 2.5% sine shimmer over 3.4s with 0.16-cycle letter offsets, the enlarged glow copy, 460ms click squash, and 280ms white ripple. Reduced motion stops the RAF and renders a static mark. The entire image is labelled NEOISM; letter artwork is hidden from assistive technology. The click is a decorative effect, not an application action/tab stop.

Generate/check geometry and Rust constants:

```sh
node scripts/generate-wordmark.mjs
node scripts/generate-wordmark.mjs --check
npx vitest run src/components/Wordmark.test.tsx
npx tsc --noEmit
```

The generator compares the desktop and web SVG geometry, not the web SVG's older CSS approximation. Native uses a cropped 1196×193 raster; the SVG uses equivalent 0.8x raster crop coordinates rather than the web asset's extra vertical padding. Raster anti-aliasing is renderer-dependent; no browser/pixel comparison was performed.
