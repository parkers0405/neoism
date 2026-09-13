---
name: "Computer-use low-latency execution path"
description: "Latency pass done: stable handles, cheaper per-effect validation, no redundant poll, immediate capture default, faster lossless PNG, per-call timings;133 non-live tests pass"
type: "perf"
scope: "project"
origin: "User requested nearinstantcomputeruse; non-live latency audit, implementation and regression checks"
created: "2026-09-13"
updated: "2026-09-13"
---

User demandednearinstantcomputeruse afterXattempttypeURLcompletedbutimmediatescreenshotoldUI; latercapturecorrect. UserSTOP remainsactive: ALLlatencywork/testsNON-LIVE, no desktop input/capture resumed. Implementedworkingtree afterunifiedtyping (notcommitted/deployed/rebuiltGUI).

windows.rs stableopaquehandles acrossunchangedlist, NO30secwindowhandleTTL/listchurn. Cachecap1024. Nativeidentity/geometrychange,observedabsence,STOP,eviction revoke; nonceblocksgeometryrevertoldframealias. Internalqualifiedid=native|lifetime#snapshot, publicserializednativeidshapeunchanged; OSAPIsmustusenative_id(). Hyprlandidentitycompositorinstance+processstart+stableIdwhenavailable;Win/Macprocessstart. Same-processunobservedIDreusewithoutbirthidentifierremainslimit. validate(target,true)LinuxONEfresh j/activewindowIPC insteadclients+activewindow(2). No timecachedfocus. Deterministic100checks=100requests. Framesstill30sec.

Keyboard: completion-awarepump skipsblockingpoll ifpendingdispatchalreadycompletedsync. Checks{wait,full};cheapwait onlycancel/drop/STOP/deadline; fullauthoritativeimmediatelybeforedevice/map/mod/keyeffects. NativecoreCheck.fast separatesoldactivepredicate fromwindowprobe; per-effectchecks andfreshmap/groupchecks retained. KeyboardSession.execute_with_checks;typing.execute_text_with_wait bridgesfacade; compatwrappersremain safe/slower. Coreusescheapopen/preflight/revalidationwait,fullatpublication/injection. Private-socket64syncs=64callbacks ZEROextra blockingpolls (~0.7–1.2ms synthetic), notliveinputbenchmark.

Screenshots: defaultsettle_ms0(schemaandruntime), exactly1capture/no64x64sample/no settling sleep. Positivebudgetkeepsopt-instability algorithm,cheapcheckswhilewaiting/fullbefore-aftercapture. Cleanup orderingwasALREADYcorrect, nowsharedafter_input_capture regression verifiesinputfinalizationbeforecapture, includingfailedoutcome. ImmediateimageNOTappreadiness. Keyboard-onlyinput/batchno longerenumeratescaptureoutputs. Newcapture.rs scopedWayshotconnectionrefreshesfulltopology/mode/rotation/logicalgeometrybefore+aftereachcapture, reusedwithinoneoperationnotglobal;capture hasno extrafinalnewWayshotprobeLinux. CompiledNOTliveverifiedthispass.

PNG: newcapture::encode no upscale smallimages, thumbnailonlyifedge>1600,Fast+Sublossless,8MiBboundedwriter retriesDefault+Adaptiveonceifneeded,otherwiseerror.64Mp nativebounds,RGBA/alpha/16bitpixelidentity tested,floatcolorsrejectinsteadsilentconvert. BenchsyntheticCPU optimizedprofile5iter AMD RyzenAI9HX370:1280x720textlike totalresizePNGbase6445.02ms->3.60ms(old upscaled1600x900,newretains1280x720),random75.70->11.11;1920x1200text47.61->30.02,random81.52->45.85. LargeroutputnewtextPNGslightlylarger1.73MBvs1.46MB;doNOTclaimthesearelive/end-to-end/debuguserprofilelatencies.

Newstd-onlylatency.rs TLSscope instrumentation,record/count/measure staticlabels no payloads. Coreauthorizedspawnworkermeasuresqueue+worker;successful/input-errorJSONfirstcontentgets timings withstages/counters (window_ipc,keyboard_sync/poll/edges/observers/cleanup,planning,capture_setup/capture/resize/png/base64). Excludesmodel/permissionwait andresponsetransport;stagesoverlap;notappconsumption. Timeoutunfinishedworker can'tsupplycompletedscopedstats. No text/titles/imagecontents inmetriclabels. Updatedstandaloneharnessesforlatencymodule;browserfixtureusesfastfacadevariantforfuturetests butNOTRUNlive now.

Verification finalcargo check --locked -pneoism -pneoism-agent-server --tests success(warningsremaining). computer_use suite137tests =>133passed/4ignored (3live +syntheticbench). Native/parserharnesspassed. Newtests immediate0 no samples/waits,keyboardactionsno topologyneeded,timingmetadata retainsunverifiedstatus,inputfinalizationbeforecapture,stabletargets/pidreuse/geometry/stop/cache,readycallback0poll,cancelwaitonlycheap,focuslossbeforeeffect. Syntheticencodingtests4passed+explicitCPUbenchmarkdone,noGUI. WindowcrosschecksblockedcrossCtoolchainsringbeforemodule,no fullWindows/Macclaim. docscomputer-use.md updatedstablehandles/defaultfast/timings/CPUbench.

Do notstartlivebenchmark orresumeXwithoutnewuserauthorization. Neednormaluserrebuild/relaunchforcode. Do notclaiminstant/worldfastest oractualGUIlatencymeasured; nextauthorizedcalltimingscanseparatenativefromLLM/appdelay. Userimpatient: finalshortdone/changes/tests, noadditionalfeaturecreep.
