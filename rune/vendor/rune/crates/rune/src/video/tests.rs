use super::*;

fn meta() -> Meta {
    Meta { duration: 1.0, size: Some((64, 36)), frame: 0.25 }
}

#[test]
fn new_videos_are_muted_paused_and_loading() {
    let v = Video::file("missing.mp4");
    assert!(v.muted());
    assert!(!v.looping());
    assert!(!v.is_playing());
    assert_eq!(v.rate(), 1.0);
    assert_eq!(v.time(), 0.0);
    assert_eq!(v.duration(), None);
    assert_eq!(v.natural_size(), None);
    assert!(!v.is_audio());
    #[cfg(target_os = "macos")]
    assert_eq!(v.state(), PlayerState::Loading);
    assert_eq!(v.source(), &MediaSource::File("missing.mp4".into()));
}

#[test]
fn sources_tell_urls_from_paths() {
    assert_eq!(MediaSource::from("https://example.com/a.mp3"), MediaSource::Url("https://example.com/a.mp3".into()));
    assert_eq!(MediaSource::from("file:///tmp/a.wav".to_string()), MediaSource::Url("file:///tmp/a.wav".into()));
    assert_eq!(MediaSource::from("sounds/a.m4a"), MediaSource::File("sounds/a.m4a".into()));
    assert_eq!(MediaSource::from(std::path::Path::new("a.mp3")), MediaSource::File("a.mp3".into()));
}

#[test]
fn audio_starts_unmuted_with_no_size() {
    let a = Player::audio("missing.m4a");
    assert!(a.is_audio());
    assert!(!a.muted(), "audio is audible by default");
    assert_eq!(a.natural_size(), None);
}

/// A clip's extension comes from the last segment of its path, never the host or query.
#[cfg(target_os = "macos")]
#[test]
fn extensions_come_from_the_last_path_segment() {
    let ext = |s: &str| super::macos::extension(&MediaSource::from(s));
    assert_eq!(ext("https://example.com/media/clip"), None);
    assert_eq!(ext("https://host.ogg/clip"), None);
    assert_eq!(ext("https://host.ogg"), None);
    assert_eq!(ext("https://example.com/a.b/clip?x=1.ogg#t=2.webm"), None);
    assert_eq!(ext("https://example.com/media/clip.OGG?v=2#t"), Some("ogg".into()));
    assert_eq!(ext("file:///tmp/a.b/clip.opus"), Some("opus".into()));
    assert_eq!(ext("sounds.d/clip"), None);
    assert_eq!(ext("sounds/clip.webm"), Some("webm".into()));
    assert_eq!(ext("sounds/clip."), None);
}

#[test]
fn webm_video_fails_at_once() {
    let a = Player::url("https://example.com/clips/tap.webm?v=2");
    let PlayerState::Failed(message) = a.state() else { panic!("{:?}", a.state()) };
    #[cfg(target_os = "macos")]
    assert!(message.contains(".webm isn't supported on macOS"), "{message}");
    #[cfg(not(target_os = "macos"))]
    assert_eq!(message, "video playback isn't supported on this platform yet");
    a.play();
    assert!(!a.is_playing(), "a failed clip doesn't play");
}

#[test]
fn the_waker_hears_each_state_change_once() {
    let a = Player::audio("missing.m4a");
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = calls.clone();
    a.set_waker(Arc::new(move || {
        seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }));
    a.shared.settle(false);
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 0, "nothing changed yet");
    a.shared.ctl.lock().unwrap().open(Ok(Meta { duration: 2.0, size: None, frame: 1.0 / 30.0 }));
    a.shared.settle(false);
    a.shared.settle(false);
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    a.shared.ctl.lock().unwrap().fail("gone".into());
    a.shared.settle(false);
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
}

