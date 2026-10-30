# GPUIX nightly platform builds

`Release Neoism GPUIX` runs Linux x86_64, macOS ARM64, and Windows x86_64
as independent jobs in one workflow run. Each job builds the GPUIX frontend,
native engines, workspace daemon, Agent service, and standalone Agent GUI from
its checkout, runs checks, and uploads its archive and SHA-256 checksum directly
to the same release. The first successful platform publishes the draft release;
other successful platforms attach their downloads as they finish. A platform
failure does not cancel or hold back the others.

`neoism update --nightly` (also `neoism --update nightly`) uses the published
nightly for the current platform. Windows updates through MSI. Runtime libraries
and the standalone updater live in `web/native`, a resource tree copied by the
already-released installers. The retired `neoism-agent-lua-runner` filename is
retained as an update compatibility executable; it reports that the Lua runtime
is retired if invoked with a Lua manifest. macOS tarballs retain both the loose
stack and signed `.app` layout expected by existing updaters.

Nightly tags are prereleases and never replace the stable `Latest` release.
Each archive and its accompanying `build-info.json` identify the exact source
commit, version, platform, and architecture.

Use GitHub's **Re-run failed jobs** for a transient failure at the same commit.
For a source fix, push it to the branch and dispatch `release-neoism.yml` from
that branch with `release_tag` set to the existing nightly and `platform` set
to `linux`, `macos`, or `windows`. This rebuilds only the selected platform and
updates its assets; its build metadata records the new commit. The checkout
version must still match the nightly tag. Stable releases must build from their
tag.

Downloads are a Linux tarball with its bundle installer, a macOS `.app` in a
DMG and tarball, and a Windows MSI plus portable ZIP. Install `Neoism-x86_64.msi` on Windows; the ZIP is an optional portable bundle.
Both include the complete GPUIX runtime. macOS builds are ad-hoc signed, not Developer ID notarized.

The release checks verify native engine loading, frontend tests, and executable
startup from the extracted archive. These checks do not establish interactive
GPU rendering or input parity on every supported machine. The preserved older
MSI investigation is [historical evidence](../docs/archive/windows-release-validation.md).
