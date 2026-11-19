//! Native, best-effort Mermaid 12.3 rendering (not a promise of browser parity).
//! All engine work is serialized off the UI thread. Keys include every host theme
//! input; completions can never overwrite another source or palette's preview.
use std::{cell::RefCell, collections::{HashMap, VecDeque}, sync::{Arc, OnceLock, mpsc, atomic::{AtomicBool, Ordering}}, time::Duration};
use sha2::{Digest, Sha256};
use crate::ui::theme::Palette;

const SOURCE_BYTES: usize = 32 * 1024;
const SLOTS: usize = 32;
const IN_FLIGHT: usize = 4;
const CACHE_BYTES: usize = 64 * 1024 * 1024;
const SVG_BYTES: usize = 2 * 1024 * 1024;
const PIXELS: u64 = 4 * 1024 * 1024;
const SIDE: u32 = 4096;
const FONT_BYTES: usize = 64 * 1024 * 1024;
const FALLBACK_PROBE: &str = "Café α Ж 世界 日本語 한국어 العربية עברית हिन्दी ไทย 😀";
type Key = [u8; 32];

#[derive(Clone, Debug)]
pub enum Entry {
    Pending,
    Ready { image: rune::Image, width: f32, height: f32 },
    /// Diagnostic only: callers retain and display their original fenced source.
    Failed(Arc<str>),
}
#[derive(Default)]
struct Notify { wake: OnceLock<rune::Waker>, completed: AtomicBool, notified: AtomicBool }
impl Notify {
    fn subscribe(&self, wake: Option<rune::Waker>) {
        if let Some(wake) = wake {
            // Only the UI ever writes this cell; the worker's get never waits.
            let _ = self.wake.set(wake);
            self.wake_if_ready();
        }
    }
    fn finish(&self) {
        self.completed.store(true, Ordering::Release);
        self.wake_if_ready();
    }
    fn wake_if_ready(&self) {
        if self.completed.load(Ordering::Acquire) {
            if let Some(wake) = self.wake.get() {
                if !self.notified.swap(true, Ordering::AcqRel) { wake(); }
            }
        }
    }
}
struct Slot { entry: Entry, used: u64, bytes: usize, notify: Arc<Notify> }
struct Job { key: Key, source: Arc<str>, colors: [u32; 5], notify: Arc<Notify> }
struct Done { key: Key, entry: Entry, bytes: usize }
struct Worker { jobs: mpsc::SyncSender<Job>, done: mpsc::Receiver<Done> }
#[derive(Default)]
struct State {
    slots: HashMap<Key, Slot>, worker: Option<Worker>, clock: u64, bytes: usize,
    submitted: usize, active: usize, waiting: VecDeque<Job>, wake: Option<rune::Waker>,
}

/// UI-owned: interior mutability is never shared with the worker, so request cannot
/// contend on a mutex. Default does not start threads or load font/engine assets.
/// The byte budget covers retained cache outputs; at most four bounded completions
/// may additionally be in transit. Views retaining Image clones own that memory.
#[derive(Default)]
pub struct Cache { state: RefCell<State>, epoch: usize }

impl Cache {
    pub fn epoch(&self) -> usize { self.epoch }

    /// Geometry discovery must not schedule work for offscreen editor fences.
    pub fn cached(&self, source: &str, palette: &Palette) -> Option<Entry> {
        if source.len() > SOURCE_BYTES { return Some(failed("Mermaid source exceeds 32 KiB preview limit")); }
        self.state.borrow().slots.get(&cache_key(source, palette)).map(|slot| slot.entry.clone())
    }