/// An unsupported format fails inside the constructor, before any waker can be set: setting
/// one afterwards still delivers that failure, once.
#[test]
fn a_waker_set_after_a_failure_hears_it_once() {
    let a = Player::url("tap.ogv");
    assert!(matches!(a.shared.snapshot().state, PlayerState::Failed(_)));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = calls.clone();
    a.set_waker(Arc::new(move || {
        seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }));
    a.shared.settle(true);
    a.play();
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[test]
fn a_seek_before_the_clip_opens_is_clamped_when_it_does() {
    let v = Video::file("missing.mp4");
    v.seek(30.0);
    assert_eq!(v.shared.ctl.lock().unwrap().seek, Some(30.0), "no duration yet");
    v.shared.ctl.lock().unwrap().open(Ok(meta()));
    let c = v.shared.ctl.lock().unwrap();
    assert_eq!((c.seek, c.time), (Some(1.0), 1.0));
}

/// `play()` on a parked clip only unparks it on its next pull or edit on the main thread;
/// until then, parking others must not release it.
#[cfg(target_os = "macos")]
#[test]
fn parking_never_releases_a_playing_clip() {
    let clips: Vec<Video> = (0..6).map(|_| Video::file(fixture())).collect();
    let mut released = Vec::new();
    released.extend(backend::parked_with(&clips[0].shared));
    clips[0].play();
    for clip in &clips[1..] {
        released.extend(backend::parked_with(&clip.shared));
    }
    assert!(!released.iter().any(|r| Arc::ptr_eq(r, &clips[0].shared)), "the playing clip stays");
    assert_eq!(released.len(), 1, "four of the five paused clips stay parked");
    assert!(Arc::ptr_eq(&released[0], &clips[1].shared), "the oldest paused clip goes");
}

#[test]
fn handle_edits_are_shared_and_validated() {
    let v = Video::url("https://example.com/clip.mp4");
    let other = v.clone();
    other.set_rate(2.0);
    other.set_rate(-1.0);
    other.set_rate(f32::NAN);
    other.set_looping(true);
    other.set_muted(false);
    other.seek(-3.0);
    assert_eq!(v.rate(), 2.0);
    assert!(v.looping());
    assert!(!v.muted());
    assert_eq!(v.time(), 0.0);
    v.play();
    #[cfg(target_os = "macos")]
    {
        assert!(other.is_playing());
        // Still loading: playing is intent until the clip opens.
        assert_eq!(v.state(), PlayerState::Loading);
    }
}

#[test]
fn state_follows_open_play_end_and_failure() {
    let mut c = Control::default();
    c.open(Ok(meta()));
    assert_eq!(c.state(), PlayerState::Ready);
    assert_eq!((c.duration, c.size), (Some(1.0), Some((64, 36))));
    c.playing = true;
    assert_eq!(c.state(), PlayerState::Playing);
    c.advance_virtual(10.0);
    c.advance_virtual(10.5);
    assert_eq!(c.time, 0.5);
    let generation = c.generation;
    c.advance_virtual(11.25);
    assert_eq!((c.time, c.playing, c.ended), (1.0, false, true));
    assert_eq!(c.state(), PlayerState::Ended);
    assert!(c.generation > generation, "ending re-runs views");
    c.fail("boom".into());
    c.fail("second".into());
    assert_eq!(c.state(), PlayerState::Failed("boom".into()));
}

#[test]
fn virtual_time_loops_rates_and_seeks() {
    let mut c = Control::default();
    c.open(Ok(meta()));
    c.playing = true;
    c.looping = true;
    c.rate = 2.0;
    c.advance_virtual(0.0);
    c.advance_virtual(0.6);
    assert!((c.time - 0.2).abs() < 1e-9, "1.2 s wraps to 0.2: {}", c.time);
    assert!(c.playing);
    c.seek = Some(0.75);
    c.anchor = None;
    c.advance_virtual(5.0);
    assert_eq!(c.time, 0.75);
    c.advance_virtual(5.1);
    assert!((c.time - 0.95).abs() < 1e-9);
    // Paused time stays put whatever the clock does.
    c.playing = false;
    c.advance_virtual(9.0);
    assert!((c.time - 0.95).abs() < 1e-9);
}

#[test]
fn playing_after_the_end_restarts_from_zero() {
    let v = Video::file("missing.mp4");
    {
        let mut c = v.shared.ctl.lock().unwrap();
        c.open(Ok(meta()));
        c.time = 1.0;
        c.ended = true;
    }
    assert_eq!(v.state(), PlayerState::Ended);
    v.play();
    let c = v.shared.ctl.lock().unwrap();
    assert_eq!(c.seek, Some(0.0));
    assert!(c.playing && !c.ended);
}

#[test]
fn display_positions_loop_or_end() {
    let mut c = Control::default();
    c.open(Ok(meta()));
    c.playing = true;
    assert!(!c.observe(0.4, false));
    assert_eq!(c.time, 0.4);
    c.looping = true;
    assert!(c.observe(1.0, true), "looping restarts the player");
    assert_eq!((c.time, c.playing), (0.0, true));
    c.looping = false;
    assert!(!c.observe(1.2, true));
    assert_eq!((c.time, c.playing, c.state()), (1.0, false, PlayerState::Ended));
}

#[test]
fn frames_snap_to_whole_frames_inside_the_clip() {
    let m = meta();
    assert_eq!(m.frames(), 4);
    assert_eq!(m.frame_index(0.0), 0);
    assert_eq!(m.frame_index(0.2499), 0);
    assert_eq!(m.frame_index(0.25), 1);
    assert_eq!(m.frame_index(1.0), 3, "the end shows the last frame");
    assert_eq!(m.frame_index(-1.0), 0);
    assert_eq!(m.frame_time(1), 0.375);
    assert_eq!(m.frame_time(3), 0.875);
}

#[test]
fn the_last_handle_or_element_frees_the_player() {
    let v = Video::file("missing.mp4");
    let shared = Arc::downgrade(&v.shared);
    // An element keeps the surface, and through it the player.
    let in_tree = v.surface().clone();
    drop(v);
    assert!(shared.upgrade().is_some());
    drop(in_tree);
    assert!(shared.upgrade().is_none());
}

#[cfg(target_os = "macos")]
fn fixture() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rgbw.mp4")
}

