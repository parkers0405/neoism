---
name: "Smooth agent stream follow"
description: "Stream-follow now shares wheel spring; bottom-relative moving lag with explicit owner, user interruption, bounded settle, f32 precision fix"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-08"
updated: "2026-10-08"
---

# Smooth agent stream-follow scrolling

Implemented automatic stream-follow using the EXISTING mouse-wheel critical spring, not a new generic Scroll physics profile. User reports manual trackpad/wheel feel is already good; keep it unchanged. Both desktop pane/timeline.rs and shared state/timeline.rs previously forced bottom-relative timeline_scroll_px=0 in set_timeline_metrics whilefollowing, causing absolute scroll-top to jump whenever modelcontent grew.

## Implementation

Shared panels/agent_pane/timeline_scroll_policy.rs exports TimelineScrollOwner::{Wheel, FollowBottom} plus clock-free step_timeline_spring(position,velocity,target,max_scroll,dt)->(position,velocity,settled). Exact currentwheel integration preserved:omega16, substeps<=1/240s, frame<=50ms, exactsnap below0.5px error and30px/s speed. Zero scrollrange settles immediately. Trackpad direct1:1 +7x velocity withtau0.28/stop50 unchanged.

Existing timeline_wheel_target_px Option<float> nowalso holds follow target0, with separate timeline_scroll_owner discriminator. On establishedfollowing growth, preserve absolute top by adding new_max-old_max to existingrelativeoffset; springthatlag to0 on subsequentframes. Preservevelocity/clock across repeatedgrowth; followlatch staysTRUE evenlag>1px. Generic manualwheel targetrebasing applies ONLY Wheelowner, never followtarget0. Initialhistory/session/cache restore/explicitbottomjumps immediate. Firstauthoritativehistory hydration uses timeline_history_position_hydrated and clearsviewportRect eveniflive/emptycanvas alreadypainted, so itdoesn't animate oldhistory. Establishedfitting reply'sfirst overflow remains smooth. No dependency onis_streaming for finalsettle afteridle/statusremoval.

IMPORTANT precisionfix: follow reconciliation must use old_relative_offset.clamp(0,old_max) + (new_max-old_max), NOT new_max-(old_max-old_relative_offset). Roundtripping through hugeabsolute f32 coordinate canround off fractional lag everyframe and leave spring permanentlyactive. Mirrored67,108,864px extent regressiontests assertunchangedmetrics preserve exactlag andsettle.

Manualpixel/wheel/halfpage/touchdrag/fling/scrollbar/selectionauto-scroll cancelFollow ownership; actualuserupward input disablesfollow. Anchors/prepend retainpriority. Follow ticks DO NOT refresh timeline_last_scroll_at/historyrequestlatches or count asuserhistorygestures. Shared fling_timeline(0) mustexcludeFollow owner from was_gliding>=120px/s classification ormobilefocustaps get swallowed. Rootview keeps existing tick-beforemeasure ordering; metrics-createdtarget ownsnextframe via timeline_is_inertial. Hostvisibility gates unchanged.

Reuses existing agent.streaming-text-animation optout through ToolMotionState::is_enabled. Disabled follows snap immediately; activeFollow clears/snapsonlyifstillfollowing, manualWheel unaffected. Helpers clear_timeline_motion, cancel_timeline_follow_motion, snap_timeline_follow_if_disabled handle owner/target/velocity/clock together.

## Verification

Final parentchecks passed:cargo check -p neoism -p neoism-ui; wasm32web check -p neoism-terminal-wasm --features web; shared follow_scroll17tests; desktop follow_scroll14tests; shared timeline_scroll_policy5tests; gitdiff--check. Subagent additionallypassed existing focusedwheel/trackpad/anchor/prepend/halfpage/warmrestore tests. Broader integrationtest invocation previously hit unrelatedmissing PanelContext.plugins inshared/tests/file_tree.rs; focused --lib tests passed. No releasebuild or GUIvisualrun performed.

Duringwork usercaught subagentusingBash/Python sourceedits despite workingnativefiletools; see feedback_subagent_native_file_tools.md. Futuredelegatedimplementation must explicitlyuse nativeRead/Search/Edit/Write andBashonlyprojectcommands.
