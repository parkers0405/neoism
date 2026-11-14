//! The macOS pasteboard's rich forms. Every form is written after one `clearContents`, so the
//! change count moves once; RTF from apps that offer nothing richer (TextEdit, Notes, Pages)
//! is converted to HTML. Runs on the clipboard worker: AppKit's RTF reader and HTML writer
//! don't need the main thread (its HTML *reader* does, and isn't used). A TIFF image (what
//! most Mac apps copy) is converted to PNG there too, by `NSBitmapImageRep`, and a copied PNG
//! to TIFF.
use std::path::PathBuf;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::ClassType;
use objc2_app_kit::{
    NSAttributedStringDocumentFormats, NSBitmapImageFileType, NSBitmapImageRep, NSCharacterEncodingDocumentAttribute,
    NSDocumentTypeDocumentAttribute, NSHTMLTextDocumentType, NSPasteboard, NSPasteboardItem, NSPasteboardTypeFileURL, NSPasteboardTypeHTML,
    NSPasteboardTypePNG, NSPasteboardTypeRTF, NSPasteboardTypeString, NSPasteboardTypeTIFF,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSCopying, NSData, NSDictionary, NSNumber, NSRange, NSString, NSURL, NSUTF8StringEncoding,
};
use zeroize::Zeroizing;
use super::{ClipboardContent, ClipboardImage, MAX_IMAGE_BYTES};
use super::html::MAX_RICH_BYTES;

/// Refuse to hand AppKit's RTF parser multi-megabyte blobs (e.g. embedded images).
const MAX_RTF_CONVERT_BYTES: usize = 1024 * 1024;

/// Replaces the pasteboard's contents with every form `content` carries. The first pasteboard
/// item holds the text (unless it's empty and another form isn't), HTML, RTF and the image as
/// PNG and TIFF (for apps that take only TIFF); each file is an item of its own with its file
/// URL, as Finder writes a copy of several files, so a file manager pastes them all.
pub(super) fn write(pb: &NSPasteboard, content: &ClipboardContent) -> Result<(), String> {
    autoreleasepool(|_| unsafe {
        let refused = |form: &str| format!("the pasteboard refused the {form}");
        let items: Vec<Retained<NSPasteboardItem>> = (0..content.files().len().max(1)).map(|_| NSPasteboardItem::new()).collect();
        let first = &items[0];
        let text = content.text();
        if (!text.is_empty() || !content.is_rich()) && !first.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) {
            return Err(refused("text"));
        }
        if let Some(html) = content.html() && !first.setString_forType(&NSString::from_str(&declare_utf8(html)), NSPasteboardTypeHTML) {
            return Err(refused("HTML"));
        }
        if let Some(rtf) = content.rtf() && !first.setData_forType(&NSData::with_bytes(rtf.as_bytes()), NSPasteboardTypeRTF) {
            return Err(refused("RTF"));
        }
        if let Some(image) = content.image() {
            let tiff = png_to_tiff(image.png()).ok_or_else(|| "the image didn't convert to TIFF".to_string())?;
            if !first.setData_forType(&NSData::with_bytes(image.png()), NSPasteboardTypePNG) || !first.setData_forType(&tiff, NSPasteboardTypeTIFF) {
                return Err(refused("image"));
            }
        }
        for (item, path) in items.iter().zip(content.files()) {
            let url = path.to_str().and_then(|p| NSURL::fileURLWithPath(&NSString::from_str(p)).absoluteString());
            if !url.is_some_and(|url| item.setString_forType(&url, NSPasteboardTypeFileURL)) { return Err(refused("file")); }
        }
        pb.clearContents();
        let objects = NSArray::from_vec(items.into_iter().map(ProtocolObject::from_retained).collect());
        if !pb.writeObjects(&objects) { return Err(refused("content")); }
        Ok(())
    })
}

/// Plain text, HTML and RTF, with HTML made from the RTF when the pasteboard has no HTML, and
/// with `all`, the pasteboard's PNG (or its TIFF as PNG) and its file URLs too.
pub(super) fn read(pb: &NSPasteboard, all: bool) -> Option<ClipboardContent> {
    autoreleasepool(|_| unsafe {
        let text = pb.stringForType(NSPasteboardTypeString).map(|s| Zeroizing::new(s.to_string()));
        let html = pb.stringForType(NSPasteboardTypeHTML).map(|s| s.to_string()).filter(|h| !h.is_empty() && h.len() <= MAX_RICH_BYTES);
        let rtf = pb.dataForType(NSPasteboardTypeRTF)
            .filter(|d| !d.is_empty() && d.len() <= MAX_RICH_BYTES)
            .map(|d| Zeroizing::new(d.bytes().to_vec()));
        let image = if all { read_image(pb) } else { None };
        let files = if all { read_files(pb) } else { Vec::new() };
        if text.is_none() && html.is_none() && rtf.is_none() && image.is_none() && files.is_empty() { return None; }
        let html = html.or_else(|| rtf.as_deref().and_then(|rtf| rtf_to_html(rtf)));
        let mut content = ClipboardContent::from(text.unwrap_or_default()).with_files(files);
        if let Some(html) = html { content = content.with_html(html); }
        if let Some(rtf) = rtf { content = content.with_rtf(String::from_utf8_lossy(&rtf)); }
        if let Some(image) = image { content = content.with_image(image); }
        Some(content)
    })
}

