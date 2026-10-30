//! Host-neutral immediate paint interface. Desktop delegates retain every
//! original draw/measure path; other GPU hosts consume the same layout.
use neoism_paint::{
    font::FontLibrary,
    text::{DrawOpts, Text, TextInstance, TextReveal},
    GraphicDataEntry, GraphicOverlay,
};

pub trait TextPaint {
    fn measure(&mut self, text: &str, opts: &DrawOpts) -> f32;
    fn measure_char_prefixes(&mut self, text: &str, opts: &DrawOpts) -> Vec<f32>;
    fn draw(&mut self, x: f32, y: f32, text: &str, opts: &DrawOpts) -> f32;
    fn draw_scaled(
        &mut self,
        x: f32,
        y: f32,
        text: &str,
        opts: &DrawOpts,
        scale: f32,
    ) -> f32;
    fn instances(&self) -> &[TextInstance];
    fn center_instances_in_rect(
        &mut self,
        first: usize,
        rect: [f32; 4],
        x: bool,
        y: bool,
    ) -> bool;
    fn baseline_offset(&mut self, opts: &DrawOpts) -> f32 {
        let _ = opts;
        0.0
    }
    fn font_cache_generation(&mut self) -> u64 {
        0
    }
    /// Draw with per-range reveal opacity (streaming text fade-in).
    fn draw_revealed(
        &mut self,
        x: f32,
        y: f32,
        text: &str,
        opts: &DrawOpts,
        reveals: &[TextReveal],
    ) -> f32 {
        let _ = reveals;
        self.draw(x, y, text, opts)
    }
}
impl TextPaint for Text {
    fn measure(&mut self, t: &str, o: &DrawOpts) -> f32 {
        Text::measure(self, t, o)
    }
    fn measure_char_prefixes(&mut self, t: &str, o: &DrawOpts) -> Vec<f32> {
        Text::measure_char_prefixes(self, t, o)
    }
    fn draw(&mut self, x: f32, y: f32, t: &str, o: &DrawOpts) -> f32 {
        Text::draw(self, x, y, t, o)
    }
    fn draw_scaled(&mut self, x: f32, y: f32, t: &str, o: &DrawOpts, s: f32) -> f32 {
        Text::draw_scaled(self, x, y, t, o, s)
    }
    fn instances(&self) -> &[TextInstance] {
        Text::instances(self)
    }
    fn center_instances_in_rect(
        &mut self,
        f: usize,
        r: [f32; 4],
        x: bool,
        y: bool,
    ) -> bool {
        Text::center_instances_in_rect(self, f, r, x, y)
    }
    fn baseline_offset(&mut self, o: &DrawOpts) -> f32 {
        Text::baseline_offset(self, o)
    }
    fn font_cache_generation(&mut self) -> u64 {
        Text::font_cache_generation(self)
    }
    fn draw_revealed(&mut self, x: f32, y: f32, t: &str, o: &DrawOpts, r: &[TextReveal]) -> f32 {
        Text::draw_revealed(self, x, y, t, o, r)
    }
}
#[allow(clippy::too_many_arguments)]
pub trait Paint {
    /// Immediate pixel renderers snap baselines; retained hosts can translate
    /// cached glyph images fractionally without reshaping their ink.
    fn snap_y(&self, y: f32) -> f32 { if y.is_finite() { y.round() } else { y } }
    fn asset_text(&mut self,path:&std::path::Path)->Option<String>{std::fs::read_to_string(path).ok()}

    fn text_mut(&mut self) -> &mut dyn TextPaint;
    fn overlay_text_mut(&mut self) -> &mut dyn TextPaint;
    fn font_library(&self) -> &FontLibrary;
    fn font_id_for_family(&self, family: &str) -> Option<usize> {
        self.font_library().font_id_for_family(family)
    }
    fn ensure_static_font(&mut self, data: &'static [u8]) -> Option<usize> {
        self.font_library().ensure_static_font(data)
    }
    fn scale_factor(&self) -> f32;
    /// Put subsequent content in the final overlay layer.
    fn set_late_overlay_mode(&mut self, enabled: bool);
    fn char_advance(&mut self, ch: char, attrs: neoism_paint::Attributes, font_size: f32) -> f32 {
        self.text_mut().measure(&ch.to_string(), &DrawOpts {
            font_size,
            bold: attrs.weight().0 >= 700,
            italic: attrs.style() != neoism_paint::Style::Normal,
            ..DrawOpts::default()
        })
    }

