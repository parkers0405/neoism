---
name: "ArtifactSearch unknown cache path — FIXED"
description: "Artifact filesystem paths failed lookup because Bash/provider spills lacked artifact registration; fixed safe path resolution, central registration, and URI replay"
type: "bug"
scope: "project"
origin: "session diagnosis and implementation"
created: "2026-08-20"
updated: "2026-08-01"
---

## Symptom
Agent timeline shows repeated `ArtifactSearch error: unknown artifact /home/.../.cache/neoism/tool-output/tool-evt_....txt` cards.

## Root cause
The truncation preview exposed a filesystem path, while `artifact_search` resolved only transcript-registered artifact IDs/URIs. Bash independently spilled stdout/stderr and stored only `outputPath`; central truncation skipped any metadata containing that key, including null, so no artifact was registered. Each repeated card was a real model retry, not frontend duplication.

## Fix
- `artifact.rs`: resolve registered artifacts by exact metadata path, legacy session `outputPath`, and canonical files only inside Neoism-managed tool-output directories (covers failed legacy calls).
- `bash.rs`: successful output is combined and left to central truncation; oversized failed output still spills for managed-path lookup.
- `tool_runtime.rs`: non-empty pre-spilled `outputPath` is enriched with artifact metadata instead of blindly skipped.
- `provider_stream_processor.rs`: provider-hosted spills register artifacts.
- `message_model.rs`: retained previews append the `artifact://` URI.
- Artifact tool schemas require their mandatory fields.

## Verification
`cargo check -p neoism-agent-server`; targeted artifact, truncation, and prompt replay tests pass. Workspace-wide fmt check is noisy from unrelated pre-existing formatting drift; changed lines were formatted and `git diff --check` passes.
