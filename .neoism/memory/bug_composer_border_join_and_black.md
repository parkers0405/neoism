---
name: "Composer material join and theme-owned backgrounds"
description: "Current composer colors are theme.bg/theme.surface, never forced black; actual border material join retained; modest4px text/control gap reduction;306 tests/check passed"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-22"
updated: "2026-10-22"
---

CURRENT POLICY, superseding the earlier literal-black patch: user complained input was black on every theme. Main composer MUST use the selected theme's `bg` through surface_background::base_color("composer.agent", theme, theme.f32(theme.bg)); lower tray uses theme.surface through the same customization/opacity resolver. No dark-mode RGB zeroing and no generic surface multiplier. Pastel Dark naturally stays black because bg=0; Tokyo Night/NvChad/Catppuccin retain their own background hues. Explicit style overrides and alpha remain respected. Replaced hardcoded-black tests with all four IdeThemeName::ALL palette cases plus a custom light palette verifying bg/surface and matching opacity. Also user requested only a BIT less space above agent/model/thinking: text bottom padding reduced4px, chat(11,17), home(15,21), minimumcard heights94/102 instead98/106, rowheight22 and44px control/touch band unchanged. Layout expected heights updated; lower directory/hints tray remains32px and Tab hint remainsremoved. Full306 sharedagentviewtests passed,1ignored; cargo check-pneoism-ui passed; scoped diffcheck clean.

Actual seam fix still REQUIRED: outer-edge snapping alone left visible bottom outline/background sliver. ComposerGeometry::tray_join_rect(border_width) provides fulltraywidth strip from floor((traytop-border)*dpi)/dpi to originaltraytop, covering ENTIREmainbottomstroke and tinyfillregionabove. render_input excludes thisstrip from mainborder+fill+custombackground using inherited scopedrendererholes, restorespriorholes, then paints strip ONCE intraycolor. This is direct material ownership, not alpha-backed overlap or mere rectangle adjacency. Originaltraybody/bottomradii/hitgeometry remain; strip is inside already masked card silhouette. Keep this join when editing colors or spacing. Geometry tests cover stroke replacement, pixel boundaries, single ownership across DPI/scales/fractionalorigins, and hidden/narrow tray. No live visual verification claimed.
