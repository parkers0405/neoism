use super::*;
use std::sync::Arc;

#[test]
fn fetched_pixels_land_as_mips_and_oversized_or_short_ones_fail_naming_the_url() {
    let decoded = land("a.webp", Fetched::Rgba(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]), 1).unwrap();
    assert_eq!((decoded.mips[0].width, decoded.mips[0].height), (2, 1));
    assert_eq!(decoded.mips.len(), 2);
    let side = MAX_SIDE + 1;
    let huge = land("huge.avif", Fetched::Rgba(side, 1, vec![0; side as usize * 4]), 1).err().unwrap();
    assert!(huge.0.starts_with("huge.avif: "), "{huge}");
    assert!(land("short.png", Fetched::Rgba(2, 2, vec![0; 4]), 1).is_err(), "byte length must match the size");
}

#[cfg(feature = "svg")]
#[test]
fn fetched_svg_rasterizes_at_the_requested_size() {
    const SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="8"><rect width="16" height="8" fill="#fff"/></svg>"##;
    let decoded = land("mark.svg", Fetched::Svg(SVG.to_vec()), 48).unwrap();
    assert_eq!((decoded.mips[0].width, decoded.mips[0].height), (48, 24));
    assert!(land("bad.svg", Fetched::Svg(b"<svg".to_vec()), 48).err().unwrap().0.starts_with("bad.svg: "));
}

#[test]
fn native_urls_name_files_and_refuse_the_network() {
    assert_eq!(file_path("assets/logo.png").unwrap(), std::path::PathBuf::from("assets/logo.png"));
    #[cfg(not(windows))]
    {
        assert_eq!(file_path("file:///tmp/a%20b.png").unwrap(), std::path::PathBuf::from("/tmp/a b.png"));
        assert_eq!(file_path("FILE://localhost/tmp/x.png").unwrap(), std::path::PathBuf::from("/tmp/x.png"));
    }
    assert_eq!(file_path(r"C:\art\x.png").unwrap(), std::path::PathBuf::from(r"C:\art\x.png"), "a drive letter isn't a scheme");
    for url in ["https://example.com/a.png", "data:image/png;base64,AAAA", "file:relative.png", "file:///bad%zz.png"] {
        assert!(file_path(url).is_err(), "{url}");
    }
}

#[test]
fn the_cache_loads_a_url_once_and_wakes_when_it_lands() {
    let dir = std::env::temp_dir().join(format!("rune-image-url-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("dot.png");
    image::RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255])).save(&path).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let cache = ImageCache::new(Arc::new(move || { let _ = tx.send(()); }));
    let by_path = cache.url(path.to_str().unwrap(), 16.0, 2.0);
    assert!(Arc::ptr_eq(&by_path.0, &cache.url(path.to_str().unwrap(), 16.0, 2.0).0), "one handle per URL, size and scale");
    rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert_eq!(by_path.size(), Some([3, 2]));
    #[cfg(not(windows))]
    assert_eq!(cache.url(&format!("file://{}", path.display()), 16.0, 2.0).wait(), Ok([3, 2]));
    let remote = cache.url("https://example.com/dot.png", 16.0, 2.0);
    assert!(remote.wait().unwrap_err().0.contains("ImageCache::load"));
    assert_eq!(remote.pixels().source_url(), Some("https://example.com/dot.png"), "the URL stays readable for a page export");
    assert_eq!(cache.rgba(1, 1, vec![0; 4]).pixels().source_url(), None);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_source_set_loads_the_narrowest_wide_enough_and_keeps_every_size() {
    let cache = ImageCache::new(Arc::new(|| {}));
    let sources = [("https://example.com/a-1280.avif", 1280), ("https://example.com/a-640.avif", 640)];
    let phone = cache.url_set(&sources, 300.0, 2.0);
    assert_eq!(phone.pixels().source_url(), Some("https://example.com/a-640.avif"));
    assert_eq!(phone.pixels().source_set(), Some("https://example.com/a-640.avif 640w, https://example.com/a-1280.avif 1280w"));
    assert!(Arc::ptr_eq(&phone.0, &cache.url_set(&sources, 310.0, 2.0).0), "one handle per chosen file");
    assert_eq!(cache.url_set(&sources, 400.0, 2.0).pixels().source_url(), Some("https://example.com/a-1280.avif"));
    assert_eq!(cache.url_set(&sources, 900.0, 2.0).pixels().source_url(), Some("https://example.com/a-1280.avif"), "the widest when none is wide enough");
    assert_eq!(cache.url("https://example.com/a-640.avif", 300.0, 2.0).pixels().source_set(), None);
}
