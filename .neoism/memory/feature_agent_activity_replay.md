---
name: "Clickable Agent activity word animations"
description: "Native Agent activity-label click replays 15 splash effects using actual pixel glyphs; clipped/displaced hits, menu preservation and bounded redraw"
type: "feature"
scope: "project"
origin: "user request for Crafting/background/subagent working labels to replay 15 splash animations"
created: "2026-10-16"
updated: "2026-10-16"
---

User requested clicking native Agent activity labels (Crafting, Background, Sub-agents working etc.) replay a random one of the same15native splash effects. Implemented in shared renderer, not browserJS. All primary activity labels use shared/panels/agent_pane/view/user_input.rs::render_streaming_status_row. Background child-summary row retains its existing task menu; both panes' activity_status_contains exclude background_status_rect includingboundaries, sooverlappingink cannotstealmenu clicks.

Engine: shared/src/primitives/wordmark_animation.rs nowAnimationFrame::draw_glyph takes Sugarloaf, char, glyph_index,glyph_count, [glyph_x,glyph_y,glyph_advance,font_size], rgba, clip, occlusions, depth, order, &mutVec<[f32;4]> hits, returnsboolsupported. Uses actual bundled PressStart2P TTF via Sugarloaf Swash reexport, CPU8px masks with256entrythreadlocalLRU/1024cellcap; no newdeps. Reusesall15samplers/sparks/layers/clock/shufflebag, preservesoriginaldraw_letterparity. Countincludesrenderedwrappedchars/spaces, indicesmayexceed6; unsupportedcharactersreturnfalse fornormaltextfallback, supportedspacesreturntruewithoutpaint. Actualpaintedtriangleboundsafterclip/occlusion/nonzeroalpha appendedtohitoutput, includingdisplacedeffectcells, soclickingflyingvisibleinkreplaysnotjustidleletterpositions. draw_letter passesNone tooptionalinternalpaintcollector unchanged.

Shared anddesktopPane eachown independent activity_animation/activity_status_rects/activity_session_id. APIs activity_animation_frame/register_activity_status_rect/clear_activity_status_rects/pop_activity_status_click/activity_status_contains. clearframeToolHitRects removesstalehits evenoffscreen/zeroheight,statusrendereralsoclearsbeforeearlyreturns; reset_transient_timeline_interactions cancelsplaybackonsessionswitch. Frameclear alone doesNOTcancelreplay. animation_reason returns boundedagent_activity_word onlywhilevisiblehitsandactiveframeexist. Existingweb_timeInstant engine avoidsrawf32epochfreeze. Desktopbridgeconsumesactivityclickbeforeselection andpreservesbackgroundmenu; mousePointeraffordance andwasmrendered/agent clickpaths wired. No home-onlyconversationgate reuse.

Renderertrait/macrohandwrittenimpl exposesthreeAPIs. ExistingidlePressStart2Pscramble/wave/colors/shadows retainedoutsideclickanimation. Duringmainanimationdrawactualglyphcells; settletaildrawidlefirstthenfadingoverlay; unsupported/fallbackfontdrawsnormally. Glyphadvance measuredseparatelyfordots/timers/layout; wrappedactualglyph_countpassed. ReusesonescratchhitVecacrossglyphs, drainsintoPanehits; stableidlewordhitsandactuallypaintedeffecthitsbothregistered. Newprimitives::geom::visible_rect_fragments clipsviewport/subtractsoverlayrects foridlehits, validatesfinitepositivegeometry. Existingbackgroundrowpriorityguardhandlesoverlap.

Finalexecuted: cargo test -pneoism-ui --lib wordmark_animation12passed; sharedactivity_25passed; desktop --binneoism activity_11passed; pureproductiongeom.rs rustc --test2passed. Combinedcargo check -pneoism --binneoism --tests -pneoism-ui andgitdiffcheckpassed. CoversrealTTFmasks/all15effects, oldwordmarkparity, extremeindices, clipping/displacedhits/zeroalpha, cachebounds, replaybag/restart/expiry/sessionreset/frame-localhits/menuoverlap/noredrawidle. NativeGUIvisual/device-scaleparitynotrun; wasmclicksourcewiredbutwasmtargetbuildnotclaimed. NoAgentserverbehavior/liveDB/installedbinary/releasebuildchanges. Rebuilddesktoptoobservefeature.

Nativeedittoolrepeatedlyfailedonlymacro/sharedtraitimplblocks inuser_input.rs despiteexactreadmatches; standardtargetedGNU patch fallbackappliedtwohunks successfully afternativeeditandapply_patchavailabilitychecks. Allothereditsusednativeedit; no blanketformat/reverts.
