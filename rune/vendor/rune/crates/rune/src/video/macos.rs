//! AVFoundation: AVPlayer + AVPlayerItemVideoOutput for windows, AVAssetImageGenerator for
//! posters and headless frames, and a bare AVPlayer for audio.
use super::*;
use crate::platform::main_queue::on_main;
use crate::native_surface::Orient;
use crate::native_surface::macos::PixelBuffer;
use objc2_av_foundation::{
    AVAsset, AVAssetImageGenerator, AVMediaTypeAudio, AVMediaTypeVideo, AVPlayer, AVPlayerItem, AVPlayerItemStatus, AVPlayerItemVideoOutput,
    AVURLAsset,
};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo, kCGColorSpaceSRGB};
use objc2_core_media::{CMTime, CMTimeFlags};
use objc2_core_video::{
    CVGetCurrentHostTime, CVGetHostClockFrequency, kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey,
    kCVPixelBufferPixelFormatTypeKey, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
};
use objc2_foundation_video::{NSDictionary, NSString, NSURL};
use objc2_video::rc::Retained;
use objc2_video::runtime::AnyObject;
use objc2_video::{AnyThread, MainThreadMarker};
use std::time::Duration;

/// Paused players that keep a native player (and its decoder) for a quick resume. Older paused
/// players are released; a video's last frame stays on screen and resuming recreates it.
const PARKED_PLAYERS: usize = 4;
/// Posters never need more than this many pixels per side; headless frames keep full size.
const POSTER_MAX: f64 = 1920.0;
/// A seek that never lands stops requesting redraws (or ticks) after this long.
const SEEK_PATIENCE: Duration = Duration::from_secs(2);
/// A player item that never becomes ready fails after this long.
const LOAD_PATIENCE: Duration = Duration::from_secs(30);
/// How often playing audio checks for its end, loops and loading.
const AUDIO_TICK: Duration = Duration::from_millis(50);
/// An audio seek has landed once the player reports a time this close to the target.
const AUDIO_SEEK_SLOP: f64 = 0.25;

#[derive(Default)]
pub(crate) struct Native {
    /// Headless decodes reuse one generator.
    generator: Option<Retained<AVAssetImageGenerator>>,
    player: Option<Player>,
    posters: Arc<Mutex<Posters>>,
    poster_started: bool,
    /// A finished poster may replace the surface's frame: set by a poster request while no
    /// player has run (first frame, or a paused seek), cleared once a player makes frames.
    poster_due: bool,
    decoded: Decoded,
}

/// Audio AVFoundation can't read, decoded once on the load worker (or a headless probe) to a WAV
/// it can, which every player for the clip then opens.
#[cfg(feature = "ogg-audio")]
type Decoded = Arc<std::sync::OnceLock<Result<Option<foreign::Wav>, String>>>;
#[cfg(not(feature = "ogg-audio"))]
type Decoded = ();
// The generator is only used under the Shared mutex. Player objects are created, used and
// released on the main thread: pulls and audio ticks run there and `Player`'s drop hops to it.
unsafe impl Send for Native {}

#[derive(Default)]
struct Posters {
    want: Option<f64>,
    busy: bool,
    /// A video poster and its clip's facts, or (no frame) an audio clip's facts.
    done: Option<Result<(Option<ByteFrame>, Meta), String>>,
}

struct Player {
    // Some until dropped; `Drop` moves the objects to the main thread.
    objects: Option<Objects>,
    /// Rate last applied to AVPlayer (0 = paused).
    rate: f32,
    muted: bool,
    ready: bool,
    created: Instant,
    /// When the last seek started, and where to.
    seeking: Option<(Instant, f64)>,
    /// Video: how its buffers turn upright (they ignore the track's `preferredTransform`).
    orient: Orient,
    /// Video: the item time of the frame on the surface.
    shown: Option<f64>,
    /// A paused seek within the frame on screen: nothing to wait for, but the player may report
    /// the old position until it lands, so the handle's time holds.
    hold: Option<f64>,
}
struct Objects {
    player: Retained<AVPlayer>,
    item: Retained<AVPlayerItem>,
    /// None for audio.
    output: Option<Retained<AVPlayerItemVideoOutput>>,
}

