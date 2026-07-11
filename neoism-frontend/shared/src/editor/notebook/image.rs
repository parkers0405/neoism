use super::*;

pub(crate) const NOTEBOOK_IMAGE_ID_NAMESPACE: u32 = 0xA100_0000;

#[derive(Clone, Debug)]
pub(crate) struct DecodedNotebookImageOutput {
    pub(crate) mime: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: Vec<u8>,
    pub(crate) is_opaque: bool,
}

pub(crate) fn decoded_notebook_image_output(
    output: &Value,
) -> Option<DecodedNotebookImageOutput> {
    let data = output.get("data")?.as_object()?;
    let (mime, value) = preferred_bitmap_image_mime(data)?;
    decoded_notebook_bitmap_value(mime, value)
}

pub(crate) fn decoded_notebook_attachment_image(
    cell: &NotebookCell,
    attachment_name: &str,
) -> Option<DecodedNotebookImageOutput> {
    let attachments = cell.extra.get("attachments")?.as_object()?;
    let bundle = attachments
        .get(attachment_name)
        .or_else(|| attachments.get(&percent_decode_attachment_name(attachment_name)))?
        .as_object()?;
    let (mime, value) = preferred_bitmap_image_mime(bundle)?;
    decoded_notebook_bitmap_value(mime, value)
}

pub(crate) fn decoded_notebook_bitmap_value(
    mime: &str,
    value: &Value,
) -> Option<DecodedNotebookImageOutput> {
    let bytes = decode_base64_value(value)?;
    let rgba = image_rs::load_from_memory(&bytes).ok()?.to_rgba8();
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let pixels = rgba.into_raw();
    let is_opaque = pixels
        .chunks_exact(4)
        .all(|pixel| pixel.get(3) == Some(&255));
    Some(DecodedNotebookImageOutput {
        mime: mime.to_string(),
        width,
        height,
        pixels,
        is_opaque,
    })
}

pub(crate) fn attachment_image_references(line: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = line;
    while let Some(ix) = rest.find("](attachment:") {
        let target = &rest[ix + 2..];
        if let Some(end) = target.find(')') {
            if let Some(name) = attachment_name_from_target(&target[..end]) {
                push_unique_attachment_name(&mut names, name);
            }
            rest = &target[end.saturating_add(1)..];
        } else {
            break;
        }
    }
    for quote in ['"', '\''] {
        let needle = format!("src={quote}attachment:");
        let mut rest = line;
        while let Some(ix) = rest.find(&needle) {
            let target = &rest[ix + "src=".len() + 1..];
            if let Some(end) = target.find(quote) {
                if let Some(name) = attachment_name_from_target(&target[..end]) {
                    push_unique_attachment_name(&mut names, name);
                }
                rest = &target[end.saturating_add(1)..];
            } else {
                break;
            }
        }
    }
    names
}

pub(crate) fn push_unique_attachment_name(names: &mut Vec<String>, name: String) {
    if !names.iter().any(|existing| existing == &name) {
        names.push(name);
    }
}

pub(crate) fn attachment_name_from_target(target: &str) -> Option<String> {
    let mut target = target.trim();
    if target.starts_with('<') && target.ends_with('>') && target.len() > 2 {
        target = &target[1..target.len().saturating_sub(1)];
    }
    let target = target.strip_prefix("attachment:")?;
    let name = target.trim().trim_matches('"').trim_matches('\'').trim();
    let end = name
        .char_indices()
        .find_map(|(ix, ch)| (ch == ')' || ch == '"' || ch == '\'').then_some(ix))
        .unwrap_or(name.len());
    let name = name[..end].trim();
    if name.is_empty() {
        None
    } else {
        Some(percent_decode_attachment_name(name))
    }
}

pub(crate) fn percent_decode_attachment_name(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut ix = 0usize;
    while ix < bytes.len() {
        if bytes[ix] == b'%' && ix + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[ix + 1]), hex_value(bytes[ix + 2]))
            {
                out.push((high << 4) | low);
                ix += 3;
                continue;
            }
        }
        out.push(bytes[ix]);
        ix += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| value.to_string())
}

