//! MCP presentation only: bundled first-connect marks and bounded inline raster
//! metadata. Remote URLs and SVG are intentionally not fetched/decoded here.
use super::icon::{register_provider_logo_atlas, ProviderLogo};
use crate::widgets::inline_picker::InlinePickerIcon;
use base64::Engine;
use serde_json::Value;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use sugarloaf::{ColorType, GraphicData, GraphicDataEntry, GraphicId};
use web_time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Brand {
    Firecrawl,
    Webflow,
    Vercel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Raster {
    source: String,
    id: u32,
    pixels: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct McpServiceIcon {
    brand: Option<Brand>,
    raster: Option<Arc<Raster>>,
}

/// Exact originating service, independent of humanized title or lifecycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpToolPresentation {
    pub server: Option<String>,
    pub icon: McpServiceIcon,
}

impl McpToolPresentation {
    pub fn new(server: Option<String>, entry: &Value) -> Self {
        let icon =
            McpServiceIcon::from_entry(server.as_deref().unwrap_or(""), entry, None);
        Self { server, icon }
    }

    /// Compact, additive history transport. Never serialize decoded pixels.
    pub fn to_wire(&self) -> Value {
        let brand = self.icon.brand.map(|brand| match brand {
            Brand::Firecrawl => "firecrawl",
            Brand::Webflow => "webflow",
            Brand::Vercel => "vercel",
        });
        let icons = self
            .icon
            .raster
            .as_ref()
            .map(|raster| vec![serde_json::json!({"src":raster.source})])
            .unwrap_or_default();
        serde_json::json!({"server":self.server, "presentation":{"brand":brand, "serverInfo":{"icons":icons}}})
    }

    pub fn from_wire(value: &Value) -> Option<Self> {
        value.as_object()?;
        let server = value["server"]
            .as_str()
            .filter(|s| {
                !s.is_empty() && s.len() <= 512 && !s.chars().any(char::is_control)
            })
            .map(str::to_owned);
        Some(Self::new(server, &value["presentation"]))
    }
}

fn brand(value: &str) -> Option<Brand> {
    match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "firecrawl" | "firecrawl-mcp" | "mcp-firecrawl" | "firecrawl-mcp-server" => {
            Some(Brand::Firecrawl)
        }
        "webflow" | "webflow-mcp" | "mcp-webflow" | "webflow-mcp-server" => {
            Some(Brand::Webflow)
        }
        "vercel" | "vercel-mcp" | "mcp-vercel" => Some(Brand::Vercel),
        _ => None,
    }
}

impl McpServiceIcon {
    /// Called on catalog ingestion, never from the renderer. Reuse decoded data
    /// across passive refreshes. Only tiny embedded PNG/JPEG are accepted.
    pub fn from_entry(name: &str, entry: &Value, previous: Option<&Self>) -> Self {
        let mut result = Self {
            brand: entry["brand"]
                .as_str()
                .and_then(brand)
                .or_else(|| brand(name))
                .or_else(|| entry["serverInfo"]["name"].as_str().and_then(brand)),
            raster: None,
        };
        if let Some(icons) = entry["serverInfo"]["icons"].as_array() {
            for icon in icons.iter().take(4) {
                let Some(source) = icon["src"].as_str().filter(|s| s.len() <= 32_768)
                else {
                    continue;
                };
                if let Some(raster) = previous
                    .and_then(|p| p.raster.as_ref())
                    .filter(|r| r.source == source)
                {
                    result.raster = Some(raster.clone());
                    break;
                }
                if let Some(raster) = decode_inline(source) {
                    result.raster = Some(Arc::new(raster));
                    break;
                }
            }
        }
        result
    }

    pub fn presentation(
        &self,
        sugarloaf: &mut dyn crate::primitives::paint::Paint,
        color: [u8; 4],
    ) -> InlinePickerIcon {
        if let Some(raster) = &self.raster {
            upload(sugarloaf, raster.id, &raster.pixels);
            return InlinePickerIcon::Image {
                panel_id: super::view::OVERLAY_PANEL_ID,
                image_id: raster.id,
                source_rect: [0.0, 0.0, 1.0, 1.0],
            };
        }
        if self.brand == Some(Brand::Vercel) {
            if let Some(id) = register_provider_logo_atlas(sugarloaf, color) {
                return InlinePickerIcon::Image {
                    panel_id: super::view::OVERLAY_PANEL_ID,
                    image_id: id,
                    source_rect: ProviderLogo::Vercel.source_rect(),
                };
            }
        }
        let asset = match self.brand {
            Some(Brand::Firecrawl) => Some((
                0xAC00_0001,
                include_bytes!("../../../assets/icons/mcp-firecrawl.png").as_slice(),
            )),
            Some(Brand::Webflow) => Some((
                0xAC00_0002,
                include_bytes!("../../../assets/icons/mcp-webflow.png").as_slice(),
            )),
            _ => None,
        };
        if let Some((id, bytes)) = asset {
            if !sugarloaf.image_data_contains(id) {
                if let Ok(image) = image_rs::load_from_memory(bytes) {
                    upload(sugarloaf, id, &image.to_rgba8().into_raw());
                }
            }
            if sugarloaf.image_data_contains(id) {
                return InlinePickerIcon::Image {
                    panel_id: super::view::OVERLAY_PANEL_ID,
                    image_id: id,
                    source_rect: [0.0, 0.0, 1.0, 1.0],
                };
            }
        }
        InlinePickerIcon::Glyph(crate::primitives::look::themed_glyph(
            "mcp.server",
            "\u{f1e6}",
        ))
    }
}

