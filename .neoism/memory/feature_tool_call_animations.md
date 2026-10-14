---
name: "Smooth tool-call animations"
description: "Tool arrival180ms fade/3.5px settle, output fog, completion crossfade, clipped expansion; actual read-group membership and rekey continuity"
type: "feature"
scope: "project"
origin: "neoism-agent"
created: "2026-10-07"
updated: "2026-10-07"
---

# Smooth live tool-call animations

Implemented shared/native Rust tool motion:180ms cubic ease-out arrival opacity and3.5logicalpx downward offset settling upward to0, paint-only with full layout space reserved immediately. Running/pending/streaming to completed/error crossfades outgoing indicator/label into currentstate; label-width reservation interpolates. Existing compact30px headers retained, no new cardchrome. Todos and live diffcards get consistent entry opacity/translation; diff previews retain fixed viewport size. Fresh ordinary text output and selected group-child previews reuse streaming cloud reveal. Syntax-highlighted diff lines do NOT have fresh-line fog; diffcard entry fade covers their text/chrome.

## Architecture

New shared helper panels/agent_pane/tool_motion.rs:ToolMotionState owned independently by shared and desktop pane implementations, session-scoped recent entries capped256, monotonicweb_time clocks. Live actualmutation ingestion records resolved IDs/priorstatuses; history/pagination/identical snapshots don't replay. Offscreen samples expire. Rendering marks visible deadlines; normal running-spinner ownership remains separate. Transparent firstframe MUST still own nextframe when futureink geometry is visible, or instantcompleted tools can remain invisible. Tool view motion.rs checks actual glyphbounds/viewport/occlusion evenalpha0; uncovered blankheader space doesn't own redraws.

Both pane types expose tool_motion_sample(id), tool_motion_state(), tool_text_reveal_state(); AgentToolPane trait plus desktop macro forward. Tool body selects nonblankdetail otherwise text; ingestion and historyreconcile compare thatsamebody via tool_reveal_body. Ordinary tooloutput projection owns cachedRc wrappedrows and uses full styledrun draw_text_revealed_clipped. Coalescedsourceupdates reuse currentlayout and only first sharpoldbaseline wraps; alpha excludedfromwrap keys. Explicitheight expand/collapse remains190ms but now starts atcurrentprogress for continuous rapidreversal. Bodydraw clips partiallyenteringrows ratherthan whole-row stepping. Turninganimationsoff clears activeexpansionmaps and invalidateslayout ONCE toavoid retainingcachedintermediateheight.

## Identity and groups

match_running_tool_part can reconcile temporary runningID to canonical completed partID. Rekey ToolMotionState and TextRevealState before recordingmergedmutation preserves arrival clocks, canonicalidentity and birthages. Destination entries win ifalready present.

IMPORTANT: real generated readgroupID is ONLY first_id.., NOT a joinofallmembers, and preview text shows atmost4members. Shared view/timeline::read_tool_group_end(messages,start) is public canonicaleligibility/sourceboundary used by BOTH read_tool_group_at and pane motion sampling. Motion samplesactualfullsourcegroup, arrivalfirstmemberonly, completionnewestactive transition among ALLmembers including hidden5th+. Never encode membership into generatedIDs or construct syntheticmessages each animationframe. has_active_motion fastpath returns sharp before source scans for settled/historygroups. Child wrapper group::child::child_id resolves rawchildID.

Reusable widgets/diff_card.rs render() remains neutral-compatible; optional render_with_paint(PaintParams {opacity,clip,occlusion_rects}) fades/clips chrome, syntax, diagnostics; Gitpanel callers unchanged.

## Configuration and verification

Reuses agent.streaming-text-animation truebydefault. SettingsAgent label now Streaming reveal animations; false disables text/cloud and toolmotion/expansion but leaves existingrunning indicators. No automatic OS reduced-motion handling added.

Final verificationpassed:cargo check -p neoism -p neoism-ui, wasm32 webtarget check -p neoism-terminal-wasm --features web, focusedshared motion16tests, toolrenderer20tests, diffcard9tests, reveal49tests, desktopmotion4tests, configtoggle1test; gitdiff--check clean. Filters mayoverlap, don't sum asunique testcount. No releasebuild, runtimevisualtuning, or GPU frame-time benchmark performed.
