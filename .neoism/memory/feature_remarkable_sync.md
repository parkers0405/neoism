---
name: feature_remarkable_sync
description: reMarkable 2 + cross-device live sync built on a Loro CRDT (neoism-sync crate)
metadata: 
  node_type: memory
  type: project
  originSessionId: 453f0cd1-334c-4147-8488-36cb0ce89c34
---

Building Neoism ↔ reMarkable 2 sync, designed from the start as **general
cross-device infra** (notes now, live multiplayer code editing later — same
engine). Decided with the user: **Loro CRDT** foundation; **true-live**
reMarkable via an **on-device companion agent** (installed via toltec).

**ARCHITECTURE — reMarkable is now a SEPARATE, OPT-IN PLUGIN (2026-06-08):**
All reMarkable sync lives in the new **`neoism-remarkable`** crate (moved
`vault_sync`/`auto`/`controller`/`ink_interop` out of desktop `src/sync/`,
which is GONE). Desktop depends on it **`optional = true`** behind cargo
feature **`remarkable`** (NOT in default) → a normal build links zero
reMarkable code (`neoism-sync` only pulled in via the plugin). Core
draw-over-markdown keeps its ink helpers, now in
**`neoism_ui::editor::neodraw::sidecar`** (`ink_sidecar_path`,
`load_ink_layer`, `strokes_only`, `migrate_legacy_ink`) — used via
`crate::editor::neodraw::*`. ALL desktop glue consolidated in
**`screen/bridges/remarkable.rs`** (cfg'd: real impl + no-op stubs for
`share_vault_with_remarkable` + `poll_remarkable_autosync`), so core call
sites stay clean (`render` just calls `self.poll_remarkable_autosync()`).
`Screen.remarkable_autosync` field + init are `#[cfg(feature="remarkable")]`.
Both build states compile + tests pass. (Minor: the "Sync with reMarkable"
context-menu items still render in default builds; can gate later.)
End-state: turn `neoism-remarkable` into a daemon-client process speaking
`neoism-protocol` (files+crdt) — the protocol already has a `crdt` module
(CrdtSyncEnvelope/presence/cursors) = the multiplayer seam. Golden no-restart
sync = impersonate the reMarkable cloud (rmfakecloud approach: DNS+cert →
tablet's native sync, live, no xochitl restart). USB `/upload` API also
restart-free but needs the web interface enabled (currently OFF on device).

**SYNC CONNECTION GOTCHAS (debugged 2026-06-08, real device):** (1) The
tablet's USB-eth iface re-enumerates under DIFFERENT names — sometimes
`usb0`, sometimes `enp197s0f3u1c2`; check whichever holds `10.11.99.2` +
`LOWER_UP`, not a fixed name. (2) BIGGEST: the tablet re-keys SSH on
firmware/reset → stale `~/.ssh/known_hosts` → "HOST IDENTIFICATION HAS
CHANGED" → every scp/ssh refused (StrictHostKeyChecking=no does NOT bypass
a *changed* key). Fix: `ssh-keygen -R 10.11.99.1` + sync now uses
`UserKnownHostsFile=/dev/null` in SSH_OPTS (vault_sync.rs). (3)
`is_remarkable_reachable` now strips `user@` (poller passed
`root@10.11.99.1` → `to_socket_addrs` couldn't parse → auto-sync NEVER ran).
(4) sync_vault gates on reachability + silences scp stderr;
`SyncOutcome.unreachable` → force-sync shows clear "plugged in/awake?" Warn,
poller skips silently. Firmware `5.4.70-v1.6.3-rm11x`. **v6 ENCODER:**
`encode_rm_v6` round-trips our decoder; on device the doc renders (thumbnail
made) but the STROKE didn't draw → line encoding needs more (likely
per-stroke timestamp CrdtIds idx6/7, or parent != (0,11)); needs
push→pull-thumbnail→fix iteration. Scaffolding decoded (AuthorIds/Migration
Info/PageInfo/SceneInfo/SceneTree×4/TreeNode/SceneGroupItem), prefix=449B,
line parent=(0,11); template = real prefix + appended line blocks.

**Why CRDT:** "live" + "offline merge" + "Notion-style multiplayer" + "easy
cloud later" all collapse to one mechanism. Transport-agnostic: same update
blobs over SSH/LAN today → WebSocket cloud relay later. The sync core is
**document-agnostic**; only the thin schema (`NoteDoc`) and the reMarkable
bridge are feature-specific. A future `CodeDoc` reuses discovery/transport/
presence wholesale.