impl Player {
    fn new(objects: Objects) -> Self {
        Self {
            objects: Some(objects),
            rate: 0.0,
            muted: false,
            ready: false,
            created: Instant::now(),
            seeking: None,
            orient: Orient::UP,
            shown: None,
            hold: None,
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let Some(objects) = self.objects.take() else { return };
        if MainThreadMarker::new().is_some() {
            unsafe { objects.player.pause() };
            return;
        }
        // The last handle went away on another thread: release on the main queue.
        struct Release(Objects);
        unsafe impl Send for Release {}
        let release = Release(objects);
        on_main(move || {
            let release = release; // Whole, so the closure is Send.
            unsafe { release.0.player.pause() };
        });
    }
}

/// Paused players holding a native player, oldest first.
static PARKED: Mutex<Vec<Weak<Shared>>> = Mutex::new(Vec::new());
/// Every live audio player, for the tick.
static AUDIO: Mutex<Vec<Weak<Shared>>> = Mutex::new(Vec::new());
/// Some audio clip wants ticking. Written on the main thread.
static AUDIO_BUSY: AtomicBool = AtomicBool::new(false);
/// The tick thread is running.
static TICKING: AtomicBool = AtomicBool::new(false);

/// The source's lowercase extension, if any.
pub(super) fn extension(source: &MediaSource) -> Option<String> {
    let name = match source {
        MediaSource::File(path) => path.file_name()?.to_string_lossy().into_owned(),
        MediaSource::Url(url) => {
            let path = url.split(['?', '#']).next().unwrap_or_default();
            // The last segment of the path, never the host.
            let path = path.split_once("://").map_or(path, |(_, rest)| rest.split_once('/').map_or("", |(_, path)| path));
            path.rsplit('/').next().unwrap_or_default().to_string()
        }
    };
    Some(name.rsplit_once('.')?.1.to_ascii_lowercase()).filter(|ext| !ext.is_empty())
}

/// Audio in a container AVFoundation can't read, which `ogg-audio` decodes instead.
fn foreign_audio(source: &MediaSource, kind: Kind) -> bool {
    cfg!(feature = "ogg-audio")
        && kind == Kind::Audio
        && extension(source).is_some_and(|ext| matches!(ext.as_str(), "ogg" | "oga" | "opus" | "webm" | "mka" | "mkv"))
}

/// AVFoundation has no Ogg, WebM or Matroska demuxer; fail at once rather than wait on a load
/// that can only fail.
fn unsupported_format(source: &MediaSource, kind: Kind) -> Option<String> {
    let ext = extension(source)?;
    (!foreign_audio(source, kind) && matches!(ext.as_str(), "ogg" | "oga" | "ogv" | "opus" | "spx" | "webm" | "mkv" | "mka"))
        .then(|| format!(".{ext} isn't supported on macOS (AVFoundation can't read it); use mp3, m4a, wav, mp4 or mov"))
}

pub(super) fn created(shared: &Arc<Shared>) {
    if let Some(e) = unsupported_format(&shared.source, shared.kind) {
        shared.ctl.lock().unwrap().fail(e);
        return;
    }
    if shared.kind == Kind::Audio {
        AUDIO.lock().unwrap().push(Arc::downgrade(shared));
        if playback_enabled() {
            load_audio(shared);
        }
    }
}

/// The window loop is starting: load audio clips made before it.
pub(super) fn enable() {
    let clips: Vec<Arc<Shared>> = AUDIO.lock().unwrap().iter().filter_map(Weak::upgrade).collect();
    for clip in clips {
        load_audio(&clip);
        on_main(move || drive(&clip));
    }
}

/// Read an audio clip's duration on a worker, so tiles know it before they play.
fn load_audio(shared: &Arc<Shared>) {
    let mut native = shared.native.lock().unwrap();
    if !native.poster_started && !shared.ctl.lock().unwrap().opened() {
        request_poster(shared, &mut native, 0.0);
    }
}

fn url(source: &MediaSource) -> Result<Retained<NSURL>, String> {
    match source {
        MediaSource::File(path) => {
            if !path.exists() {
                return Err(format!("no such file: {}", path.display()));
            }
            let path = std::path::absolute(path).unwrap_or_else(|_| path.clone());
            Ok(NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy())))
        }
        MediaSource::Url(url) => NSURL::URLWithString(&NSString::from_str(url)).ok_or_else(|| format!("invalid URL: {url}")),
    }
}

fn asset(source: &MediaSource, headers: &[(String, String)]) -> Result<Retained<AVURLAsset>, String> {
    let url = url(source)?;
    if headers.is_empty() {
        return Ok(unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) });
    }
    let names: Vec<_> = headers.iter().map(|(name, _)| NSString::from_str(name)).collect();
    let values: Vec<_> = headers.iter().map(|(_, value)| NSString::from_str(value)).collect();
    let fields = NSDictionary::from_slices(&names.iter().map(|n| &**n).collect::<Vec<_>>(), &values.iter().map(|v| &**v).collect::<Vec<_>>());
    // Long-standing and widely used, though missing from AVFoundation's headers.
    let key = NSString::from_str("AVURLAssetHTTPHeaderFieldsKey");
    let fields: &AnyObject = &fields;
    let options = NSDictionary::<NSString, AnyObject>::from_slices(&[&*key], &[fields]);
    Ok(unsafe { AVURLAsset::URLAssetWithURL_options(&url, Some(&options)) })
}

