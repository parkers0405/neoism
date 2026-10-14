---
name: "Style-only Lua overrides discarded - fixed"
description: "Style-only init.lua rejected because empty configpatch roundtripped ui.margin; no-op fix unblocks personal styles, requires debug rebuild/relaunch"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-08"
updated: "2026-10-08"
---

User sidebar opacity test .25 looked unchanged; parent set ~/.config/neoism/init.lua sidebar_opacity0 and explicittransparent, both leftConversations and rightAgentdetail stillfullyopaque in two livescreenshots of focused debugPID231424. Executable target/debug/neoism, HOME/XDG_CONFIG_HOME normal; recursiveconfigwatch exists, LuaRuntime correctlyloads4selectors. Reproduced ROOTCAUSE Config::apply_json_patch({}) needless typedserialize/merge/deserialize fails ui.margin invalidtype map expectedsequence; style-only Lua emits{} so startup discardsRuntime, hotreload keeps oldstyles. FIX config/mod.rs returnOk onemptyobject ORnull beforeanytypedroundtrip; acceptsstyleonlywithout alteringconfig, rejectsarrayroot, regression testConfigMarginnondefaults unchanged. Native cargo check -p neoism-backend -p neoism --tests passed, testscompilednotexecuted. Existingdebugbinarymtime Oct8 11:28:05CDT vsfixedsource11:50:44CDT => requiresusernormaldebugrebuild+relaunch; doNOTclaim live0alpha applied beforeRustfix loaded. Didnotrestart/rebuild/killwindow userbusyotherAgent. SeparateNONEMPTY Lua configpatch typedroundtrip issue (ui.margin serialization shape) remains open, fix specificallyno-op{} path semanticallycorrect. Sourceopacityhelpers/frame/snapshotwiring alreadyinrunningbinary. Fullytransparent overridepersists andwillpublishcorrectlyonfixedbuild.
