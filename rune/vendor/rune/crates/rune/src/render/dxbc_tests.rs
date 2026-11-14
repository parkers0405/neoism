//! The precompiled uber shader is FXC's compile of the HLSL wgpu generates today. Run
//! `cargo run -p rune --example regenerate_dxbc` (or `RUNE_REGENERATE_DXBC=1` with this test) on
//! Windows to rewrite the bytecode and its hashes after the WGSL, the uber layout or wgpu changes.

use std::sync::{Mutex, OnceLock};

/// HLSL wgpu-hal logged at debug level, with the thread that logged it.
static HLSL: Mutex<Vec<(std::thread::ThreadId, String)>> = Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, metadata: &log::Metadata) -> bool { metadata.target().starts_with("wgpu_hal") }
    fn log(&self, record: &log::Record) {
        let text = record.args().to_string();
        if text.starts_with("Naga generated shader for") {
            HLSL.lock().unwrap().push((std::thread::current().id(), text));
        }
    }
    fn flush(&self) {}
}

fn capture() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        log::set_logger(&Capture).expect("no other test installs a logger");
        log::set_max_level(log::LevelFilter::Debug);
    });
}

#[link(name = "d3dcompiler")]
unsafe extern "system" {
    fn D3DCompile(source: *const u8, len: usize, name: *const i8, defines: *const u8, include: *mut u8,
        entry: *const i8, target: *const i8, flags1: u32, flags2: u32, code: *mut *mut Blob, errors: *mut *mut Blob) -> i32;
}

#[repr(C)]
struct Blob { vtbl: *const BlobVtbl }

#[repr(C)]
struct BlobVtbl {
    _query_interface: usize,
    _add_ref: usize,
    release: unsafe extern "system" fn(*mut Blob) -> u32,
    pointer: unsafe extern "system" fn(*mut Blob) -> *const u8,
    size: unsafe extern "system" fn(*mut Blob) -> usize,
}

/// FXC as wgpu-hal calls it: strictness only, shader model 5.1.
fn fxc(source: &str, entry: &str, target: &str) -> Vec<u8> {
    const D3DCOMPILE_ENABLE_STRICTNESS: u32 = 1 << 11;
    let entry = std::ffi::CString::new(entry).unwrap();
    let target = std::ffi::CString::new(target).unwrap();
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
            panic!("FXC failed ({hr:#x}): {}", String::from_utf8_lossy(&read(errors)));
        }
        if !errors.is_null() { read(errors); }
        read(code)
    }
}

/// The HLSL wgpu generates for the uber pipeline's vertex and fragment stages.
fn wgpu_hlsl() -> (String, String) {
    capture();
    let gpu = crate::render::Gpu::headless();
    super::with_dxbc_enabled(false, || drop(crate::render::Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm)));
    let me = std::thread::current().id();
    let logged: Vec<String> = HLSL.lock().unwrap().iter().filter(|(thread, _)| *thread == me).map(|(_, text)| text.clone()).collect();
    let stage = |name: &str| {
        let text = logged.iter().rev().find(|text| text.starts_with(&format!("Naga generated shader for \"{name}\""))).unwrap_or_else(|| panic!("wgpu logged no HLSL for {name}: is the device DX12 with FXC?"));
        text.split_once(":\n").unwrap().1.to_owned()
    };
    (stage("vs"), stage("fs"))
}

fn hlsl_hash(vertex: &str, fragment: &str) -> String {
    blake3::hash(format!("{vertex}\0{fragment}").as_bytes()).to_hex().to_string()
}

#[test]
fn uber_dxbc_is_fresh() {
    let (vertex, fragment) = wgpu_hlsl();
    let (vs, fs) = (fxc(&vertex, "vs", "vs_5_1"), fxc(&fragment, "fs", "ps_5_1"));
    let wgsl = blake3::hash(super::super::metallib::uber_wgsl().as_bytes()).to_hex().to_string();
    let hlsl = hlsl_hash(&vertex, &fragment);
    let abi = super::super::metallib::uber_abi_hash();
    if std::env::var_os("RUNE_REGENERATE_DXBC").is_some() {
        let root = env!("CARGO_MANIFEST_DIR");
        std::fs::write(format!("{root}/uber.vs.dxbc"), &vs).unwrap();
        std::fs::write(format!("{root}/uber.fs.dxbc"), &fs).unwrap();
        std::fs::write(format!("{root}/src/render/uber_dxbc_hash.rs"), format!(
            "// Written by `cargo run -p rune --example regenerate_dxbc`, or by\n\
             // `RUNE_REGENERATE_DXBC=1 cargo test -p rune --lib uber_dxbc_is_fresh`, on Windows.\n\
             pub(super) const UBER_DXBC_WGSL_BLAKE3: &str = \"{wgsl}\";\n\
             pub(super) const UBER_DXBC_ABI_FNV64: u64 = {abi:#018x};\n\
             #[cfg(test)]\npub(super) const UBER_DXBC_HLSL_BLAKE3: &str = \"{hlsl}\";\n")).unwrap();
        return;
    }
    let stale = "the uber shader's HLSL changed: run `cargo run -p rune --example regenerate_dxbc` on Windows";
    assert_eq!(wgsl, super::UBER_DXBC_WGSL_BLAKE3, "{stale}");
    assert_eq!(abi, super::UBER_DXBC_ABI_FNV64, "{stale}");
    assert_eq!(hlsl, super::UBER_DXBC_HLSL_BLAKE3, "{stale}");
    assert!(vs == super::VERTEX && fs == super::FRAGMENT, "{stale}");
}

/// A DX12 device makes the uber pipeline from the bytecode, and without the passthrough
/// feature compiles the WGSL.
#[test]
fn renderers_load_the_precompiled_uber_shader() {
    let gpu = crate::render::Gpu::headless();
    let (wgsl, abi) = crate::render::uber_shader_identity();
    assert!(super::uber(&gpu.device, &wgsl, abi, |_| ()).is_some(), "a DX12 device takes the bytecode");
    assert!(super::uber(&gpu.device, "changed", abi, |_| ()).is_none(), "other WGSL compiles");
    assert!(super::with_dxbc_enabled(false, || super::uber(&gpu.device, &wgsl, abi, |_| ())).is_none());
    drop(crate::render::Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm));
}