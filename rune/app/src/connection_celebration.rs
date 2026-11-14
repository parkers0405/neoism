//! Local-only, bounded one-shot artwork; never included in release assets.
use crate::{app::App, agent::activity_flames as flames};
use image::{AnimationDecoder, ImageDecoder};
use rune::{El, Live, Pixels, el};
use std::{io::{BufReader, Read}, sync::mpsc, time::{Duration, Instant}};
use std::sync::atomic::{AtomicBool, Ordering};
static DECODING: AtomicBool = AtomicBool::new(false);

struct Frame { end: f32, rgba: Vec<u8> }
struct Clip { frames: Vec<Frame>, width: u32, height: u32 }
impl Clip {
    fn duration(&self) -> f32 { self.frames.last().unwrap().end }
    fn index(&self, t: f32) -> usize {
        self.frames.partition_point(|f| f.end <= t).min(self.frames.len() - 1)
    }
}

fn decode(path: &std::path::Path) -> Option<Clip> {
    // Bound encoded bytes before constructing the decoder, then dimensions before frames.
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 8 * 1024 * 1024 { return None; }
    let mut decoder = image::codecs::webp::WebPDecoder::new(BufReader::new(std::io::Cursor::new(bytes))).ok()?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || width > 512 || height > 512 { return None; }
    // image-webp defaults to no disposal background, which leaves pixels outside the
    // next frame's rectangle on its canvas. Explicit transparent disposal is essential
    // for an overlay. Keep the encoded per-frame blend/dispose flags and soft alpha.
    if decoder.has_animation() { decoder.set_background_color(image::Rgba([0, 0, 0, 0])).ok()?; }
    let mut clip = Clip { frames: Vec::new(), width, height };
    let mut end = 0.0;
    for frame in decoder.into_frames() {
        if clip.frames.len() >= 180 { return None; }
        let frame = frame.ok()?;
        let (n, d) = frame.delay().numer_denom_ms();
        end += (n as f32 / d.max(1) as f32 / 1000.0).max(0.01);
        if end > 8.0 || (clip.frames.len() + 1) * width as usize * height as usize * 4 > 96 * 1024 * 1024 { return None; }
        clip.frames.push(Frame { end, rgba: frame.into_buffer().into_raw() });
    }
    (!clip.frames.is_empty()).then_some(clip)
}

#[derive(Default)]
pub struct Celebration { playing: Option<Playing> }
struct Playing {
    start: Instant,
    reduced: bool,
    loading: Option<mpsc::Receiver<Option<Clip>>>,
    clip: Option<Clip>,
    image: Pixels,
    frame: Option<usize>,
    fire_frame: Option<(u32, [u32; 5])>,
    fire: [Pixels; 9],
    opacity: Live<f32>,
}
impl Celebration {
    #[cfg(test)]
    pub(crate) fn active(&self) -> bool { self.playing.is_some() }
    /// Called only after consuming a user connection job's verified success.
    pub fn confirmed(&mut self) {
        let reduced = crate::goal_celebration::plain() || crate::glass_cursor::reduce_motion()
            || std::env::var("NEOISM_REDUCED_MOTION").as_deref() == Ok("1");
        let loading = if reduced || DECODING.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() { None } else {
            let (tx, rx) = mpsc::sync_channel(1);
            std::thread::spawn(move || {
                struct Release;
                impl Drop for Release { fn drop(&mut self) { DECODING.store(false, Ordering::Release); } }
                let _release = Release;
                let path = std::env::var_os("NEOISM_CONNECTION_CELEBRATION_ASSET").map(std::path::PathBuf::from)
                    .unwrap_or_else(|| crate::config::dir().join("assets/fist-bump.webp"));
                let _ = tx.send(decode(&path));
            });
            Some(rx)
        };
        self.playing = Some(Playing { start: Instant::now(), reduced, loading, clip: None,
            image: Pixels::new(), frame: None, fire_frame: None,
            fire: std::array::from_fn(|_| Pixels::new()), opacity: Live::new(1.0) });
    }
    pub fn cancel(&mut self) { self.playing = None; }
}

