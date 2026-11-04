//! Precompute wallpaper blur once; presentation only decodes the cached PNG.
use image::{imageops::FilterType, ImageReader};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, time::UNIX_EPOCH};

pub fn prepare(request: &Value) -> Result<String, String> {
    let source = PathBuf::from(request["path"].as_str().ok_or("Missing background path")?);
    let blur = request["blur"].as_f64().ok_or("Missing background blur")?;
    if !blur.is_finite() || !(0.0..=100.0).contains(&blur) { return Err("Invalid background blur".into()); }
    if blur == 0.0 { return Ok(source.to_string_lossy().into_owned()); }
    let metadata = fs::metadata(&source).map_err(|error| error.to_string())?;
    let modified = metadata.modified().map_err(|error| error.to_string())?.duration_since(UNIX_EPOCH).map_err(|error| error.to_string())?;
    let canonical = source.canonicalize().map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    hash.update(canonical.to_string_lossy().as_bytes());
    hash.update(metadata.len().to_le_bytes());
    hash.update(modified.as_nanos().to_le_bytes());
    hash.update(blur.to_bits().to_le_bytes());
    let cache = dirs::cache_dir().unwrap_or_else(std::env::temp_dir).join("neoism/gpuix/backgrounds");
    let output = cache.join(format!("{:x}.png", hash.finalize()));
    if output.is_file() { return Ok(output.to_string_lossy().into_owned()); }
    fs::create_dir_all(&cache).map_err(|error| error.to_string())?;
    let mut reader = ImageReader::open(&source).map_err(|error| error.to_string())?.with_guessed_format().map_err(|error| error.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|error| error.to_string())?;
    // A wallpaper never needs more pixels than a high-density desktop display.
    let image = if image.width().max(image.height()) > 4096 { image.resize(4096, 4096, FilterType::Triangle) } else { image };
    let blurred = image.blur(blur as f32);
    let temporary = tempfile::NamedTempFile::new_in(&cache).map_err(|error| error.to_string())?;
    // The cache is always PNG; static dispatch also avoids the dynamic encoder
    // vtable's by-value tracked caller, which Blaze cannot compile yet.
    blurred.write_with_encoder(image::codecs::png::PngEncoder::new(temporary.as_file())).map_err(|error| error.to_string())?;
    temporary.persist(&output).map_err(|error| error.to_string())?;
    Ok(output.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn blur_is_cached_and_source_changes_invalidate_it() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("wallpaper.png");
        let pixels = image::RgbaImage::from_pixel(1024, 1024, image::Rgba([255, 0, 0, 255]));
        pixels.save(&source).unwrap();
        let request = json!({"path":source,"blur":3.0});
        let build_started=std::time::Instant::now();
        let first = prepare(&request).unwrap();
        let build_time=build_started.elapsed();
        let first_metadata = fs::metadata(&first).unwrap().modified().unwrap();
        let cache_started=std::time::Instant::now();
        for _ in 0..100 { assert_eq!(prepare(&request).unwrap(), first); }
        let cache_time=cache_started.elapsed();
        eprintln!("wallpaper blur: first build {build_time:?}; 100 cache hits {cache_time:?}");
        assert!(cache_time<build_time,"cache hits should cost less than a single blur");
        assert_eq!(fs::metadata(&first).unwrap().modified().unwrap(), first_metadata);
        let blue = image::RgbaImage::from_pixel(1024, 1024, image::Rgba([0, 0, 255, 255]));
        blue.save(&source).unwrap();
        let next = prepare(&request).unwrap();
        assert_ne!(next, first);
        assert_eq!(image::open(next).unwrap().to_rgba8().get_pixel(512, 512).0, [0, 0, 255, 255]);
    }
}
