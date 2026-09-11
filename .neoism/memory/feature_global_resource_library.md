---
name: "Skills and workflows default GLOBAL — implemented"
description: "Latest correction: Library global-only; explicit installation API scope, configured roots, real global workflow writes; no project gate or CWD fallback"
type: "feature"
scope: "project"
origin: "user correction + implementation"
created: "2026-09-10"
updated: "2026-09-10"
---

# Global Skills/Workflows Library — latest user correction

User explicitly superseded prior project-scoped Library interpretation: Skills and Workflows MUST default to global Agent definitions. No project picker, Choose Project gate, selected-directory storage, or silently treating an omitted directory as global. Global-only Library is intentional; explicit project helper/API consumers remain compatible.

Implemented in GUI Library.tsx, resourceScope.ts, management.ts, resource editor forms and workflow catalog helpers. Library ignores its optional directory prop; switching a project does not remount or redirect a global draft. Labels Global skills / Global workflows. Skills create body scope=installation; get/delete/version/restore use installation scope. Workflow requests (CRUD, activate/pause/history/preview/run) explicitly scope=installation. Execution-directory field is independent of definition destination. Agents/skills/configured-model catalogs and account selection use installation context by default. Rich forms and complete skill bundles retained.

Actual roots verified from source, not guessed: StandardConfigSourceService::from_environment uses XDG_CONFIG_HOME/agent or HOME/.config/agent, configured agent.json. NeoismConfigSourceService uses parent of neoism_extensions::agent_config::agent_config_path (NEOISM_AGENT_CONFIG_DIR override, otherwise XDG_CONFIG_HOME/neoism or HOME/.config/neoism on Unix; native Windows config path). Added explicit ConfigDiscoveryScope and ConfigSnapshotRequest::installation so global snapshots omit project layers/roots. Do not reintroduce path.starts_with(workspace) as scope classification.

Backend workflows previously only wrote workspace/.agent/workflows. Added WorkflowScope { Installation, Workspace }, default Workspace for API compatibility. Installation definitions really write <configured-installation-root>/workflows/<id>. Internal persisted context agent-installation:<configured-root> is a runtime identity, NEVER an execution directory; canonical_location and config snapshot recognize it. Root-qualified context shares CRUD, activation/history and filesystem watches. Public callers select scope=installation; direct marker directory requests are rejected. No empty-directory/CWD inference. Workspace omission and explicit workspace continue old behavior.

Skills scoped reads load installation-only configuration. New skill version IDs are bound to canonical storage-root hash. Explicit global history cannot guess provenance of legacy unbound versions; current complete disk bundles remain authoritative. Legacy id-only workspace version reads preserved.

Management opt-in and operator/hosted/root boundaries retained. Installation request authorization checks configured real global root rather than optional project query directory. New capability neoism.resources.installation prevents old backend responses being mislabeled global; GUI asks for backend rebuild rather than falling back to project files. Requires rebuilding the Agent backend (server/service-api/native adapter, or containing binary); no restart/release build/browser was performed.

Regression files: management_global_tests.rs and workflow_global_tests.rs verify temp configured roots, full bundle versions/restore, actual workflow paths, global lifecycle, watch events, workspace compatibility, real HTTP operator/root/hosted/disabled-management boundaries. Native adapter config test verifies global-only layers/root. GUI full suite passed 563 tests; SDK consumer/contract tests and typechecks pass. App.tsx not modified; Library owns ignoring project prop.
