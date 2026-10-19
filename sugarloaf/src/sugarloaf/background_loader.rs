//! Native-only CPU background preparation. The parent owns generation filtering and GPU upload.
//! All filesystem work (including cache maintenance) runs on the single worker.

use super::BackgroundFileIdentity;
use crate::{renderer::BackgroundImagePixels, ImageProperties};
use std::collections::hash_map::DefaultHasher;
use std::fs::{self, File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Instant;

const VERSION: u32 = 1;
const NAMESPACE: &str = "background-rgba-v1";
const EXTENSION: &str = "neobgrgba";
const MAGIC: &[u8; 8] = b"NEOBGRGB";
const HEADER_LEN: u64 = 36;
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const MAX_DIMENSION: u32 = 16_384;
const MAX_ENTRIES: usize = 64;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

type Wake = Arc<dyn Fn() + Send + Sync>;

pub(super) struct PreparedBackground {
    pub(super) identity: BackgroundFileIdentity,
    pub(super) pixels: BackgroundImagePixels,
    pub(super) cache_hit: bool,
}

pub(super) struct BackgroundImageResult {
    pub(super) generation: u64,
    pub(super) image: ImageProperties,
    pub(super) prepared: Result<PreparedBackground, String>,
}

struct Request {
    generation: u64,
    image: ImageProperties,
    cache_dir: PathBuf,
    wake: Wake,
}

#[derive(Default)]
struct Slot {
    stopped: bool,
    pending: Option<Request>,
}

pub(super) struct BackgroundImageLoader {
    shared: Arc<(Mutex<Slot>, Condvar)>,
    results: mpsc::Receiver<BackgroundImageResult>,
}

impl BackgroundImageLoader {
    pub(super) fn new() -> Result<Self, String> {
        let shared = Arc::new((Mutex::new(Slot::default()), Condvar::new()));
        // Bound completed pixel buffers too. A slow UI backpressures this worker,
        // never the requester. Dropping the receiver unblocks a waiting send.
        let (sender, results) = mpsc::sync_channel(1);
        let worker_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("background-image-loader".into())
            .spawn(move || worker(worker_shared, sender))
            .map_err(|error| format!("start background image worker: {error}"))?;
        Ok(Self { shared, results })
    }

    /// Replaces only the pending request; an already-running request may complete.
    /// Superseded pending requests intentionally produce no result or wake.
    pub(super) fn request(
        &self,
        generation: u64,
        image: ImageProperties,
        cache_dir: PathBuf,
        wake: Wake,
    ) -> Result<(), String> {
        let (mutex, changed) = &*self.shared;
        let mut slot = mutex
            .lock()
            .map_err(|_| "background worker slot poisoned")?;
        if slot.stopped {
            return Err("background worker stopped".into());
        }
        slot.pending = Some(Request {
            generation,
            image,
            cache_dir,
            wake,
        });
        changed.notify_one();
        Ok(())
    }

    pub(super) fn cancel_pending(&self) {
        let (mutex, _) = &*self.shared;
        mutex
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pending = None;
    }

    pub(super) fn try_recv(&self) -> Option<BackgroundImageResult> {
        self.results.try_recv().ok()
    }
}

impl Drop for BackgroundImageLoader {
    fn drop(&mut self) {
        let (mutex, changed) = &*self.shared;
        let mut slot = mutex.lock().unwrap_or_else(|error| error.into_inner());
        slot.stopped = true;
        slot.pending = None;
        changed.notify_one();
        // No JoinHandle is retained: a decode in progress must not block UI teardown.
    }
}

fn worker(
    shared: Arc<(Mutex<Slot>, Condvar)>,
    sender: mpsc::SyncSender<BackgroundImageResult>,
) {
    loop {
        let request = {
            let (mutex, changed) = &*shared;
            let mut slot = mutex.lock().unwrap_or_else(|error| error.into_inner());
            while !slot.stopped && slot.pending.is_none() {
                slot = changed
                    .wait(slot)
                    .unwrap_or_else(|error| error.into_inner());
            }
            if slot.stopped {
                return;
            }
            slot.pending.take().expect("pending request")
        };
        let started = Instant::now();
        let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            prepare(&request.image, &request.cache_dir)
        }))
        .unwrap_or_else(|_| Err("background image preparation panicked".into()));
        tracing::debug!(
            target: "sugarloaf::background_loader",
            generation = request.generation,
            cache_hit = prepared.as_ref().map(|p| p.cache_hit).unwrap_or(false),
            preparation_ms = started.elapsed().as_secs_f64() * 1000.0,
            success = prepared.is_ok(),
            "background image prepared"
        );
        if shared
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .stopped
        {
            return;
        }
        if sender
            .send(BackgroundImageResult {
                generation: request.generation,
                image: request.image,
                prepared,
            })
            .is_err()
        {
            return;
        }
        // User code is never called under a loader lock, even on preparation error.
        let _ =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (request.wake)()));
    }
}