pub fn poll(app: &mut App, tx: &mut rune::Tx) -> bool {
    let Some(p) = &mut app.connection_celebration.playing else { return false };
    let mut changed = false;
    if let Some(rx) = &p.loading {
        match rx.try_recv() {
            Ok(clip) => { p.clip = clip; p.loading = None; p.start = Instant::now(); changed = true; }
            Err(mpsc::TryRecvError::Disconnected) => { p.loading = None; p.start = Instant::now(); changed = true; }
            Err(mpsc::TryRecvError::Empty) => {
                if p.start.elapsed().as_secs_f32() < 2.0 { tx.wake_after(Duration::from_millis(16)); return false; }
                p.loading = None; p.start = Instant::now(); changed = true;
            }
        }
    }
    let t = p.start.elapsed().as_secs_f32();
    let life = p.clip.as_ref().map_or(1.8, |c| c.duration() + 0.6);
    if t >= life { app.connection_celebration.cancel(); return true; }
    p.opacity.set(if p.reduced { 1.0 } else { ((life - t) / 0.45).min(1.0) });
    if let Some(clip) = &p.clip {
        let index = clip.index(t);
        if p.frame != Some(index) {
            p.image.set(clip.width, clip.height, clip.frames[index].rgba.clone());
            p.frame = Some(index);
        }
    }
    let colors = crate::ui::fire::colors(app.pal);
    let frame = ((t * 16.0) as u32, colors);
    if !p.reduced && app.wordmark.fire_enabled && p.fire_frame != Some(frame) {
        p.fire_frame = Some(frame);
        for (i, c) in "Connected".chars().enumerate() {
            if let Some(fire) = flames::glyph(c) { p.fire[i].set(40, 88, flames::rgba(fire, t, colors)); }
        }
    }
    tx.wake_after(if p.reduced { Duration::from_secs_f32(life-t) } else { Duration::from_millis(16) });
    changed
}

fn geometry(window: [f32; 2], scale: f32, hands: bool) -> (f32, f32, f32, f32) {
    let size = (28.0 * scale).min((window[0]-32.0).max(1.0)/11.0)
        .min((window[1]-32.0).max(1.0)/if hands { 15.0 } else { 4.0 });
    let image = if hands { size * 10.0 } else { 0.0 };
    let height = size * 4.0 + image;
    ((window[0]-11.0*size)/2.0, (window[1]-height)/2.0, size, image)
}

pub fn view(app: &App) -> El {
    let Some(p) = &app.connection_celebration.playing else { return el() };
    if p.loading.is_some() { return el(); }
    let (x, y, size, image) = geometry(app.window, app.ty.scale(), p.clip.is_some());
    let mut overlay = el().key("connection-celebration").absolute(x,y).w(size*11.0).h(size*4.0+image)
        .inert().instant_changes().live_opacity(&p.opacity);
    if p.reduced { overlay = overlay.fill(crate::ui::theme::rgb(app.pal.bg)); }
    overlay = overlay.child(crate::goal_celebration::lettering(app,"Connected",size,p.reduced,&p.fire).absolute(size, size*2.0));
    if image > 0.0 { overlay = overlay.child(el().pixels(&p.image).absolute((size*11.0-image)/2.0,size*4.0).w(image).h(image)); }
    overlay
}