/// Opens a clip and reads its facts, blocking. Audio AVFoundation can't read plays from a WAV
/// decoded from it: up front for a foreign extension, or when its own asset has no audio track
/// and its bytes are Ogg or WebM.
fn open(source: &MediaSource, headers: &[(String, String)], kind: Kind, decoded: &Decoded) -> Result<(Retained<AVURLAsset>, Meta), String> {
    let opened = |asset: Retained<AVURLAsset>| meta(&asset, kind).map(|meta| (asset, meta));
    #[cfg(feature = "ogg-audio")]
    {
        let wav = |wav: &foreign::Wav| opened(asset(&MediaSource::File(wav.path().into()), &[])?);
        if foreign_audio(source, kind) {
            return match decoded.get_or_init(|| decode_foreign(source, headers, false)) {
                Ok(Some(decoded)) => wav(decoded),
                Ok(None) => Err("no Ogg or WebM audio".into()),
                Err(e) => Err(e.clone()),
            };
        }
        let own = opened(asset(source, headers)?);
        if own.is_err() && kind == Kind::Audio {
            match decoded.get_or_init(|| decode_foreign(source, headers, true)) {
                Ok(Some(decoded)) => return wav(decoded),
                // Ogg or WebM that didn't decode: say why, not that AVFoundation found nothing.
                Err(e) => return Err(e.clone()),
                Ok(None) => {}
            }
        }
        own
    }
    #[cfg(not(feature = "ogg-audio"))]
    {
        let () = decoded;
        opened(asset(source, headers)?)
    }
}

/// The asset a player opens: the decoded WAV once there is one.
fn player_asset(source: &MediaSource, headers: &[(String, String)], decoded: &Decoded) -> Result<Retained<AVURLAsset>, String> {
    #[cfg(feature = "ogg-audio")]
    if let Some(Ok(Some(wav))) = decoded.get() {
        return asset(&MediaSource::File(wav.path().into()), &[]);
    }
    #[cfg(not(feature = "ogg-audio"))]
    let () = decoded;
    asset(source, headers)
}

/// Fetches a clip and decodes it to a WAV. When sniffing, bytes that aren't Ogg or WebM give
/// `None`, and a fetch that fails gives `None` too, leaving AVFoundation's error to report.
#[cfg(feature = "ogg-audio")]
fn decode_foreign(source: &MediaSource, headers: &[(String, String)], sniff: bool) -> Result<Option<foreign::Wav>, String> {
    let bytes = match fetch(source, headers) {
        Ok(bytes) => bytes,
        Err(_) if sniff => return Ok(None),
        Err(e) => return Err(e),
    };
    if sniff && !foreign::sniff(&bytes) {
        return Ok(None);
    }
    foreign::Wav::decode(bytes).map(Some)
}

/// A clip's bytes: a file read, or a blocking fetch with its request headers.
#[cfg(feature = "ogg-audio")]
#[allow(deprecated)] // NSURLConnection's synchronous load is what a worker or headless run wants.
fn fetch(source: &MediaSource, headers: &[(String, String)]) -> Result<Vec<u8>, String> {
    use objc2_foundation_video::{NSHTTPURLResponse, NSMutableURLRequest, NSURLConnection};
    if let MediaSource::File(path) = source {
        return std::fs::read(path).map_err(|e| format!("can't read {}: {e}", path.display()));
    }
    let request = NSMutableURLRequest::requestWithURL(&*url(source)?);
    for (name, value) in headers {
        request.setValue_forHTTPHeaderField(Some(&NSString::from_str(value)), &NSString::from_str(name));
    }
    let mut response = None;
    let data = NSURLConnection::sendSynchronousRequest_returningResponse_error(&request, Some(&mut response))
        .map_err(|e| e.localizedDescription().to_string())?;
    let status = response.and_then(|r| r.downcast::<NSHTTPURLResponse>().ok()).map(|r| r.statusCode());
    if let Some(status) = status.filter(|s| !(200..300).contains(s)) {
        return Err(format!("HTTP {status}"));
    }
    Ok(data.to_vec())
}

fn seconds(t: CMTime) -> f64 {
    let s = unsafe { t.seconds() };
    if s.is_finite() { s } else { 0.0 }
}
fn cm(seconds: f64) -> CMTime {
    unsafe { CMTime::with_seconds(seconds, 600_000) }
}
fn zero() -> CMTime {
    CMTime { value: 0, timescale: 1, flags: CMTimeFlags::Valid, epoch: 0 }
}