fn upload(sugarloaf: &mut dyn crate::primitives::paint::Paint, id: u32, pixels: &[u8]) {
    if sugarloaf.image_data_contains(id) {
        return;
    }
    // Keep changing server metadata from growing the per-window upload store.
    if id & 0xff00_0000 == 0xAD00_0000 {
        let dynamic = sugarloaf
            .image_data_ids()
            .into_iter()
            .filter(|key| *key & 0xff00_0000 == 0xAD00_0000)
            .collect::<Vec<_>>();
        if dynamic.len() >= 128 {
            for victim in dynamic.iter().take(dynamic.len() - 127) {
                sugarloaf.image_data_remove(*victim);
            }
        }
    }
    sugarloaf.image_data_insert(
        id,
        GraphicDataEntry::from_graphic_data(GraphicData {
            id: GraphicId::new(id as u64),
            width: 64,
            height: 64,
            color_type: ColorType::Rgba,
            pixels: pixels.to_vec(),
            is_opaque: false,
            resize: None,
            display_width: None,
            display_height: None,
            transmit_time: Instant::now(),
        }),
    );
}

fn decode_inline(source: &str) -> Option<Raster> {
    let (format, data) = if let Some(data) = source.strip_prefix("data:image/png;base64,")
    {
        (image_rs::ImageFormat::Png, data)
    } else {
        (
            image_rs::ImageFormat::Jpeg,
            source.strip_prefix("data:image/jpeg;base64,")?,
        )
    };
    if source.len() > 32_768 {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    let mut reader =
        image_rs::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image_rs::Limits::default();
    limits.max_image_width = Some(256);
    limits.max_image_height = Some(256);
    limits.max_alloc = Some(1_048_576);
    reader.limits(limits);
    let image = reader.decode().ok()?.thumbnail(64, 64).to_rgba8();
    let mut canvas = image_rs::RgbaImage::new(64, 64);
    image_rs::imageops::overlay(
        &mut canvas,
        &image,
        ((64 - image.width()) / 2) as i64,
        ((64 - image.height()) / 2) as i64,
    );
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hash);
    Some(Raster {
        source: source.into(),
        id: 0xAD00_0000 | (hash.finish() as u32 & 0x00ff_ffff),
        pixels: canvas.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn timeline_history_roundtrip_reuses_bundled_embedded_and_default_resolution() {
        for (server, entry) in [
            (
                Some("custom".into()),
                serde_json::json!({"brand":"firecrawl"}),
            ),
            (Some("webflow".into()), Value::Null),
            (Some("local".into()), Value::Null),
            (None, Value::Null),
        ] {
            let presentation = McpToolPresentation::new(server, &entry);
            assert_eq!(
                McpToolPresentation::from_wire(&presentation.to_wire()).unwrap(),
                presentation
            );
        }
        let source = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD
                .encode(include_bytes!("../../../assets/icons/mcp-webflow.png"))
        );
        let entry = serde_json::json!({"serverInfo":{"icons":[{"src":source}]}});
        let presentation = McpToolPresentation::new(Some("custom".into()), &entry);
        assert!(presentation.icon.raster.is_some());
        assert_eq!(
            McpToolPresentation::from_wire(&presentation.to_wire()).unwrap(),
            presentation
        );
        assert!(presentation.to_wire().get("pixels").is_none());
        assert!(McpToolPresentation::new(Some("local".into()), &Value::Null)
            .icon
            .brand
            .is_none());
    }

    #[test]
    fn brands_are_exact_and_available_before_initialize() {
        assert_eq!(
            McpServiceIcon::from_entry("firecrawl", &Value::Null, None).brand,
            Some(Brand::Firecrawl)
        );
        assert_eq!(
            McpServiceIcon::from_entry("custom", &json!({"brand":"webflow"}), None).brand,
            Some(Brand::Webflow)
        );
        assert_eq!(
            McpServiceIcon::from_entry("not-firecrawl-evil", &Value::Null, None).brand,
            None
        );
    }
    #[test]
    fn unsupported_or_bad_metadata_keeps_brand_fallback() {
        for source in [
            "https://example.com/icon.svg",
            "file:///etc/passwd",
            "data:image/svg+xml;base64,PHN2Zz4=",
            "data:image/png;base64,broken",
        ] {
            let icon = McpServiceIcon::from_entry(
                "firecrawl",
                &json!({"serverInfo":{"icons":[{"src":source}]}}),
                None,
            );
            assert_eq!(icon.brand, Some(Brand::Firecrawl));
            assert!(icon.raster.is_none());
        }
    }
    #[test]
    fn inline_rasters_reject_oversized_dimensions_before_allocation() {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image_rs::DynamicImage::new_rgba8(257, 1)
            .write_to(&mut bytes, image_rs::ImageFormat::Png)
            .unwrap();
        let source = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
        );
        assert!(decode_inline(&source).is_none());
        assert!(
            decode_inline(&format!("data:image/png;base64,{}", "A".repeat(32_768)))
                .is_none()
        );
    }

    #[test]
    fn tiny_inline_png_is_normalized_and_reused() {
        let source = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD
                .encode(include_bytes!("../../../assets/icons/mcp-firecrawl.png"))
        );
        let entry = json!({"serverInfo":{"icons":[{"src":source}]}});
        let first = McpServiceIcon::from_entry("local", &entry, None);
        let second = McpServiceIcon::from_entry("local", &entry, Some(&first));
        assert_eq!(first.raster.as_ref().unwrap().pixels.len(), 64 * 64 * 4);
        assert!(Arc::ptr_eq(
            first.raster.as_ref().unwrap(),
            second.raster.as_ref().unwrap()
        ));
    }
}