pub fn fixture(app: &mut App) {
    app.load_fixture("connected", "Connected", "[]");
    app.connection_celebration.confirmed();
    // Offline fixture only: settle loading before the headless renderer takes its snapshot.
    let p = app.connection_celebration.playing.as_mut().unwrap();
    if let Some(rx) = p.loading.take() { p.clip = rx.recv_timeout(Duration::from_secs(2)).ok().flatten(); }
    p.start = Instant::now();
    if p.clip.is_some() && std::env::var_os("NEOISM_RUNE_DRIVE").is_none() { p.start -= Duration::from_millis(1800); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn disposed_hand_pixels_and_blank_frames_are_transparent() {
        // First-party synthetic animated WebP: blank, yellow hand, blank. Blended
        // transparent pixels alone cannot erase the hand; disposal must clear it.
        fn chunk(tag: &[u8;4], data: &[u8], out: &mut Vec<u8>) {
            out.extend(tag); out.extend((data.len() as u32).to_le_bytes()); out.extend(data);
            if data.len()%2 != 0 { out.push(0); }
        }
        let mut body = b"WEBP".to_vec();
        chunk(b"VP8X", &[0x12,0,0,0,3,0,0,1,0,0], &mut body);
        chunk(b"ANIM", &[0,0,0,0,0,0], &mut body);
        for color in [[0,0,0,0],[255,220,30,255],[0,0,0,0]] {
            let pixels = image::RgbaImage::from_pixel(4,2,image::Rgba(color));
            let mut encoded = Vec::new();
            image::codecs::webp::WebPEncoder::new_lossless(&mut encoded).encode(&pixels,4,2,image::ExtendedColorType::Rgba8).unwrap();
            let mut frame = vec![0,0,0,0,0,0,3,0,0,1,0,0,40,0,0,1];
            frame.extend_from_slice(&encoded[12..]);
            chunk(b"ANMF", &frame, &mut body);
        }
        let mut encoded = b"RIFF".to_vec(); encoded.extend((body.len() as u32).to_le_bytes()); encoded.extend(body);
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), encoded).unwrap();
        let clip = decode(file.path()).unwrap();
        assert_eq!(clip.frames.len(),3);
        assert!(clip.frames[0].rgba.chunks_exact(4).all(|p| p[3]==0));
        assert!(clip.frames[1].rgba.chunks_exact(4).all(|p| p[3]==255));
        assert!(clip.frames[2].rgba.chunks_exact(4).all(|p| p == [0,0,0,0]), "vacated hand must restore the original background, not retain yellow");
    }
    #[test] fn timing_and_small_windows() {
        let clip = Clip { width:1,height:1,frames:vec![Frame{end:0.1,rgba:vec![]},Frame{end:0.3,rgba:vec![]}] };
        assert_eq!(clip.index(0.0),0); assert_eq!(clip.index(0.1),1); assert_eq!(clip.index(20.0),1);
        for window in [[1252.0,674.0],[240.0,180.0],[32.0,32.0]] {
            let (x,y,s,i) = geometry(window,1.0,true);
            assert!(x>=0.0 && y>=0.0 && y+s*4.0+i<=window[1]);
        }
    }
    #[test] fn teardown() {
        let mut app = App::new();
        app.connection_celebration.confirmed();
        let p = app.connection_celebration.playing.as_mut().unwrap();
        p.loading = None; p.start -= Duration::from_secs(10);
        assert!(poll(&mut app, &mut rune::Tx::default()));
        assert!(!poll(&mut app, &mut rune::Tx::default()));
    }
    #[test] fn local_asset_if_available() {
        let path = crate::config::dir().join("assets/fist-bump.webp");
        if !path.exists() { return; } // The third-party asset deliberately isn't in the repository.
        let clip = decode(&path).expect("local WebP must decode within bounds");
        assert_eq!((clip.width,clip.height),(400,400));
        assert_eq!(clip.frames.len(),123);
        assert!((clip.duration()-3.84).abs()<0.02);
        assert!(clip.frames[0].rgba.chunks_exact(4).any(|p| p[3]==0));
        for index in [0,122] {
            assert!(clip.frames[index].rgba.chunks_exact(4).all(|p| p[3]==0), "blank frame {index} must erase all prior hand pixels");
        }
        assert!(clip.frames.iter().any(|f| f.rgba.chunks_exact(4).any(|p| p[3]>0 && p[3]<255)), "soft transparent edges must survive");
        assert_ne!(clip.frames[0].rgba,clip.frames[60].rgba);
        if std::env::var_os("NEOISM_CELEBRATION_CONTACT_SHEETS").is_some() {
            let old = image::codecs::webp::WebPDecoder::new(BufReader::new(std::fs::File::open(&path).unwrap())).unwrap()
                .into_frames().collect_frames().unwrap();
            for (name, frames) in [("before", old.iter().map(|f| f.buffer().clone()).collect::<Vec<_>>()),
                ("after", clip.frames.iter().map(|f| image::RgbaImage::from_raw(400,400,f.rgba.clone()).unwrap()).collect())] {
                let mut sheet = image::RgbaImage::from_pixel(1600,800,image::Rgba([0,0,0,255]));
                for (slot,index) in [0,20,40,60,70,80,100,122].into_iter().enumerate() {
                    image::imageops::overlay(&mut sheet,&frames[index],(slot%4*400) as i64,(slot/4*400) as i64);
                }
                sheet.save(format!("/tmp/connected-decoder-{name}.png")).unwrap();
            }
        }
    }
    #[test]
    #[ignore = "requires isolated native captures from /tmp/connected-live-capture.swift"]
    fn native_finished_captures_have_zero_residue() {
        for scene in ["connected", "connected-light"] {
            let image = image::open(format!("/tmp/{scene}-live-finished.png")).unwrap().into_rgba8();
            let (w,h) = image.dimensions();
            let background = *image.get_pixel(w/2,h/4);
            for y in h*30/100..h*75/100 {
                for x in w*30/100..w*70/100 {
                    assert_eq!(*image.get_pixel(x,y),background,"{scene}: residue at {x},{y}");
                }
            }
        }
    }
    #[test]
    #[ignore = "requires actual source mattes from tools/rekey_fist_bump.py"]
    fn actual_asset_matches_all_source_mattes() {
        let reference = std::path::PathBuf::from(std::env::var_os("NEOISM_FIST_SOURCE_MATTES").expect("source matte directory"));
        let clip = decode(&crate::config::dir().join("assets/fist-bump.webp")).unwrap();
        for (i, frame) in clip.frames.iter().enumerate() {
            let expected = image::open(reference.join(format!("{i:03}.png"))).unwrap().into_rgba8();
            assert_eq!((expected.width(),expected.height()),(clip.width,clip.height));
            for (pixel,(a,b)) in frame.rgba.chunks_exact(4).zip(expected.as_raw().chunks_exact(4)).enumerate() {
                assert_eq!(a[3],b[3],"actual frame {i}, pixel {pixel}: hole or stale alpha");
                if b[3]==255 { assert_eq!(a,b,"actual frame {i}, pixel {pixel}: opaque interior changed"); }
            }
        }
    }
}