fn identity(image: &ImageProperties) -> Result<BackgroundFileIdentity, String> {
    BackgroundFileIdentity::read(&image.path)
        .ok_or_else(|| format!("cannot stat background image '{}'", image.path))
}

fn cache_key(image: &ImageProperties, identity: &BackgroundFileIdentity) -> String {
    let path = fs::canonicalize(&image.path).unwrap_or_else(|_| {
        let path = PathBuf::from(&image.path);
        if path.is_absolute() {
            path
        } else {
            std::env::current_dir().unwrap_or_default().join(path)
        }
    });
    let mut hash = DefaultHasher::new();
    VERSION.hash(&mut hash);
    path.hash(&mut hash);
    identity.len.hash(&mut hash);
    identity.modified.hash(&mut hash);
    #[cfg(unix)]
    identity.inode.hash(&mut hash);
    image.opacity.to_bits().hash(&mut hash);
    image.blur.to_bits().hash(&mut hash);
    // DefaultHasher::new is deterministic within a toolchain; this disposable
    // versioned cache does not require a cross-toolchain hash stability promise.
    format!("{:016x}.{EXTENSION}", hash.finish())
}

fn prepare(
    image: &ImageProperties,
    cache_root: &Path,
) -> Result<PreparedBackground, String> {
    let directory = cache_root.join(NAMESPACE);
    for attempt in 0..2 {
        let before = identity(image)?;
        let cache_path = directory.join(cache_key(image, &before));
        let cached = read_cache(&cache_path).ok();
        let cache_hit = cached.is_some();
        let pixels = match cached {
            Some(pixels) => Ok(pixels),
            None => decode(image),
        };
        // Check even after a decode failure: a partially replaced file gets one retry.
        if BackgroundFileIdentity::read(&image.path).as_ref() != Some(&before) {
            if attempt == 0 {
                continue;
            }
            return Err(format!(
                "background image '{}' changed during preparation",
                image.path
            ));
        }
        let pixels = pixels?;
        if !cache_hit {
            // Cache failures are strictly optional and never reject valid pixels.
            if let Err(error) = write_cache(&directory, &cache_path, &pixels) {
                tracing::debug!(target: "sugarloaf::background_loader", %error, "background cache write skipped");
            }
            prune_cache(&directory);
        }
        // Catch replacement during cache I/O as well. Old-identity cache entries
        // remain valid only for that old identity and can never hit the new source.
        if BackgroundFileIdentity::read(&image.path).as_ref() != Some(&before) {
            if attempt == 0 {
                continue;
            }
            return Err(format!(
                "background image '{}' changed during preparation",
                image.path
            ));
        }
        return Ok(PreparedBackground {
            identity: before,
            pixels,
            cache_hit,
        });
    }
    unreachable!()
}