/// Blocks until the asset's tracks load (a network fetch for URLs).
#[allow(deprecated)] // The synchronous loaders are what a worker or headless run wants.
fn meta(asset: &AVURLAsset, kind: Kind) -> Result<Meta, String> {
    let duration = || seconds(unsafe { asset.duration() });
    if kind == Kind::Audio {
        let audio = unsafe { AVMediaTypeAudio }.ok_or("AVMediaTypeAudio unavailable")?;
        if unsafe { asset.tracksWithMediaType(audio) }.firstObject().is_none() {
            return Err("no playable audio track (unreachable, or a format AVFoundation can't read)".into());
        }
        return Ok(Meta { duration: duration(), size: None, frame: 1.0 / 30.0 });
    }
    let video = unsafe { AVMediaTypeVideo }.ok_or("AVMediaTypeVideo unavailable")?;
    let tracks = unsafe { asset.tracksWithMediaType(video) };
    let Some(track) = tracks.firstObject() else {
        return Err("no playable video track".into());
    };
    let size = unsafe { track.naturalSize() };
    let t = unsafe { track.preferredTransform() };
    let w = (size.width * t.a + size.height * t.c).abs().round();
    let h = (size.width * t.b + size.height * t.d).abs().round();
    let min = seconds(unsafe { track.minFrameDuration() });
    let nominal = unsafe { track.nominalFrameRate() } as f64;
    let frame = if min > 0.0 { min } else if nominal > 0.0 { 1.0 / nominal } else { 1.0 / 30.0 };
    if w < 1.0 || h < 1.0 {
        return Err("video track has no size".into());
    }
    Ok(Meta { duration: duration(), size: Some((w as u32, h as u32)), frame })
}

/// How the first video track's buffers turn upright. Blocks until tracks load; free once an
/// item using the asset is ready to play.
#[allow(deprecated)]
fn orient(asset: &AVAsset) -> Orient {
    let Some(video) = (unsafe { AVMediaTypeVideo }) else { return Orient::UP };
    let Some(track) = unsafe { asset.tracksWithMediaType(video) }.firstObject() else { return Orient::UP };
    let t = unsafe { track.preferredTransform() };
    Orient::from_transform(t.a, t.b, t.c, t.d)
}

fn generator(asset: &AVURLAsset, max: Option<f64>) -> Retained<AVAssetImageGenerator> {
    let g = unsafe { AVAssetImageGenerator::assetImageGeneratorWithAsset(asset) };
    unsafe {
        g.setAppliesPreferredTrackTransform(true);
        g.setRequestedTimeToleranceBefore(zero());
        g.setRequestedTimeToleranceAfter(zero());
        if let Some(max) = max {
            g.setMaximumSize(CGSize::new(max, max));
        }
    }
    g
}

#[allow(deprecated)] // copyCGImageAtTime is the synchronous form.
fn image(generator: &AVAssetImageGenerator, at: f64) -> Result<ByteFrame, String> {
    let image = unsafe { generator.copyCGImageAtTime_actualTime_error(cm(at), std::ptr::null_mut()) }
        .map_err(|e| e.localizedDescription().to_string())?;
    bgra(&image)
}

/// Draws a decoded image into sRGB BGRA bytes (opaque, so premultiplied equals straight).
fn bgra(image: &CGImage) -> Result<ByteFrame, String> {
    let (w, h) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
    if w == 0 || h == 0 {
        return Err("decoded an empty frame".into());
    }
    let mut bytes = vec![0u8; w * h * 4];
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }));
    let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
    let context = unsafe { CGBitmapContextCreate(bytes.as_mut_ptr().cast(), w, h, 8, w * 4, space.as_deref(), info) }
        .ok_or("CGBitmapContextCreate failed")?;
    CGContext::draw_image(Some(&context), CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64)), Some(image));
    drop(context);
    Ok(ByteFrame {
        width: w as u32,
        height: h as u32,
        format: Format::Bgra,
        color: ColorInfo { matrix: crate::native_surface::Matrix::Bt709, transfer: crate::native_surface::Transfer::Srgb },
        planes: [bytes, Vec::new()],
    })
}

pub(super) fn probe(native: &mut Native, source: &MediaSource, headers: &[(String, String)], kind: Kind) -> Result<Meta, String> {
    let (asset, meta) = open(source, headers, kind, &native.decoded)?;
    if kind == Kind::Video {
        native.generator = Some(generator(&asset, None));
    }
    Ok(meta)
}

pub(super) fn decode(native: &mut Native, source: &MediaSource, headers: &[(String, String)], at: f64) -> Result<ByteFrame, String> {
    if native.generator.is_none() {
        let asset = asset(source, headers)?;
        native.generator = Some(generator(&asset, None));
    }
    image(native.generator.as_ref().unwrap(), at)
}

/// A clip's frame at `at` upright (as posters and headless runs show it), and as its buffer
/// stores it (as the player hands it out) with how that turns upright.
#[cfg(test)]
pub(crate) fn test_frames(source: &MediaSource, at: f64) -> Result<(ByteFrame, ByteFrame, Orient), String> {
    let asset = asset(source, &[])?;
    let raw = generator(&asset, None);
    unsafe { raw.setAppliesPreferredTrackTransform(false) };
    Ok((image(&generator(&asset, None), at)?, image(&raw, at)?, orient(&asset)))
}

