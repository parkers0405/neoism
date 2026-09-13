# Unified computer typing

`computer.input` has one canonical `type` action. It chooses a complete method before input begins; the model does not have to split ordinary text into individual keys. `text` remains a compatibility alias. `press_key` (`key` alias) is separate for Enter, shortcuts and navigation.

```json
{"action":"type","target":"<window token>","text":"Hello, world!"}
```

## Method selection

The optional `method` is `auto` (default), `keyboard`, `native`, or `paste`.

| Platform | `auto` | Other methods |
| --- | --- | --- |
| Linux Wayland | Existing-layout keyboard strokes if the whole string is supported; otherwise explicitly authorized clipboard paste | `keyboard` refuses unrepresentable text; `native` is unsupported; `paste` requires replacement policy and consent |
| Windows | Native Unicode packets | Forced layout-keyboard and clipboard methods are currently unsupported |
| macOS | Native Unicode events | Forced layout-keyboard and clipboard methods are currently unsupported |

Linux keyboard typing uses the captured active layout and actual Shift/level-three selectors. It does not rewrite the layout, choose another group, guess Ctrl+Alt for AltGr, or silently synthesize Compose/dead-key sequences. Consumer/media-key aliases are not substituted for printable keys. For example, `#` on US uses Shift+3, not the extended numeric-pound key.

Windows/macOS native insertion is not an application acknowledgement. Accessibility, desktop integrity, held modifiers and application behavior still affect delivery.

## Clipboard eligibility is not permission

`clipboard_policy` defaults to `forbid`. To allow selection of clipboard paste for otherwise unsupported printable text:

```json
{"action":"type","target":"<window token>","text":"Hello 🦀","clipboard_policy":"replace"}
```

The model-supplied `replace` value only makes paste eligible. Actual clipboard replacement also requires the separate, explicit `computer_clipboard` permission through the normal permission system. MCP enablement, a wildcard allow rule, or ordinary `computer_use` consent alone does not grant that permission.

The entire string is pasted, not just its unsupported suffix. Replacement can disclose the payload to clipboard history and other clients. The previous clipboard is neither read nor restored. No failed keyboard operation is automatically retried as paste.

For deliberately pasting multiline text, force the method explicitly:

```json
{"action":"type","target":"<window token>","text":"line one\nline two\tvalue","method":"paste","clipboard_policy":"replace","paste_shortcut":"control-v"}
```

`control-shift-v` is available for applications that require it. The older `paste` action remains an explicit-paste alias. Paste is currently Linux-only.

`auto`, `keyboard`, and `native` reject C0/C1 control characters, including LF, CR and Tab, rather than silently converting them into Enter/Tab keys. Forced paste permits literal controls except NUL. A terminal can execute pasted commands: paste is not a sandbox or a guarantee against submission.

## Preflight and dispatch

- Limits are 512 Unicode scalars per request and across text/paste actions in a batch.
- All deterministically detectable action, keyboard-plan and clipboard-capability failures are checked before the first batch input action.
- The paste chord is prepared before clipboard publication. Keyboard layout/group assumptions are revalidated before publication and dispatch.
- Every method is selected for the whole string before execution. A runtime error does not trigger replanning, a second method, or an automatic retry.
- Focus, cancellation, permissions and layout assumptions can still change after preparation. Delivery is not an atomic transaction; a dispatched prefix may remain.
- Cleanup releases only the integration's own held keys. It does not replay or release the user's physical modifiers.

## Results are evidence, not promises

Successful dispatch remains `applicationVerified:false`. Results identify the selected method, completed native dispatch units, current-unit uncertainty, clipboard effects and cleanup failures. A native unit is not necessarily a character inserted into an application: Windows reports UTF-16 units, and paste dispatches a shortcut rather than acknowledging the receiving field.

Inspect the resulting field before Enter or another committing action. Never treat an unverified result as permission to retry: the first insertion may already have arrived.

## Verification boundary

Deterministic tests cover layout planning, Shift/AltGr, unsafe key actions, unsupported suffixes, clipboard policy/consent, full-batch preflight, cancellation, partial native packets and cleanup. Real application tests must exercise the unified facade and check exact received values; parsing a keymap or acknowledging a Wayland request does not establish application fidelity.

Verified live on Linux: native-Wayland Helium (including omnibox navigation, standard edit controls, consented Unicode fallback, and a 512-uppercase-character call under the public deadline), GTK4 Entry, and an isolated offline Minecraft 26.2 EditBox through private XWayland. Minecraft values were read by an observation-only Java agent; no world was created and no user account or server was accessed. Reproduction commands and evidence are in `scripts/native-typing-acceptance.md` and [Computer use](computer-use.md).

Windows/macOS adapters passed isolated cross-compilation and mock-native cleanup tests. Full server cross-checks were blocked by missing native C toolchains (`lib.exe` for Windows and Apple compiler/SDK flags on this Linux host); no Windows/macOS live-app result is claimed.

Active IMEs and application handlers can transform or intercept input. Window focus is not proof of editable-field focus. There is no universal host protocol that guarantees literal insertion into every application, and this API does not claim one.
