---
name: "Chat-open UI locks and constructor I/O"
description: "UI pre-switch blockers fixed: hidden-terminal mutex wait + 242–322ms config reread; build/39tests pass, updated GUI latency unmeasured"
type: "perf"
scope: "project"
origin: "neoism-agent"
created: "2026-10-23"
updated: "2026-10-23"
---

# Native chat opening UI-side blockers, follow-up 2026-10-23

User still saw1-3s after backend work. Do NOT call overall latency solved. Current increment debug build passed,39targetedtests passed/1ignored, actual updated GUI paint NOT measured. At start userdebugPID335652/embedded335669 agent32975 was running onHyprland3; screenshot showed multiple savedchat tabs; that process exited while inspecting it, leaving originalPID3950 only. Did not launch any replacements (user explicitly forbid duplicatewindows). Earlier instruction requestedpermission forrestart; still no affirmative permission.

## Proven UI-side missing work

Read-onlyExplore ses_e954d5091ffembUV8Sf5eZqblo found open_neoism_agent_tab -> contextmanager.add_stacked_neoism_agent -> grid.add_stacked_context -> apply_taffy_layout before switch_session. Layout iterates ALLcontexts inclhidden and took blocking terminal.lock; reapply_chrome_layout also laysoutallworkspaces repeatedly. Changed layout/grid/layout.rs to apply_layout_dimensions helper using try_lock_unfair, always commitslatestdimension, sets pending_terminal_resize+dirty onbusy, clears onlyafteracceptedresize, sendsPTyresizeonlyvisible afterlocalacceptance. Existing paint retryuseslatestctx.dimension inclchromeinsets. screen/chrome_geom.rs apply_context_resize generalized overEventListener for actualretrytest. Subagentimplementation ses_e9543d27affeGUXqW27cXSJzwL. Heldrealmutexregressionbusy_layout_resize_is_nonblocking_and_paint_retry_sends_latest_winsize passes; checks hidden&visible contention, latestgeometry,pendingpaintretry,exactPTYnotifications. DidNOTskipchrome reapply: existing signature incomplete forpanelwidths.

Pane with_directory synchronouslyloadedglobalprompt_history and Config::load solelydefault_chat_source. These steps excludedbycfg(test), sooldDefaultbenchmarkmissedthem. Measured actual configread242ms then322ms; history4.8-4.9ms/1000entries. Removed configread: Screen.default_chat_source storesalreadyloadedsetting initializedscreen/mod.rs,updatedchrome_geom::update_config,appliedafterserverreset in screen/daemon_layout.rs window/current setters and catalogpaneafterdashboardreconcile in screen/panes/grid_workspace.rs. pane/session.rs set_default_chat_source avoidsresetwhensame value, preservingexplicitdraft/savedsource.

with_directory nowloads prompt_history onworker and sends PromptHistoryLoaded(Vec<String>). pane/ingest.rs prependsloadedhistorywhilepreservingprompts sentduringload, maximal suffix-prefixoverlapdedupe,1000cap, adjusts existingrecallindex; doesn'treplaceinput/historydraft. Two tests coverlocalprompt/recallpreservationandcap. Constructorstillapply_config_defaults queuesworker normally; preservesanimation/admission.

Fixedconcurrentcompilemismatchminimally api.rs agent_server_credential visibility pub(super),becauseexistingpane/connect.rs newcallerrequiredaccess. Nootherconnectchanges.

## Verification and limitations

Final cargo build -p neoism --bin neoism passed (/tmp/neoism-chat-ui-stall-build.log). cargo test -p neoism --bin neoism -- conversation_open busy_layout_resize defaults_tests responsiveness_tests readiness_tests config_chip_ prompt_history reload_preserves_explicit --test-threads=1 underNEOISM_SERVER=invalid://agent-unit-tests:39passed0failed1ignored (/tmp/neoism-chat-ui-stall-tests.log). gitdiff--checkpassed. Previewfailuretest initiallyfailedoneframeassertionunderconcurrentbuildload,passedinisolation; changedonlytesttobounded2sdrainloopwhilestillpreview,matchingbudgetednextframeingestion,noassertionsremoved.

Ignoredconversation_open_live_backend_timings updatedtoinvokewith_directoryandcachedsource; separatelytimessynchronoushistory/configoutsideclickloopdiagnostic. Initialattemptusedcloseddebug'ssessiononproduction4096 andtimedout(notsameDB). Retriedconfirmedproductionactive session ses_e9551ffbbffeUTDecpuNPUWQXC:construct964/400/262us,activation1555/767/779us,firstANYmessages339393/944343/779us,blocks5/2/96. This wasactiveconversation liveevents and originalproductionbackend, NOTupdatedbackendnorfirsttranscriptbaseline/paint; doNOTclaimthoseasend-to-endinstantfigures. Historybenchmarkpassedbutnotstablelatencyfixture. /tmp/neoism-chat-constructor-bench.log.

Remainingunmeasured: actualfirstGPUframeonupdateddebug. Remainingcode-suspectsnotchanged: repeatednormalize_workspace_rootcanonicalization in workspace_root_for_new_shell bypassesjoinedopaquepathguard; possiblecoldtextlayout/shaping; repeatedchromegeometrypasses. Parenttolduser5-10minutesforbuildchecks; theycompleted. NoelapsedGUIguarantee. Do not launchreplacementwindowwithoutpermission; useuserprovideddebugifitreturns.