    pub fn request(&self, source: &str, palette: &Palette, waker: Option<rune::Waker>) -> Entry {
        // Reject before hashing/copying arbitrarily large streamed fences.
        if source.len() > SOURCE_BYTES { return failed("Mermaid source exceeds 32 KiB preview limit"); }
        let colors = [palette.bg, palette.fg, palette.surface, palette.blue, palette.border];
        let key = cache_key(source, palette);
        let mut s = self.state.borrow_mut();
        s.clock = s.clock.wrapping_add(1);
        let clock = s.clock;
        if let Some(wake) = &waker {
            s.wake = Some(wake.clone());
            // A late subscriber must also wake for earlier None-waker jobs,
            // otherwise deferred work could wait forever for a UI poll.
            for slot in s.slots.values().filter(|v| matches!(v.entry, Entry::Pending)) {
                slot.notify.subscribe(Some(wake.clone()));
            }
        }
        if let Some(slot) = s.slots.get_mut(&key) {
            slot.used = clock;
            if matches!(slot.entry, Entry::Pending) { slot.notify.subscribe(waker); }
            return slot.entry.clone();
        }
        while s.slots.len() >= SLOTS { if !s.evict() { return failed("Mermaid preview cache is busy"); } }
        let notify = Arc::new(Notify::default());
        notify.subscribe(waker.or_else(|| s.wake.clone()));
        if s.worker.is_none() { s.worker = start_worker().ok(); }
        let entry = if s.worker.is_some() { Entry::Pending } else { failed("Could not start native Mermaid worker") };
        s.slots.insert(key, Slot { entry, used: clock, bytes: 0, notify: notify.clone() });
        if s.worker.is_some() {
            s.waiting.push_back(Job { key, source: source.into(), colors, notify });
            s.dispatch();
        }
        s.slots[&key].entry.clone()
    }

    /// Bounded completion application only. No parsing, decoding, waits or timer.
    /// The completion wake happens after publication, so this sees finished images.
    pub fn poll(&mut self) -> bool {
        let s = self.state.get_mut();
        let mut changed = false;
        for _ in 0..IN_FLIGHT {
            let done = s.worker.as_ref().and_then(|w| w.done.try_recv().ok());
            let Some(done) = done else { break };
            s.active -= 1;
            if let Some(slot) = s.slots.get_mut(&done.key) {
                if matches!(slot.entry, Entry::Pending) {
                    slot.entry = done.entry;
                    slot.bytes = done.bytes;
                    s.bytes += done.bytes;
                    changed = true;
                }
            }
        }
        changed |= s.dispatch();
        while s.bytes > CACHE_BYTES { if !s.evict() { break; } }
        if changed { self.epoch = self.epoch.wrapping_add(1); }
        changed
    }
}
impl State {
    /// Admit only after completion application frees an active slot. Sources in
    /// the deferred FIFO are bounded by the same 32-slot cache admission limit.
    fn dispatch(&mut self) -> bool {
        let mut changed = false;
        for _ in 0..SLOTS {
            if self.active >= IN_FLIGHT { break; }
            let Some(job) = self.waiting.pop_front() else { break };
            let result = self.worker.as_ref().unwrap().jobs.try_send(job);
            match result {
                Ok(()) => { self.active += 1; self.submitted += 1; }
                Err(error) => {
                    let (mpsc::TrySendError::Full(job) | mpsc::TrySendError::Disconnected(job)) = error;
                    if let Some(slot) = self.slots.get_mut(&job.key) {
                        slot.entry = failed("Native Mermaid worker is unavailable");
                        changed = true;
                    }
                    job.notify.finish();
                }
            }
        }
        changed
    }
    fn evict(&mut self) -> bool {
        let key = self.slots.iter().filter(|(_, v)| !matches!(v.entry, Entry::Pending))
            .min_by_key(|(_, v)| v.used).map(|(k, _)| *k);
        if let Some(key) = key {
            self.bytes -= self.slots.remove(&key).unwrap().bytes;
            true
        } else { false }
    }
}
fn cache_key(source: &str, palette: &Palette) -> Key {
    let mut hash = Sha256::new();
    hash.update(source.as_bytes());
    for color in [palette.bg, palette.fg, palette.surface, palette.blue, palette.border] { hash.update(color.to_le_bytes()); }
    hash.finalize().into()
}

