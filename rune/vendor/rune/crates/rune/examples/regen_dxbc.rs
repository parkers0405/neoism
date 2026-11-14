//! Rewrites the precompiled DX12 uber shader (`uber.vs.dxbc`, `uber.fs.dxbc` and
//! `src/render/uber_dxbc_hash.rs`) after `shader.wgsl`, `clip.wgsl`, the uber layout or wgpu
//! changes, without building Rune's unit tests: `cargo run -p rune --example regen_dxbc` on
//! Windows. It does what `RUNE_REGENERATE_DXBC=1 cargo test -p rune --lib uber_dxbc_is_fresh`
//! does: makes the uber pipeline from WGSL on a DX12 device, takes the HLSL wgpu-hal logs, and
//! compiles it with FXC as wgpu-hal does. `uber_dxbc_is_fresh` checks the result.

#[cfg(target_os = "windows")]
fn main() {
    // The bytecode in the tree may match nothing now; make the pipeline from WGSL either way.
    // SAFETY: set before any other thread starts.
    unsafe { std::env::set_var("RUNE_DISABLE_DXBC", "1") };
    log::set_logger(&Capture).expect("the only logger");
    log::set_max_level(log::LevelFilter::Debug);
    let gpu = rune::render::Gpu::headless();
    drop(rune::render::Renderer::new(&gpu.device, wgpu::TextureFormat::Bgra8Unorm));
    let logged = HLSL.lock().unwrap();
    let stage = |name: &str| {
        let text = logged.iter().rev().find(|text| text.starts_with(&format!("Naga generated shader for \"{name}\"")))
            .unwrap_or_else(|| panic!("wgpu logged no HLSL for {name}: is the device DX12 with FXC?"));
        text.split_once(":\n").unwrap().1.to_owned()
    };
    let (vertex, fragment) = (stage("vs"), stage("fs"));
    let (vs, fs) = (fxc(&vertex, "vs", "vs_5_1"), fxc(&fragment, "fs", "ps_5_1"));
    let (wgsl, abi) = rune::render::uber_shader_identity();
    let wgsl = blake3::hash(wgsl.as_bytes()).to_hex();
    let hlsl = blake3::hash(format!("{vertex}\0{fragment}").as_bytes()).to_hex();
    let root = env!("CARGO_MANIFEST_DIR");
    std::fs::write(format!("{root}/uber.vs.dxbc"), &vs).unwrap();
    std::fs::write(format!("{root}/uber.fs.dxbc"), &fs).unwrap();
    std::fs::write(format!("{root}/src/render/uber_dxbc_hash.rs"), format!(
        "// Written by `cargo run -p rune --example regen_dxbc` (or `RUNE_REGENERATE_DXBC=1 cargo test -p rune --lib uber_dxbc_is_fresh`) on Windows.\n\
         pub(super) const UBER_DXBC_WGSL_BLAKE3: &str = \"{wgsl}\";\n\
         pub(super) const UBER_DXBC_ABI_FNV64: u64 = {abi:#018x};\n\
         #[cfg(test)]\npub(super) const UBER_DXBC_HLSL_BLAKE3: &str = \"{hlsl}\";\n")).unwrap();
    println!("uber.vs.dxbc {} bytes, uber.fs.dxbc {} bytes, WGSL {wgsl}", vs.len(), fs.len());
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("regen_dxbc runs on Windows, where FXC is");
}

/// HLSL wgpu-hal logged at debug level.
#[cfg(target_os = "windows")]
static HLSL: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

#[cfg(target_os = "windows")]
struct Capture;

#[cfg(target_os = "windows")]
impl log::Log for Capture {
    fn enabled(&self, metadata: &log::Metadata) -> bool { metadata.target().starts_with("wgpu_hal") }
    fn log(&self, record: &log::Record) {
        let text = record.args().to_string();
        if text.starts_with("Naga generated shader for") {
            HLSL.lock().unwrap().push(text);
        }
    }
    fn flush(&self) {}
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
