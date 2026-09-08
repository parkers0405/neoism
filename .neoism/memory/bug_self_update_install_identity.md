---
name: "Self-update loops: installation and relaunch identity"
description: "Windows/macOS updater targets, version/hash verification, rollback, receipts, pinned modal and actual worker CI tests; native acceptance limits recorded"
type: "bug"
scope: "project"
origin: "Updaterfixes preparing0.7.97 afteruserauthorizedallGitYOLO"
created: "2026-09-08"
updated: "2026-09-08"
---

User reported Windows/macOS update then relaunch repeatedly offered same release. Read-only audit found Windows MSI installed managed LocalAppDataProgramsNeoism but relaunched original current_exe portable/PATHcopy; Mac updating looseCLI selected a standard.app and returned beforeupdating actualCLI. Helperlaunch was mislabeledsuccess, errors afterhandoff hidden.

Implemented fornext releaseafter0.7.96: Windows windows_update.rs/ps1 resolves64-bitHKCU managedtarget vs invokingportablepath; portableMSIpayloadextraction updatesactualstackinplace. Payload andinstalledall3versions+hashes verified, explicitverifiedrelaunchpath, scopedquiescence, update lock/cancel/rollback, durable receipts %LOCALAPPDATA%/Neoism/updates. 3010/1641 =reboot_required noimmediaterelaunch. ExactcandidateProductCodealreadyinstalled usesREINSTALL=ALL REINSTALLMODE=vamus; initial/majorupgradeskeepnormalflags (REINSTALLMODEalone doesnotrepairfeatures). Nativefixture nowtampersmanaged ANDportablebinaries/web, requirescandidatehashrestoration andunrelatedfile/registry preservation; normalreleaseMSIsmoke invokesrealhelperbeforeGUIcheck. WineConPTY mockedhelper/parser suitepassed; actualMSIrepair/locks runinWindowsCI, notlocally.

Mac macos_update.rs identifiesactualcanonicalinvokingbundle/looseCLI; noApplicationsfallback towronginstallation. Renamedbundles/symlinklaunchers supported; mounted/translocated/foreignbundles failbeforewriting. Wholebundle andloose3bins/web staged+hash/versionvalidated withlock/journal/rollback andretainedbackup. Boundednativeversionprobes inclall3; helperdetachedfromterminal; explicitopenstatus checked withoutclaimingGUIhealth. Oldtargets/backupsnotrequired tosupport--version. Deployed5-argument --macos-update-helper ABI preserved becauseoldclients invokeNEWdownloadedhelper. macOS receipts ~/Library/Application Support/Neoism/updates include target_executable/status/expected_version/log/backup. 21LinuxFS/transaction testsandisolatedDarwinaarch64modulecheckpassed; nativeMac signing/replacement/relaunchuntested.

Parentcommon: modalbuttonpinsdisplayedversion with --target-version; boundedhiddencurl commonlatestresolver; semanticnodowngrade/no-op unless--force; privateunpredictablestaging dirs; modalshowsinstallationpath. handoff95% meansstagednotinstalled andisdeferreduntilupdaterchildexit0; directUnixready100 remainsimmediatebecauseupdaterwaitsGUIexit (doNOTdeadlockbydeferringallreadyrecords). Latechildfailure nolongersuppressedbyearlierhandoff. Noopcompletesmodalwithoutrestart. Failed/rebootrequired receiptsaresurfacedonceonnextlaunch, scopedcanonicalactualinstallation andfingerprinted, retainfulllogs; nofalse 'Neoism wasnotchanged' blanketmessage. Addedstandalonedaemon --version (Clap version; previouslyunsupported!) withnostartuptest. Releaseworkflowgates tag=Cargoversion andall3packaged --version outputs beforepublication; lintpasses.

31combinedupdater/Macfixturetests+daemonversiontest passed; Linuxdesktop/daemon/shared --tests check, Windowsproductioncrosscheck, wasmcheckpassed. NO liveupdates/install/restart duringdevelopment. UserlaterauthorizedYOLOreleaseALLgitafterbothagentsfinish. Releasepreflightnextv0.7.97. Alreadyshippedbrokenupdatercannotberetroactivelyfixed; one-timemanualupgrade maybe neededforoldwrong-path copies. Independentinstalls remainindependent: updateinvokedsamecopy, noteverydifferentapp/PATHcopy.