**Crate `neoism-sync`** (workspace member, native): DONE + 10 tests pass.
- `core.rs` `SyncDoc` — generic wrapper over `loro::LoroDoc`: snapshot,
  export_from(version), version(), import, on_local_update (the spout
  transports drain). `loro = "1.13"`.
- `note.rs` `NoteDoc` — markdown `LoroText` + ink `LoroList` (strokes as
  `LoroValue::Binary` JSON). Strokes anchor to a Loro `Cursor` (relative
  text pos) so ink follows words on reflow — golden-standard, TESTED.
- `stroke.rs` `Stroke{id,points,width,color,anchor}` / `StrokePoint{x,y,pressure}` / `Color([u8;4])`.
- `remarkable.rs` `.rm` codec. v3/v5 parse+encode DONE+tested. **v6
  (firmware 3.x) parser DONE + VALIDATED against real 3.26 notebook**
  (rm-samples/, gitignored). v6 = flat block stream after 43B header; each
  block: len u32|unk u8|min u8|cur u8|type u8|body. Strokes = type 0x05
  SceneLineItem. Tagged fields: tag varint, idx=tag>>4 type=tag&0xF
  (ID=0xF u8+varint, Byte4=0x4, Byte8=0x8 f64, Byte1=0x1, Length4=0xC sub).
  Line value=subblock idx6; inside: color=Byte4 idx2, thickness=f64 idx3,
  points=Length4 idx5. v2 points (cur_ver>=2)=14B: x f32,y f32,speed u16,
  width u16,dir u8,pres u8; v1=24B all f32. **Coords are PAGE-CENTERED →
  add PAGE_WIDTH/2 (702), PAGE_HEIGHT/2 (936).** Pages SCROLL → y can exceed
  1872 (canvas taller than screen; see .content verticalScroll). Stroke id
  from item_id CrdtId (stable). Validate tool: `cargo run -p neoism-sync
  --example dump_rm -- <file.rm>`. Device SSH: USB iface needs manual
  `sudo ip addr add 10.11.99.2/24 dev <usb-iface>` + `ufw allow on` it (box
  uses iwd, no auto-config); device is BusyBox (head -n 1 not head -1).
- Shared page frame `PAGE_WIDTH=1404`, `PAGE_HEIGHT=1872` (rM2 portrait px).

**Pull loop DONE at library level (15 tests):** `BridgeServer::bind/poll`
(non-blocking TCP listener for the agent) → `BridgeMsg` → `NoteDoc::
apply_bridge` → `sync_page(page_id, strokes)` = idempotent per-page merge
(add new / drop erased by stable id / no dup; CRDT-friendly). `Stroke.page:
Option<String>` tags which rM page (multi-page notebook → one note, default
one note↔one notebook, pages stack by y-offset). `NoteDoc::page_strokes`.
Page-mapping decision: one note ↔ one notebook, pages stacked by y-offset.

**PUSH half DONE (19 tests):** `export/pdf.rs` `markdown_to_pdf` =
dependency-free PDF writer (Helvetica, 1404×1872 pages) + `RenderedPdf`
with a `LayoutItem` map (page,x,y top-left ↔ source_offset) for anchoring.
Validated: pdfinfo parses it. `export/xochitl.rs` `pdf_document_bundle`
emits the 4-file annotated-PDF bundle (.pdf/.metadata/.content
formatVersion 2 fileType pdf + cPages/.pagedata) + `folder_bundle`
(CollectionType vault folder) + v4-ish uuid gen. **PUSH VALIDATED on real
device fw 3.27** (scp'd a Neoism folder+note; PDF text renders, pdftotext
confirms). **KEY SCHEMA FIX (from inspecting how xochitl rewrote the pushed
.content):** each cPages page needs inline `"redir":{"timestamp":"1:1",
"value":<pdf_page_idx>}` — NO top-level `redirectionPageMap`. Without redir
the device shows a BLANK page (symptom: note opens empty + extra blank
page). Push flow: scp note's 4 files + folder's 2 files into
`~/.local/share/remarkable/xochitl/`, then `systemctl restart xochitl`.

**Desktop controller DONE:** `desktop/src/sync/controller.rs`
`RemarkableSync`: owns BridgeServer + per-device NoteDoc map; `listen(port)`,
`poll()->changed doc ids`, `overlay_scene(doc_id)->neodraw Scene` (pages
stacked by y-offset), `build_bundle(title,md,parent)`, `push_bundle(bundle,
host)` (scp + restart xochitl via system ssh). DEFAULT_BRIDGE_PORT=47800.