#[cfg(target_os = "macos")]
fn sound(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn view(v: &Video, _: &mut crate::Cx<Video>) -> crate::El {
    crate::el().key("clip").w(32.0).h(18.0).video(v, crate::SurfaceFit::Fit)
}

#[test]
fn failures_draw_a_placeholder_and_stop_drawing() {
    let v = Video::file("/nonexistent/clip.mp4");
    let mut rt = crate::Runtime::new(v.clone(), view, (32.0, 18.0), crate::Env::new(true, 2.0));
    rt.frame(0.0);
    let PlayerState::Failed(message) = v.state() else { panic!("{:?}", v.state()) };
    #[cfg(target_os = "macos")]
    assert!(message.contains("no such file"), "{message}");
    #[cfg(not(target_os = "macos"))]
    assert_eq!(message, "video playback isn't supported on this platform yet");
    let frame = v.surface().latest().expect("placeholder frame");
    assert_eq!((frame.size, frame.format), ([32, 18], Format::Bgra));
    rt.frame(0.1);
    assert!(!rt.frame(0.1), "a failed video requests no frames");
}

/// Headless audio opens on first read and never plays: no player, no sound. With no runtime
/// stepping on its thread, time holds.
#[cfg(target_os = "macos")]
#[test]
fn headless_audio_opens_on_read_and_holds_time() {
    let a = Player::audio(sound("silence.m4a"));
    a.set_muted(true);
    assert_eq!(a.state(), PlayerState::Ready);
    let d = a.duration().unwrap();
    assert!((d - 1.0).abs() < 0.1, "{d}");
    assert_eq!(a.natural_size(), None);
    a.play();
    assert_eq!(a.state(), PlayerState::Playing);
    assert_eq!(a.time(), 0.0);
    a.seek(0.4);
    assert_eq!(a.time(), 0.4);

    let wav = Player::audio(sound("silence.wav"));
    assert!((wav.duration().unwrap() - 0.5).abs() < 0.01, "{:?}", wav.duration());
    let video = Player::audio(fixture());
    assert!(matches!(video.state(), PlayerState::Failed(e) if e.contains("no playable audio track")));
}

#[cfg(target_os = "macos")]
mod headless {
    use super::*;
    use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};

    fn render(gpu: &Gpu, rt: &mut crate::Runtime<Video>) -> Vec<u8> {
        let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (64, 36));
        renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (64, 36));
        let target = Offscreen::new(&gpu.device, 64, 36);
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.draw(&mut encoder, &target.view, [0.0; 4]);
        target.copy(&mut encoder);
        renderer.submit(&gpu.queue, encoder);
        target.read(&gpu.device)
    }

    fn centre(p: &[u8]) -> [u8; 3] {
        let i = (18 * 64 + 32) * 4;
        [p[i], p[i + 1], p[i + 2]]
    }

    fn near(got: [u8; 3], want: [u8; 3], what: &str) {
        assert!(got.iter().zip(want).all(|(g, w)| g.abs_diff(w) <= 24), "{what}: got {got:?}, want ~{want:?}");
    }

    /// The fixture is four 0.25 s frames: red, green, blue, white.
    #[test]
    fn headless_playback_decodes_the_frame_at_virtual_time() {
        let gpu = Gpu::new(Gpu::instance(), None);
        let v = Video::file(fixture());
        let mut rt = crate::Runtime::new(v.clone(), view, (32.0, 18.0), crate::Env::new(true, 2.0));
        rt.frame(0.0);
        assert_eq!(v.state(), PlayerState::Ready);
        assert_eq!(v.natural_size(), Some((64, 36)));
        assert!((v.duration().unwrap() - 1.0).abs() < 1e-3);
        let first = render(&gpu, &mut rt);
        near(centre(&first), [255, 0, 0], "first frame");
        rt.frame(0.0);
        assert!(!rt.frame(0.5), "a paused video requests no frames");
        assert_eq!(render(&gpu, &mut rt), first, "deterministic");

        v.play();
        assert!(rt.frame(0.0), "playing keeps frames coming");
        assert!(rt.frame(0.3));
        assert_eq!(v.state(), PlayerState::Playing);
        assert!((v.time() - 0.3).abs() < 1e-9);
        near(centre(&render(&gpu, &mut rt)), [0, 255, 0], "0.3 s");
        rt.frame(0.3);
        near(centre(&render(&gpu, &mut rt)), [0, 0, 255], "0.6 s");
        let tree = crate::tooling::tree_json(&rt, 0.6);
        let clip = tree["elements"].as_array().unwrap().iter().find(|e| e["key"] == "clip").unwrap();
        assert_eq!(clip["video"]["state"], "playing");
        assert_eq!(clip["video"]["time"], 0.6);
        assert_eq!(clip["video"]["size"], serde_json::json!([64, 36]));
        assert_eq!(clip["video"]["muted"], true);

        rt.frame(0.3);
        near(centre(&render(&gpu, &mut rt)), [255, 255, 255], "0.9 s");
        rt.frame(0.3);
        assert_eq!(v.state(), PlayerState::Ended);
        assert_eq!(v.time(), 1.0);
        near(centre(&render(&gpu, &mut rt)), [255, 255, 255], "the end holds the last frame");
        rt.frame(0.0);
        assert!(!rt.frame(0.1), "an ended video requests no frames");

        v.seek(0.3);
        rt.frame(0.0);
        near(centre(&render(&gpu, &mut rt)), [0, 255, 0], "seek while paused");
        v.set_looping(true);
        v.set_rate(2.0);
        v.play();
        rt.frame(0.0);
        rt.frame(0.5);
        assert!((v.time() - 0.3).abs() < 1e-9, "0.3 + 2 x 0.5 wraps to 0.3: {}", v.time());
        near(centre(&render(&gpu, &mut rt)), [0, 255, 0], "looped");
    }

    /// A playing clip scrolled out of its list and back commits nothing, and shows the frame
    /// at the time it comes back, not the one it left on.
    #[test]
    fn a_playing_clip_scrolling_across_the_clip_edge_commits_nothing() {
        let gpu = Gpu::new(Gpu::instance(), None);
        let v = Video::file(fixture());
        v.set_looping(true);
        let mut rt = crate::Runtime::new(v.clone(), scrolled, (32.0, 18.0), crate::Env::new(true, 2.0));
        v.play();
        rt.frame(0.0);
        rt.frame(0.1);
        near(centre(&render(&gpu, &mut rt)), [255, 0, 0], "0.1 s");
        let commits = rt.stats.commits;
        rt.wheel(10.0, 10.0, -40.0);
        rt.frame(0.1);
        assert!(rt.surfaces.is_empty(), "scrolled out of sight");
        rt.frame(0.4);
        rt.wheel(10.0, 10.0, 40.0);
        rt.frame(0.0);
        assert_eq!(rt.surfaces.len(), 1, "back in sight");
        near(centre(&render(&gpu, &mut rt)), [0, 0, 255], "0.6 s, pulled as it comes back");
        assert_eq!(v.state(), PlayerState::Playing);
        assert_eq!(rt.stats.commits, commits, "scrolling across the edge commits nothing");
    }
}

