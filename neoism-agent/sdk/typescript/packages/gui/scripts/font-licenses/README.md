# Curated browser font license sources

All included font binaries are **unmodified copies of tracked Neoism files**.
No font binary is downloaded during generation. `public/fonts/SOURCES.json` records
source paths, SHA-256 hashes, sizes, intended HTTP MIME types and upstream projects.

The notices here are the offline inputs copied into `public/fonts/licenses/`:

- `GeistMono-OFL.txt`: verbatim from
  <https://raw.githubusercontent.com/vercel/geist-font/1.5.0/OFL.txt>.
  Bundled OTF name table reports Geist Mono version 1.500 and
  `Copyright 2024 The Geist Project Authors (https://github.com/vercel/geist-font.git)`.
- `JetBrainsMono-OFL.txt`: verbatim from
  <https://raw.githubusercontent.com/JetBrains/JetBrainsMono/v2.304/OFL.txt>.
  Bundled WOFF2 reports JetBrains Mono version 2.304.
- `CinzelDecorative-OFL.txt`: font-specific copyright/reserved name copied from the
  bundled font's name table (ID 0), followed by the SIL OFL text already in
  `neoism-frontend/shared/assets/illuminated/fonts/google-ofl/OFL.txt`.
  The font declares OFL 1.1 in name IDs 13/14. Upstream:
  <https://github.com/google/fonts/tree/main/ofl/cinzeldecorative>.
- `MedievalSharp-OFL.txt`: font-specific copyright/reserved name copied from the
  bundled font's name table (ID 13), followed by the same existing SIL OFL text.
  The unusual reserved name `NovaRound` is preserved exactly as embedded, not corrected
  or replaced with a guessed name. Upstream:
  <https://github.com/google/fonts/tree/main/ofl/medievalsharp>.

The existing shared `google-ofl/OFL.txt` header names UnifrakturCook's authors, so
copying that notice alone for unrelated fonts would omit their actual authors.
These per-family notices preserve the specific embedded copyright statements.

The CSS family aliases (`Neoism Geist Mono`, etc.) only scope browser registration;
the actual binaries/internal family names are unchanged. Preserve every generated
license notice when deploying the GUI. OFL fonts remain OFL, not the application's
MIT license. Do not sell the font files by themselves.
