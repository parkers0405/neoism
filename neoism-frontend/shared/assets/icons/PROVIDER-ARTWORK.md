# Provider artwork

`claude-starburst.svg` is the Claude mark from [Lobe Icons](https://github.com/lobehub/lobe-icons/blob/master/packages/static-svg/icons/claude.svg), retrieved from the [`@lobehub/icons-static-svg` package](https://cdn.jsdelivr.net/npm/@lobehub/icons-static-svg@latest/icons/claude.svg). The path is unchanged; the source uses a white fill as a tintable mask.

Lobe Icons is MIT licensed, Copyright (c) 2023 LobeHub. The full license is reproduced in [MCP-ARTWORK.md](MCP-ARTWORK.md#lobe-icons-license). The Claude mark remains a trademark of Anthropic; inclusion identifies the configured provider and does not imply endorsement.

`xai.svg` is the xAI mark from [Lobe Icons](https://github.com/lobehub/lobe-icons/blob/master/packages/static-svg/icons/xai.svg), retrieved from [the same static SVG package](https://cdn.jsdelivr.net/npm/@lobehub/icons-static-svg@latest/icons/xai.svg). The path is unchanged; the fill is white for tinting and the title identifies xAI. The same MIT license applies. The xAI/Grok marks remain trademarks of xAI; inclusion identifies the configured provider and does not imply endorsement.

## Atlas generation

The provider atlas is a 512x64 PNG encoded in `provider-logos.png.b64`. Each provider occupies a 64x64 cell. In order: OpenRouter, Claude, OpenAI, Google, GitHub Copilot, Vercel, OpenCode, xAI. xAI occupies the eighth cell (x=448); the original seven cells retain their pixels. Provider IDs `xai`, `x-ai`, and `grok` map to that cell, including provider-prefixed model IDs.

Run `python3 neoism-frontend/shared/assets/icons/generate-provider-atlas.py` from the repository root to rebuild the xAI cell from the checked-in SVG. The script requires `rsvg-convert` and ImageMagick's `convert`, preserves the legacy seven-cell strip, and supports `--check` for drift. Then run `bun run sync:design` in `neoism-frontend/gpuix` to synchronize aliases, atlas width, and pixels into the generated TypeScript design artifact. The xAI cell's row-major alpha FNV-1a fingerprint is `0x07e85412449c43fc`, checked by a Rust regression test.

Rasterize the source with `rsvg-convert -w 64 -h 64 claude-starburst.svg -o /tmp/neoism-claude-starburst.png`. Decode the existing atlas, composite the complete raster using ImageMagick's `-geometry +64+0 -compose Copy -composite`, and encode the resulting PNG as base64. `Copy` is important: transparent parts of the new cell must replace, not retain, the old Anthropic lettermark. Use `-strip -depth 8` for the PNG output.

The Claude cell's row-major alpha bytes have FNV-1a fingerprint `0x0d0de8fdaa9684bd`. The Rust artwork regression test checks this mask, and the generic atlas test checks dimensions and nonempty provider cells.

Composer colors are applied at runtime: Claude uses orange `#D97757`; OpenAI, OpenRouter, and xAI use white in dark themes and black in light themes. Model text colors and provider/model IDs are independent of these logo colors.