**IDEMPOTENT + SUBFOLDERS (fixed dup folders).** `stable_uuid(seed)` is
now DETERMINISTIC (pure FNV of seed, no time/counter) — bundle fns take an
explicit `uuid`. Share handler keys ids by path: `stable_uuid("neoism-
folder:{path}")` / `"neoism-note:{path}"` and walks the vault tree creating
NESTED CollectionType folders (parent=containing folder's id) + notes
parented correctly. So re-sharing overwrites in place (proven: pushed twice
→ still 1 folder) and sub-folders/sub-notes mirror onto the device.
Signatures: `folder_bundle(uuid,title,parent)`, `pdf_document_bundle(uuid,
title,parent,pdf,pages)`. Examples + controller.build_bundle updated.

**SHARE (push) UI DONE + validated on hardware.** Right-click a vault in
the Alt+N notes sidebar (vault-folder context menu) → "Share with
reMarkable" → `Screen::share_vault_with_remarkable(vault)` (in
bridges/workspace.rs): renders every .md in the vault → folder_bundle +
pdf_document_bundle → scp (BatchMode+ConnectTimeout=8, passwordless) into
xochitl + `systemctl restart xochitl` → notification. Host = env
`NEOISM_RM_HOST` default `root@10.11.99.1`. Wiring: ModalAction::
NotesVaultShareWithRemarkable{vault} (modal.rs) + ModalActionTag
(chrome_policy.rs, CloseBeforeAction) + dispatch arm + tag arm
(lifecycle.rs ~2184/~3178) + menu item (workspace.rs is_vault_folder
branch, key "r"). **SSH KEY INSTALLED on device** (~/.ssh/id_ed25519.pub →
device /home/root/.ssh/authorized_keys) so the GUI scp is passwordless —
dropbear reads authorized_keys, perms 700/.ssh 600/authorized_keys.
Validated: shared the "Neoism" vault (2 notes) live, no password.

