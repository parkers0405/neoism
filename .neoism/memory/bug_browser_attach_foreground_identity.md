---
name: "Browser attachment falsely reported lost window"
description: "browser_attach reused foreground-only active-window guard, falsely rejected background Zen. Only attachment permits background; observe/action keep focus checks; error now truthful."
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-17"
updated: "2026-09-17"
---

User's new browser_attach failed twice with Target window disappeared or identity changed though Zen nativewindow/PID remainedsame. Live debugrecords target290ad046f84290d4a0468a88081a7c4d; native0x6321c6cd74e0 pid65259 bounds10,40 1516x910 stableId18000009 procstart111244 identicalacrosssnapshots. ROOT browser.attach scopedCheck inherited foreground=true, Linuxvalidation used only j/activewindow; focusreturnedNeoism so searchonlyactivewindow falselyreportedmissingZen. FIX Check hasrequire_foreground bool (defaulttrue); ONLY browser_attach scopedcheck usesfalse. Browserobserve/action retainrequire_foregroundTRUE + DOMvisible/hasFocus checks. Parent specifically revertedchildoverbroadchange that removedforegroundfromobserve/action. windows.validate_list nowchecksgeometryalsoforfalse mode, retainsPIDbirth/stableID/nonce/revocation. validate_active_snapshot firstchecknativeaddressmatching selected: anotheractivewindow nowerrors 'Target is not foreground; call computer.focus with this target...' instead offalsedisappeared; noinspectunrelatedidentity/revoke. Genuine selectedaddresslifetime/boundsreuse stillreject. Attachsuccess/alreadyattached includesnext instructions model shouldcall computer.focus itself beforeobserve/browser_step if needed; connectiondoesn'tchangefocus. Regression tests backgroundlist->resolve->validate stable/resize/reuse/closed andanotheractivewindow focuserror.150 computer tests pass5ignored, cargo checkserver--tests pass. Noactualinput/focus/browserprofilechangesusedforverification. Sourcechangesrequireusernormaldebugrebuild; no release build.
