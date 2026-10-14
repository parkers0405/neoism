---
name: "Published nightly 0.7.119, no Servo runtime"
description: "Real nightly v0.7.119 published with all platform assets; Servo explicitly excluded and CI-guarded; stable remains v0.7.118"
type: "project"
scope: "project"
origin: "neoism-agent"
created: "2026-10-07"
updated: "2026-10-07"
---

Published real nightly v0.7.119-nightly.20261007.1 on 2026-10-07. Commit57f59e098e7e6b5bd601c042737073becc5c8400 pushed atomically to origin/nightly plus annotated tag. Local release branch release/nightly-0.7.119-20261007-1. Source snapshot commit94807cb7c included155 intentional source files across performance, reveal, usage picker, artifacts, effects, then merged origin/main e7da63ac8 to retain v0.7.118 Windows Lua panic=unwind fix and smoke test. Local memory and tmp-ui-check.txt deliberately left uncommitted. No main push.
Release Neoism run37575179373 SUCCESS: https://github.com/parkers0405/neoism/actions/runs/37575179373. Published release https://github.com/parkers0405/neoism/releases/tag/v0.7.119-nightly.20261007.1 verified isDraft=false,isPrerelease=true with8 assets (Linux x64 tar+sha, macOS arm64 tar+sha and DMG+sha, Windows x64 MSI+sha). Stable Latest remained v0.7.118. Linux finished~45min, Windows~85min with MSI terminal/install tests, macOS~140min; initial monitor2h timeout did NOT stop GitHub job, resumed watching successfully.
USER EXPLICITLY REQUIRES Servo runtime NOT BUILT FOR THIS RELEASE. Source retained, excluded workspace standalone servo-runtime0.1.0; desktop servo-artifacts opt-in disabled, production builds only wgpu. Added release-neoism.yml step cargo tree --locked --target matrix.target --edges normal,build --prefix none -p neoism -p neoism-workspace-daemon -p neoism-agent -p neoism-agent-lua-runner --features wgpu, fails if ^(neoism-servo-runtime|servo|libservo) v. Guard PASSED ALL3 platforms. Docker builds only daemon/agent/runner, not Servo. Do not enable Servo in future releases without user request.
Real publication requires tag push v*.*.*, not branch workflow_dispatch (dispatch is dry-run). Nightly tag contains -nightly.; workflow publishes prerelease latest=false after entire matrix. Scripts/release.sh unsafe as-is (hardcodes main etc). Root Cargo workspace version and31 internal pins set nightly, cargo update --workspace --offline updates34 lock entries; CHANGELOG exact nightly section and AppStream development entry added. cargo check --locked -p neoism --features wgpu passed. Never relabel this nightly stable/latest.