/// Whether a poster decode has landed, and with a picture.
#[cfg(test)]
pub(crate) fn test_poster(shared: &Shared) -> Option<bool> {
    let native = shared.native.lock().unwrap();
    let posters = native.posters.lock().unwrap();
    posters.done.as_ref().map(|d| matches!(d, Ok((Some(_), _))))
}

/// An app warming the page it shows next: decode the clip's first frame now, so the first frame
/// that draws it already has it. Nothing once a poster or player has started, or for audio.
pub(super) fn preload(shared: &Arc<Shared>) {
    let mut native = shared.native.lock().unwrap();
    let c = shared.ctl.lock().unwrap();
    if shared.kind == Kind::Audio || native.player.is_some() || native.poster_started || c.failed.is_some() {
        return;
    }
    let at = c.time;
    drop(c);
    native.poster_due = true;
    request_poster(shared, &mut native, at);
}

/// One worker per clip opens it and decodes the newest requested poster (none for audio),
/// then wakes its windows, or for audio drives it on the main queue.
fn request_poster(shared: &Arc<Shared>, native: &mut Native, at: f64) {
    native.poster_started = true;
    let mut slot = native.posters.lock().unwrap();
    slot.want = Some(at);
    if slot.busy {
        return;
    }
    slot.busy = true;
    let posters = native.posters.clone();
    let clip = Arc::downgrade(shared);
    let (source, headers, kind, decoded) = (shared.source.clone(), shared.headers.clone(), shared.kind, native.decoded.clone());
    std::thread::Builder::new()
        .name("rune-media-poster".into())
        .spawn(move || {
            let mut opened: Option<(Option<Retained<AVAssetImageGenerator>>, Meta)> = None;
            loop {
                let Some(at) = ({
                    let mut slot = posters.lock().unwrap();
                    let want = slot.want.take();
                    if want.is_none() {
                        slot.busy = false;
                    }
                    want
                }) else {
                    return;
                };
                if clip.strong_count() == 0 {
                    return;
                }
                // AVFoundation autoreleases; this thread has no pool of its own.
                let result = objc2_video::rc::autoreleasepool(|_| {
                    if opened.is_none() {
                        let (asset, meta) = open(&source, &headers, kind, &decoded)?;
                        opened = Some(((kind == Kind::Video).then(|| generator(&asset, Some(POSTER_MAX))), meta));
                    }
                    let (generator, meta) = opened.as_ref().unwrap();
                    let frame = match generator {
                        Some(g) => Some(image(g, meta.frame_time(meta.frame_index(at)))?),
                        None => None,
                    };
                    Ok((frame, *meta))
                });
                posters.lock().unwrap().done = Some(result);
                let Some(clip) = clip.upgrade() else { return };
                match kind {
                    Kind::Video => clip.wake_windows(),
                    Kind::Audio => on_main(move || drive(&clip)),
                }
            }
        })
        .expect("spawn media poster worker");
}

fn create_player(source: &MediaSource, headers: &[(String, String)], kind: Kind, decoded: &Decoded, mtm: MainThreadMarker) -> Result<Objects, String> {
    let asset = player_asset(source, headers, decoded)?;
    let item = unsafe { AVPlayerItem::playerItemWithAsset(&asset, mtm) };
    let output = (kind == Kind::Video).then(|| {
        let empty = CFDictionary::<CFString, CFType>::from_slices(&[], &[]);
        let format = CFNumber::new_i32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange as i32);
        let attributes = unsafe {
            CFDictionary::<CFString, CFType>::from_slices(
                &[kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferMetalCompatibilityKey],
                &[format.as_ref(), AsRef::<CFType>::as_ref(&*empty), CFBoolean::new(true).as_ref()],
            )
        };
        // CFDictionary and NSDictionary are toll-free bridged.
        let attributes = unsafe { &*(CFRetained::as_ptr(&attributes).as_ptr() as *const NSDictionary<NSString, AnyObject>) };
        unsafe {
            let output = AVPlayerItemVideoOutput::initWithPixelBufferAttributes(AVPlayerItemVideoOutput::alloc(), Some(attributes));
            output.setSuppressesPlayerRendering(true);
            item.addOutput(&output);
            output
        }
    });
    let player = unsafe { AVPlayer::playerWithPlayerItem(Some(&item), mtm) };
    Ok(Objects { player, item, output })
}

/// Seconds on the host clock (`CACurrentMediaTime`'s) for an `Instant`.
fn host_seconds(at: Instant) -> f64 {
    let now = Instant::now();
    let host = CVGetCurrentHostTime() as f64 / CVGetHostClockFrequency();
    if at >= now { host + (at - now).as_secs_f64() } else { host - (now - at).as_secs_f64() }
}

