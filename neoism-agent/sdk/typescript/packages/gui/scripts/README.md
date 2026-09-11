# Native parity assets

## Standalone GUI fonts

```sh
node scripts/generate-font-assets.mjs
node scripts/generate-font-assets.mjs --check
node scripts/test-font-assets.mjs
```

These commands are independent of theme/command generation below, need no package
changes, and never download fonts or enumerate installed desktop fonts. Suggested
GUI-owner scripts: `fonts:generate`, `fonts:check`, `test:fonts` respectively.
Generation is dependency-free Node; tests need Node >=22.13.

Import `src/generated/fonts.css` **once** in the frontend entry point. Then use
`font.cssFamily` as the CSS `font-family` value. CSS public asset URLs start at
`/fonts/`; a deployment under a path prefix must apply its bundler's base URL to
public assets and metadata URLs. Serve OTF as `font/otf`, TTF as `font/ttf`, WOFF2
as `font/woff2`. Some `file --mime-type` databases report OTF as
`application/vnd.ms-opentype` and these older TTFs as `application/octet-stream`;
the actual signatures are validated and all eleven fonts parse in Fontconfig.
Fonts use `font-display: swap`, load only when their face is used, and do not use
`local()` substitution. Browser CSS aliases are scoped with `Neoism ` to avoid
accidentally selecting a different installed version.

Exact exports from `src/generated/fonts.ts`:

```ts
export type FontFaceAsset = {
  url: string;
  weight: number;
  style: 'normal' | 'italic';
  format: 'opentype' | 'truetype' | 'woff2';
};
export type FontOption = {
  id: string;
  name: string;
  family: string;      // Single registered CSS family (no fallback)
  cssFamily: string;   // Quoted CSS family plus generic fallback
  kind: 'monospace' | 'sans-serif' | 'serif' | 'display';
  availability: 'bundled' | 'system-fallback';
  description: string;
  faces: FontFaceAsset[];
  licenseUrl?: string;
};
export const fonts: FontOption[];
export const systemFontOptions: FontOption[];
export const localFontAccess: {
  readonly api: 'queryLocalFonts';
  readonly requiresSecureContext: true;
  readonly requiresUserPermission: true;
  readonly availability: 'runtime-only';
  readonly description: string;
  readonly fallback: string;
};
```

Bundled `fonts` options:

| ID | Name | Faces | Intended use |
| --- | --- | --- | --- |
| `geist-mono` | Geist Mono | 400/700, normal/italic | Native browser monospace |
| `jetbrains-mono` | JetBrains Mono | 400/700, normal/italic | Native browser coding font |
| `cinzel-decorative` | Cinzel Decorative | 400/700 normal | Proportional display serif from native illuminated markdown |
| `medieval-sharp` | MedievalSharp | 400 normal | Proportional display face from native illuminated markdown |

The 11 faces total **1,235,272 bytes** (about 1.18 MiB). Do not advertise unavailable
italic/bold files for the display faces; browsers may synthesize unsupported styles
unless the frontend sets `font-synthesis: none`. Display faces are not sensible code
fonts and are labeled separately for the picker. No full CJK, emoji, Nerd Font, extra
black weight, or reference-checkout fonts are copied merely to inflate the list.

`systemFontOptions` contains `system-sans`, `system-serif`, `system-mono`: generic
browser/OS fallbacks, never named installed-font availability claims. For additional
local fonts, feature-detect `window.queryLocalFonts`, use HTTPS/localhost, and invoke
only from a user gesture after explaining the permission request. Browser support
varies; handle refusal/absence. Do not automatically prompt on page load. Alternatively,
accept an explicit user-entered CSS family with a generic fallback and label it
unverified. `document.fonts.check` alone is not reliable installed-font discovery.
No local-font enumeration is performed by generated metadata.

Full font-specific SIL OFL notices are copied into `public/fonts/licenses/`; source
and license provenance is documented in `scripts/font-licenses/README.md` and
`public/fonts/SOURCES.json`. Preserve those notices in deployment. Tests verify
binary signatures, byte-for-byte source identity, SHA-256, metadata/CSS coverage,
licenses, payload size, reproducibility, and drift from modified/missing files.

Run from `neoism-agent/sdk/typescript/packages/gui` (scripts also work from any cwd):

```sh
node scripts/generate-parity-assets.mjs
node scripts/generate-parity-assets.mjs --check
node scripts/test-parity-assets.mjs
node scripts/test-parity-assets.mjs --native
```

