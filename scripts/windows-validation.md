# Windows validation on the GPUIX branch

The current desktop bundle and installer target Linux x86_64. The old desktop
and MSI release pipelines have been removed. This branch does not publish or
claim validation of a Windows GPUIX desktop release.

[Tool Composition](../.github/workflows/tool-composition.yml) retains Linux,
macOS, and Windows checks for the Rust tool runtime. Its shared
[Blaze setup](../.github/actions/setup-blaze/action.yml) pins the compiler and
nightly. Windows bootstraps the compiler from that exact source revision because
upstream has no Windows release archive. Application code is checked and tested
with Blaze, using its documented host proc-macro mode.

The runtime checks are:

```sh
bash scripts/blaze.sh check --locked -p neoism-agent-composition --all-targets
bash scripts/blaze.sh test --locked -p neoism-agent-composition
```

Native Windows PTY and process helpers remain in their service crates. Their
platform-specific tests do not establish GPUIX window, input, installer, or
rendering acceptance. See the [frontend verification gates](../neoism-frontend/gpuix/parity.json)
for the current desktop scope.

The previous MSI acceptance and runner-loss investigation notes are retained as
[historical evidence](../docs/archive/windows-release-validation.md).
