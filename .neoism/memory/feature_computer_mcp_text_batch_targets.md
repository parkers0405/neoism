---
name: "Computer MCP: text fidelity, batches, window targets and waits"
description: "Immutable native Linux text map; bounded batches + model images/progress; Hyprland/Win/mac exact foreground targets + bounded window waits; no browser/AX content integration"
type: "feature"
scope: "project"
origin: "implementation and native source review"
created: "2026-09-12"
updated: "2026-09-12"
---

Built-in computer MCP improvements extend existing picker-only enablement and explicit computer_use permission; no GUI/env gate/browser integration added. Main source computer_use.rs; new computer_use/linux_text.rs and computer_use/windows.rs (cross-platform window module), existing platform.rs/shortcuts.rs retained.

Linux literal text now uses our direct Wayland virtual keyboard with one immutable ONE_LEVEL XKB keymap per bounded text action, fixed per-character codes, paired down/up, zero device modifiers, memfd rewound before publish, single seat. Avoid Enigo mutable Unicode symbol lookup: inspected Enigo0.6.1 keymap2::key_to_keycode min..max EXCLUDES max; missing symbols mapped at max can be remapped repeatedly. This is a concrete source defect, NOT proof all reported case/punctuation failures stemmed from it. Native XKB tests compile actual generated maps and decode uppercase/punctuation/Unicode/repeats/tabs/newlines, all512 entries/max code, modifier release. CR and LF each Return (CRLF two Returns). No native desktop events in tests. Shortcut raw-code resolution/FD offset-zero fix unchanged.

New computer.batch: 1..16 existing action objects; prevalidate whole list, total text512; optional top-level target + screenshot display ID. execute owns one SERIAL; execute_locked handles nested operations without re-lock. Fail-first completed/total/failedIndex/error/partialActionPossible; optional final MCP Image with frame token uses existing attachment pipeline. Capture errors don't erase progress; cancellation/revocation prohibits final capture. Arc AtomicUsize tracks completed count for timeout response while native call may still be in flight. Generic batch_with operation seam tests actual orchestration without desktop: failure tail skipped, capture+image returned after native failure, no capture after cancel. Check::check_probe checks revocation/deadline BOTH before and after potentially blocking target validation; test revocation/cancel during probe. Focus checks before each focus API (incl separate mac app foreground/raise).

New computer.windows/focus: opaque30s snapshot tokens, replaced on re-list, revoked by stop; native id/PID/app identity + geometry foreground revalidated. Linux Hyprland native Unix IPC only (no hyprctl); bounded4MiB,2s IO timeout; other compositors windows unsupported. Windows XCap enum under per-monitor DPI + exact HWND focus + SetForegroundWindow. macOS XCap enum BUT NOT is_focused (only app PID!): exact AXFocusedApplication/AXFocusedWindow to native ID via undocumented _AXUIElementGetWindow, exact AXRaise. TCC required and failures closed. This narrow AX use supports window focus only, no accessibility tree/content integration.

Optional target on screenshot/input/batch is foreground PRECONDITION, NOT sandbox/background input/click confinement. Screenshots remain full display; input image-pixel coords, never window-relative. Frame stores target snapshot; identity+geometry must match input target (can't omit target to reuse targeted frame). Focus invalidates frame. Native handle reuse/focus races cannot be excluded; docs explicit. Linux pointer remains single-output-only.

computer.wait: explicit target, foreground|closed, timeout_ms100..3000,100ms polling, cancellation; errors not interpreted as closed. No browser/page load/network idle/task-completion inference. Tool schemas and neoism-agent/docs/computer-use.md updated. Broad accessibility/browser integration remains future work after need assessment. Native OS smoke tests still pending user authorization; all native source mac/windows cross-checks use scratch manifest /tmp/neoism-computer-use-platform-check importing exact source. Preserve unrelated workspace changes.
