//! Rune's, rune-fx's and rune-light's own pipelines, precompiled into `builtin_native.metallib`,
//! so their first use skips naga and Metal's front end (MSL to AIR), which is most of what a
//! pipeline costs on a cold shader cache. On iOS that includes the uber shader, which macOS
//! loads from `uber_native.metallib` instead.
//!
//! A pipeline's key hashes everything that decides the MSL naga writes for it: the WGSL, the
//! stage and entry point, override constants, runtime checks, whether it draws points, and the
//! slots its pipeline layout hands out. A key in the table names a function in the library,
//! so a precompiled function always binds the slots the runtime path would have given it. Any
//! other pipeline, an app's shader or a changed one, compiles at runtime as before.
//!
//! `scripts/precompile-metal-shaders.sh` regenerates the library and `builtin_table.rs`: it runs
//! rune-light's `builtin_shaders` test, which builds every built-in pipeline with
//! `RUNE_METAL_CAPTURE` set, so this module writes each one's MSL, in MSL 3.0, into its own
//! namespace, and compiles it for macOS, iOS and the iOS simulator. `RUNE_DISABLE_METALLIB=1`
//! turns the library off.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use wgpu::naga::{self, back::msl};

use super::Shared;
use super::shader::{Compiled, Ir, Point, function, library, translate};

include!("builtin_table.rs");

#[cfg(target_os = "macos")]
const LIBRARY: &[u8] = include_bytes!("../../builtin_native.metallib");
#[cfg(all(target_os = "ios", not(target_abi = "sim")))]
const LIBRARY: &[u8] = include_bytes!("../../builtin_native_ios.metallib");
#[cfg(all(target_os = "ios", target_abi = "sim"))]
const LIBRARY: &[u8] = include_bytes!("../../builtin_native_ios_sim.metallib");
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
const LIBRARY: &[u8] = &[];

/// Pipelines that found no precompiled function, for the test that keeps the table current.
static MISSES: AtomicUsize = AtomicUsize::new(0);

fn capture_dir() -> Option<&'static std::path::Path> {
    static DIR: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| std::env::var_os("RUNE_METAL_CAPTURE").map(Into::into)).as_deref()
}

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let off = matches!(std::env::var("RUNE_DISABLE_METALLIB").as_deref(), Ok("1" | "true" | "TRUE" | "yes"));
        !LIBRARY.is_empty() && !off && capture_dir().is_none()
    })
}

fn short(hash: &blake3::Hash) -> String {
    hash.to_hex()[..32].to_string()
}

/// Whether `source` is a built-in shader's WGSL.
pub(super) fn known(source: &blake3::Hash) -> bool {
    enabled() && SOURCES.binary_search(&short(source).as_str()).is_ok()
}

pub(super) fn key(ir: &Ir, point: &Point<'_>, resources: &msl::EntryPointResources, points: bool) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ir.hash.as_bytes());
    let Point { kind, entry, constants } = point;
    hasher.update(format!("{kind:?}|{entry:?}|{constants:?}|{:?}|{points}|{resources:?}", ir.checks).as_bytes());
    format!("k{}", short(&hasher.finalize()))
}

/// The precompiled function for the pipeline `key` names, if the table has it.
pub(super) fn find(shared: &Shared, key: &str) -> Option<Compiled> {
    if !enabled() { return None }
    let Ok(at) = FUNCTIONS.binary_search_by(|f| f.0.cmp(key)) else {
        MISSES.fetch_add(1, Ordering::Relaxed);
        return None;
    };
    let (_, name, sized, workgroup) = FUNCTIONS[at];
    let library = shared.builtin.get_or_init(|| library(shared, LIBRARY).ok()).as_ref()?;
    let function = function(library, name).ok()?;
    let sized = sized.iter().map(|&(group, binding)| naga::ResourceBinding { group, binding }).collect();
    Some(Compiled { function, sized, workgroup })
}

/// How many pipelines compiled at runtime because the table lacked them.
pub(crate) fn misses() -> usize {
    MISSES.load(Ordering::Relaxed)
}

/// Under `RUNE_METAL_CAPTURE`, writes the pipeline's MSL 3.0 into the directory it names, in a
/// namespace named for its hash, with its table row and its source's hash beside it.
pub(super) fn capture(ir: &Ir, point: &Point<'_>, resources: &msl::EntryPointResources, points: bool, key: &str) {
    let Some(dir) = capture_dir() else { return };
    let msl = translate(ir, point, resources, points, (3, 0)).unwrap_or_else(|e| panic!("{e}"));
    let split = msl.source.rfind("#include").and_then(|at| msl.source[at..].find('\n').map(|n| at + n + 1)).unwrap_or(0);
    let (head, body) = msl.source.split_at(split);
    let sized: Vec<String> = msl.sized.iter().map(|b| format!("({}, {})", b.group, b.binding)).collect();
    let [x, y, z] = msl.workgroup;
    // Pipelines whose MSL is the same share one function.
    let space = format!("m{}", short(&blake3::hash(body.as_bytes())));
    let write = |name: String, text: String| std::fs::write(dir.join(name), text).expect("RUNE_METAL_CAPTURE: write");
    write(format!("{space}.metal"), format!("{head}namespace {space} {{\n{body}\n}}\n"));
    write(format!("{key}.row"), format!("    (\"{key}\", \"{space}::{}\", &[{}], [{x}, {y}, {z}]),\n", msl.function, sized.join(", ")));
    write(format!("{}.source", short(&ir.hash)), String::new());
}
