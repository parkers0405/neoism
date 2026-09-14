---
name: "TS GUI identity and native avatar animation"
description: "TS animated shared avatar + native-config/server OS identity endpoint; remote authors preserved; full GUI 671 tests and tsc pass"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-09-13"
updated: "2026-09-13"
---

TS GUI Identity.tsx Avatar was frozen because avatarCells(seed) always uses native still-frame phase 0.6; not GIF/format issue. Now RAF directly paints SVG fills with performance.now/1000; matching seeds synchronized without React transcript rerenders; reduced motion returns 0.6; visibility + IntersectionObserver stop inactive animation and cleanup removes listeners/frames. UserAvatar shared at trailing/right side of human user messages with hover/focus tooltip; runtime cards excluded. Preserve info.author string before local fallback; do not replace remote authors.

identity.ts useIdentity resolves explicit GUI name (legacy auto-persisted You treated unset) > server configuredName > server systemName > Unknown user; default Preferences.name now blank. Controller uses resolved name for sidebar, Timeline local fallback, outbound prompt author (omits unknown). Server/client scope guards prevent stale identity responses. Native source desktop screen/presence.rs uses NEOISM_DISPLAY_NAME > presence.display-name > hostname, with 32-char cap. User specifically requested actual server OS user instead of browser hostname inference.

Added GET /v2/identity and neoism.identity capability, OpenAPI snapshot/generated SDK operation. ConfigSourceService::display_name default None; Neoism adapter reads only native presence/display-name from JSONC. Endpoint resolves env display override > adapter config and exposes USER/USERNAME as systemName; spawn_blocking config read. Endpoint returns only configuredName/systemName, no config secrets, directory-independent, standard auth; hosted callers denied and capability disabled (process owner is not authenticated tenant identity). Old live servers need deployment of new code; until then configured browser name works or Unknown user. No browser/backend restarted.

AttachmentPreview.ordering assertions now explicitly exclude trailing .user-message-avatar presentation from wire-part ordering and verify last-child SVG/tooltip; images still before text, immutable wire parts. App.layout profile test uses resolved Native Name rather than You.
Validation final: full GUI 671 tests / 66 files passed, full GUI tsc --noEmit passed, focused identity tsc passed, cargo check -p neoism-agent-server -p neoism-agent-neoism-adapter --tests passed (Rust tests compiled not executed), contract generator tests and git diff --check passed. Concurrent composer/settings/server integration edits preserved.
