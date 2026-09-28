---
name: "ACP reconcile panic and composer dedup"
description: "Embedded ACP panic, Claude options and sidebar duplicates"
type: "bug"
scope: "project"
origin: "user screenshots and panic report"
created: "2026-09-24"
updated: "2026-09-24"
---

2026-09-24 user-reported native GUI issues. Embedded daemon panicked repeatedly at external_agent/events.rs:780 (`child.extra["externalAgent"]`) when restart reconciliation iterated ordinary Neoism sessions missing that BTreeMap key; fix = `get` and continue. Regression `nested_reconciliation_skips_ordinary_sessions_on_restart`; 29 ACP tests pass. Native Alt+C used to explicitly hide FileTree+Notes and restore snapshot; shared Chrome used exclusive takeover. Frontend now independently toggles Files, Notes, Conversations and lays them side-by-side (native fixed widths on extremely narrow windows still limitation); shared Chrome updated focus/hits/layout.

Claude composer screenshot only provider label and empty `/` picker: pinned @agentclientprotocol/claude-agent-acp@0.81.1 emits configOptions mode/model/effort (effort when supported) and available_commands_update; its commands without args contain `input:null`. Backend validate_commands treated any present input as object with hint, rejecting whole update and possibly failing GET external/options before controls loaded. Fix accept null, strict non-null hint validation; unit + full fake HTTP ACP test command `input:null` pass. Frontend shows loading while root/options initialize, slash picker opens during pending and fills once snapshot arrives; failed GET/root creation has retry action, draft retained, refresh on binding.

Duplicate native+preview row root cause: auto Neoism ACP roots lacked catalog `sourceHost`/`sourceKey`, even after externalSessionId bound; catalog requires verified host and same opaque ID, so provider preview not associated. Shared catalog::source_key_for hashes host+tenant+provider+canonical cwd+external id; update_external_session_metadata stamps root on trusted binding (never children/imported foreign roots). Frontend refreshes catalog immediately on options GET success instead of 45s poll, prunes preview by identity not title. Tests `newly_bound_root_matches_its_provider_catalog_identity`, negative IDs/tenant/provider, user still needs live app rebuild/restart for validation. Existing old roots rebind on GET load and gain source key. No release build, cargo check -p neoism -p neoism-ui -p neoism-terminal-wasm passes; no live signed-in provider GUI run yet.
