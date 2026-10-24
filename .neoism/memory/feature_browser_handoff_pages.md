---
name: "Website-matched join and MCP browser pages"
description: "Shared website SVG/Geist join and MCP pages, exact 15-effect random click animation, CSP and render-only previews"
type: "feature"
scope: "project"
origin: "user requested website styling followed by exact 15-variation random click animation"
created: "2026-10-16"
updated: "2026-10-16"
---

Join and MCP OAuth success browser pages share neoism_agent_service_api::browser_pages, styled after sibling ~/projects/neoism-web: exact inline hero wordmark.svg (six path defs/18 uses/viewBox -24 566 1550 338), light beige/colored letters and dark black/white palettes, embedded Geist WOFF2 with full adjacent and CSS-embedded SIL license plus root NOTICE. No runtime dependency on website checkout or network assets. Desktop fast_servers/invite_page.rs preserves canonical URL validation, escaped name, native deep link, clipboard fallback (hidden field unhidden before select), and automatic handoff. MCP mcp_routes callback only replaces final success Html after original state/scoped auth/token-exchange flow; error JSON behavior unchanged. HTML font requires256KiB body test bound, not64KiB.

User subsequently requested SAME website on-press random15 animations. Implemented exact dependency-free neoism-web/src/pixel-wordmark.ts compiled offline using alreadyinstalled esbuild into shared browser_pages/pixel-wordmark.js, IIFE global NeoismWordmark with footer mounting #hero-title. Copied pixel-wordmark.css; sharedrender wrapsSVG in #wordmark-trigger nativebutton, enables siteletterhover/hitboxes, embeds WORDMARK_SCRIPT on BOTHpages. All15 effects: scan, scatter, rain, beams, decrypt, crumble, slice, wipe, waves, fireworks, burn, vhs, rise, checkerboard, spiral. Actual website Fisher-Yates shuffle-bag semantics: each oncebefore repeat, no adjacent bagboundary repeat, intro consumesfirsteffect, clicksreplaceactiveanimation; keyboardEnter/Space/reducedmotion/visibilitycleanup retained unchanged. Generatedbundle README has exactregeneration command; don'tvendorwebsite main.ts/analytics or inventapproximateeffects.

Join CSP authorizes exact WORDMARK_SCRIPT and unchanged handoff SCRIPT with separate SHA256hashes, plus sharedstyle hash/font-srcdata; no unsafe-inline/external scripts. Existinglandingsecuritytest nowvalidates EVERY inline script/style hash. Sharedcallbackunit tests nowexpectoneanimationscript, notzero. Productioncallbackflow/tokenredirect unchanged, onlysuccessrendering.

Actualrenderer previews: ignored desktop fast_servers_backend::render_browser_page_previews with NEOISM_BROWSER_PREVIEW_DIR=/tmp/neoism-browser-page-preview; exports join.html,mcp.html andmatchingjoin-csp.txt. Previewremoves ONLY automaticcustomschemehandoff and recomputes thatscript's CSP hash; neverrunsOAuth/tunnel. Python loopback4188 servespreviews, xdg-open openedbothpages, laterreopenedboth with ?animation=15 foruncachedinteractiveversion. Serverjob job_145721270001bF6fyKkdYETCc2 has1hourtimeout, notpermanentservice.

Verificationpassed: sharedbrowser2unit, Fastbackend21tests (previewignorednormally, explicitlyexecuted1), desktop+Agentcargocheck andgitdiffcheck. InitialPlaywrightvisualQA8combinations2pages×light/dark×1440/390: Geistloaded,18SVGglyphs,matchingpalette,nooverflow/errors, actualCSP. Latesttemporary /tmp/.../check-wordmark.cjs usingwebsite'sinstalledPlaywright verifiedALL15actualvariants, bagboundarynonrepeat, settle, keyboardactivation,reducedmotioncancellation/no-replay,nooverflow/pageerrors forbothpages×lightmobile/darkdesktop underactualjoinCSP. Browserpreviewsopenedfortheuser; sourcesupdated, noinstalledbinaryoverwrites/liveOAuth/credentialmutations. Productionprocessesneedrebuiltversionstoservenewpages.
