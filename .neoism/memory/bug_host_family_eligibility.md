---
name: "Host family eligibility, transition revocation, and guest privacy — source fixed"
description: "Confirmed startup 409 fixed; authoritative whole-family revocation handles persisted overlays and rehosting transitions, child/artifact auth hardened; 6 hosting + 9 tenant + 1 artifact tests and daemon check pass"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-10-14"
updated: "2026-10-14"
---

# Host immediate pre-readiness 409 and family transition privacy — SOURCE FIXED

Installed 0.7.126-nightly.20261013.2 child stderr confirmed headlessly: POST /v2/hosting/associate returned 409 "local conversation family contains an owned or out-of-root child; hosting association not changed". Read-only Agent metadata showed a local project-root conversation with historical children rooted at /home/parkersettle outside /home/parkersettle/projects/neoism. Strict combined ownership/containment validation blocked every Host association for that directory, before daemon readiness. This is independent of the earlier provider certificate wait.

Final source: validate namespace/foreign ownership, then leave path-ineligible families entirely local/private instead of aborting unrelated eligible families. Preserve canonical-directory namespace reuse and atomic ownership-conflict refusal. SECURITY REVIEW found merely skipping unsafe families insufficient after prior hosting or persisted workspace overlays. Added authoritative hosted_chat_revocations(session_id, workspace_id): successful rehosting deletes prior grants and records whole-family revocations atomically. Reconcile persisted same-namespace roots as well as raw local roots. Hydration neutralizes only local/same-revoked-namespace ownership to effective local; never rewrite info_json or override explicit foreign reassignment. Re-eligible families can be granted again atomically.

Scoped session reads recognize authoritative associations/revocations and verify effective tenant. Artifact listing excludes revoked-family artifacts even if their raw storage tenant is the former hosted namespace; trusted local operators retain access via matching revocation provenance. authorize_artifact now checks effective linked-session authorization even when storage tenant matches caller (the transition test caught that additional bypass). Cross-namespace local restoration requires DB revocation proof, not imported JSON markers. v2_session_children authorizes every child by caller tenant/directory, independently of parent authorization.

Changed backend source: neoism-agent-server/src/state/hosting.rs, state.rs, artifact_routes.rs, v2_routes.rs; tests_hosting.rs. No session_routes changes needed. No live DB/history/credential edits, auth bypasses, capability widening, GUI use, timeout increases, or release builds.

Final executed verification (all exit 0): cargo test -p neoism-agent-server hosting_ --lib: 6 passed; cargo test -p neoism-agent-server tenant_ --lib: 9 passed; cargo test -p neoism-agent-server v2_artifacts_round_trip_binary_content --lib: 1 passed; cargo check -p neoism-agent-server -p neoism-workspace-daemon passes. New hosting_revokes_existing_and_persisted_family_grants_after_local_continuation uses signed real Axum router requests, actual local POST /v2/sessions outside root inheriting parent namespace, a persisted root overlay, repeat rehosts, and then a fresh AppState/router on the reopened store plus another new daemon ID. Guest root/history/children/individual artifacts denied; general artifact listing retains only another eligible family's artifact; each child filtered before rehost. Trusted local operator reads original IDs/history/children and both original-local and namespace-stored artifacts. Raw history JSON byte-identical before/after, including after reopen. Existing guest-association refusal and foreign-ownership tests remain passing. Running installed Agent remains old until user-controlled rebuild/relaunch; source tests alone do not verify live Host success.
