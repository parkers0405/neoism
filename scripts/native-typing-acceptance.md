# Real native typing acceptance — Linux handoff

## Implemented

New acceptance files only; no edits to existing browser harness or production code:

- `check-computer-native-live.py`: bounded private nested Hyprland launcher,
  offline Rust snapshot build, GTK or vanilla Minecraft mode, source-drift checks.
- `native-typing-live-tests.rs`: ignored real-app tests through the SAME production
  `typing::prepare_text` / `typing::execute_text` facade used by the host.
- `native-typing-gtk.py`: ordinary GTK4 Entry with read-only key/value reporting.
- `NativeMinecraftObserver.java`: tiny read-only startup javaagent. Waits for the
  loaded Minecraft class, obtains its instance, schedules observations with
  `Minecraft.execute(Runnable)`, walks current UI children and reads EditBox values,
  labels, focus, bounds, window metrics and world-presence flag. Atomic JSON plus
  JSONL history. No transformers, no input events, no field writes, no state/focus
  setters, no authentication/profile reads. In 26.2 the screen is `mc.gui.screen()`.

Both tests use Auto+Forbid for keyboard typing. Unsupported Unicode and Replace
without a trusted permit must reject before observed mutation. GTK's deliberate
fallback uses Auto+Replace and a `ClipboardPermit::granted()` only after checking
its private fixture's explicit `--allow-private-clipboard` opt-in. This is a
trusted test capability, not a substitute for MCP human permission enforcement.

## Run (serially with other live tests)

```sh
NEOISM_LIVE_KEYBOARD_TEST=1 python3 scripts/check-computer-native-live.py \
  --nested --render-node /dev/dri/renderD129 --app minecraft

NEOISM_LIVE_KEYBOARD_TEST=1 python3 scripts/check-computer-native-live.py \
  --nested --render-node /dev/dri/renderD129 --app gtk --allow-private-clipboard
```

Uses cached offline Rust dependencies and the installed Java 25 compiler for the
small observer only. GTK4/PyGObject installed; PySide6 absent. No dependency
install/download, release build, mod, or system package changes.

## Actual Minecraft 26.2 PASS — `/tmp/nn-live-byiyut7h`

**Frozen-source pass**, launcher exit 0, ignored test 1 passed, no production source
changes during this run. Four exact real EditBox values, each <=32 characters:

1. `/balance Mr_Settle 42`
2. `AbC xyz 0123456789`
3. `!@#$%^&*()_+-=[]{}`
4. `;:'",.<>/?\|` followed by backtick and tilde

Native pointer navigation, verified by observer bounds + private screenshots:
Accessibility Continue -> Singleplayer -> temporary Preparing for world creation
message -> CreateWorldScreen NAME EditBox. An empty fresh saves directory takes
Minecraft directly into the creation form. There is **no final Create World click,
no Return dispatch, no world creation or server login**. The private process is
terminated while still editing its name. `hasWorld=false` throughout, and the
post-run `level.dat` check is empty.

Evidence:
- `test.log`: exact assertions and production TextEffects (keyboard, complete,
  clipboard unchanged, no cleanup failure or uncertain current unit).
- `report.json`, `minecraft-reports.jsonl`: read-only app-thread observations.
- `menu-0.png`, `menu-1.png`, `world-name-before.png`, `editbox-0.png` through
  `editbox-3.png`: actual private screenshot evidence.
- `src/`, `sources.sha256`, `production-after.sha256`, `source-check.json`.
- `NativeMinecraftObserver.java`, `observer.jar`, commands and private app logs.

The actual installed client used **XWayland within the private compositor**, not
native Wayland; this is recorded in each click's foreground-window evidence.
Injection still uses the production Wayland virtual pointer/keyboard, traversing
private XWayland and Minecraft's normal input path. The agent does not inject
input or instrument keyEvent bytecode; exact EditBox value + screenshot is the
observation proof. No claim of a separate Java keyEvent trace.

## Actual GTK facade PASS — `/tmp/nn-live-0md9o1ge`

Ignored test 1 passed:
- Exact whole ASCII/case/punctuation string including the previously missing `#`.
- Default forbidden unsupported Unicode before prefix: no GTK key/value mutation.
- Replace eligibility without trusted permit: also rejected without mutation.
- Explicitly authorized Auto clipboard fallback: exact `🦀 Native clipboard:
  Grüße 日本語 e\u0301 END` (actual combining accent, not literal escape text).
- Facade effects keyboard/clipboard unchanged, then paste/clipboard changed.

**Final verification:** BOTH launchers exit 0 and each ignored test passes. Both
call `KeyboardSession::finish()` explicitly and succeed. Both source-check files
show `changed_during_run: []` and `created_world_level_dat: []`. All actual
production sources and copied test modules were frozen through these final runs.
Earlier runs correctly flagged concurrent source drift; those are superseded.

Final actual-module SHA-256 verification:

```
2070cac346e06c77f32dbba0491a39a758c81f0959f838ef024edc0cee5dd45f  typing.rs
5c4b15e4cfbf7906b5a1e70aa1f6f0f50e10ced332b09441e0f4e57af3204275  linux_text.rs
9ca7d1cecb27af9d33f7b839ef0b59c0edba0c4847c6ce6c32067046c27e3249  layout_plan.rs
a67625e5a8bcb68f662d09ee51d9afbf627c81dd1c3d9aa055de0a953203ee35  linux_clipboard.rs
0c9d832a43d7d94aa3838fe1308fdac2283438a9d2b32e696dff7341a76af4a6  linux_pointer.rs
e4f6bed007e2f42e18972a942fa89d6e98ea1451b92e7a0aa7709cd8b395d856  shortcuts.rs
```

## Isolation and bounded failures retained

No accounts.json/auth/tokens or Prism instance files were read. Only installed
version metadata/libraries/assets/runtime were reused read-only. Synthetic offline
identity/accessToken `0`, fresh gameDir, unshared network namespace. `/home`,
`/root`, `/opt`, `/run`, `/tmp`, `/mnt` masked; allowlisted installed assets mounted
at `/opt/mc`. Only outer compositor gets host Wayland socket and one render node.
Inner clients have fresh `/dev`, no DRM/input nodes, masked host socket, private
HOME/config/runtime/D-Bus. The presentation output is removed BEFORE app startup;
all testing/capture uses the private `NATIVE-TEST` headless output.

Failures were not suppressed:
- Original `#` omission reproduced in GTK; fixed by parent planner owner.
- Early Minecraft probe captured onboarding only. New observer/navigation above
  closes that gap; startup alone is no longer reported as typing proof.
- Exact temporary preparation screen was added to the safety guard for native
  click cleanup. No input is dispatched while waiting on that screen.
- Some nested compositor startups disconnected clients during output removal;
  a one-second pre-app outer-configure settle improved startup. App readiness and
  screenshot timeouts still fail closed. Compositor exit status is retained.
- `/tmp/nn-live-b_sbx81h`: ASCII passed but GTK Ctrl+A clear failed with Super
  already present on first key event (67108864 mask). Likely inherited startup
  modifier contamination; it was not hidden or fixed by changing expected text.
  Subsequent GTK run above passed. Do not claim startup isolation is flake-free.

No remaining facade wiring required in THESE tests. They do not exercise MCP
permission prompts/target admission or Windows/macOS. All launched private clients
and compositors were terminated; no live host GUI injection tools were used.