/// The clip at the top of a list that scrolls it out of sight.
fn scrolled(v: &Video, _: &mut crate::Cx<Video>) -> crate::El {
    crate::col().key("list").w(32.0).h(18.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(
        crate::col().w_full()
            .child(crate::el().key("clip").w(32.0).h(18.0).video(v, crate::SurfaceFit::Fit))
            .child(crate::el().w_full().h(200.0)),
    )
}

#[test]
fn a_failed_clip_scrolling_across_the_clip_edge_commits_nothing() {
    let v = Video::file("/nonexistent/clip.mp4");
    let mut rt = crate::Runtime::new(v.clone(), scrolled, (32.0, 18.0), crate::Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(0.1);
    assert!(matches!(v.state(), PlayerState::Failed(_)));
    let commits = rt.stats.commits;
    for dy in [-40.0, 40.0, -40.0, 40.0] {
        let shown = rt.surfaces.len();
        rt.wheel(10.0, 10.0, dy);
        rt.frame(0.1);
        assert_ne!(rt.surfaces.len(), shown, "crossed the edge ({dy})");
    }
    assert_eq!(rt.stats.commits, commits, "scrolling across the edge commits nothing");
}

#[test]
fn requests_carry_headers_without_printing_their_values() {
    let request = Player::request("https://example.com/media/a.m4a")
        .header("Authorization", "Bearer secret-token")
        .header("authorization", "Bearer newer-token")
        .header("X-Board", "1");
    let printed = format!("{request:?}");
    assert!(printed.contains("Authorization") || printed.contains("authorization"), "{printed}");
    assert!(!printed.contains("token"), "header values never print: {printed}");
    let a = request.audio();
    assert!(a.is_audio());
    assert_eq!(a.source(), &MediaSource::Url("https://example.com/media/a.m4a".into()));
    assert_eq!(a.shared.headers, [("authorization".to_string(), "Bearer newer-token".to_string()), ("X-Board".into(), "1".into())]);
    assert!(!format!("{a:?}").contains("token"));
    let v = Player::request("https://example.com/media/a.mp4").header("Authorization", "Bearer t").video();
    assert!(!v.is_audio() && v.muted());
}

/// A local HTTP server that serves `body` (with byte ranges) only to requests carrying
/// `Authorization: Bearer let-me-in`, and counts the ones that didn't.
#[cfg(target_os = "macos")]
fn serve_with_auth(body: Vec<u8>, content_type: &'static str) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/media/clip", listener.local_addr().unwrap().port());
    let refused = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = refused.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let (body, count) = (body.clone(), count.clone());
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut stream = stream;
                loop {
                    let (mut authorized, mut range) = (false, None);
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 { return; }
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) == 0 { return; }
                        let header = header.trim_end();
                        if header.is_empty() { break; }
                        let (name, value) = header.split_once(':').unwrap_or((header, ""));
                        match name.trim().to_ascii_lowercase().as_str() {
                            "authorization" => authorized = value.trim() == "Bearer let-me-in",
                            "range" => range = value.trim().strip_prefix("bytes=").map(str::to_owned),
                            _ => {}
                        }
                    }
                    if !authorized {
                        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let _ = stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n");
                        continue;
                    }
                    let len = body.len();
                    let (start, end) = range.and_then(|r| {
                        let (a, b) = r.split_once('-')?;
                        let a: usize = a.parse().ok()?;
                        let b = b.parse::<usize>().map_or(len - 1, |b| b.min(len - 1));
                        Some((a, b))
                    }).unwrap_or((0, len - 1));
                    let head = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Type: {content_type}\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{len}\r\nContent-Length: {}\r\n\r\n",
                        end + 1 - start
                    );
                    if stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(&body[start..=end])).is_err() { return; }
                }
            });
        }
    });
    (url, refused)
}

