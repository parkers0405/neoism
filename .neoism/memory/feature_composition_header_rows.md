---
name: "Tool Composition header and ordinary child rows"
description: "Clickable Tool Composition header reveals actual JS; ordinary child rows independently expand; shared/desktop/server tests pass"
type: "feature"
scope: "project"
origin: "User-requested timeline redesign and final verification"
created: "2026-10-13"
updated: "2026-10-13"
---

User-requested timeline redesign complete: compact clickable Tool Composition header opens actual executed JavaScript first; ordinary native/MCP child rows beneath remain visible whether parentexpanded/collapsed, eachindependentlyopensdetails. UsesexistingnativeGrep/read/edit/MCPmapper+styles/status/diffcards; composed editscollapseduntilclicked. ChildrenareUI-only typedcomposition_children Vec<sharedNeoismAgentMessage> andcomposition_child marker, neverproviderParts/messages. StableIDs parent/compose/call-id, no readbatchprovenance. Shared/desktopchildowner mapsarchive/anchor/dirty/anim toparent sourceindex; actualnestedlookup supportschilddetails. Childcontent includedvirtual/cacheidentity; ingestion recordschildstatus/revealupdates; desktopmacro/constructors/conversion preservescanonicalsharedchildren. Full boundedactualJS source retained/syntaxhighlighted (composition.code preferred withpersistedinputfallback).

Server tool_composition.rs publishes/stores composition.code actualposthookscript, notduplicatingitinto modelresultreplay. Auditpresentation {tool,title,output,metadata,isError,truncated} captured beforeprotocolerror/hydrationfailure. Displaycaps title1KiB,output16KiB,metadata16KiB,aggregate512KiB; onlydisplayallowlist mcp client/tool/runtimeId,artifact/outputPath/files/diff/diffs/media/etc; NOrawmcp.result. Missing/oversizeddetails flagged detailsTruncated; legacyrecords showhonest input/errorfallback, neverparentselectedreturn. partial_unknown mergesstatus/errorpreservingpresentation. Authoritativeeffects/undo/media ledgerunchanged andseparate.

Firecrawl liveprobe: onlyMainContent body omittedheading butmetadata.title remained Example Domain. No interpreterfix; usertestfilter shoulduse result.data.metadata.title; markdownforbodymatches. Productguide updatedactualfields andheader/child interaction.

Verification PASS:17 servercompositiontests;9 sharedcompositionUItests;31 toolmessage,7readgroup,1groupingregressions;3 desktopcompositionregressions;4 docs tests;normal cargo check --locked -p neoism;git diff --check andnewbackendmodulerustfmtcheck. Initialdesktoptests incorrectlychecked TimelineDirtyMarks.ids; desktopinvalidationuses indices, corrected exactowner indices.contains(&0), fixtureactualcompose+childmarkers; no production dirtylogicfix required. Existingunrelated warningspreserved. No screenshot/pixelvalidation, no releasebuilds. Bothimplementationagentsfinished/stopped;do not continuepolishing aftergreen. Workspacenightlyversion changedconcurrentlyto0.7.126-nightly.20261013.1;preserveotherfastservers/IPC changes.
