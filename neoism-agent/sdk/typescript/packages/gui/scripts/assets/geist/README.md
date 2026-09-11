# Bundled Geist UI + native Press Start 2P headings

## Geist proportional sans-serif

`Geist-Variable.ttf` is an unmodified copy of Vercel's official
`fonts/Geist/variable/Geist[wght].ttf` at commit
`10dc7658f13c38a474cde201bb09a4617267545b`.

- Source: https://raw.githubusercontent.com/vercel/geist-font/10dc7658f13c38a474cde201bb09a4617267545b/fonts/Geist/variable/Geist%5Bwght%5D.ttf
- SHA-256: `73894e0448cae90a92b6c2f8732b7bb9acb7b94c418bff559dad4a18e1de9659`
- Size: 169,056 bytes; one full upright variable TTF, no static-weight duplicates.
- Actual `wght` axis: 100–900, default 400. Intermediate UI weights supported.
- License: `../../font-licenses/Geist-OFL.txt`, SIL OFL 1.1.
- License source: https://raw.githubusercontent.com/vercel/geist-font/10dc7658f13c38a474cde201bb09a4617267545b/OFL.txt
- License SHA-256: `c683bfbcc7e087f5d37a54ef628f10387c451a83ddc459b151403a164ac46c90`

Both files were downloaded with `curl -fLsS` from these pinned URLs. Builds and
regeneration use only the checked-in local copies; no download, local font
installation, Synapse checkout, conversion or font tooling is required.

## Exact native pixel heading font

The generator copies Neoism's existing
`sugarloaf/src/font/resources/PressStart2P/PressStart2P-Regular.ttf` unchanged.
This is the face loaded by `FONT_PRESS_START_2P` in
`sugarloaf/src/font/constants.rs`, resolved by
`neoism-frontend/shared/src/primitives/pixel_font.rs`, and used for headings in
`neoism-frontend/shared/src/panels/agent_pane/view/side_panel/sections.rs` and
`draw.rs`. It is **Press Start 2P**, not Geist Pixel or a Synapse font.

- SHA-256: `034c77f1f05ec89421e4a63f0e3a4ca1ecf852cc6d2bf611f126f275728e017d`
- Size: 118,204 bytes; upright regular 400, not variable.
- License copied from the adjacent native `OFL.txt` to
  `../../font-licenses/PressStart2P-OFL.txt`.
- License SHA-256: `705960c3281a5765ecc0b59bd4ed7ca59eed165748076bc2fc3e8fdbfeb944b0`
- Upstream: https://github.com/google/fonts/tree/main/ofl/pressstart2p

Catalog ID `press-start-2p`, family `Press Start 2P`, CSS stack
`"Press Start 2P", monospace`. This outline TTF supports arbitrary CSS sizes;
there is no fixed bitmap strike restriction. Native date-group headings are
approximately 12px, while section headings subtract 3 scaled pixels from their
normal heading size because the face is wide. Use weight 400 to avoid browser
synthetic bold; larger titles can scale as needed. Existing Geist Mono is kept
separate for code.

## Reproduction

Generator inputs live outside `public/fonts` and `src/generated`, so removing
those outputs cannot remove their sources. From the GUI package:

```
node scripts/generate-font-assets.mjs
node scripts/generate-font-assets.mjs --check
node scripts/test-font-assets.mjs
```

Generated `public/fonts/SOURCES.json` records source/destination byte hashes for
all fonts and distributed OFL notices. Tests inspect the actual Geist variation
axis and pin the native pixel source identity.