pub(crate) fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn preferred_bitmap_image_mime<'a>(
    data: &'a serde_json::Map<String, Value>,
) -> Option<(&'a str, &'a Value)> {
    const PRIORITY: &[&str] = &["image/png", "image/jpeg", "image/webp", "image/gif"];
    for mime in PRIORITY {
        if let Some(value) = data.get(*mime) {
            return Some((*mime, value));
        }
    }
    data.iter()
        .find(|(mime, _)| mime.starts_with("image/") && mime.as_str() != "image/svg+xml")
        .map(|(mime, value)| (mime.as_str(), value))
}

pub(crate) fn notebook_image_output_id(
    path: &Path,
    cell: &NotebookCell,
    output_index: usize,
    mime: &str,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> u32 {
    let mut hasher = DefaultHasher::new();
    "neoism-notebook-image-output".hash(&mut hasher);
    path.to_string_lossy().hash(&mut hasher);
    notebook_cell_id(cell).hash(&mut hasher);
    output_index.hash(&mut hasher);
    mime.hash(&mut hasher);
    width.hash(&mut hasher);
    height.hash(&mut hasher);
    pixels.hash(&mut hasher);
    NOTEBOOK_IMAGE_ID_NAMESPACE | ((hasher.finish() as u32) & 0x00ff_ffff)
}

pub(crate) fn notebook_attachment_image_id(
    path: &Path,
    cell: &NotebookCell,
    attachment_index: usize,
    attachment_name: &str,
    line: usize,
    mime: &str,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> u32 {
    let mut hasher = DefaultHasher::new();
    "neoism-notebook-attachment-image".hash(&mut hasher);
    path.to_string_lossy().hash(&mut hasher);
    notebook_cell_id(cell).hash(&mut hasher);
    attachment_index.hash(&mut hasher);
    attachment_name.hash(&mut hasher);
    line.hash(&mut hasher);
    mime.hash(&mut hasher);
    width.hash(&mut hasher);
    height.hash(&mut hasher);
    pixels.hash(&mut hasher);
    NOTEBOOK_IMAGE_ID_NAMESPACE | ((hasher.finish() as u32) & 0x00ff_ffff)
}

pub(crate) fn image_output_summary(mime: &str, value: &Value) -> String {
    if mime == "image/svg+xml" {
        let chars = value_text(Some(value))
            .map(|text| text.chars().count())
            .unwrap_or_default();
        return format!("Image output: {mime}, {chars} chars");
    }
    let Some(bytes) = decode_base64_value(value) else {
        let encoded_chars = value_text(Some(value))
            .map(|text| text.chars().count())
            .unwrap_or_default();
        return format!("Image output: {mime}, {encoded_chars} encoded chars");
    };
    let size = format_bytes(bytes.len());
    if let Some((width, height)) = image_dimensions_from_header(mime, &bytes) {
        format!("Image output: {mime}, {width}x{height}, {size}")
    } else if let Ok(image) = image_rs::load_from_memory(&bytes) {
        let rgba = image.to_rgba8();
        let (width, height) = rgba.dimensions();
        format!("Image output: {mime}, {width}x{height}, {size}")
    } else {
        format!("Image output: {mime}, {size}")
    }
}

pub(crate) fn decode_base64_value(value: &Value) -> Option<Vec<u8>> {
    let encoded = value_text(Some(value))?;
    let compact = encoded
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    BASE64_STANDARD.decode(compact).ok()
}

pub(crate) fn image_dimensions_from_header(
    mime: &str,
    bytes: &[u8],
) -> Option<(u32, u32)> {
    match mime {
        "image/png" if bytes.len() >= 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") => {
            let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
            let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
            Some((width, height))
        }
        "image/gif"
            if bytes.len() >= 10
                && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) =>
        {
            let width = u16::from_le_bytes(bytes[6..8].try_into().ok()?) as u32;
            let height = u16::from_le_bytes(bytes[8..10].try_into().ok()?) as u32;
            Some((width, height))
        }
        _ => None,
    }
}
