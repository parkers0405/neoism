//! Rewrites the precompiled uber shader for Windows (`crates/rune/uber.vs.dxbc`,
//! `uber.fs.dxbc` and `src/render/uber_dxbc_hash.rs`) after `shader.wgsl` or `clip.wgsl`
//! changes, without building the library's tests:
//!
//! `cargo run -p rune --example regenerate_dxbc` on Windows.
//!
//! It builds the uber pipeline from WGSL on a DX12 device, captures the HLSL wgpu generates for
//! it, compiles that with FXC as wgpu does, and writes the bytecode and its hashes. The uber
//! layout's ABI hash is kept from the file: when the layout changes, `uber_dxbc_is_fresh` names
//! the new one. That test (`RUNE_REGENERATE_DXBC=1 cargo test -p rune --lib uber_dxbc_is_fresh`)
//! writes the same files and checks them.

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("regenerate_dxbc runs on Windows, where FXC and DX12 are");
    std::process::exit(1);
}

#[cfg(target_os = "windows")]
fn main() {
    use std::sync::Mutex;

    /// HLSL wgpu-hal logs at debug level as it compiles a shader stage.
    static HLSL: Mutex<Vec<String>> = Mutex::new(Vec::new());
    struct Capture;
    impl log::Log for Capture {
        fn enabled(&self, metadata: &log::Metadata) -> bool { metadata.target().starts_with("wgpu_hal") }
        fn log(&self, record: &log::Record) {
            let text = record.args().to_string();
            if text.starts_with("Naga generated shader for") {
                HLSL.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(text);
            }
        }
        fn flush(&self) {}
    }

    let root = env!("CARGO_MANIFEST_DIR");
    // SAFETY: set before any other thread starts; Rune reads it when it makes the pipeline.
    unsafe { std::env::set_var("RUNE_DISABLE_DXBC", "1") };
    if log::set_logger(&Capture).is_err() {
        fail("another logger is installed");
    }
    log::set_max_level(log::LevelFilter::Debug);
    let gpu = rune::render::Gpu::headless();
    drop(rune::render::Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm));
    let logged = HLSL.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    let stage = |name: &str| {
        let text = logged.iter().rev().find(|t| t.starts_with(&format!("Naga generated shader for \"{name}\"")))
            .unwrap_or_else(|| fail(&format!("wgpu logged no HLSL for {name}: is the device DX12 with FXC?")));
        text.split_once(":\n").map(|(_, hlsl)| hlsl.to_owned()).unwrap_or_else(|| fail("unexpected log format"))
    };
    let (vertex, fragment) = (stage("vs"), stage("fs"));
    let (vs, fs) = (fxc(&vertex, "vs", "vs_5_1"), fxc(&fragment, "fs", "ps_5_1"));
    let wgsl = blake3::hash(format!("{}\n{}", rune::render::CLIP_WGSL, include_str!("../src/shader.wgsl")).as_bytes()).to_hex();
    let hlsl = blake3::hash(format!("{vertex}\0{fragment}").as_bytes()).to_hex();
    let hash_rs = format!("{root}/src/render/uber_dxbc_hash.rs");
    let old = std::fs::read_to_string(&hash_rs).unwrap_or_else(|e| fail(&format!("{hash_rs}: {e}")));
    let abi = old.lines().find_map(|l| l.strip_prefix("pub(super) const UBER_DXBC_ABI_FNV64: u64 = "))
        .and_then(|v| v.strip_suffix(';'))
        .unwrap_or_else(|| fail("no UBER_DXBC_ABI_FNV64 in uber_dxbc_hash.rs"));
    let write = |path: String, bytes: &[u8]| std::fs::write(&path, bytes).unwrap_or_else(|e| fail(&format!("{path}: {e}")));
    write(format!("{root}/uber.vs.dxbc"), &vs);
    write(format!("{root}/uber.fs.dxbc"), &fs);
    write(hash_rs, format!(
        "// Written by `cargo run -p rune --example regenerate_dxbc`, or by\n\
         // `RUNE_REGENERATE_DXBC=1 cargo test -p rune --lib uber_dxbc_is_fresh`, on Windows.\n\
         pub(super) const UBER_DXBC_WGSL_BLAKE3: &str = \"{wgsl}\";\n\
         pub(super) const UBER_DXBC_ABI_FNV64: u64 = {abi};\n\
         #[cfg(test)]\npub(super) const UBER_DXBC_HLSL_BLAKE3: &str = \"{hlsl}\";\n").as_bytes());
    println!("uber.vs.dxbc {} bytes, uber.fs.dxbc {} bytes, WGSL {wgsl}", vs.len(), fs.len());
}

#[cfg(target_os = "windows")]
fn fail(message: &str) -> ! {
    eprintln!("regenerate_dxbc: {message}");
    std::process::exit(1)
}

#[cfg(target_os = "windows")]
#[link(name = "d3dcompiler")]
unsafe extern "system" {
    fn D3DCompile(source: *const u8, len: usize, name: *const i8, defines: *const u8, include: *mut u8,
        entry: *const i8, target: *const i8, flags1: u32, flags2: u32, code: *mut *mut Blob, errors: *mut *mut Blob) -> i32;
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct Blob { vtbl: *const BlobVtbl }

#[cfg(target_os = "windows")]
#[repr(C)]
struct BlobVtbl {
    _query_interface: usize,
    _add_ref: usize,
    release: unsafe extern "system" fn(*mut Blob) -> u32,
    pointer: unsafe extern "system" fn(*mut Blob) -> *const u8,
    size: unsafe extern "system" fn(*mut Blob) -> usize,
}

/// FXC as wgpu-hal calls it: strictness only, shader model 5.1.
#[cfg(target_os = "windows")]
fn fxc(source: &str, entry: &str, target: &str) -> Vec<u8> {
    const D3DCOMPILE_ENABLE_STRICTNESS: u32 = 1 << 11;
    let entry = std::ffi::CString::new(entry).unwrap_or_else(|_| fail("entry point name"));
    let target = std::ffi::CString::new(target).unwrap_or_else(|_| fail("target name"));
    let (mut code, mut errors) = (std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: D3DCompile reads `source` and fills two COM blobs, each read and released once.
    unsafe {
        let hr = D3DCompile(source.as_ptr(), source.len(), std::ptr::null(), std::ptr::null(), std::ptr::null_mut(),
            entry.as_ptr(), target.as_ptr(), D3DCOMPILE_ENABLE_STRICTNESS, 0, &mut code, &mut errors);
        let read = |blob: *mut Blob| {
            let bytes = std::slice::from_raw_parts(((*(*blob).vtbl).pointer)(blob), ((*(*blob).vtbl).size)(blob)).to_vec();
            ((*(*blob).vtbl).release)(blob);
            bytes
        };
        if hr < 0 {
            fail(&format!("FXC failed ({hr:#x}): {}", String::from_utf8_lossy(&read(errors))));
        }
        if !errors.is_null() { read(errors); }
        read(code)
    }
}