fn failed(message: impl Into<Arc<str>>) -> Entry { Entry::Failed(message.into()) }

fn start_worker() -> std::io::Result<Worker> {
    let (jobs, receive) = mpsc::sync_channel::<Job>(IN_FLIGHT);
    let (send, done) = mpsc::sync_channel(IN_FLIGHT);
    std::thread::Builder::new().name("mermaid-native".into()).spawn(move || {
        // Rune currently requires its fixed image decoder pool to prepare mips.
        // This pool is lazy too, never per diagram. All waits are on this worker.
        let initialized = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Ok::<_, String>((Engine::new()?, rune::ImageCache::new(Arc::new(|| {}))))
        })).unwrap_or_else(|_| Err("Could not initialize native Mermaid assets".into()));
        while let Ok(job) = receive.recv() {
            let output = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let (engine, images) = initialized.as_ref().map_err(Clone::clone)?;
                let raster = engine.render(&job.source, job.colors)?;
                let image = images.rgba(raster.w, raster.h, raster.rgba);
                image.wait().map_err(|e| e.to_string())?;
                // Full mip pyramid plus base storage; intentionally conservative.
                let bytes = raster.w as usize * raster.h as usize * 6;
                Ok::<_, String>((Entry::Ready { image, width: raster.width, height: raster.height }, bytes))
            })).unwrap_or_else(|_| Err("Native Mermaid engine panicked".into()));
            let (entry, bytes) = output.unwrap_or_else(|e| (failed(e), 0));
            if send.send(Done { key: job.key, entry, bytes }).is_err() { break; }
            job.notify.finish();
        }
    })?;
    Ok(Worker { jobs, done })
}

struct Raster {
    w: u32, h: u32, width: f32, height: f32, rgba: Vec<u8>,
    #[cfg(test)]
    text_paths: usize,
}
#[derive(Clone)]
struct Engine { fonts: merman::svg::export::NativeFontContext, fallback_note: String }
impl Engine {
    fn new() -> Result<Self, String> {
        // Shared immutable font buffers across windows/caches, initialized only
        // by a Mermaid worker. Other workers may wait here; the UI never does.
        static FONTS: OnceLock<Result<Engine, String>> = OnceLock::new();
        FONTS.get_or_init(Self::load_fonts).clone()
    }

    fn load_fonts() -> Result<Self, String> {
        use merman::svg::export::{NativeFontContext, NativeFontLimits};
        let mut bytes = vec![
            include_bytes!("../../../neoism-paint/src/font/resources/Geist/Geist-Variable.ttf").to_vec(),
            include_bytes!("../assets/fonts/GeistMono-Regular.otf").to_vec(),
            include_bytes!("../assets/fonts/CascadiaCodeNF-Regular.otf").to_vec(),
        ];
        let fallback_note = match platform_fallbacks(&mut bytes) {
            Ok(count) => format!("{count} installed outline-font buffers admitted"),
            Err(error) => format!("platform font discovery unavailable: {error}"),
        };
        let fonts = NativeFontContext::from_font_bytes(bytes,
            NativeFontLimits { max_bytes: FONT_BYTES, max_measurements: 4096, max_label_bytes: 8192, ..Default::default() },
            &merman::OperationControl::new()).map_err(|e| e.to_string())?
            .with_strict_glyph_coverage(true);
        Ok(Self { fonts, fallback_note })
    }

    fn render(&self, source: &str, colors: [u32; 5]) -> Result<Raster, String> {
        self.render_inner(source, colors).map_err(|error| {
            if error.contains("cannot shape") || error.contains("glyph") {
                format!("{error}. Install an outline font for the label's script and restart Neoism ({}). Bitmap-only/color emoji fonts are not supported by this native SVG path; the original source is retained.", self.fallback_note)
            } else { error }
        })
    }