/// Bring the player in line with the handle: mute, seek, rate. Seeks wait for the item to be
/// ready, which drops earlier ones.
fn reconcile(p: &mut Player, c: &mut Control, now: Instant) {
    let Some(o) = &p.objects else { return };
    unsafe {
        if p.muted != c.muted {
            o.player.setMuted(c.muted);
            p.muted = c.muted;
        }
        if p.ready
            && let Some(t) = c.seek.take()
        {
            o.player.seekToTime_toleranceBefore_toleranceAfter(cm(t), zero(), zero());
            // A paused seek within the frame on screen brings no new frame to wait for.
            let same_frame = !c.playing && c.meta.zip(p.shown).is_some_and(|(m, shown)| m.frame_index(shown) == m.frame_index(t));
            if same_frame {
                p.hold = Some(t);
            } else {
                p.seeking = Some((now, t));
                p.hold = None;
            }
        }
        if p.ready {
            let rate = if c.playing { c.rate } else { 0.0 };
            if p.rate != rate {
                if rate > 0.0 { o.player.setRate(rate) } else { o.player.pause() }
                p.rate = rate;
            }
        }
    }
}

/// Applies a handle edit at once on the main thread, so a paused, undrawn video stops (and goes
/// quiet) without waiting for a frame that draws it. Edits from other threads hop to it.
pub(super) fn apply(shared: &Arc<Shared>) {
    if !playback_enabled() || shared.ctl.lock().unwrap().failed.is_some() {
        return;
    }
    if MainThreadMarker::new().is_none() {
        let clip = Arc::downgrade(shared);
        on_main(move || {
            if let Some(clip) = clip.upgrade() {
                apply(&clip);
            }
        });
        return;
    }
    if shared.kind == Kind::Audio {
        drive(shared);
        return;
    }
    // Already being pulled further up this stack; that pull reconciles.
    let Ok(mut native) = shared.native.try_lock() else { return };
    let Some(p) = native.player.as_mut() else { return };
    let playing = {
        let mut c = shared.ctl.lock().unwrap();
        reconcile(p, &mut c, Instant::now());
        c.playing
    };
    drop(native);
    // A parked player that plays again must not be released.
    if playing {
        unpark(shared);
    }
}

/// Audio, on the main thread: what the player reports now, while it plays.
pub(super) fn live_time(shared: &Shared) -> Option<f64> {
    if shared.kind != Kind::Audio || MainThreadMarker::new().is_none() {
        return None;
    }
    let native = shared.native.try_lock().ok()?;
    let p = native.player.as_ref().filter(|p| p.ready && p.rate > 0.0 && p.seeking.is_none())?;
    let time = seconds(unsafe { p.objects.as_ref()?.player.currentTime() });
    let c = shared.ctl.lock().unwrap();
    (c.playing && !c.ended).then(|| c.duration.map_or(time, |d| time.clamp(0.0, d)))
}

/// Takes a finished poster load: the clip's facts and, for video, its poster frame.
fn take_poster(native: &mut Native, c: &mut Control, surface: Option<&NativeSurface>) {
    let Some(done) = native.posters.lock().unwrap().done.take() else { return };
    match done {
        Ok((bytes, meta)) => {
            if !c.opened() {
                c.open(Ok(meta));
            }
            // A late poster never covers a frame a player made.
            if let (Some(bytes), Some(surface), None, true) = (bytes, surface, &native.player, native.poster_due) {
                let id = c.next_frame_id();
                if let Err(e) = surface.push_bytes(bytes, id) {
                    c.fail(e);
                }
            }
        }
        Err(e) => c.fail(e),
    }
}

/// Fails the clip if its item failed or never loaded; marks it ready once it can play.
/// Returns false when the player is gone.
fn check_item(native: &mut Native, c: &mut Control, now: Instant) -> bool {
    let p = native.player.as_mut().unwrap();
    let o = p.objects.as_ref().unwrap();
    let status = unsafe { o.item.status() };
    let failure = if status == AVPlayerItemStatus::Failed {
        Some(unsafe { o.item.error() }.map_or_else(|| "playback failed".into(), |e| e.localizedDescription().to_string()))
    } else if status != AVPlayerItemStatus::ReadyToPlay && now - p.created > LOAD_PATIENCE {
        Some(format!("timed out loading after {} s", LOAD_PATIENCE.as_secs()))
    } else {
        None
    };
    if let Some(e) = failure {
        c.fail(e);
        native.player = None;
        return false;
    }
    if status == AVPlayerItemStatus::ReadyToPlay && !p.ready {
        p.ready = true;
        if o.output.is_some() {
            let asset = unsafe { o.item.asset() };
            p.orient = orient(&asset);
        }
        if !c.opened() {
            let size = unsafe { o.item.presentationSize() };
            let duration = seconds(unsafe { o.item.duration() });
            let size = (size.width >= 1.0 && size.height >= 1.0).then(|| (size.width.round() as u32, size.height.round() as u32));
            c.open(Ok(Meta { duration, size, frame: 1.0 / 30.0 }));
        }
    }
    true
}