/// AVFoundation sends the request's headers: the clip opens from a server that refuses
/// anything without them, and fails without them.
#[cfg(target_os = "macos")]
#[test]
fn headless_audio_opens_with_request_headers() {
    let (url, refused) = serve_with_auth(std::fs::read(sound("silence.wav")).unwrap(), "audio/wav");
    let a = Player::request(format!("{url}.wav")).header("Authorization", "Bearer let-me-in").audio();
    assert_eq!(a.state(), PlayerState::Ready);
    assert!((a.duration().unwrap() - 0.5).abs() < 0.01, "{:?}", a.duration());
    assert_eq!(refused.load(std::sync::atomic::Ordering::SeqCst), 0, "every request carried the header");

    let without = Player::audio(format!("{url}-2.wav"));
    assert!(matches!(without.state(), PlayerState::Failed(_)), "{:?}", without.state());
    assert!(refused.load(std::sync::atomic::Ordering::SeqCst) > 0);
}

/// Ogg and WebM audio, which AVFoundation can't read, decode and open like any clip: by file
/// extension, by URL with request headers, and by their bytes when the URL has no extension.
/// Each fixture is 0.5 s.
#[cfg(all(target_os = "macos", feature = "ogg-audio"))]
#[test]
fn headless_ogg_and_webm_audio_open_and_follow_the_clock() {
    let near = |a: &Player, what: &str| assert!((a.duration().unwrap_or(0.0) - 0.5).abs() < 0.03, "{what}: {:?} {:?}", a.state(), a.duration());
    for name in ["sine-vorbis.ogg", "sine-opus.ogg", "sine-opus.webm"] {
        let a = Player::audio(sound(name));
        assert_eq!(a.state(), PlayerState::Ready, "{name}");
        near(&a, name);
    }
    let (url, refused) = serve_with_auth(std::fs::read(sound("sine-opus.webm")).unwrap(), "application/octet-stream");
    let by_url = Player::request(format!("{url}.webm")).header("Authorization", "Bearer let-me-in").audio();
    near(&by_url, "url");
    let sniffed = Player::request(url).header("Authorization", "Bearer let-me-in").audio();
    near(&sniffed, "no extension");
    assert_eq!(refused.load(std::sync::atomic::Ordering::SeqCst), 0, "every request carried the header");

    let a = Player::audio(sound("sine-vorbis.ogg"));
    let mut rt = crate::Runtime::new((), |_: &(), _: &mut crate::Cx<()>| crate::el().size(10.0, 10.0), (10.0, 10.0), crate::Env::new(true, 2.0));
    a.play();
    rt.frame(0.0);
    rt.frame(0.2);
    assert!((a.time() - 0.2).abs() < 1e-6, "{}", a.time());
    rt.frame(0.5);
    assert_eq!(a.state(), PlayerState::Ended);

    let broken = Player::audio("/nonexistent/clip.ogg");
    assert!(matches!(broken.state(), PlayerState::Failed(e) if e.contains("can't read")), "{:?}", broken.state());
    // Ogg bytes that don't decode, at a URL with no extension, report the decode error.
    let mut bytes = b"OggS".to_vec();
    bytes.extend([0x55; 400]);
    let (url, _) = serve_with_auth(bytes, "application/octet-stream");
    let garbled = Player::request(url).header("Authorization", "Bearer let-me-in").audio();
    assert!(matches!(garbled.state(), PlayerState::Failed(e) if e.contains("can't decode")), "{:?}", garbled.state());
}

