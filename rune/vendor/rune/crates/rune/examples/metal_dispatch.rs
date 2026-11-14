//! What wgpu's `custom` backend costs per frame on top of the Metal calls it makes, offscreen
//! (research/34). Run with `--profile perf --features metal-backend`.

fn main() {
    #[cfg(target_os = "macos")]
    rune::__metal_dispatch_overhead();
}