No package dependencies. Tests require Node >=22.13 (`node:module.stripTypeScriptTypes`);
`--native` additionally requires `rustc`. It compiles isolated harnesses in a temporary
directory, not the Cargo workspace, and removes them afterward. No release build.
Suggested manifest scripts for the GUI owner: `assets:generate`, `assets:check`, and
`test:assets` mapped to the first three commands. CI can additionally run `--native`.

## Exports

- `src/generated/themes.ts`: `type Theme = { id: string; name: string; colors: Record<string, string> }`,
  `themes: Theme[]`. All **101** bundled palettes: four builtins in native order,
  then all 97 bundled Base46 palettes in native order. IDs are exact Rust identifiers;
  names are readable labels mechanically derived from IDs, not lookup keys.
- `src/generated/commands.ts`: `type SlashCommand = { name: string; description: string; aliases: string[] }`,
  `commands: SlashCommand[]`. All **32** native picker entries and **52** accepted spellings.
  Both names and aliases include `/`. `/skills` is the picker name with `/skill` an alias;
  `/model` has `/models`. Even the native `/comapction` typo alias is preserved.
- `src/generated/logo.ts`: `neoismLogoPath: string`, `neoismLogoViewBox: string`.
  Path is byte-for-byte the favicon's sole N path, excluding the tile rectangles and
  group transform. ViewBox is its tight local bounds: `0 -187.5 218.75 156.25`.
  Example: `<svg viewBox={neoismLogoViewBox}><path d={neoismLogoPath} fill="currentColor" /></svg>`.
- `src/generated/avatar.ts`: `avatarCells(seed: string, phase = 0.6): { x: number; y: number; color: string }[]`,
  `avatarGridSize(seed: string): number`. Grid size is seed-dependent (11–14).
  Draw each returned cell as a 1×1 square in `viewBox="0 0 grid grid"`, or round both
  scaled cell edges for pixel-snapped canvas quads. Outside-circle cells are absent.
  Colors are opaque `#rrggbb`. Phase is elapsed seconds, not Unix time; default is the
  native still frame. Empty seed equals one space, as in Rust.

## Palette fields

All **27 Rust fields** retain their original spelling:

```
bg fg surface hover border muted dim accent folder
red green yellow blue magenta cyan white black
syn_comment syn_string syn_number syn_keyword syn_statement
syn_func syn_type syn_property syn_constructor syn_special
```

Two convenience keys bring the exported total to **29**: `background = bg`,
`foreground = fg`. Every value is a six-digit CSS hex color. The generator reads the
actual Rust struct fields and packed-index mapping (including Base46 `white = c[1]`),
not an assumed positional label list. Builtin `white` remains its own native value.

Native runtime custom themes come from user-installed files/Mash Up Packs and cannot
be discovered by a standalone browser. This catalog deliberately contains bundled
palettes only; a future host API can supply actual runtime entries. Likewise server
commands are dynamic and are not invented here as local commands.

## Source of truth and maintenance

The generator extracts themes from `shared/src/primitives/ide_theme.rs` and
`nvchad_themes.rs`, commands/descriptions from `slash_option_specs` and aliases from
`plan_slash_command` in `command_controller.rs`, and the letterform from the web
favicon SVG. Source-format/count mismatches fail loudly rather than silently dropping
entries. Update count expectations deliberately if native catalogs grow.

`avatar.template.ts` is a maintained port of `shared/src/editor/crdt/presence_avatar.rs`,
not an automatic Rust transpilation. Generated `avatar.ts` embeds a hash of the native
source so changes are detected by `--check`. When native avatar logic changes, review
and update the template **before regenerating**, then run `test-parity-assets.mjs --native`.
The port uses UTF-16 code-unit FNV-1a via `Math.imul`, unsigned xorshift32, and f32-rounded
arithmetic. Native comparison checks grid/cell positions and RGB within one byte step
(platform libm sine rounding), across ASCII, Unicode, astral characters and four phases.
Native mode also runs all included Rust avatar/controller tests and compares every
exported alias against its canonical native action with and without arguments.

The Node-only tests check counts, keys, uniqueness, alias coverage, avatar geometry
and determinism, and actual drift failures in an isolated temporary copy of the inputs.

## Attribution

Base46 palettes retain the full NvChad MIT notice both in
`src/generated/NvChad-base46.LICENSE.txt` and a `/*! ... */` comment in `themes.ts`,
including the upstream revision. When distributing a minified app, preserve legal
comments or ship the accompanying notice in your third-party licenses bundle.
