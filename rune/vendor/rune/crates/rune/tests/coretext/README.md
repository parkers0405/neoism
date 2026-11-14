# CoreText references

Reference sheets for `rune::text::pixel_compare`: what CoreText draws for each case (Rune's
glyphs and pen positions, macOS's default antialiasing and subpixel quantisation, into an
opaque sRGB bitmap), with font smoothing on (`<pair>-<scale>x.png`, compared with
`FontSmoothing::On`) and off (`flat-<pair>-<scale>x.png`, light and dark, compared with
`FontSmoothing::Off`). Each greyscale PNG holds the green channel. `layout.txt` is a digest of
the layout they were drawn from; when Rune's layout of the cases changes, the comparison
refuses to run until the sheets are regenerated.

`sf/` holds the same for San Francisco, drawn from the Mac's own system font. It is ignored by
git (Apple's font can't be redistributed): regenerate it locally, and its comparisons are
skipped where it is missing. `mono/` is SF Mono (`Family::SystemMono`) at 11–16 pt and a
13.5/22 pt diff paragraph, drawn with AppKit's `NSFont.monospacedSystemFont` (the instance
native apps draw code in), kept the same way.

- Regenerate (macOS): `scripts/coretext-reference.sh`. Drawn on macOS 15.3 (24D60).
- Compare: `cargo run -p rune --example coretext_compare -- [--set inter|sf|mono]
  [--smoothing on|off] [--blend srgb|linear] [--against smoothed|unsmoothed]
  [--out DIR]`, or `-- shot SHOT.png --pair light|dark` for a 2× browser screenshot of the
  report paragraph.
- Test (macOS): `cargo test -p rune pixel_compare`.

`Inter-Bold-subset.ttf` is Inter 4.1 Bold (SHA-256 `288316099b1e0a47…`) cut to printable
ASCII, the em dash and curly double quotes with fontTools (`pyftsubset --no-hinting`), under the
SIL Open Font License in `../../assets/Inter-LICENSE.txt`. Inter Regular is Rune's own
`assets/Inter-Regular.ttf`.
