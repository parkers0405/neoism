//! MCP-supplied presentation shared by the server picker and timeline tool rows.
//! Decode bounded inline artwork on ingestion; frames only reuse uploaded images.
use base64::Engine;
use rune::{El, Image, ImageCache, el, text};
use serde_json::Value;
use std::{cell::RefCell, collections::{HashMap, VecDeque}};
use crate::{app::App, ui::theme::rgb};

#[derive(Default)]
pub struct Icons {
    rasters: HashMap<String, Option<Raster>>,
    order: VecDeque<String>,
}
struct Raster {
    pixels: Vec<u8>,
    uploaded: RefCell<Option<Image>>,
}
impl Raster {
    fn image(&self, images: &ImageCache) -> Image {
        let mut uploaded = self.uploaded.borrow_mut();
        uploaded.get_or_insert_with(|| images.rgba(64, 64, self.pixels.clone())).clone()
    }
}
fn sources(entry: &Value) -> impl Iterator<Item = &str> {
    entry["serverInfo"]["icons"].as_array().into_iter().flatten().take(4)
        .filter_map(|icon| icon["src"].as_str()).filter(|src| src.len() <= 32_768)
}
impl Icons {
    pub fn prepare(&mut self, entry: &Value) {
        for source in sources(entry) {
            if self.rasters.contains_key(source) { continue; }
            while self.rasters.len() >= 128 {
                if let Some(old) = self.order.pop_front() { self.rasters.remove(&old); }
            }
            self.rasters.insert(source.into(), decode(source));
            self.order.push_back(source.into());
        }
    }
    pub fn prepare_message(&mut self, message: &neoism_protocol::agent::HistoryMessage) {
        if let Some(mcp) = &message.mcp { self.prepare(&mcp["presentation"]); }
        for child in &message.composition_children { self.prepare_message(child); }
    }
    fn image(&self, entry: &Value, images: &ImageCache) -> Option<Image> {
        sources(entry).find_map(|src| self.rasters.get(src)?.as_ref().map(|r| r.image(images)))
    }
}
fn decode(source: &str) -> Option<Raster> {
    if source.len() > 32_768 { return None; }
    let (format, data) = if let Some(data) = source.strip_prefix("data:image/png;base64,") {
        (image::ImageFormat::Png, data)
    } else { (image::ImageFormat::Jpeg, source.strip_prefix("data:image/jpeg;base64,")?) };
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).ok()?;
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(256);
    limits.max_image_height = Some(256);
    limits.max_alloc = Some(1_048_576);
    reader.limits(limits);
    let image = reader.decode().ok()?.thumbnail(64, 64).to_rgba8();
    let mut canvas = image::RgbaImage::new(64, 64);
    image::imageops::overlay(&mut canvas, &image, ((64-image.width())/2) as i64, ((64-image.height())/2) as i64);
    Some(Raster { pixels: canvas.into_raw(), uploaded: RefCell::new(None) })
}
fn brand(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "firecrawl" | "firecrawl-mcp" | "mcp-firecrawl" | "firecrawl-mcp-server" => Some("firecrawl"),
        "webflow" | "webflow-mcp" | "mcp-webflow" | "webflow-mcp-server" => Some("webflow"),
        "vercel" | "vercel-mcp" | "mcp-vercel" => Some("vercel"),
        _ => None,
    }
}
/// Server artwork wins over bundled brands; missing/unsupported artwork gets the original plug.
pub fn view(app: &App, name: &str, entry: &Value, size: f32) -> El {
    let raster = app.images.as_ref().and_then(|images| app.mcp_icons.image(entry, images));
    let brand = entry["brand"].as_str().and_then(brand).or_else(|| brand(name))
        .or_else(|| entry["serverInfo"]["name"].as_str().and_then(brand));
    let image = raster.or_else(|| match brand {
        Some("vercel") => app.images.as_ref().and_then(|images| app.composer.logos.image(images, 5, app.pal.fg)),
        Some(name) => app.tools.icons.mcp_brand(name).cloned(),
        None => None,
    });
    match image {
        Some(image) => el().w(size).h(size).pixels(image.pixels()),
        None => el().w(size).h(size).center().child(text("\u{f1e6}").font(app.ty.icon_sized(size * 0.86)).color(rgb(app.pal.fg))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn inline(format: image::ImageFormat, width: u32) -> String {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(width, 8).write_to(&mut bytes, format).unwrap();
        format!("data:image/{};base64,{}", if format == image::ImageFormat::Png { "png" } else { "jpeg" }, base64::engine::general_purpose::STANDARD.encode(bytes.into_inner()))
    }
    #[test]
    fn supplied_png_and_jpeg_are_bounded_cached_and_shared_with_history() {
        for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
            let source = inline(format, 16);
            let entry = json!({"serverInfo":{"icons":[{"src": source}]}});
            let mut icons = Icons::default(); icons.prepare(&entry);
            let pixels = &icons.rasters[&source].as_ref().unwrap().pixels;
            assert_eq!(pixels.len(), 64*64*4);
            let pointer = pixels.as_ptr();
            icons.prepare(&entry);
            assert_eq!(icons.rasters[&source].as_ref().unwrap().pixels.as_ptr(), pointer);
            assert!(decode(&inline(format, 257)).is_none());
        }
    }
    #[test]
    fn catalog_history_and_live_compositions_ingest_supplied_artwork() {
        use neoism_protocol::agent::{AgentServerMessage as Server, HistoryMessage};
        let catalog: Value = serde_json::from_str(include_str!("../../fixtures/mcp-catalog.json")).unwrap();
        let source = catalog["custom-supplied"]["serverInfo"]["icons"][0]["src"].as_str().unwrap();
        let mut picker = App::new();
        picker.apply_agent(Server::McpCatalog { status: catalog.clone(), directory: None });
        assert!(picker.mcp_icons.rasters[source].is_some());
        let messages: Vec<HistoryMessage> = serde_json::from_str(include_str!("../../fixtures/mcp-timeline.json")).unwrap();
        let mut history = App::new();
        history.apply_agent(Server::HistoryChunk { session_id: "history".into(), messages: messages.clone(), next_cursor: None });
        assert!(history.mcp_icons.rasters[source].is_some());
        let mut parent = messages[0].clone();
        parent.mcp = None;
        parent.composition_children = messages;
        let mut live = App::new();
        live.mcp_icons.prepare_message(&parent);
        assert!(live.mcp_icons.rasters[source].is_some());
    }
    #[test]
    fn malformed_external_and_oversized_icons_fall_back_without_fetching() {
        for source in ["https://example.com/logo.png", "file:///tmp/icon", "data:image/svg+xml;base64,PHN2Zz4=", "data:image/png;base64,broken"] { assert!(decode(source).is_none()); }
        assert!(decode(&format!("data:image/png;base64,{}", "A".repeat(32768))).is_none());
        assert_eq!(brand("mcp_webflow"), Some("webflow"));
        assert_eq!(brand("not-webflow"), None);
    }
}