    fn render_inner(&self, source: &str, colors: [u32; 5]) -> Result<Raster, String> {
        use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest};
        use merman::svg::{RenderResourcePolicy, SvgPipeline};
        if source.len() > SOURCE_BYTES { return Err("Mermaid source exceeds 32 KiB preview limit".into()); }
        let mut resources = RenderResourcePolicy::constrained();
        // Model items charge both nodes AND relationships across every family.
        for (id, value) in [("max_source_bytes", SOURCE_BYTES), ("max_model_items", 512),
            ("max_model_text_bytes", SOURCE_BYTES), ("max_model_nesting_depth", 32),
            ("max_svg_bytes", SVG_BYTES), ("max_svg_elements", 16000),
            ("max_layout_work_units", 250000), ("max_prepared_text_retained_bytes", 2 * 1024 * 1024)] {
            resources.apply_override(id, value).map_err(|e| e.to_string())?;
        }
        let hex = |n: u32| format!("#{n:06x}");
        let config = merman::MermaidConfig::from_value(serde_json::json!({
            "theme": "base", "fontFamily": "Geist", "htmlLabels": false,
            "securityLevel": "strict", "layout": "dagre",
            "flowchart": { "htmlLabels": false },
            "themeVariables": { "background": hex(colors[0]), "primaryTextColor": hex(colors[1]),
                "textColor": hex(colors[1]), "primaryColor": hex(colors[2]),
                "primaryBorderColor": hex(colors[3]), "lineColor": hex(colors[3]),
                "secondaryColor": hex(colors[2]), "tertiaryColor": hex(colors[2]),
                "edgeLabelBackground": hex(colors[0]), "clusterBkg": hex(colors[2]),
                "clusterBorder": hex(colors[4]), "fontFamily": "Geist" }
        }));
        let renderer = Renderer::new().with_semantic_parser(merman::SemanticParser::new().with_site_config(config));
        let request = SvgRequest {
            environment: merman::SvgEnvironment::deterministic().with_resource_policy(resources)
                .with_native_font_context(self.fonts.clone()).map_err(|e| e.to_string())?,
            pipeline: Some(SvgPipeline::resvg_safe()), ..Default::default()
        };
        let control = OperationControl::new().with_deadline(Duration::from_secs(2));
        let output = renderer.render(RenderRequest::svg(source, control.clone(), request)).map_err(|e| e.to_string())?;
        let RenderOutput::Svg(Some(svg)) = output else { return Err("No complete Mermaid diagram detected".into()); };
        #[cfg(test)]
        assert!(!svg.svg().contains("<foreignObject"), "native pipeline retained HTML-only labels");
        if svg.svg().len() > SVG_BYTES { return Err("Mermaid SVG exceeds preview limit".into()); }
        control.checkpoint().map_err(|e| e.to_string())?;
        let mut options = resvg::usvg::Options::default();
        options.font_family = "Geist".into();
        // No filesystem, external image resolver or ambient system font discovery.
        options.image_href_resolver = resvg::usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None), resolve_string: Box::new(|_, _| None),
        };
        self.fonts.apply_to_usvg_options(&mut options);
        let tree = resvg::usvg::Tree::from_str(svg.svg(), &options).map_err(|e| e.to_string())?;
        let size = tree.size();
        let width = size.width();
        let height = size.height();
        if !width.is_finite() || !height.is_finite() || width <= 0. || height <= 0. || width > 16384. || height > 16384. {
            return Err("Mermaid dimensions exceed preview limit".into());
        }
        // 2x when possible, downscale large charts before allocation. Logical size
        // stays the SVG size, independent of raster scale and view width.
        let scale = 2.0_f32.min(SIDE as f32 / width).min(SIDE as f32 / height)
            .min((PIXELS as f32 / (width * height)).sqrt());
        let w = (width * scale).floor().max(1.) as u32;
        let h = (height * scale).floor().max(1.) as u32;
        if u64::from(w) * u64::from(h) > PIXELS { return Err("Mermaid pixel budget exceeded".into()); }
        control.checkpoint().map_err(|e| e.to_string())?;
        let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or("Mermaid raster allocation failed")?;
        // resvg has no interruption API: the deadline is cooperative, not a hard
        // wall-time kill. SVG/model/work/pixel ceilings bound this final stage.
        resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
        control.checkpoint().map_err(|e| e.to_string())?;
        // tiny-skia is premultiplied sRGB; Rune's RGBA API requires straight alpha.
        let mut rgba = pixmap.take();
        for p in rgba.chunks_exact_mut(4) {
            if p[3] != 0 { for i in 0..3 { p[i] = ((p[i] as u32 * 255 + p[3] as u32 / 2) / p[3] as u32).min(255) as u8; } }
        }
        Ok(Raster { w, h, width, height, rgba,
            #[cfg(test)]
            text_paths: text_paths(tree.root()),
        })
    }
}