    fn rect(
        &mut self,
        id: Option<usize>,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        depth: f32,
        order: u8,
    );
    fn rounded_rect(
        &mut self,
        id: Option<usize>,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        depth: f32,
        radius: f32,
        order: u8,
    );
    fn quad(
        &mut self,
        id: Option<usize>,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        radii: [f32; 4],
        depth: f32,
        order: u8,
    ) {
        let _ = id;
        self.quad_clipped([x, y, w, h], color, radii, depth, order, [x, y, w, h]);
    }
    fn quad_clipped(
        &mut self,
        rect: [f32; 4],
        color: [f32; 4],
        radii: [f32; 4],
        depth: f32,
        order: u8,
        clip: [f32; 4],
    );
    fn set_bounds(&mut self, id: usize, bounds: Option<[f32; 4]>);
    fn triangle(
        &mut self,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        x3: f32,
        y3: f32,
        depth: f32,
        color: [f32; 4],
    );
    fn triangle_ordered(&mut self, x1:f32,y1:f32,x2:f32,y2:f32,x3:f32,y3:f32,depth:f32,color:[f32;4],order:u8) {
        let _=order; self.triangle(x1,y1,x2,y2,x3,y3,depth,color);
    }
    /// Same rounded-frame contours, represented as one ring by recording hosts.
    fn border_ring(&mut self, outer:&[(f32,f32)],inner:&[(f32,f32)],depth:f32,color:[f32;4],order:u8) {
        for i in 0..outer.len().min(inner.len()) {
            let next=(i+1)%outer.len();let(a,b,c,d)=(outer[i],outer[next],inner[next],inner[i]);
            self.triangle_ordered(a.0,a.1,b.0,b.1,c.0,c.1,depth,color,order);
            self.triangle_ordered(a.0,a.1,c.0,c.1,d.0,d.1,depth,color,order);
        }
    }
    fn polygon(&mut self, points: &[(f32, f32)], depth: f32, color: [f32; 4]);
    fn line(
        &mut self,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        width: f32,
        depth: f32,
        color: [f32; 4],
    );
    /// Hosts without a retained occlusion stack keep the original geometry;
    /// returns the previous rects like Sugarloaf.
    fn set_content_occlusion_rects(&mut self, rects: Vec<[f32; 4]>) -> Vec<[f32; 4]> {
        let _ = rects;
        Vec::new()
    }
    #[allow(clippy::too_many_arguments)]
    fn ring_clipped(
        &mut self,
        center: [f32; 2],
        radius: f32,
        stroke_width: f32,
        color: [f32; 4],
        depth: f32,
        order: u8,
        clip: [f32; 4],
    ) {
        let _ = clip;
        let steps = 48;
        let point = |r: f32, i: usize| {
            let a = i as f32 / steps as f32 * std::f32::consts::TAU;
            (center[0] + r * a.cos(), center[1] + r * a.sin())
        };
        let outer: Vec<_> = (0..steps).map(|i| point(radius + stroke_width * 0.5, i)).collect();
        let inner: Vec<_> = (0..steps).map(|i| point((radius - stroke_width * 0.5).max(0.0), i)).collect();
        self.border_ring(&outer, &inner, depth, color, order);
    }
    /// Retained image store (Sugarloaf `image_data`), keyed by image id.
    fn image_data_contains(&self, id: u32) -> bool {
        let _ = id;
        false
    }
    fn image_data_get(&self, id: u32) -> Option<&GraphicDataEntry> {
        let _ = id;
        None
    }
    fn image_data_insert(&mut self, id: u32, entry: GraphicDataEntry) {
        let _ = (id, entry);
    }
    fn image_data_remove(&mut self, id: u32) {
        let _ = id;
    }
    fn image_data_ids(&self) -> Vec<u32> {
        Vec::new()
    }
    /// Place a stored image for this frame (coordinates in host pixels).
    fn push_image_overlay(&mut self, panel_id: usize, overlay: GraphicOverlay) {
        let _ = (panel_id, overlay);
    }
    fn clear_image_overlays_for(&mut self, panel_id: usize) {
        let _ = panel_id;
    }
    fn overlay_rounded_rect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        depth: f32,
        radius: f32,
        order: u8,
    );
    fn overlay_quad(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        radii: [f32; 4],
        depth: f32,
        order: u8,
    ) {
        self.overlay_rounded_rect(x, y, w, h, color, depth, radii[0], order);
    }
    fn overlay_rect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: [f32; 4],
        depth: f32,
        order: u8,
    ) {
        self.overlay_rounded_rect(x, y, w, h, color, depth, 0.0, order);
    }
}
