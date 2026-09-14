---
name: "TS GUI curved bubble tail and You fallback"
description: "Supersedes Unknown user fallback: local You; explicit anonymous author distinct; small curved masked iMessage tail"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-09-13"
updated: "2026-09-13"
---

Follow-up to feature_ts_gui_identity_avatar.md: user explicitly wants local fallback You (NOT Unknown user). identity.ts now falls back You after explicit GUI name (legacy saved You treated unset), server env/configuredName, server USER/USERNAME. Outbound prompt author omits fallback You. Timeline default localName You. messageAuthor preserves nonblank author exactly after trim; missing/undefined author retains legacy local fallback; explicit null/blank/malformed attribution now Anonymous user (not asserted local ownership).

Bubble: style.css user-only rule now --bubble-blue:#007aff shared by bubble + tail, lower-right radius16 (other corners20). Replaced two offset rect/cutout ::before/::after tail with single ::before SVG mask, 16x18, right:-5px, rounded cubic/quadratic tip. No bg-color cutout, border triangle, or content clipping. Avatar unchanged right:-44px size28; mobile width constraints and images-before-text retained. Tests UserBubble.test.ts source geometry/palette/layout plus identity/attachment expectations updated.

Validation: 138 targeted tests across 8 files passed; full GUI tsc --noEmit and diff --check pass. Full-suite attempt: 646 passed, 14 setup failures solely in concurrently edited ServerConnections.test.tsx (localStorage undefined), left to active registry agent. No browser/backend restart; registry/status icon work untouched. Native desktop actual name lookup NEOISM_DISPLAY_NAME > presence.display-name > native hostname, 32-char trim/cap. GUI explicit name > endpoint env/config name > server OS USER/USERNAME > You; browser never guesses hostname. Recommend setting presence.display-name to desired human name.