/// Run only during lazy worker initialization. Merman's native catalog bounds
/// directory traversal, temporary metadata reads, faces and resident font bytes.
/// Probe its resolver for common script fallback families, then freeze the exact
/// selected database buffers. No discovery/filesystem reads occur during charts.
/// This is not universal Unicode coverage; missing scripts remain explicit errors.
fn platform_fallbacks(bytes: &mut Vec<Vec<u8>>) -> Result<usize, String> {
    use merman::svg::export::{NativeFontLimits, NativeFontOptions};
    let control = merman::OperationControl::new().with_deadline(Duration::from_secs(5));
    let limits = NativeFontLimits {
        max_bytes: FONT_BYTES,
        max_system_scan_bytes: 1024 * 1024 * 1024,
        max_system_files: 4096,
        max_system_faces: 8192,
        max_system_cmap_bytes: 64 * 1024 * 1024,
        ..Default::default()
    };
    let platform = NativeFontOptions::default().with_system_fonts(true).with_limits(limits)
        .load(&control).map_err(|e| e.to_string())?;
    let mut options = resvg::usvg::Options::default();
    platform.apply_to_usvg_options(&mut options);
    let probe = format!("<svg xmlns='http://www.w3.org/2000/svg' width='1000' height='100'><text y='20' font-family='sans-serif'>{FALLBACK_PROBE}</text><text y='50' font-family='DejaVu Sans,Symbola,Segoe UI Symbol,Apple Symbols,Noto Sans Symbols 2,sans-serif'>😀</text></svg>");
    let tree = resvg::usvg::Tree::from_str(&probe, &options).map_err(|e| e.to_string())?;
    platform.check().map_err(|e| e.to_string())?;
    control.checkpoint().map_err(|e| e.to_string())?;
    let mut seen = std::collections::HashSet::<[u8; 32]>::new();
    for data in bytes.iter() { seen.insert(Sha256::digest(data).into()); }
    let mut retained: usize = bytes.iter().map(Vec::len).sum();
    let mut selected = Vec::new();
    for face in tree.fontdb().faces() {
        control.checkpoint().map_err(|e| e.to_string())?;
        tree.fontdb().with_face_data(face.id, |data, index| {
            let Ok(face) = ttf_parser::Face::parse(data, index) else { return };
            // usvg's native text path does not paint CBDT/sbix bitmap glyphs.
            // Admitting such a font could make shaping succeed but labels vanish.
            let tables = face.tables();
            if tables.glyf.is_none() && tables.cff.is_none() && tables.cff2.is_none() { return; }
            if FALLBACK_PROBE.chars().filter(|c| !c.is_whitespace()).any(|c| {
                face.glyph_index(c).is_some_and(|g| face.glyph_bounding_box(g).is_none())
            }) { return; }
            let key = Sha256::digest(data).into();
            if seen.insert(key) && retained + data.len() <= FONT_BYTES {
                retained += data.len();
                selected.push(data.to_vec());
            }
        });
    }
    let count = selected.len();
    bytes.extend(selected);
    Ok(count)
}

