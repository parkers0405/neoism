---
name: "Public neoism.ai website"
description: "Vite public site: self-hosted Geist, system PastelDark/Retro95, Scalar+canonical categorized guides+skeletons, pixel hero, Synapse canvas plasma, stable GitHub downloads"
type: "project"
scope: "project"
origin: "neoism-agent"
created: "2026-10-14"
updated: "2026-10-14"
---

# Public neoism.ai website

Standalone marketing site `website/`: Vite7 + vanilla TS, semanticHTML/CSS, no Rust/WASM/applicationbuild. User requested self-hosted Geist, icon-only header/footer (existing lowercase n), terminal splash SVG hero split per letter. Per-letter hover and click whole-word original pixel scan/scatter/rain effects are in `src/pixel-wordmark.ts/css` (390 tiles18layers, no consecutive repeat, boundedRAF, no copied unlicensed Omarchy code/WASM). Native buttonkeyboardactivation, staticSVGfallback, reducedmotion/offscreen/hidden restoration.

Site follows system color scheme: `src/theme.css` exact PastelDark pureblack/nearwhite from shared/primitives/ide_theme.rs for dark; Retro95 mash-up warmgray#dedbd2/navy/squareedges andRGBY wordmark forlight. Bothmarketing+docs use it. User removed hero badge, under-photo captions, and entire bottom download section. Hero primarydownload OS suggests stableGitHubasset with architecturelabel. Entire note line `x86-64 · Latest stable release · All downloads` links to repo/releases; navdownload andunsupported/noJSprimaryfallback repo/releases/latest. HeaderAPI points `/api/docs/#reference`, footerDocs `/api/docs/`.

Stable URLs root `https://github.com/parkers0405/neoism/releases/latest/download/`: exactassets `neoism-darwin-aarch64.dmg`, `Neoism-x86_64.msi`, `neoism-linux-x86_64.tar.gz`. AppleSilicononlymac andx86-64onlyWindowsLinux. No token/frontendversionAPI orredeploy tofollowstable. BinariesGitHubnotVercel.

Open-source sectionfullbleed `#open-source` has Synapse exact researched Webflow pixelplasma background (not legacyimage): src/plasma.ts/css 43-column canvas seededfromsynapse-time-is-valuable palette/fields, opacity.68 desktop/.6mobile, blackbasebothschemes, whiteforeground. Interactivepointerripples, RAFstopsoutview/hidden, reducedmotionstatic t=.6. ReferenceuserSynapse publicsection 'You Need Modern Communication / Your Time Is Too Valuable' selector .section-849.new on www.synapsecommunications.ai.

Docs: `/api/docs/` handbook via docs-guides.ts/css + guide-links.ts reads canonical Rust BUNDLED_DOCS manifest (`neoism-product-docs/src/lib.rs?raw`) andMarkdownsources welcome directory.45guides groupedGettingStarted,Neoism,NeoismAgent,NeoismDaemon; wikialias/scopedsibling links, search, headinghash/TOC, pagination; Marked18.1.1+DOMPurify3.4.17. `/api/docs/#reference` uses bundled/lazy Scalar1.74.0, modernlayout like Synapse project /home/parkersettle/projects/synapse-ai-hub/apps/public-api-docs/index.html. Actual9API tags from neoism-agent/openapi/v2.json distinctfromproductcategories. Localserver4096 documented; no hostedAPIorvisitorlocalrequests. ScalarAI/MCP/requestbuttons/telemetry/authpersistence disabled. @ai-sdk/provider-utils override4.0.33 fixestransitiveadvisory; npmauditzero.

Animated doc skeleton in api/docs/index.html and src/docs-skeleton.css; clonesstaticSSRlayout tocoverlazyimport, remains until ScalaronLoaded; aria-busy status andstaticreducedmotion. Parenthashrouter/api-docs.ts and Scalar lazy module separate; skiplink focusesmain without changingdocmode.

Vercel Hobby suitable userpersonal noncommercialOSSfunproject, customdomainneoism.ai+HTTPSsupported. Root Directory website, Viteoutputdist Node24 npmci, MUSTenableInclude sourcefilesoutsideRoot Directory (schema+canonicalmanifest+MD). Do notskipbuildsforoutsidewebsitechanges. READMEdeploymentDNSinstructions preserveMX/TXT/no domaintransfer; nodeployment/account/DNSchangesperformed.

Commands `cd website && npm run build && npm test && npm run test:e2e && npm audit`. Verified9unit+66browserpassed4intentionalplatformskips; desktopmobile320-1920, themeslive, skeletonblockedimport, keyboard, docslinkssearch/API, replaylifecycle, plasmastill/offscreen. MainAPI Scalar bundle~1MBgzip lazy; notloadedhome orGuides. Preview launched localhost4321 (backgroundserver hasfinite2hrtimeout).

Assets: public/workspace.png existingREADME historicalscreenshotfromcommit241e6daaea1249d2eff6ca94b91dbacc2c426b0f; featurevisualsillustrationsnotproductcaptures. SelfhostGeistOFLlicense public/geist-LICENSE.txt, no thirdpartyfont/analyticsrequests.