/// Makes a player for a clip that should play; returns false when there is none to drive.
fn ensure_player(shared: &Arc<Shared>, native: &mut Native, c: &mut Control, mtm: MainThreadMarker) -> bool {
    if native.player.is_some() {
        return true;
    }
    // Audio opens on the load worker first, which may find it needs decoding (an Ogg clip at a
    // URL with no extension); the worker then drives the clip again, starting a queued play.
    if shared.kind == Kind::Audio && !c.opened() {
        if !native.poster_started {
            request_poster(shared, native, 0.0);
        }
        return false;
    }
    match create_player(&shared.source, &shared.headers, shared.kind, &native.decoded, mtm) {
        Ok(objects) => {
            // A fresh player starts at the handle's position.
            if c.time > 0.0 && c.seek.is_none() {
                c.seek = Some(c.time);
            }
            native.player = Some(Player::new(objects));
            // From here on, frames come from players: an older poster load must not land.
            native.poster_started = true;
            native.poster_due = false;
            true
        }
        Err(e) => {
            c.fail(e);
            false
        }
    }
}

/// Folds the player's position into the state: seeks landing, the end, loops.
fn follow(p: &mut Player, c: &mut Control, now: Instant, frame_time: Option<f64>) {
    let o = p.objects.as_ref().unwrap();
    let current = seconds(unsafe { o.player.currentTime() });
    if frame_time.is_none() && p.seeking.is_some_and(|(_, to)| (current - to).abs() < AUDIO_SEEK_SLOP) {
        p.seeking = None;
    }
    if c.playing || p.hold.is_some_and(|to| (current - to).abs() < 1e-3) {
        p.hold = None;
    }
    // Seeks land asynchronously: until then the player still reports the old position, which
    // may be the end.
    let seeking = p.seeking.is_some() || p.hold.is_some();
    let stopped = p.rate > 0.0 && unsafe { o.player.rate() } == 0.0;
    let at_end = !seeking && (c.duration.is_some_and(|d| current >= d - 1e-3) || stopped);
    let time = if seeking {
        c.time
    } else if c.playing && !at_end {
        frame_time.unwrap_or(current).max(0.0)
    } else {
        current
    };
    if c.observe(time, at_end) {
        unsafe {
            o.player.seekToTime_toleranceBefore_toleranceAfter(zero(), zero(), zero());
            o.player.setRate(c.rate);
        }
        p.rate = c.rate;
        p.seeking = Some((now, 0.0));
        // A loop is news for anything showing play counts or progress from zero.
        c.generation += 1;
    } else if !c.playing {
        p.rate = 0.0;
    }
}

pub(super) fn pull(shared: &Arc<Shared>, surface: &NativeSurface, target: Instant) -> bool {
    let now = Instant::now();
    let mut native = shared.native.lock().unwrap();
    let mut c = shared.ctl.lock().unwrap();

    take_poster(&mut native, &mut c, Some(surface));
    if c.failed.is_some() {
        native.player = None;
        return false;
    }

    let Some(mtm) = MainThreadMarker::new() else { return false };
    if native.player.is_none() && !c.playing {
        // Poster mode: no player, no decoder session.
        let want = c.seek.take().or_else(|| (!native.poster_started).then_some(c.time));
        if let Some(at) = want {
            drop(c);
            native.poster_due = true;
            request_poster(shared, &mut native, at);
        }
        return false;
    }
    let fresh = native.player.is_none();
    if !ensure_player(shared, &mut native, &mut c, mtm) {
        return false;
    }
    if fresh {
        unpark(shared);
    }
    if !check_item(&mut native, &mut c, now) {
        return false;
    }

    let p = native.player.as_mut().unwrap();
    reconcile(p, &mut c, now);
    if p.ready {
        let o = p.objects.as_ref().unwrap();
        let output = o.output.as_ref().unwrap();
        let item_time = unsafe { output.itemTimeForHostTime(host_seconds(target)) };
        let mut shown = item_time;
        if unsafe { output.hasNewPixelBufferForItemTime(item_time) }
            && let Some(buffer) = unsafe { output.copyPixelBufferForItemTime_itemTimeForDisplay(item_time, &mut shown) }
        {
            let id = c.next_frame_id();
            let orient = p.orient;
            // SAFETY: AVPlayerItemVideoOutput hands out finished buffers it no longer writes.
            let pushed = unsafe { PixelBuffer::from_raw(Retained::as_ptr(&buffer) as *mut _) }
                .and_then(|b| surface.push_oriented(b, id, orient));
            if let Err(e) = pushed {
                c.fail(e);
                native.player = None;
                return false;
            }
            p.shown = Some(seconds(shown));
            p.seeking = None;
        }
        follow(p, &mut c, now, Some(seconds(item_time)));
    }
    finish(shared, native, c, now)
}