/// Headless audio follows the runtime clock silently: play, pause, rate, seek and the end.
#[cfg(target_os = "macos")]
#[test]
fn headless_audio_time_follows_the_virtual_clock() {
    let a = Player::audio(sound("silence.wav"));
    let ends = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let heard = ends.clone();
    let clip = a.clone();
    a.set_waker(Arc::new(move || {
        if clip.shared.ctl.lock().unwrap().ended {
            heard.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }));
    let mut rt = crate::Runtime::new((), |_: &(), _: &mut crate::Cx<()>| crate::el().size(10.0, 10.0), (10.0, 10.0), crate::Env::new(true, 2.0));
    let near = |t: f64, want: f64, what: &str| assert!((t - want).abs() < 1e-6, "{what}: {t}, want {want}");
    a.play();
    rt.frame(0.0);
    rt.frame(0.1);
    near(a.time(), 0.1, "playing");
    assert_eq!(a.state(), PlayerState::Playing);
    a.pause();
    rt.frame(0.2);
    near(a.time(), 0.1, "paused");
    a.set_rate(2.0);
    a.play();
    rt.frame(0.05);
    near(a.time(), 0.2, "double speed");
    a.seek(0.05);
    near(a.time(), 0.05, "seek");
    rt.frame(0.1);
    near(a.time(), 0.25, "plays on from the seek");
    assert_eq!(ends.load(std::sync::atomic::Ordering::SeqCst), 0);
    rt.frame(0.5);
    assert_eq!(a.state(), PlayerState::Ended);
    near(a.time(), 0.5, "holds the end");
    assert_eq!(ends.load(std::sync::atomic::Ordering::SeqCst), 1, "the waker hears the end once");
    a.set_looping(true);
    a.set_rate(1.0);
    a.play();
    rt.frame(0.6);
    near(a.time(), 0.1, "loops");
    assert!(a.is_playing());
}

/// A new runtime's clock starts at 0, never where an earlier runtime on this thread left off.
#[cfg(target_os = "macos")]
#[test]
fn a_new_runtime_starts_the_headless_clock_again() {
    let view = |_: &(), _: &mut crate::Cx<()>| crate::el().size(10.0, 10.0);
    let mut old = crate::Runtime::new((), view, (10.0, 10.0), crate::Env::new(true, 2.0));
    old.frame(5.0);
    drop(old);
    let a = Player::audio(sound("silence.wav"));
    let mut rt = crate::Runtime::new((), view, (10.0, 10.0), crate::Env::new(true, 2.0));
    a.play();
    rt.frame(0.1);
    assert!((a.time() - 0.1).abs() < 1e-6, "{}", a.time());
}

#[cfg(target_os = "macos")]
#[test]
fn preload_decodes_the_first_frame_before_anything_draws_it() {
    let v = Video::file(fixture());
    assert_eq!(macos::test_poster(&v.shared), None);
    // Preload is a no-op headless (frames decode as they draw), so call the backend directly.
    macos::preload(&v.shared);
    let start = std::time::Instant::now();
    while macos::test_poster(&v.shared).is_none() && start.elapsed() < std::time::Duration::from_secs(10) {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(macos::test_poster(&v.shared), Some(true), "the first frame decoded off the main thread");
    v.preload();
    let audio = Player::audio(sound("silence.wav"));
    macos::preload(&audio.shared);
    assert_eq!(macos::test_poster(&audio.shared), None, "audio has no poster to warm");
}
