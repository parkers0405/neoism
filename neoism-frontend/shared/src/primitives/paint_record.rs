//! Records the original immediate UI painter for another native GPU host.
//! Measurement and ink centering still use the original font/shaping engine.
use super::paint::{Paint, TextPaint};
use serde::Serialize;
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    rc::Rc,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use sugarloaf::{
    font::{FontLibrary, SugarloafFonts},
    text::{DrawOpts, Text, TextInstance, TextReveal},
    GraphicDataEntry, GraphicOverlay,
};

#[derive(Clone, Debug, Serialize)]
pub struct PaintRaster {
    pub id: u64,
    pub ptr: usize,
    pub len: usize,
    pub width: u32,
    pub height: u32,
    pub scale:f32,
    pub dx: f32,
    pub dy: f32,
    #[serde(skip)]
    _backing: Arc<Vec<u8>>,
}
impl PaintRaster {
    /// Immutable pixels stay owned by this retained paint command.
    pub fn pixels(&self)->&[u8]{self._backing.as_slice()}
}
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PaintCommand {
    Rect {
        rect: [f32; 4],
        color: [f32; 4],
        radii: [f32; 4],
        clip: Option<[f32; 4]>,
        depth: f32,
        order: u8,
        layer: u8,
    },
    Ring { outer:Vec<(f32,f32)>,inner:Vec<(f32,f32)>,color:[f32;4],depth:f32,order:u8,layer:u8 },
    Polygon {
        points: Vec<(f32, f32)>,
        color: [f32; 4],
        depth: f32,
        layer: u8,
    },
    Text {
        x: f32,
        y: f32,
        text: String,
        size: f32,
        width: f32,
        color: [u8; 4],
        bold: bool,
        italic: bool,
        extrude: bool,
        family: Option<String>,
        clip: Option<[f32; 4]>,
        layer: u8,
        raster: Option<PaintRaster>,
    },
    /// A stored image drawn into `rect`; `source` is the normalized
    /// `[u0, v0, u1, v1]` slice of the image to show.
    Image {
        rect: [f32; 4],
        source: [f32; 4],
        depth: f32,
        layer: u8,
        raster: PaintRaster,
    },
}
type Commands = Rc<RefCell<Vec<PaintCommand>>>;
/// The recorder is a 2x device: text rasters are produced at 2x, so layout
/// measures and shapes at 2x too. Measuring at 1x while painting 2x glyphs let
/// right-aligned rows drift past their slot (bold faces run ~2.5% wider).
const DEVICE_SCALE: f32 = 2.0;
struct RecordedText {
    original: Text,
    rasterizer:Text,
    fonts: FontLibrary,
    commands: Commands,
    emitted: Vec<(usize, usize)>,
    layer: u8,
    raster_cache: HashMap<String, PaintRaster>,
    raster_order: VecDeque<String>,
    raster_bytes: usize,
    generation: u64,
}
impl RecordedText {
    fn new(fonts: &FontLibrary, commands: Commands, layer: u8) -> Self {
        let mut original = Text::new(fonts);
        #[cfg(not(target_arch = "wasm32"))]
        original.init_cpu();
        original.set_scale_factor(DEVICE_SCALE);
        let mut rasterizer=Text::new(fonts);rasterizer.init_cpu();rasterizer.set_scale_factor(DEVICE_SCALE);
        let generation = original.font_cache_generation();
        Self {
            original,
            rasterizer,
            fonts: fonts.clone(),
            commands,
            emitted: Vec::new(),
            layer,
            raster_cache: HashMap::new(),
            raster_order: VecDeque::new(),
            raster_bytes: 0,
            generation,
        }
    }
    fn clear(&mut self) {
        self.original.clear();
        self.emitted.clear();
    }
    fn emit(&mut self, x: f32, y: f32, text: &str, opts: &DrawOpts, scale: f32) -> f32 {
        let generation = self.original.font_cache_generation();
        if generation != self.generation {
            self.emitted.clear();
            self.raster_cache.clear();
            self.raster_order.clear();
            self.raster_bytes = 0;
            self.generation = generation;
        }
        let first = self.original.instances().len();
        let width = if scale == 1.0 {
            self.original.draw(x, y, text, opts)
        } else {
            self.original.draw_scaled(x, y, text, opts, scale)
        };
        if text.is_empty() {
            return width;
        }
        let family = self
            .fonts
            .inner
            .read()
            .inner
            .get(&opts.font_id.unwrap_or(0))
            .and_then(|f| f.family_name())
            .map(str::to_owned);
        let key = format!(
            "{text}\0{}:{}:{}:{:?}:{}:{}:{}",
            opts.font_size.to_bits(),
            opts.bold,
            opts.italic,
            opts.font_id,
            opts.extrude,
            u32::from_be_bytes(opts.color),
            scale.to_bits()
        );
        let mut raster = self.raster_cache.get(&key).cloned();
        #[cfg(not(target_arch = "wasm32"))]
        if raster.is_none() {
            self.rasterizer.clear();
            if scale==1.0{self.rasterizer.draw(x,y,text,opts);}else{self.rasterizer.draw_scaled(x,y,text,opts,scale);}
            if let Some(image) = self.rasterizer.host_run_raster(0) {
                let data = Arc::new(image.rgba);
                static NEXT_RASTER: AtomicU64 = AtomicU64::new(1);
                let item = PaintRaster {
                    id: NEXT_RASTER.fetch_add(1, Ordering::Relaxed),
                    ptr: data.as_ptr() as usize,
                    len: data.len(),
                    width: image.width,
                    height: image.height,
                    scale:2.0,
                    dx: image.left/2.0 - x,
                    dy: image.top/2.0 - y,
                    _backing: data,
                };
                self.raster_bytes += item.len;
                self.raster_cache.insert(key.clone(), item.clone());
                self.raster_order.push_back(key.clone());
                raster = Some(item);
                while self.raster_bytes > 32 * 1024 * 1024
                    || self.raster_cache.len() > 2048
                {
                    let Some(old) = self.raster_order.pop_front() else {
                        break;
                    };
                    if let Some(old) = self.raster_cache.remove(&old) {
                        self.raster_bytes -= old.len;
                    }
                }
            }
        }
        let mut commands = self.commands.borrow_mut();
        let index = commands.len();
        commands.push(PaintCommand::Text {
            x,
            y,
            text: text.to_owned(),
            size: opts.font_size * scale,
            width,
            color: opts.color,
            bold: opts.bold,
            italic: opts.italic,
            extrude: opts.extrude,
            family,
            clip: opts.clip_rect,
            layer: self.layer,
            raster,
        });
        self.emitted.push((first, index));
        width
    }
}
impl TextPaint for RecordedText {
    fn measure(&mut self, t: &str, o: &DrawOpts) -> f32 {
        self.original.measure(t, o)
    }
    fn measure_char_prefixes(&mut self, t: &str, o: &DrawOpts) -> Vec<f32> {
        self.original.measure_char_prefixes(t, o)
    }
    fn draw(&mut self, x: f32, y: f32, t: &str, o: &DrawOpts) -> f32 {
        self.emit(x, y, t, o, 1.0)
    }
    fn draw_scaled(&mut self, x: f32, y: f32, t: &str, o: &DrawOpts, s: f32) -> f32 {
        self.emit(x, y, t, o, s)
    }
    fn instances(&self) -> &[TextInstance] {
        self.original.instances()
    }
    fn baseline_offset(&mut self, o: &DrawOpts) -> f32 {
        self.original.baseline_offset(o)
    }
    fn font_cache_generation(&mut self) -> u64 {
        self.original.font_cache_generation()
    }
    /// Streaming reveal: each range keeps its own opacity. Ranges are drawn as
    /// separate runs at their shaped prefix offsets; opacity is quantized so a
    /// fade does not mint a new cached raster every frame.
    fn draw_revealed(&mut self, x: f32, y: f32, t: &str, o: &DrawOpts, reveals: &[TextReveal]) -> f32 {
        if reveals.is_empty() || t.is_empty() {
            return self.emit(x, y, t, o, 1.0);
        }
        let mut cuts = vec![0usize, t.len()];
        for reveal in reveals {
            for edge in [reveal.range.start, reveal.range.end] {
                if edge <= t.len() && t.is_char_boundary(edge) {
                    cuts.push(edge);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for pair in cuts.windows(2) {
            let (start, end) = (pair[0], pair[1]);
            if start == end {
                continue;
            }
            let opacity = reveals
                .iter()
                .filter(|reveal| reveal.range.start <= start && end <= reveal.range.end)
                .map(|reveal| if reveal.opacity.is_finite() { reveal.opacity.clamp(0.0, 1.0) } else { 1.0 })
                .fold(1.0f32, f32::min);
            let opacity = (opacity * 16.0).round() / 16.0;
            if opacity <= 0.0 {
                continue;
            }
            let offset = if start == 0 { 0.0 } else { self.original.measure(&t[..start], o) };
            let mut segment = o.clone();
            segment.color[3] = (segment.color[3] as f32 * opacity).round() as u8;
            self.emit(x + offset, y, &t[start..end], &segment, 1.0);
        }
        self.original.measure(t, o)
    }
    fn center_instances_in_rect(
        &mut self,
        first: usize,
        rect: [f32; 4],
        x: bool,
        y: bool,
    ) -> bool {
        let before = self.original.instances().get(first).map(|i| i.pos);
        let changed = self.original.center_instances_in_rect(first, rect, x, y);
        if let Some(before) = before {
            if let Some(after) = self.original.instances().get(first).map(|i| i.pos) {
                // Instance positions are device pixels; commands are logical.
                let dx = (after[0] - before[0]) / DEVICE_SCALE;
                let dy = (after[1] - before[1]) / DEVICE_SCALE;
                let mut commands = self.commands.borrow_mut();
                for (_, index) in self.emitted.iter().filter(|(start, _)| *start >= first)
                {
                    if let PaintCommand::Text { x, y, .. } = &mut commands[*index] {
                        *x += dx;
                        *y += dy;
                    }
                }
            }
        }
        changed
    }
}
thread_local! {static FONT_LIBRARIES:RefCell<Vec<((String,u16,Vec<String>),FontLibrary)>>=RefCell::new(Vec::new());}

pub struct RecordingPainter {
    fonts: FontLibrary,
    text: RecordedText,
    overlay: RecordedText,
    commands: Commands,
    assets:HashMap<std::path::PathBuf,Option<String>>,
    asset_requests:std::collections::HashSet<std::path::PathBuf>,
    images: HashMap<u32, GraphicDataEntry>,
    /// Decoded RGBA per image id, keyed by the entry's handle identity.
    decoded: HashMap<u32, (u64, Option<PaintRaster>)>,
}
impl RecordingPainter {
    pub fn new(family: &str, weight: u16, extra_families: Vec<String>) -> Self {
        let key = (family.to_owned(), weight, extra_families.clone());
        let fonts = FONT_LIBRARIES.with(|cache| {
            let mut cache = cache.borrow_mut();
            if let Some((_, fonts)) = cache.iter().find(|(item, _)| item == &key) {
                return fonts.clone();
            }
            let (fonts, _) = FontLibrary::new(SugarloafFonts {
                family: Some(family.to_owned()),
                weight: Some(weight),
                extra_families: Some(extra_families),
                disable_warnings_not_found: true,
                ..Default::default()
            });
            if cache.len() >= 8 {
                cache.remove(0);
            }
            cache.push((key, fonts.clone()));
            fonts
        });
        Self::from_fonts(fonts)
    }
    pub fn from_fonts(fonts: FontLibrary) -> Self {
        let commands = Rc::new(RefCell::new(Vec::new()));
        let text = RecordedText::new(&fonts, commands.clone(), 1);
        let overlay = RecordedText::new(&fonts, commands.clone(), 3);
        Self {
            fonts,
            text,
            overlay,
            commands,
            assets:HashMap::new(),asset_requests:Default::default(),
            images: HashMap::new(),
            decoded: HashMap::new(),
        }
    }
    pub fn set_asset(&mut self,path:std::path::PathBuf,text:Option<String>){self.assets.insert(path,text);}
    pub fn take_asset_requests(&mut self)->Vec<std::path::PathBuf>{self.asset_requests.drain().collect()}
    pub fn begin_frame(&mut self) {
        self.commands.borrow_mut().clear();
        self.text.clear();
        self.overlay.clear();
    }
    fn image_raster(&mut self, id: u32) -> Option<PaintRaster> {
        let entry = self.images.get(&id)?;
        let identity = entry.handle.id();
        if let Some((cached, raster)) = self.decoded.get(&id) {
            if *cached == identity {
                return raster.clone();
            }
        }
        let rgba = match entry.handle.data() {
            sugarloaf::components::core::image::Data::Rgba { width, height, pixels } => {
                Some((*width, *height, pixels.to_vec()))
            }
            sugarloaf::components::core::image::Data::Bytes(bytes) => image_rs::load_from_memory(bytes)
                .ok()
                .map(|image| { let image = image.to_rgba8(); (image.width(), image.height(), image.into_raw()) }),
            sugarloaf::components::core::image::Data::Path(path) => image_rs::open(path)
                .ok()
                .map(|image| { let image = image.to_rgba8(); (image.width(), image.height(), image.into_raw()) }),
        };
        let raster = rgba.filter(|(w, h, px)| *w > 0 && *h > 0 && px.len() == (*w as usize) * (*h as usize) * 4).map(|(width, height, pixels)| {
            static NEXT_IMAGE: AtomicU64 = AtomicU64::new(1);
            let data = Arc::new(pixels);
            PaintRaster {
                id: NEXT_IMAGE.fetch_add(1, Ordering::Relaxed),
                ptr: data.as_ptr() as usize,
                len: data.len(),
                width,
                height,
                scale: 1.0,
                dx: 0.0,
                dy: 0.0,
                _backing: data,
            }
        });
        self.decoded.insert(id, (identity, raster.clone()));
        raster
    }
    pub fn commands(&self) -> Vec<PaintCommand> {
        self.commands.borrow().clone()
    }
    fn shape(
        &mut self,
        rect: [f32; 4],
        color: [f32; 4],
        radii: [f32; 4],
        clip: Option<[f32; 4]>,
        depth: f32,
        order: u8,
        layer: u8,
    ) {
        self.commands.borrow_mut().push(PaintCommand::Rect {
            rect,
            color,
            radii,
            clip,
            depth,
            order,
            layer,
        });
    }
}
impl Paint for RecordingPainter {
    fn asset_text(&mut self,path:&std::path::Path)->Option<String>{if let Some(text)=self.assets.get(path){return text.clone();}self.asset_requests.insert(path.to_path_buf());None}

    fn text_mut(&mut self) -> &mut dyn TextPaint {
        &mut self.text
    }
    fn overlay_text_mut(&mut self) -> &mut dyn TextPaint {
        &mut self.overlay
    }
    fn font_library(&self) -> &FontLibrary {
        &self.fonts
    }
    fn scale_factor(&self) -> f32 {
        DEVICE_SCALE
    }
    fn rect(
        &mut self,
        _: Option<usize>,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        c: [f32; 4],
        d: f32,
        o: u8,
    ) {
        self.shape([x, y, w, h], c, [0.0; 4], None, d, o, 0)
    }
    fn rounded_rect(
        &mut self,
        _: Option<usize>,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        c: [f32; 4],
        d: f32,
        r: f32,
        o: u8,
    ) {
        self.shape([x, y, w, h], c, [r; 4], None, d, o, 0)
    }
    fn quad_clipped(
        &mut self,
        r: [f32; 4],
        c: [f32; 4],
        rs: [f32; 4],
        d: f32,
        o: u8,
        clip: [f32; 4],
    ) {
        self.shape(r, c, rs, Some(clip), d, o, 0)
    }
    fn set_bounds(&mut self, _: usize, b: Option<[f32; 4]>) {
        if let Some(PaintCommand::Rect { clip, .. }) =
            self.commands.borrow_mut().last_mut()
        {
            *clip = b;
        }
    }
    fn triangle(
        &mut self,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        x3: f32,
        y3: f32,
        d: f32,
        c: [f32; 4],
    ) {
        self.polygon(&[(x1, y1), (x2, y2), (x3, y3)], d, c)
    }
    fn border_ring(&mut self,outer:&[(f32,f32)],inner:&[(f32,f32)],depth:f32,color:[f32;4],order:u8){
        self.commands.borrow_mut().push(PaintCommand::Ring{outer:outer.to_vec(),inner:inner.to_vec(),color,depth,order,layer:0});
    }
    fn polygon(&mut self, p: &[(f32, f32)], d: f32, c: [f32; 4]) {
        self.commands.borrow_mut().push(PaintCommand::Polygon {
            points: p.to_vec(),
            color: c,
            depth: d,
            layer: 0,
        });
    }
    fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, w: f32, d: f32, c: [f32; 4]) {
        let length = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt().max(0.001);
        let dx = -(y2 - y1) / length * w * 0.5;
        let dy = (x2 - x1) / length * w * 0.5;
        self.polygon(
            &[
                (x1 + dx, y1 + dy),
                (x2 + dx, y2 + dy),
                (x2 - dx, y2 - dy),
                (x1 - dx, y1 - dy),
            ],
            d,
            c,
        );
    }
    fn set_content_occlusion_rects(&mut self, rects: Vec<[f32; 4]>) -> Vec<[f32; 4]> {
        // The host canvas owns clipping; callers restore the returned set.
        let _ = rects;
        Vec::new()
    }
    fn image_data_contains(&self, id: u32) -> bool {
        self.images.contains_key(&id)
    }
    fn image_data_get(&self, id: u32) -> Option<&GraphicDataEntry> {
        self.images.get(&id)
    }
    fn image_data_insert(&mut self, id: u32, entry: GraphicDataEntry) {
        self.images.insert(id, entry);
    }
    fn image_data_remove(&mut self, id: u32) {
        self.images.remove(&id);
        self.decoded.remove(&id);
    }
    fn image_data_ids(&self) -> Vec<u32> {
        self.images.keys().copied().collect()
    }
    fn push_image_overlay(&mut self, _panel_id: usize, overlay: GraphicOverlay) {
        let Some(raster) = self.image_raster(overlay.image_id) else { return };
        let layer = if overlay.z_index < 0 { 0 } else if overlay.z_index == GraphicOverlay::LATE_OVERLAY_Z_INDEX { 3 } else { 2 };
        // Overlays are placed in device pixels (callers multiply by scale_factor).
        self.commands.borrow_mut().push(PaintCommand::Image {
            rect: [overlay.x / DEVICE_SCALE, overlay.y / DEVICE_SCALE, overlay.width / DEVICE_SCALE, overlay.height / DEVICE_SCALE],
            source: overlay.source_rect,
            depth: 0.0,
            layer,
            raster,
        });
    }
    fn overlay_quad(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        c: [f32; 4],
        r: [f32; 4],
        d: f32,
        o: u8,
    ) {
        self.shape([x, y, w, h], c, r, None, d, o, 2)
    }
    fn overlay_rounded_rect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        c: [f32; 4],
        d: f32,
        r: f32,
        o: u8,
    ) {
        self.shape([x, y, w, h], c, [r; 4], None, d, o, 2)
    }
}