/// The path of each pasteboard item's file URL, in order.
fn read_files(pb: &NSPasteboard) -> Vec<PathBuf> {
    let Some(items) = (unsafe { pb.pasteboardItems() }) else { return Vec::new() };
    items.iter().filter_map(|item| unsafe { item.stringForType(NSPasteboardTypeFileURL) }.and_then(|url| crate::platform::macos::drop::file_path(&url))).collect()
}

/// TIFF for PNG bytes, through `NSBitmapImageRep`.
fn png_to_tiff(png: &[u8]) -> Option<Retained<NSData>> {
    unsafe { NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(png))?.TIFFRepresentation() }
}

const MAX_TIFF_BYTES: usize = 8 * MAX_IMAGE_BYTES;

/// The pasteboard's PNG as is, else its TIFF converted.
fn read_image(pb: &NSPasteboard) -> Option<ClipboardImage> {
    let fits = |d: &Retained<NSData>| !d.is_empty() && d.len() <= MAX_IMAGE_BYTES;
    unsafe {
        if let Some(png) = pb.dataForType(NSPasteboardTypePNG).filter(fits).and_then(|d| ClipboardImage::from_png(d.bytes())) {
            return Some(png);
        }
        // TIFF is uncompressed more often than not: allow it more room than the PNG it becomes.
        let tiff = pb.dataForType(NSPasteboardTypeTIFF).filter(|d| !d.is_empty() && d.len() <= MAX_TIFF_BYTES)?;
        tiff_to_png(tiff.bytes())
    }
}

/// PNG for TIFF bytes (the first image of a multi-page TIFF), through `NSBitmapImageRep`.
pub(crate) fn tiff_to_png(tiff: &[u8]) -> Option<ClipboardImage> {
    if tiff.is_empty() || tiff.len() > MAX_TIFF_BYTES || !tiff_dimensions_ok(tiff) {
        return None;
    }
    autoreleasepool(|_| unsafe {
        let rep = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(tiff))?;
        let (w, h) = (rep.pixelsWide(), rep.pixelsHigh());
        if w <= 0 || h <= 0 || !super::valid_image_size(w as u32, h as u32) {
            return None;
        }
        let png = rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())?;
        if png.len() > MAX_IMAGE_BYTES { return None; }
        ClipboardImage::from_png(png.bytes())
    })
}