fn rgba_len(width: u32, height: u32) -> io::Result<usize> {
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(invalid("background image dimensions exceed 16384 pixels"));
    }
    let bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .filter(|bytes| *bytes > 0 && *bytes <= MAX_BYTES)
        .ok_or_else(|| invalid("invalid or oversized RGBA dimensions"))?;
    usize::try_from(bytes).map_err(|_| invalid("RGBA size exceeds address space"))
}

fn decode(image: &ImageProperties) -> Result<BackgroundImagePixels, String> {
    let load = || -> Result<BackgroundImagePixels, String> {
        let (width, height) =
            image_rs::image_dimensions(&image.path).map_err(|e| e.to_string())?;
        rgba_len(width, height).map_err(|e| e.to_string())?;
        let mut reader = image_rs::ImageReader::open(&image.path)
            .map_err(|e| e.to_string())?
            .with_guessed_format()
            .map_err(|e| e.to_string())?;
        let mut limits = image_rs::Limits::default();
        limits.max_alloc = Some(MAX_BYTES);
        limits.max_image_width = Some(MAX_DIMENSION);
        limits.max_image_height = Some(MAX_DIMENSION);
        reader.limits(limits);
        let decoded = reader.decode().map_err(|e| e.to_string())?;
        rgba_len(decoded.width(), decoded.height()).map_err(|e| e.to_string())?;
        let mut decoded = super::blur_background_image(decoded.to_rgba8(), image.blur);
        let opacity = image.opacity.clamp(0.0, 1.0);
        if opacity < 1.0 {
            let opacity_byte = (opacity * 255.0).round() as u16;
            for pixel in decoded.pixels_mut() {
                pixel[3] = ((pixel[3] as u16 * opacity_byte) / 255) as u8;
            }
        }
        Ok(BackgroundImagePixels {
            width: decoded.width(),
            height: decoded.height(),
            pixels: decoded.into_raw(),
        })
    };
    load().map_err(|error| format!("background image '{}': {error}", image.path))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn checksum(bytes: &[u8]) -> u64 {
    let mut hash = DefaultHasher::new();
    bytes.hash(&mut hash);
    hash.finish()
}

fn read_cache(path: &Path) -> io::Result<BackgroundImagePixels> {
    let mut file = File::open(path)?;
    let mut header = [0u8; HEADER_LEN as usize];
    file.read_exact(&mut header)?;
    if &header[..8] != MAGIC
        || u32::from_le_bytes(header[8..12].try_into().unwrap()) != VERSION
    {
        return Err(invalid("bad background cache header"));
    }
    let width = u32::from_le_bytes(header[12..16].try_into().unwrap());
    let height = u32::from_le_bytes(header[16..20].try_into().unwrap());
    let length = u64::from_le_bytes(header[20..28].try_into().unwrap());
    let expected = u64::from_le_bytes(header[28..36].try_into().unwrap());
    let count = rgba_len(width, height)?;
    if length != count as u64 || file.metadata()?.len() != HEADER_LEN + length {
        return Err(invalid("bad background cache length"));
    }
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(count)
        .map_err(|_| invalid("background cache allocation failed"))?;
    pixels.resize(count, 0);
    file.read_exact(&mut pixels)?;
    let mut extra = [0];
    if file.read(&mut extra)? != 0 || checksum(&pixels) != expected {
        return Err(invalid("corrupt background cache payload"));
    }
    Ok(BackgroundImagePixels {
        width,
        height,
        pixels,
    })
}

fn write_cache(
    directory: &Path,
    path: &Path,
    pixels: &BackgroundImagePixels,
) -> io::Result<()> {
    let count = rgba_len(pixels.width, pixels.height)?;
    if count != pixels.pixels.len() {
        return Err(invalid("RGBA length mismatch"));
    }
    fs::create_dir_all(directory)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let (temp, mut file) = loop {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temp = directory.join(format!(".{}-{sequence}.tmp", std::process::id()));
        match options.open(&temp) {
            Ok(file) => break (temp, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        file.write_all(MAGIC)?;
        file.write_all(&VERSION.to_le_bytes())?;
        file.write_all(&pixels.width.to_le_bytes())?;
        file.write_all(&pixels.height.to_le_bytes())?;
        file.write_all(&(count as u64).to_le_bytes())?;
        file.write_all(&checksum(&pixels.pixels).to_le_bytes())?;
        file.write_all(&pixels.pixels)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn prune_cache(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut files = Vec::new();
    let mut total = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some(EXTENSION) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        total = total.saturating_add(metadata.len());
        files.push((metadata.modified().ok(), path, metadata.len()));
    }
    files.sort_by_key(|entry| entry.0);
    let mut count = files.len();
    for (_, path, length) in files {
        if count <= MAX_ENTRIES && total <= MAX_BYTES {
            break;
        }
        if fs::remove_file(path).is_ok() {
            count -= 1;
            total = total.saturating_sub(length);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "neoism-background-test-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn image(&self) -> ImageProperties {
            let path = self.0.join("source.png");
            image_rs::RgbaImage::from_pixel(4, 3, image_rs::Rgba([20, 40, 80, 200]))
                .save(&path)
                .unwrap();
            ImageProperties {
                path: path.to_str().unwrap().into(),
                opacity: 0.5,
                blur: 0.0,
                fit: Default::default(),
                position_x: 0.5,
                position_y: 0.5,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn cache_key_tracks_source_blur_opacity_not_layout() {
        let fixture = Fixture::new();
        let mut image = fixture.image();
        let original = identity(&image).unwrap();
        let key = cache_key(&image, &original);
        image.position_x = 0.0;
        image.position_y = 1.0;
        image.fit = crate::sugarloaf::primitives::BackgroundImageFit::Contain;
        assert_eq!(key, cache_key(&image, &original));
        image.blur = 2.0;
        assert_ne!(key, cache_key(&image, &original));
        image.blur = 0.0;
        image.opacity = 1.0;
        assert_ne!(key, cache_key(&image, &original));
        image.opacity = 0.5;
        let mut changed = original.clone();
        changed.len += 1;
        assert_ne!(key, cache_key(&image, &changed));
        image.path.push_str(".different");
        assert_ne!(key, cache_key(&image, &original));
    }

    #[test]
    fn cache_miss_hit_and_corrupt_fallback_preserve_pixels() {
        let fixture = Fixture::new();
        let image = fixture.image();
        let first = prepare(&image, &fixture.0).unwrap();
        assert!(!first.cache_hit);
        assert_eq!(first.pixels.pixels[3], 100);
        let second = prepare(&image, &fixture.0).unwrap();
        assert!(second.cache_hit);
        assert_eq!(first.pixels.pixels, second.pixels.pixels);
        let path = fixture
            .0
            .join(NAMESPACE)
            .join(cache_key(&image, &first.identity));
        fs::write(&path, b"corrupt").unwrap();
        let third = prepare(&image, &fixture.0).unwrap();
        assert!(!third.cache_hit);
        assert_eq!(first.pixels.pixels, third.pixels.pixels);
        assert!(prepare(&image, &fixture.0).unwrap().cache_hit);
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(&path, bytes).unwrap();
        assert!(!prepare(&image, &fixture.0).unwrap().cache_hit);
    }

    #[test]
    fn cache_rejects_forged_lengths_dimensions_and_trailing_bytes() {
        let fixture = Fixture::new();
        let image = fixture.image();
        let prepared = prepare(&image, &fixture.0).unwrap();
        let path = fixture
            .0
            .join(NAMESPACE)
            .join(cache_key(&image, &prepared.identity));
        let original = fs::read(&path).unwrap();
        let mut forged = original.clone();
        forged[20..28].copy_from_slice(&u64::MAX.to_le_bytes());
        fs::write(&path, forged).unwrap();
        assert!(read_cache(&path).is_err());
        let mut forged = original.clone();
        forged[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        fs::write(&path, forged).unwrap();
        assert!(read_cache(&path).is_err());
        let mut forged = original;
        forged.push(0);
        fs::write(&path, forged).unwrap();
        assert!(read_cache(&path).is_err());
    }

    #[test]
    fn pruning_bounds_entries_and_leaves_unrelated_files() {
        let fixture = Fixture::new();
        let directory = fixture.0.join(NAMESPACE);
        fs::create_dir_all(&directory).unwrap();
        let unrelated = directory.join("unrelated.txt");
        fs::write(&unrelated, b"keep").unwrap();
        for index in 0..(MAX_ENTRIES + 3) {
            fs::write(directory.join(format!("{index}.{EXTENSION}")), b"cache").unwrap();
        }
        prune_cache(&directory);
        let count = fs::read_dir(&directory)
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry.path().extension().and_then(|s| s.to_str()) == Some(EXTENSION)
            })
            .count();
        assert_eq!(count, MAX_ENTRIES);
        assert!(unrelated.exists());
    }

    #[test]
    fn source_replacement_invalidates_cached_pixels() {
        let fixture = Fixture::new();
        let image = fixture.image();
        let first = prepare(&image, &fixture.0).unwrap();
        let replacement = fixture.0.join("replacement.png");
        image_rs::RgbaImage::from_pixel(5, 2, image_rs::Rgba([1, 2, 3, 255]))
            .save(&replacement)
            .unwrap();
        // Windows rename does not replace an existing destination.
        fs::remove_file(&image.path).unwrap();
        fs::rename(replacement, &image.path).unwrap();
        let second = prepare(&image, &fixture.0).unwrap();
        assert!(!second.cache_hit);
        assert_ne!(first.identity, second.identity);
        assert_eq!((second.pixels.width, second.pixels.height), (5, 2));
    }

    #[test]
    fn bad_dimensions_and_unwritable_cache_are_safe() {
        assert!(rgba_len(0, 1).is_err());
        assert!(rgba_len(u32::MAX, u32::MAX).is_err());
        assert!(rgba_len(MAX_DIMENSION + 1, 1).is_err());
        assert!(rgba_len(1, MAX_DIMENSION + 1).is_err());
        assert!(rgba_len(8192, 8192).is_ok());
        let fixture = Fixture::new();
        let image = fixture.image();
        let not_a_directory = fixture.0.join("file");
        fs::write(&not_a_directory, b"file").unwrap();
        assert!(!prepare(&image, &not_a_directory).unwrap().cache_hit);
    }

    #[test]
    fn worker_coalesces_pending_requests_and_wakes_on_errors() {
        let fixture = Fixture::new();
        let image = fixture.image();
        let loader = BackgroundImageLoader::new().unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        loader
            .request(
                1,
                image.clone(),
                fixture.0.clone(),
                Arc::new(move || {
                    entered_tx.send(()).unwrap();
                    release_rx.lock().unwrap().recv().unwrap();
                }),
            )
            .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        // The first callback holds the worker outside every loader lock.
        let (wake_tx, wake_rx) = mpsc::channel();
        let wake: Wake = Arc::new(move || {
            wake_tx.send(()).unwrap();
        });
        loader
            .request(2, image.clone(), fixture.0.clone(), Arc::clone(&wake))
            .unwrap();
        loader.cancel_pending();
        assert!(loader.shared.0.lock().unwrap().pending.is_none());
        // Reuse the same worker after cancellation; only the newest queued request runs.
        loader
            .request(3, image.clone(), fixture.0.clone(), Arc::clone(&wake))
            .unwrap();
        let mut missing = image;
        missing.path.push_str(".missing");
        loader.request(4, missing, fixture.0.clone(), wake).unwrap();
        assert_eq!(loader.try_recv().unwrap().generation, 1);
        release_tx.send(()).unwrap();
        wake_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let result = loader.try_recv().unwrap();
        assert_eq!(result.generation, 4);
        assert!(result.prepared.is_err());
        assert!(loader.try_recv().is_none());
    }
}
