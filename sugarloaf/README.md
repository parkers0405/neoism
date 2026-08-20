# Sugarloaf

Sugarloaf is the Neoism rendering engine, designed to be multiplatform. It is based on WebGPU, Rust library for Desktops and WebAssembly for Web (JavaScript).

```bash
cargo run --example text
```

## WGPU Shader Overlays

Sugarloaf can run Ghostty/Shadertoy-style post-processing shaders when built with
the `wgpu` feature. The app renders normally into an offscreen frame, then each
configured shader receives that frame as `iChannel0` and renders a fullscreen
pass into the next shader or the final swapchain image.

```rust
use sugarloaf::ShaderOverlayConfig;

sugarloaf.set_shader_overlay(ShaderOverlayConfig::new([
    "sugarloaf/examples/shaders/hypno_crt.glsl",
]))?;
```

Neoism config can pass shader paths through the renderer section:

```toml
[renderer]
shader-overlays = ["sugarloaf/examples/shaders/hypno_crt.glsl"]
```

Shader files implement the Ghostty-compatible entry point:

```glsl
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord.xy / iResolution.xy;
    fragColor = texture(iChannel0, uv);
}
```

Supported uniforms include `iResolution`, `iTime`, `iTimeDelta`, `iFrameRate`,
`iFrame`, `iChannelTime`, `iChannelResolution`, `iMouse`, `iDate`, `iFocus`,
`iTimeFocused`, and `iTimeUnfocused`. Currently only `iChannel0` is backed by a
real texture; the other channels are reserved for compatibility.

## Build dependencies

### Linux — Vulkan backend

The native Vulkan backend (default on Linux) compiles built-in GLSL shaders to SPIR-V at build time and user shader overlays to SPIR-V at runtime through `shaderc`. You need one GLSL -> SPIR-V compiler installed on the build host:

| Distro | Command |
|---|---|
| Debian / Ubuntu | `apt install glslang-tools` (or `apt install glslc`) |
| Arch | `pacman -S shaderc` (provides `glslc`) |
| Fedora | `dnf install glslang` (or `dnf install glslc`) |

`glslc` is preferred when both are present. Override with `GLSLC=/path/to/binary` or `GLSLANG_VALIDATOR=/path/to/binary`.

The compiled SPIR-V lives in `OUT_DIR` per build — the source `.glsl` files are checked in but the `.spv` artifacts are gitignored.

## WASM Tests

### Setup

Install `wasm-bindgen-cli` globally: `cargo install wasm-bindgen-cli`.
`wasm-bindgen-cli` provides a test runner harness.

### Running Tests

Run (in the root sugarloaf directory):

```
CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner cargo test --target wasm32-unknown-unknown -p sugarloaf --tests
```

Flag explanation:

- `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner`: Tells
  Cargo to use the test harness provided by `wasm-bindgen-cli`.
- `-p sugarloaf`: Only run tests in the sugarloaf directory.
- `--tests`: Only run tests; do not build examples. Many (possibly all) of the
  examples in sugarloaf/examples currently do not compile to WASM because they
  use networking.