**TWO-WAY SSH SYNC DONE (user's chosen priority over live agent).** Menu
label now "Sync with reMarkable". `share_vault_with_remarkable` does PUSH
then `pull_vault_ink(vault, host, ssh_opts)`: for each .md note, scp its
`xochitl/<stable-uuid>/` page dir, parse each `.rm` (v6), stack pages by
i*PAGE_HEIGHT, `scene_from_strokes` → write `"<note> (reMarkable).neodraw"`
sidecar next to the .md (openable/editable in the existing neodraw editor).
**No on-device agent — pure SSH (passwordless key)**, so it's force-sync
now + background-timer later. Validated: converted 2 real device pages →
158 strokes → valid neodraw, dropped into the user's Neoism vault as
"reMarkable handwriting sample.neodraw" (example rm_to_neodraw.rs / desktop
pull_vault_ink). Workflow: write on a synced note on tablet → Sync → a
"<note> (reMarkable).neodraw" appears to open.

**ROBUST DIFF SYNC ("golden not sloppy") DONE (23 tests).** `sync_plan.rs`:
`SyncManifest{notes: path->SyncRecord{device_uuid,hash}}` persisted per
vault as hidden `.neoism-remarkable.json`; `plan_sync(local, manifest) ->
(Vec<SyncOp::{Push,Delete}>, next_manifest)` — pushes only NEW/CHANGED
(FNV content_hash), DELETES notes removed locally (rm `<uuid>*` on device).
Desktop handler now: walk tree → folders(idempotent) + collect notes →
load manifest → plan_sync → push changed + delete removed + pull ink →
save manifest. `is_remarkable_reachable(host, timeout)` (TCP:22 check) for
auto-detect. Notification: "X pushed, Y deleted, Z pulled".

**AUTO-SYNC DONE + WIRED.** sync decoupled into `desktop/src/sync/
vault_sync.rs` `sync_vault(vault_dir, name, host) -> SyncOutcome` (no UI
state; the right-click wrapper + the auto thread both call it). Background
poller `desktop/src/sync/auto.rs` `RemarkableAutoSync::start()` — thread
polls `is_remarkable_reachable` every 8s, syncs ALL vaults on connect + every
60s while connected, sends SyncOutcome over mpsc. Wired into Screen: field
`remarkable_autosync: Option<RemarkableAutoSync>` (on unless
`NEOISM_RM_AUTOSYNC=0`), started in the ctor, DRAINED each frame at the top
of `screen/render/mod.rs::render` → notifications. Compiles green.

**INK OVERLAY = EXACT "PAGE VIEW" (golden align, desktop-side).** For a
note with a `"<stem> (reMarkable).neodraw"` sidecar (+ overlay on),
`render_markdown_panels` builds a page-view Scene = the note's text laid out
at the DEVICE page geometry (`neoism_sync::markdown_to_pdf(md).layout` →
Text shapes at item.x, page*1872+item.y, item.size, colored theme.fg) +
the ink shapes (already page-stacked) appended on top — ONE Scene in the
1404×1872 frame. Renders: cover the reflowed markdown with a `theme.bg`
rect (order 9 > ORDER_TEXT 8), then `render_scene(scene, cam, rect, 0.0,
11)` with zoom=rect_w/1404, pan.y=rect[1]-scroll_y. **Alignment is EXACT BY
CONSTRUCTION** — text is drawn at the same positions the ink was drawn over
(both from the same markdown_to_pdf layout). `ensure_page_view` caches by
(note_mtime, ink_mtime). Screen fields `show_ink_overlay` (default on;
`NEOISM_RM_OVERLAY=0` to hide) + `ink_overlay_cache: HashMap<md_path,
(md_mtime, ink_mtime, Option<Scene>)>`. `toggle_ink_overlay()` ready, NOT
keybound yet (needs Act/PaletteAction variant). Desktop-only (no shared/
wasm changes). To SEE alignment: real round-trip (sync note → write on its
device page → sync → open note). ⚠️ minor: pane scroll-range is the normal
renderer's; if ink extent >> text it may cap scroll (fine for normal notes).

**INK-LAYER MODEL (corrected, "perfect"):** ink overlays the REAL rendered
markdown (rich, scrolls) — NOT a page-view of raw text. Overlay renders
STROKES ONLY (text shapes filtered) so leftover baked files can't double
the markdown. Ink stored in a HIDDEN dotfile `.<stem>.ink.neodraw` (out of
the file tree), via `crate::sync::ink_sidecar_path` (+ `legacy_ink_sidecar_
path`, `strokes_only`, `migrate_legacy_ink`, `load_ink_layer` in sync/mod.rs).
Auto-migrates old visible `<stem> (reMarkable).neodraw` → hidden strokes-only
on note open. All 4 sites use it: overlay (ensure_ink_overlay), draw mode
(draw.rs), device pull (vault_sync). Overlay render: content coords, zoom 1,
pan.y=rect[1]-scroll_y, order 12. **In-place DRAW MODE** ("Draw on Note"
toggles `Screen.draw_over_note`): left-drag over the markdown → freehand
strokes in content coords (draw_over_note_pointer phases 0/1/2 via the
markdown press/drag/release handlers; markdown_drag_active() true while
drawing); live render via draw_over_note_live_scene; saves hidden sidecar;
run again to finish. Fixes shipped: eraser skips Text; `u`=undo;
fit_to_view width-fits tall docs. **Future modular vision (user wants):**
unify so neodraw can embed rendered markdown (inverse of the existing
```draw embed) → every doc = composable markdown layer + stroke layer.

**"DRAW ON NOTE" command DONE.** Command palette → "Draw on Note"
(`PaletteAction::DrawOnNote` in command_palette/actions.rs enum + category
+ commands.rs registry "draw annotate ink" + palette.rs dispatch →
`Screen::draw_on_current_note` in bridges/draw.rs). Opens the active md
note's `"<stem> (reMarkable).neodraw"` in the FULL neodraw editor (toolbar/
tools/scroll/save). NEW sidecars are SEEDED with the note's text at page
geometry (markdown_to_pdf layout → Text shapes, theme.fg) so you draw OVER
the words like on the tablet. The overlay (`ensure_page_view`) heuristic:
if a sidecar HAS Text shapes → render directly (Neoism-drawn page view);
else (device-pulled ink-only) → lay out text + append ink. So Neoism-draw
and reMarkable-write feed the SAME ink layer symmetrically. Compiles green.

Note: a SQL DB exists (neoism-workspace-index) — could hold sync manifest
state instead of the per-vault `.neoism-remarkable.json` file later.

**Live-streaming PULL (optional later, NOT priority):** call RemarkableSync::
poll() in the app tick + paint overlay_scene into the note pane; deploy
agent (docker present → `cargo install cross` + `cross build --target
armv7-unknown-linux-gnueabihf -p neoism-rm-agent --release`, scp, run
--connect <laptop-ip>:47800); presence cursors. Deep render-loop work —
not visible/testable until both agent + paint are wired. Mode B (typing on
device = markdown→rM RootTextBlock) still deferred.

**Desktop wiring** (`neoism-frontend/desktop`): `mod sync` added.
`sync/ink_interop.rs` converts CRDT `Stroke` ⇄ neodraw `Shape`(Freehand)/
`Scene` so pulled ink renders via the existing neodraw engine. Lossy axis:
per-point pressure (neodraw freehand has none). Desktop depends on
neoism-sync; full `cargo check -p neoism` is GREEN.

**Why not the shared UI crate:** neoism-ui compiles to wasm; Loro/SSH/mDNS
are native — keep all sync code in `neoism-sync` + desktop, never in shared.

**Remaining phases (tasks #14-16):** ④ LAN transport (mdns-sd discovery +
length-prefixed socket SyncPeer, "AirPods-fast" auto-connect, presence) —
device-independent, testable via loopback. ⑤ reMarkable bridge: SSH/SFTP
(rM2 has root SSH by default at 10.11.99.1), push = markdown→page-sized
PDF→annotatable import; pull = on-device ARM (armv7) agent tailing
`~/.local/share/remarkable/xochitl/<uuid>/*.rm` streaming diffs. ⑥
extension entry (extensions_page is Mason-style) + right-click "Share with
reMarkable" + Loro ephemeral presence cursors.

**reMarkable storage model (researched, confirmed) — the device has NO
markdown concept.** Each doc in `~/.local/share/remarkable/xochitl/` is a
UUID bundle: `<uuid>.metadata` (JSON: visibleName, type
DocumentType/CollectionType, parent, lastModified), `<uuid>.content` (JSON:
pages, fileType, orientation, **formatVersion — firmware-versioned, v1 vs
v2/"new file format"**), `<uuid>.pagedata` (template per page),
`<uuid>/<page>.rm` (binary strokes), `<uuid>.thumbnails/` (jpegs),
`<uuid>.pdf`/`.epub` ONLY for imported docs. Two doc kinds: **native
notebook** (template + ink, no base file) vs **annotated PDF** (keeps
`.pdf` + ink layers on top).

**GOLDEN design decision (long-run):** adaptive hybrid. Markdown is
ALWAYS canonical in Neoism's CRDT; the device is a lossy **projection**
(never the source of truth, so rich markdown never degrades). Per-note:
default = display-fidelity (PDF projection, ink-only on device), opt-in =
editable-text (project into reMarkable typed-text/RootTextBlock, patch
edits back best-effort). Ink always two-way. BUILD ORDER: Mode A first
(its render/bundle/pull infra is needed by both modes — not throwaway),
then Mode B as additive per-note layer after validating v6 on device. Why
not a "thin conversion" for text: ink is ~1:1 (both are strokes) but text
has no shared model (markdown vs rM typed-text) so it's lossy, not thin.

**on-device agent `neoism-rm-agent`** (workspace member): DONE, compiles on
host. Tails xochitl `.rm` files, sends `BridgeMsg::PageInk{page_id="doc/page",
strokes}` to Neoism over TCP; reconnect loop; v6 errors logged+skipped.
Deps: neoism-sync + std only. Deploy: `rustup target add
armv7-unknown-linux-gnueabihf` (or `cross`), build, scp to tablet, run
`--connect <neoism-host:port>` (+ systemd unit / toltec for autostart). No
ARM toolchain in this env — give user the recipe.

**Thin-layer design (markdown never lives on device):** PUSH = render
markdown → page-sized **PDF** → write an **annotated-PDF bundle** (.pdf +
.metadata + .content + .pagedata) into the Neoism vault folder; text is the
PDF background, user writes on top, text stays authoritative in Neoism.
PULL = read per-page `.rm` ink → parse → merge into CRDT ink overlay.
**Anchor mapping:** device strokes arrive in PDF page coords, so the
markdown→PDF renderer must ALSO emit a text layout map (char/line →
page,x,y) to anchor strokes to words (reflow-aware). v6 `.rm` is a
block/tagged format (SceneLineItemBlock=strokes, RootTextBlock=typed text,
tombstones, version fields) — itself CRDT-ish; v6 ALSO carries typed text
(bonus pull). Rust v6 parser exists: `Lyr-7D1h/remarkable-lines`; Python
ref `ricklupton/rmscene`; Kaitai spec `matomatical/reMarkable-kaitai`.

**Device-dependent unknowns to resolve with the user's tablet:** firmware
version (picks .content schema + v5/v6 decoder); scp a sample bundle +
`.rm` page to validate the v6 parser & bundle writer against real data
(don't write blind); confirm SSH (default on at 10.11.99.1) + toltec for
the agent. Firmware-INDEPENDENT pieces I can still build: on-device agent
(`neoism-rm-agent`, bridge.rs protocol done), markdown→PDF+layout renderer.
See [[feature_neodraw]] (ink engine reused), [[feedback_build_workflow]]
(cargo check only).