/// Checks the first TIFF IFD's `ImageWidth` (256) and `ImageLength` (257) before handing the
/// bytes to AppKit, so a compressed decompression bomb cannot allocate a giant bitmap.
fn tiff_dimensions_ok(tiff: &[u8]) -> bool {
    if tiff.len() < 8 {
        return false;
    }
    let le = match &tiff[..4] {
        b"II\x2a\x00" => true,
        b"MM\x00\x2a" => false,
        _ => return false,
    };
    let u16_at = |at: usize| {
        let b: [u8; 2] = tiff.get(at..at + 2)?.try_into().ok()?;
        Some(if le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    };
    let u32_at = |at: usize| {
        let b: [u8; 4] = tiff.get(at..at + 4)?.try_into().ok()?;
        Some(if le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    };
    let Some(ifd) = u32_at(4).map(|o| o as usize) else { return false };
    let Some(entries) = u16_at(ifd).map(usize::from) else { return false };
    let (mut width, mut height) = (None, None);
    for i in 0..entries.min(4096) {
        let Some(base) = ifd.checked_add(2).and_then(|b| b.checked_add(i.checked_mul(12)?)) else { break };
        if base + 12 > tiff.len() {
            break;
        }
        let (Some(tag), Some(kind), Some(count)) = (u16_at(base), u16_at(base + 2), u32_at(base + 4)) else { break };
        if (tag == 256 || tag == 257) && count == 1 {
            let val = match kind {
                1 => tiff.get(base + 8).copied().map(u32::from),
                3 => u16_at(base + 8).map(u32::from),
                4 => u32_at(base + 8),
                _ => None,
            };
            if tag == 256 {
                width = val;
            } else {
                height = val;
            }
        }
    }
    width.zip(height).is_some_and(|(w, h)| super::valid_image_size(w, h))
}

/// A UTF-8 HTML document for RTF, through `NSAttributedString`.
pub(super) fn rtf_to_html(rtf: &[u8]) -> Option<String> {
    if rtf.is_empty() || rtf.len() > MAX_RTF_CONVERT_BYTES { return None; }
    autoreleasepool(|_| unsafe {
        let string = NSAttributedString::initWithRTF_documentAttributes(NSAttributedString::alloc(), &NSData::with_bytes(rtf), None)?;
        let html_type: Retained<AnyObject> = Retained::cast(NSHTMLTextDocumentType.copy());
        let utf8: Retained<AnyObject> = Retained::cast(NSNumber::new_usize(NSUTF8StringEncoding));
        let attributes = NSDictionary::from_id_slice(&[NSDocumentTypeDocumentAttribute, NSCharacterEncodingDocumentAttribute], &[html_type, utf8]);
        let data = string.dataFromRange_documentAttributes_error(NSRange::new(0, string.length()), &attributes).ok()?;
        if data.len() > MAX_RICH_BYTES { return None; }
        String::from_utf8(data.bytes().to_vec()).ok()
    })
}

/// Browsers read pasteboard HTML without a charset as Latin-1; say it's UTF-8.
fn declare_utf8(html: &str) -> std::borrow::Cow<'_, str> {
    let head = &html.as_bytes()[..html.len().min(1024)];
    if head.windows(7).any(|w| w.eq_ignore_ascii_case(b"charset")) { return html.into(); }
    format!("<meta charset=\"utf-8\">{html}").into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RTF: &str = r"{\rtf1\ansi\ansicpg1252\cocoartf2761
{\fonttbl\f0\fswiss\fcharset0 Helvetica;}
\pard\f0\fs24 Hello \b bold\b0  and \i italic\i0  caf\'e9}";

    /// Off the main thread, as on the clipboard worker, and bounded so a main-thread wait in
    /// AppKit fails the test instead of hanging it.
    #[test]
    fn rtf_becomes_html_off_the_main_thread() {
        let (send, recv) = std::sync::mpsc::channel();
        std::thread::spawn(move || { let _ = send.send(rtf_to_html(RTF.as_bytes())); });
        let html = recv.recv_timeout(std::time::Duration::from_secs(20)).expect("conversion finished").expect("converted");
        let text: String = super::super::sanitize_html(&html);
        assert!(text.contains("Hello") && text.contains("bold") && text.contains("café"), "{text}");
        assert!(html.contains("font-weight: bold") || html.contains("<b>"), "{html}");
        assert!(rtf_to_html(b"not rtf").is_none());
        assert!(rtf_to_html(&[]).is_none());
        assert!(rtf_to_html(&vec![b'a'; MAX_RTF_CONVERT_BYTES + 1]).is_none(), "oversized RTF is refused before AppKit");
    }

    /// A private pasteboard, never the user's. Skipped where there is no pasteboard server (a
    /// build machine's SSH session).
    #[test]
    fn every_form_round_trips_in_one_change() {
        let pb: Option<Retained<NSPasteboard>> = unsafe { objc2::msg_send_id![NSPasteboard::class(), pasteboardWithUniqueName] };
        let Some(pb) = pb else { return eprintln!("no pasteboard server; skipped") };
        let before = unsafe { pb.changeCount() };
        let content = ClipboardContent::new("Hi there").with_html("<b>Hi</b> there").with_rtf(RTF);
        write(&pb, &content).unwrap();
        assert_eq!(unsafe { pb.changeCount() }, before + 1);
        let back = read(&pb, true).unwrap();
        assert_eq!(back.text(), "Hi there");
        assert!(back.html().unwrap().ends_with("<b>Hi</b> there"));
        assert_eq!(back.rtf(), Some(RTF));

        unsafe {
            pb.clearContents();
            pb.setData_forType(Some(&NSData::with_bytes(RTF.as_bytes())), NSPasteboardTypeRTF);
        }
        let rtf_only = read(&pb, true).unwrap();
        assert_eq!(rtf_only.text(), "Hello bold and italic café", "the pasteboard derives text from RTF");
        assert!(rtf_only.html().unwrap().contains("bold"));

        let tiff = tiff_of(&png_2x3());
        unsafe {
            pb.clearContents();
            pb.setData_forType(Some(&NSData::with_bytes(&tiff)), NSPasteboardTypeTIFF);
        }
        assert_eq!(read(&pb, true).unwrap().image().map(ClipboardImage::size), Some((2, 3)), "a TIFF image pastes as PNG");
        assert!(read(&pb, false).is_none(), "an image is read only when asked for");
        unsafe {
            pb.clearContents();
            pb.setString_forType(&NSString::from_str("shot.png"), NSPasteboardTypeString);
            pb.setData_forType(Some(&NSData::with_bytes(&png_2x3())), NSPasteboardTypePNG);
        }
        let both = read(&pb, true).unwrap();
        assert_eq!((both.text(), both.image().map(|i| i.png().to_vec())), ("shot.png", Some(png_2x3())), "PNG passes through as is");
        let _: () = unsafe { objc2::msg_send![&pb, releaseGlobally] };
    }

    /// An image Rune copies goes on as PNG and TIFF with no empty text beside it; files go on
    /// one item each, with every other form on the first.
    #[test]
    fn images_and_files_are_written_for_other_apps() {
        let pb: Option<Retained<NSPasteboard>> = unsafe { objc2::msg_send_id![NSPasteboard::class(), pasteboardWithUniqueName] };
        let Some(pb) = pb else { return eprintln!("no pasteboard server; skipped") };
        let before = unsafe { pb.changeCount() };
        let image = ClipboardImage::from_png(png_2x3()).unwrap();
        write(&pb, &ClipboardContent::default().with_image(image.clone())).unwrap();
        assert_eq!(unsafe { pb.changeCount() }, before + 1);
        unsafe {
            assert_eq!(pb.dataForType(NSPasteboardTypePNG).map(|d| d.bytes().to_vec()), Some(png_2x3()));
            assert_eq!(pb.dataForType(NSPasteboardTypeTIFF).and_then(|d| tiff_to_png(d.bytes())).map(|i| i.size()), Some((2, 3)));
            assert!(pb.stringForType(NSPasteboardTypeString).is_none(), "an empty text isn't written beside the image");
        }
        assert_eq!(read(&pb, true).unwrap(), ClipboardContent::default().with_image(image), "the image pastes back as is");

        let files = [PathBuf::from("/fixtures/a.txt"), PathBuf::from("/fixtures/b c.png")];
        write(&pb, &ClipboardContent::new("a.txt b c.png").with_html("<b>files</b>").with_files(files.clone())).unwrap();
        let items = unsafe { pb.pasteboardItems() }.unwrap();
        assert_eq!(items.count(), 2, "one item per file");
        let url = unsafe { items.objectAtIndex(1).stringForType(NSPasteboardTypeFileURL) }.unwrap().to_string();
        assert_eq!(url, "file:///fixtures/b%20c.png");
        assert!(unsafe { items.objectAtIndex(1).stringForType(NSPasteboardTypeString) }.is_none(), "other forms ride on the first item");
        let back = read(&pb, true).unwrap();
        assert_eq!((back.text(), back.files()), ("a.txt b c.png", &files[..]));
        assert!(read(&pb, false).unwrap().files().is_empty(), "files are read only when asked for");
        let _: () = unsafe { objc2::msg_send![&pb, releaseGlobally] };
    }

    fn png_2x3() -> Vec<u8> {
        let rgba: Vec<u8> = (0..6u8).flat_map(|i| [i * 40, 255 - i * 40, 128, 255]).collect();
        crate::drag_out::encode_png(2, 3, &rgba).unwrap()
    }

    fn tiff_of(png: &[u8]) -> Vec<u8> { png_to_tiff(png).unwrap().bytes().to_vec() }

    #[test]
    fn tiff_becomes_png_off_the_main_thread() {
        let (send, recv) = std::sync::mpsc::channel();
        std::thread::spawn(move || { let _ = send.send(tiff_to_png(&tiff_of(&png_2x3())).map(|i| (i.size(), i.png().starts_with(b"\x89PNG")))); });
        let converted = recv.recv_timeout(std::time::Duration::from_secs(20)).expect("conversion finished");
        assert_eq!(converted, Some(((2, 3), true)));
        assert!(tiff_to_png(b"not a tiff").is_none());
        assert!(tiff_to_png(&[]).is_none());
        // A tiny TIFF header claiming 65535×65535 pixels (~4.3 GP) is refused before AppKit.
        let mut bomb = Vec::from(&b"II\x2a\x00\x08\x00\x00\x00\x02\x00"[..]);
        bomb.extend_from_slice(&[0x00, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0xff, 0xff, 0x00, 0x00]);
        bomb.extend_from_slice(&[0x01, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0xff, 0xff, 0x00, 0x00]);
        assert!(!tiff_dimensions_ok(&bomb));
        assert!(tiff_to_png(&bomb).is_none());
    }
}
