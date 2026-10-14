---
name: "Website pixel titles and navigation"
description: "Latest website refinements: Press Start2P short/shadowed scroll titles, hover pixel field, 15 shuffled effects, rounded plasma typography, Back/Discord/X/icons"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-14"
updated: "2026-10-14"
---

# Website pixel titles, pointer field, and current navigation

Latest refinements to website/ supersede older public-website notes where overlapping. Main hero click has15 originaleffects with shufflebag (all15before repeat/no boundaryrepeat): scan,scatter,rain,beams,decrypt,crumble,slice,wipe,waves,fireworks,burn,vhs,rise,checkerboard,spiral. Same child agent ses_ec4c9012affe42CLgP5LqlgHrd researched Omarchy and ownsimplementation and hero-field. No copiedunlicensedOmarchycode/WASM.

User's 'player2 pixel font' is exact Press Start2P regular400. Copiedunchanged nativebinary from sugarloaf/src/font/resources/PressStart2P/PressStart2P-Regular.ttf to website/public/press-start-2p.ttf, OFLcopyright/license press-start-2p-LICENSE.txt. Fontface in mainstyleCSS. Featureh3 titlesnow max3words: 'Terminal first.', 'Agents in context.', 'Work together.'. Font clamp18px,2vw,24px lineheight1.7. Static2echo shadows (+2,-2,.5) and(+4,-4,.3) matchingNeoismSVG. src/pixel-titles.ts/css usesactualDOMRangeglyphpositions+Canvasmask, three matchinginklayers, 4pxoverlaypadding, scan/slice/waves one-time scrollreveals. OneRAF,3000tile draws/headingtotal acrosslayers, fontfallback/reducedmotion/resize/offscreenrestore.

Hero src/hero-field.ts/css: localditheredshards/shortoutwardtrails behindhero onfinepointerhover. Max384particles DPRcap2 crispintegerdevicepixels. No ambientnoiseoridleRAF. Touch/coarse/reducedmotiondisabled, hidden/offscreen cleanup, ResizeObserver, livepalettechanges. Canvasinside .hero as .hero-pixel-field negativez isolatedlayer, doesn'tinterceptclicks orwordmark/screenshottilt.

Plasma sectionkeepsSynapse43columnrenderer/baseblack/ripplebothschemes, now24pxroundedclipping (CSS plasma.css). Reworkedtypography bigGeistheadline 'Not a walled garden.', smallerblocksubtitle 'A work in progress. Yours, too.', uppercaseeyebrow, body max650, highcontrastwhiteGitHubbutton. No overlay/recoloring ofbackground. Fixedfractionalbufferresize: root.getBoundingClientRect() not integerclientHeight beforeDPRrounding.

Docs home linknowlabelBack withn icon andariaBacktoNeoism, visiblemobile, href /. FooterDocs/Discord/X/MITLicense all14px inlineicons; renamedCommunityDiscord, X https://x.com/Mr_Settle. LandingAPIandGitHubnavicons; docsGuides/APIreference/GitHubicons. DocsGitHubicononlyatmobile tofit320, accessibleGitHublabel preserved. Footerbrandstilln-only. MainAPI href /api/docs/#reference,footerDocs /api/docs/.

Verified latesttypecheck/prodbuild,9unittests,117browserpassed9capabilityskips (126total). Screens320-1920, themes, titles/raster/shadows, scrollonce, pointerfieldidle/lifecycle, navigationicons/links, responsiveBack, existingScalar/skeletons/15effects/plasma unchangedtested. npmauditzero. No publicdeploy/DNSchanges performed. READMEupdated. LocalVitepreviewlocalhost4321 (finitebackgroundtimeout).
