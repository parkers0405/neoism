---
name: "Linux and Docker computer-use linker dependencies"
description: "v0.7.99 Linux missing GBM; Docker GBM+xkbcommon due Agent computer-use deps; build/runtime fixes and SDK0.7.100"
type: "bug"
scope: "project"
origin: "neoism-agent"
created: "2026-09-14"
updated: "2026-09-14"
---

Original v0.7.99 SHA3336ac8035d5c0269240d331be3922d73af2265d failed Release Neoism run34785248226 Linux only: rust-lld unable to find library -lgbm. Docker Publish daemon image run34785248216 failed rust-lld unable to find -lgbm and -lxkbcommon. cargo tree shows libwayshot -> gbm -> gbm-sys and enigo -> xkbcommon in Agent server, also linked by headless daemon. Fix release-neoism.yml + build-stack.yml apt libgbm-dev; Docker builder libgbm-dev libxkbcommon-dev, runtime libgbm1 libxkbcommon0, run both binaries --version in runtime image to verify shared loader before publish. No cargo build --release locally. SDK identity is own eight @neoism packages; checked-in0.7.55 vs npm latest0.7.76; align package versions/internal pins+lock to server0.7.100, not random provider major bumps. Updated identity OpenAPI hash/TS contract verified byte-for-byte generator. GUI full673 tests/68files+assets, SDK tests/typecheck/build and Rust three release binary cargo check wgpu pass. User authorizes all outstanding main source/memory changes (GUI polish, identity, server registry migration, local launch, workspace discovery); generated Python __pycache__ ignored, not published. Release flow mandatory neoism-yolo-release; next unused v0.7.100, never retag failed0.7.99. Exact-SHA validation reuse only; new tag without validation builds full matrix.