/// Seek patience and parking, shared by video pulls and audio drives. Returns whether the
/// clip is playing, seeking or loading (a seek waits for it).
fn finish(shared: &Arc<Shared>, mut native: MutexGuard<'_, Native>, c: MutexGuard<'_, Control>, now: Instant) -> bool {
    let p = native.player.as_mut().unwrap();
    if p.seeking.is_some_and(|(since, _)| now - since > SEEK_PATIENCE) {
        p.seeking = None;
    }
    let active = c.playing || p.seeking.is_some() || !p.ready;
    let park = p.ready && !active;
    drop(c);
    drop(native);
    if park { self::park(shared) } else { unpark(shared) }
    active
}

/// Audio, on the main thread: take a finished load, make or update the player, follow it to
/// the end, tell the app about changes, and keep the tick going while it plays or seeks.
pub(super) fn drive(shared: &Arc<Shared>) {
    if step_audio(shared) {
        AUDIO_BUSY.store(true, Ordering::SeqCst);
        start_ticking();
    }
    shared.settle(true);
}

/// Returns whether the clip needs ticking.
fn step_audio(shared: &Arc<Shared>) -> bool {
    let Some(mtm) = MainThreadMarker::new() else { return false };
    let now = Instant::now();
    // Already being driven further up this stack (a waker that edits the clip).
    let Ok(mut native) = shared.native.try_lock() else { return true };
    let mut c = shared.ctl.lock().unwrap();
    take_poster(&mut native, &mut c, None);
    if c.failed.is_some() {
        native.player = None;
        return false;
    }
    if native.player.is_none() && !c.playing {
        return false;
    }
    let fresh = native.player.is_none();
    if !ensure_player(shared, &mut native, &mut c, mtm) {
        return false;
    }
    if fresh {
        unpark(shared);
    }
    if !check_item(&mut native, &mut c, now) {
        return false;
    }
    let p = native.player.as_mut().unwrap();
    reconcile(p, &mut c, now);
    if !p.ready {
        // Tick until it loads (or times out), so a pending seek or play lands.
        return true;
    }
    follow(p, &mut c, now, None);
    finish(shared, native, c, now)
}

/// Main thread: drive every audio clip that might be playing.
fn tick() {
    let clips: Vec<Arc<Shared>> = {
        let mut audio = AUDIO.lock().unwrap();
        audio.retain(|w| w.strong_count() > 0);
        audio.iter().filter_map(Weak::upgrade).collect()
    };
    let mut busy = false;
    for clip in &clips {
        busy |= step_audio(clip);
        clip.settle(true);
    }
    AUDIO_BUSY.store(busy, Ordering::SeqCst);
}

/// A thread that posts `tick` to the main queue every `AUDIO_TICK` while audio is busy.
fn start_ticking() {
    // SeqCst throughout: each side stores one flag, then loads the other.
    if TICKING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("rune-audio-tick".into())
        .spawn(|| {
            loop {
                std::thread::sleep(AUDIO_TICK);
                if !AUDIO_BUSY.load(Ordering::SeqCst) {
                    TICKING.store(false, Ordering::SeqCst);
                    // Something started between the check and the store: keep going if no
                    // other ticker took over.
                    if !AUDIO_BUSY.load(Ordering::SeqCst) || TICKING.swap(true, Ordering::SeqCst) {
                        return;
                    }
                }
                on_main(tick);
            }
        })
        .expect("spawn audio tick");
}

fn unpark(shared: &Arc<Shared>) {
    PARKED.lock().unwrap().retain(|w| w.strong_count() > 0 && !std::ptr::eq(w.as_ptr(), Arc::as_ptr(shared)));
}

/// Keep at most `PARKED_PLAYERS` paused players; release the oldest. Main thread only.
fn park(shared: &Arc<Shared>) {
    for clip in parked_with(shared) {
        // A video's last frame stays on its surface; resuming recreates the player.
        let player = {
            let mut native = clip.native.lock().unwrap();
            let player = native.player.take();
            // A video that never showed a frame shows a poster instead.
            if clip.kind == Kind::Video && player.as_ref().is_some_and(|p| p.shown.is_none()) {
                native.poster_started = false;
            }
            player
        };
        drop(player);
    }
}

/// Parks `shared` and returns the clips to release: the oldest past `PARKED_PLAYERS`. A clip
/// that plays again (before its next pull unparks it) leaves the list and is never released.
pub(super) fn parked_with(shared: &Arc<Shared>) -> Vec<Arc<Shared>> {
    // A clip being edited on another thread right now counts as playing.
    let playing = |clip: &Shared| clip.ctl.try_lock().map_or(true, |c| c.playing);
    let mut parked = PARKED.lock().unwrap();
    parked.retain(|w| w.upgrade().is_some_and(|clip| Arc::ptr_eq(&clip, shared) || !playing(&clip)));
    if !parked.iter().any(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(shared))) {
        parked.push(Arc::downgrade(shared));
    }
    let excess = parked.len().saturating_sub(PARKED_PLAYERS);
    parked.drain(..excess).filter_map(|w| w.upgrade()).collect()
}