#[cfg(test)]
fn text_paths(group: &resvg::usvg::Group) -> usize {
    group.children().iter().map(|node| match node {
        resvg::usvg::Node::Group(g) => text_paths(g),
        resvg::usvg::Node::Text(t) => t.flattened().children().len(),
        _ => 0,
    }).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Instant, Duration};
    const COLORS: [u32; 5] = [0, 0xe8e8e8, 0x1a1a1a, 0x99aee5, 0x444444];
    const FLOW: &str = "flowchart TD\nsubgraph Group\nA[\"Café α\"] -->|yes| B{Ready?}\nB --> C[Done]\nC --> A\nend";

    #[test]
    fn renders_standard_families_with_visible_pixels() {
        let engine = Engine::new().unwrap();
        for source in [FLOW,
            "%%{init: {\"flowchart\":{\"htmlLabels\":true}}}%%\nflowchart LR\nA[\"<b>Café α</b><br/>Done\"] --> B[End]",
            "sequenceDiagram\nAlice->>Bob: Bonjour café\nBob-->>Alice: OK",
            "classDiagram\nAnimal <|-- Duck\nAnimal : +int age\nDuck : +quack()",
            "stateDiagram-v2\n[*] --> Idle\nIdle --> Running: start\nRunning --> [*]",
            "erDiagram\nCUSTOMER ||--o{ ORDER : places",
            "pie title Pets\n\"Dogs\" : 42\n\"Cats\" : 58",
            "gantt\ntitle Plan\ndateFormat YYYY-MM-DD\nsection Work\nTask :a1, 2026-01-01, 3d",
        ] {
            let start = Instant::now();
            let raster = engine.render(source, COLORS).unwrap_or_else(|e| panic!("{source}: {e}"));
            assert!(raster.width > 0. && raster.height > 0.);
            assert!(raster.w <= SIDE && raster.h <= SIDE);
            assert!(u64::from(raster.w) * u64::from(raster.h) <= PIXELS);
            assert!(raster.text_paths > 0, "labels produced no native glyph outlines: {source}");
            assert!(raster.rgba.chunks_exact(4).any(|p| p[3] != 0 && p[0] > 60));
            eprintln!("{}: {:?}, {}x{}", source.lines().next().unwrap(), start.elapsed(), raster.w, raster.h);
        }
    }

    #[test]
    fn rejects_bad_sources_and_resource_exhaustion() {
        let engine = Engine::new().unwrap();
        for source in ["", "not a diagram", "flowchart TD\nA[unterminated"] {
            assert!(engine.render(source, COLORS).is_err(), "accepted {source:?}");
        }
        assert!(engine.render(&"x".repeat(SOURCE_BYTES + 1), COLORS).is_err());
        let mut large = String::from("flowchart TD\n");
        for n in 0..600 { large.push_str(&format!("N{n} --> N{}\n", n + 1)); }
        assert!(engine.render(&large, COLORS).is_err());
    }

    #[test]
    fn unicode_uses_real_frozen_platform_fonts_or_reports_missing_script() {
        let engine = Engine::new().unwrap();
        eprintln!("fonts: {}; {} faces, {} resident bytes", engine.fallback_note,
            engine.fonts.face_count(), engine.fonts.resident_font_bytes());
        let mut options = resvg::usvg::Options::default();
        engine.fonts.apply_to_usvg_options(&mut options);
        let cjk_available = options.fontdb.faces().any(|face| {
            options.fontdb.with_face_data(face.id, |data, index| {
                ttf_parser::Face::parse(data, index).ok().is_some_and(|font| {
                    ['世', '界'].into_iter().all(|c| font.glyph_index(c)
                        .is_some_and(|g| font.glyph_bounding_box(g).is_some()))
                })
            }).unwrap_or(false)
        });
        let cjk = engine.render("flowchart LR\nA[世界] --> B[日本語]", COLORS);
        if cjk_available {
            let raster = cjk.unwrap_or_else(|e| panic!("installed CJK outline font was not rendered: {e}"));
            assert!(raster.text_paths >= 2);
            eprintln!("CJK native rendering ready: {}x{}", raster.w, raster.h);
        } else {
            let error = cjk.err().expect("missing CJK must not silently lose labels");
            assert!(error.contains("Install an outline font"), "{error}");
            eprintln!("CJK explicitly unavailable: {error}");
        }
        let latin_greek = engine.render("flowchart LR\nA[hé Καλημέρα] --> B[Café α]", COLORS).unwrap();
        assert!(latin_greek.text_paths >= 2);
        match engine.render("flowchart LR\nA[hé😀] --> B[Done]", COLORS) {
            Ok(raster) => { assert!(raster.text_paths >= 2); eprintln!("Outline emoji rendering ready"); }
            Err(error) => {
                assert!(error.contains("Install an outline font"), "{error}");
                eprintln!("Emoji explicitly unavailable: {error}");
            }
        }
    }

    fn finish(cache: &mut Cache, source: &str) -> Entry {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            cache.poll();
            let entry = cache.request(source, crate::ui::theme::palette("neoism_dark"), None);
            if !matches!(entry, Entry::Pending) { return entry; }
            assert!(Instant::now() < until, "worker did not complete");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn worker_dedup_warm_identity_failure_and_epoch() {
        let mut cache = Cache::default();
        assert!(cache.state.borrow().worker.is_none());
        assert!(!cache.poll());
        let palette = crate::ui::theme::palette("neoism_dark");
        assert!(matches!(cache.request(FLOW, palette, None), Entry::Pending));
        for _ in 0..100 { assert!(matches!(cache.request(FLOW, palette, None), Entry::Pending)); }
        assert_eq!(cache.state.borrow().submitted, 1);
        let Entry::Ready { image, width, height } = finish(&mut cache, FLOW) else { panic!("failed flowchart") };
        assert!(image.is_ready());
        let epoch = cache.epoch();
        for _ in 0..100 {
            let Entry::Ready { image: same, width: w, height: h } = cache.request(FLOW, palette, None) else { panic!() };
            assert_eq!(image.id(), same.id());
            assert_eq!((w, h), (width, height));
            assert!(!cache.poll());
        }
        assert_eq!(cache.epoch(), epoch);
        assert_eq!(cache.state.borrow().submitted, 1);
        assert!(matches!(finish(&mut cache, "broken Mermaid"), Entry::Failed(_)));
        let count = cache.state.borrow().submitted;
        assert!(matches!(cache.request("broken Mermaid", palette, None), Entry::Failed(_)));
        assert_eq!(cache.state.borrow().submitted, count);
        // A new palette is a distinct job; completion cannot mutate the old chart.
        let light = crate::ui::theme::palette("neoism_light");
        assert!(matches!(cache.request(FLOW, light, None), Entry::Pending));
        assert!(matches!(cache.request(FLOW, palette, None), Entry::Ready { .. }));
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            cache.poll();
            if let Entry::Ready { image: light_image, .. } = cache.request(FLOW, light, None) {
                assert_ne!(light_image.id(), image.id());
                break;
            }
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(2));
        }
        let Entry::Ready { image: original, .. } = cache.request(FLOW, palette, None) else { panic!() };
        assert_eq!(original.id(), image.id());
    }

    #[test]
    fn bounded_admission_and_lru() {
        let cache = Cache::default();
        let palette = crate::ui::theme::palette("neoism_dark");
        for n in 0..100 { cache.request(&format!("flowchart LR\nN{n} --> B"), palette, None); }
        let state = cache.state.borrow();
        assert_eq!(state.submitted, IN_FLIGHT);
        assert!(state.slots.len() <= SLOTS);
        assert_eq!(state.slots.values().filter(|s| matches!(s.entry, Entry::Pending)).count(), SLOTS);
        assert_eq!(state.waiting.len(), SLOTS - IN_FLIGHT);
    }

    #[test]
    fn eight_charts_finish_via_completion_wakes_without_admission_errors() {
        let mut cache = Cache::default();
        let palette = crate::ui::theme::palette("neoism_dark");
        let (send, receive) = mpsc::channel();
        let wake: rune::Waker = Arc::new(move || { let _ = send.send(()); });
        let sources: Vec<_> = (0..8).map(|n| format!("flowchart LR\nA[Chart {n}] --> B[Done]")).collect();
        for (i, source) in sources.iter().enumerate() {
            // Also exercise late waker registration for previously None jobs.
            let wake = if i < IN_FLIGHT { None } else { Some(wake.clone()) };
            assert!(matches!(cache.request(source, palette, wake), Entry::Pending));
        }
        assert_eq!(cache.state.borrow().active, IN_FLIGHT);
        assert_eq!(cache.state.borrow().waiting.len(), 4);
        loop {
            // A wait timeout is a test watchdog, not a poll timer: poll only
            // runs following a real worker completion event.
            receive.recv_timeout(Duration::from_secs(20)).expect("pending charts lost their wakeup");
            cache.poll();
            let mut ready = 0;
            for source in &sources {
                match cache.request(source, palette, None) {
                    Entry::Ready { .. } => ready += 1,
                    Entry::Pending => {},
                    Entry::Failed(e) => panic!("valid deferred chart failed: {e}"),
                }
            }
            if ready == sources.len() { break; }
        }
        assert_eq!(cache.state.borrow().submitted, 8);
        assert_eq!(cache.state.borrow().active, 0);
        assert!(cache.state.borrow().waiting.is_empty());
    }

    #[test]
    fn wakes_only_after_finished_output_is_published() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let wakes = Arc::new(AtomicUsize::new(0));
        let count = wakes.clone();
        let wake: rune::Waker = Arc::new(move || { count.fetch_add(1, Ordering::SeqCst); });
        let mut cache = Cache::default();
        cache.request(FLOW, crate::ui::theme::palette("neoism_dark"), None);
        cache.request(FLOW, crate::ui::theme::palette("neoism_dark"), Some(wake));
        let until = Instant::now() + Duration::from_secs(20);
        while wakes.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(cache.poll());
        assert!(matches!(cache.request(FLOW, crate::ui::theme::palette("neoism_dark"), None), Entry::Ready { .. }));
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        for _ in 0..100 { assert!(!cache.poll()); }
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }

    #[test]
    #[ignore = "diagnostic timings, not a CI performance threshold"]
    fn diagnostic_warm_request_benchmark() {
        let mut cache = Cache::default();
        let cold = Instant::now();
        assert!(matches!(finish(&mut cache, FLOW), Entry::Ready { .. }));
        eprintln!("first request through ready (worker/font initialization included): {:?}", cold.elapsed());
        let start = Instant::now();
        for _ in 0..10000 { std::hint::black_box(cache.request(FLOW, crate::ui::theme::palette("neoism_dark"), None)); }
        eprintln!("10,000 warm requests: {:?}", start.elapsed());
        assert_eq!(cache.state.borrow().submitted, 1);
        let start = Instant::now();
        for _ in 0..10000 { assert!(!std::hint::black_box(cache.poll())); }
        eprintln!("10,000 idle polls: {:?}", start.elapsed());
        let mut large = String::from("flowchart LR\nA[Comment padded] --> B\n");
        while large.len() + 68 <= SOURCE_BYTES { large.push_str("%% padding to measure bounded source-key hashing, not engine work.\n"); }
        assert!(matches!(finish(&mut cache, &large), Entry::Ready { .. }));
        let start = Instant::now();
        for _ in 0..2000 { std::hint::black_box(cache.request(&large, crate::ui::theme::palette("neoism_dark"), None)); }
        eprintln!("2,000 warm requests for {}-byte source: {:?}; {} raster bytes retained",
            large.len(), start.elapsed(), cache.state.borrow().bytes);
        assert_eq!(cache.state.borrow().submitted, 2);
    }
}