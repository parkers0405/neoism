---
name: "Sugarloaf file browser + Agent image attachment"
description: "Alt+E/file-tree density and optical text centering pass"
type: "feature"
scope: "project"
origin: "density follow-up"
created: "2026-08-01"
updated: "2026-08-01"
---

Density follow-up: `FileBrowserModal` now directly imports public file-tree density constants (`FONT_SIZE=13`, `ICON_FONT_SIZE=13`, `ROW_HEIGHT=26`, `ROW_PADDING_X=12`, `FRAME_RADIUS`, `FRAME_STROKE`) into `FILE_BROWSER_DENSITY`. Desktop card max is 700x462 and targets 14 useful rows; bands are 30/32/36, sidebar 148, visual controls 24. Narrow layouts reserve 44px toolbar/footer slots and hit rectangles while keeping 24px painted controls. Nav/path/search/button and row/icon text use Sugarloaf `measure`, `instance_count`, and `center_instances_in_rect` for raster-ink optical centering; title is small 13px bold. Oversized dialog constants removed. 8 focused tests pass, shared/wasm32/desktop checks pass, 109 web tests pass. Existing unrelated warnings only.
