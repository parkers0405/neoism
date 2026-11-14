use super::*;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

/// A temporary directory, removed on drop.
struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rune-recorder-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A headless runtime, which starts this thread's virtual clock at 0.
fn runtime() -> crate::Runtime<()> {
    crate::Runtime::new((), |_: &(), _: &mut crate::Cx<()>| crate::el().size(10.0, 10.0), (10.0, 10.0), crate::Env::new(true, 2.0))
}

#[track_caller]
fn near(got: f64, want: f64, tolerance: f64) {
    assert!((got - want).abs() <= tolerance, "got {got}, want {want}");
}

#[test]
fn a_recording_starts_pauses_resumes_and_stops() {
    FakeMicrophone::silence().install();
    let dir = Scratch::new("states");
    let rec = Recorder::to_file(dir.join("note.wav"));
    let mut rt = runtime();
    assert_eq!(rec.state(), RecorderState::Idle);
    assert_eq!(rec.stop(), Err("not recording".into()));
    rec.start();
    assert_eq!(rec.state(), RecorderState::Recording);
    assert!(rec.is_recording());
    rt.frame(0.25);
    near(rec.elapsed(), 0.25, 1e-9);
    rec.pause();
    assert_eq!(rec.state(), RecorderState::Paused);
    rt.frame(0.5);
    near(rec.elapsed(), 0.25, 1e-9);
    rec.resume();
    rec.start(); // Already recording: nothing happens.
    rt.frame(0.1);
    near(rec.elapsed(), 0.35, 1e-9);
    let note = rec.stop().unwrap();
    assert_eq!(note.path, dir.join("note.wav"));
    near(note.duration, 0.35, 1e-9);
    assert_eq!(rec.state(), RecorderState::Stopped);
    rt.frame(0.2);
    near(rec.elapsed(), 0.35, 1e-9);
    assert_eq!(rec.stop(), Err("not recording".into()));
    rec.start();
    assert_eq!(rec.elapsed(), 0.0, "a new recording starts from zero");
    rt.frame(0.1);
    near(rec.elapsed(), 0.1, 1e-9);
}

#[test]
fn a_new_runtime_restarts_elapsed_from_its_own_clock() {
    FakeMicrophone::silence().install();
    let dir = Scratch::new("clock");
    let mut old = runtime();
    old.frame(5.0);
    let rec = Recorder::to_file(dir.join("note.wav"));
    rec.start();
    let mut rt = runtime();
    rt.frame(0.1);
    rt.frame(0.1);
    near(rec.elapsed(), 0.2, 1e-9);
}

#[test]
fn a_known_sine_reads_its_known_peak_and_rms() {
    let dir = Scratch::new("sine");
    FakeMicrophone::sine(1000.0, 0.5).install();
    let rec = Recorder::to_file(dir.join("a.wav"));
    let mut rt = runtime();
    assert_eq!(rec.level(), Level::SILENT);
    rec.start();
    rt.frame(1.0);
    let level = rec.level();
    // Peak 0.5 is -6.02 dBFS; a sine's RMS is 3.01 dB below its peak.
    near(level.peak_db() as f64, 20.0 * 0.5f64.log10(), 0.01);
    near(level.rms_db() as f64, 20.0 * (0.5 / 2f64.sqrt()).log10(), 0.01);
    near(level.meter() as f64, (60.0 - 9.0309) / 60.0, 0.001);
    assert_eq!(rec.live_level().get(), level.meter());
    rec.pause();
    assert_eq!(rec.level(), Level::SILENT, "a paused recorder hears nothing");
    assert_eq!(rec.live_level().get(), 0.0);
    rec.stop().unwrap();

    // The same second in 64 frames reads exactly the same: levels don't depend on frame rate.
    let rec = Recorder::to_file(dir.join("b.wav"));
    let mut rt = runtime();
    rec.start();
    for _ in 0..64 {
        rt.frame(1.0 / 64.0);
    }
    assert_eq!(rec.level(), level);

    FakeMicrophone::silence().install();
    let quiet = Recorder::to_file(dir.join("c.wav"));
    quiet.start();
    rt.frame(0.5);
    assert_eq!(quiet.level(), Level::SILENT);
    assert_eq!(quiet.level().meter(), 0.0);
}

#[test]
fn the_meter_rises_fast_and_falls_slowly() {
    let mut m = Meter::default();
    for _ in 0..3 {
        m.push(0.5, 0.125, 0.01);
    }
    let rising = m.level();
    assert!(rising.rms_db() > -12.0, "30 ms in, the RMS is close: {rising:?}");
    for _ in 0..10 {
        m.push(0.0, 0.0, 0.01);
    }
    let falling = m.level();
    assert!(falling.rms_db() > -13.0 && falling.rms_db() < rising.rms_db(), "100 ms of silence: {falling:?}");
    near(falling.peak_db() as f64, -6.0206 - 2.0, 0.01);
    m.push(f64::NAN, f64::INFINITY, 0.01);
    assert!(m.level().rms_db().is_finite(), "bad samples never poison the meter");
}

/// A bound meter shows this frame's level in this frame, and draws nothing while idle.
#[test]
fn a_bound_meter_follows_the_level_on_the_same_frame() {
    let dir = Scratch::new("bound");
    FakeMicrophone::sine(1000.0, 0.5).install();
    let rec = Recorder::to_file(dir.join("a.wav"));
    let view = |r: &Recorder, _: &mut crate::Cx<Recorder>| {
        crate::el().key("meter").size(10.0, 10.0).fill(crate::Color::WHITE).live_opacity(&r.live_level())
    };
    let mut rt = crate::Runtime::new(rec.clone(), view, (10.0, 10.0), crate::Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(rt.presented("meter").unwrap().1, 0.0);
    assert!(!rt.frame(0.1), "an idle recorder asks for no frames");
    rec.start();
    rt.frame(1.0);
    near(rt.presented("meter").unwrap().1 as f64, rec.level().meter() as f64, 1e-6);
    assert!(rec.level().meter() > 0.8);
    rec.stop().unwrap();
    rt.frame(0.1);
    assert_eq!(rt.presented("meter").unwrap().1, 0.0);
}

#[test]
fn stopping_writes_a_wav_of_the_recorded_length() {
    let dir = Scratch::new("length");
    FakeMicrophone::sine(1000.0, 0.5).install();
    let rec = Recorder::to_file(dir.join("note.wav"));
    let mut rt = runtime();
    rec.start();
    rt.frame(1.5);
    let note = rec.stop().unwrap();
    near(note.duration, 1.5, 1e-9);
    let (rate, samples) = fake::read_wav(&note.path).unwrap();
    assert_eq!((rate, samples.len()), (48_000, 72_000));
    near(samples[12] as f64, 0.5, 1.0 / 32_000.0);
    near(samples[36] as f64, -0.5, 1.0 / 32_000.0);
    // Players open it.
    #[cfg(target_os = "macos")]
    {
        let clip = crate::Player::audio(note.path.clone());
        near(clip.duration().expect("the recording opens"), 1.5, 0.01);
    }
}

#[test]
fn an_aac_recorder_on_a_fake_microphone_writes_wav_beside_its_path() {
    let dir = Scratch::new("aac");
    FakeMicrophone::silence().install();
    let rec = Recorder::to_file(dir.join("note.m4a"));
    let mut rt = runtime();
    rec.start();
    rt.frame(0.2);
    let note = rec.stop().unwrap();
    assert_eq!(note.path, dir.join("note.wav"));
    assert!(note.path.exists());
    assert!(!dir.join("note.m4a").exists());
    assert_eq!(rec.path(), dir.join("note.m4a"), "a device would write the m4a");
}

/// The format picks the extension, so AVAudioRecorder (which picks its container by the
/// extension) never gets a mismatch, and a fake never writes RIFF into an `.m4a`.
#[test]
fn the_format_decides_the_file_extension() {
    let dir = Scratch::new("extension");
    FakeMicrophone::silence().install();
    let path = |rec: Recorder| rec.path().strip_prefix(&dir.0).unwrap().to_path_buf();
    assert_eq!(path(Recorder::to_file(dir.join("x.wav")).format(RecordFormat::Aac)), PathBuf::from("x.m4a"));
    assert_eq!(path(Recorder::to_file(dir.join("x.m4a")).format(RecordFormat::Wav)), PathBuf::from("x.wav"));
    assert_eq!(path(Recorder::to_file(dir.join("x"))), PathBuf::from("x.m4a"));
    assert_eq!(path(Recorder::to_file(dir.join("x.mp3"))), PathBuf::from("x.m4a"));
    assert_eq!(path(Recorder::to_file(dir.join("x.MP4"))), PathBuf::from("x.MP4"));
    assert_eq!(path(Recorder::to_file(dir.join("x.WAV"))), PathBuf::from("x.WAV"));

    let wav = Recorder::to_file(dir.join("note.m4a")).format(RecordFormat::Wav);
    wav.start();
    let note = wav.stop().unwrap();
    assert_eq!(note.path, dir.join("note.wav"));
    assert_eq!(&std::fs::read(&note.path).unwrap()[..4], b"RIFF");
    assert!(!dir.join("note.m4a").exists(), "no RIFF bytes behind an .m4a name");
}

/// Dropping the last handle mid-recording keeps what was captured, whether recording or
/// paused; a clone keeps the recording going.
#[test]
fn dropping_the_last_handle_keeps_the_recording() {
    let dir = Scratch::new("drop");
    FakeMicrophone::sine(1000.0, 0.5).install();
    let mut rt = runtime();
    let rec = Recorder::to_file(dir.join("a.wav"));
    rec.start();
    rt.frame(0.25);
    let clone = rec.clone();
    drop(rec);
    rt.frame(0.25);
    assert!(clone.is_recording());
    drop(clone);
    let (_, samples) = fake::read_wav(&dir.join("a.wav")).unwrap();
    assert_eq!(samples.len(), 24_000);

    let rec = Recorder::to_file(dir.join("b.wav"));
    rec.start();
    rt.frame(0.1);
    rec.pause();
    rt.frame(0.5);
    drop(rec);
    assert_eq!(fake::read_wav(&dir.join("b.wav")).unwrap().1.len(), 4_800);

    // A cancelled or stopped recorder leaves nothing more to do.
    let rec = Recorder::to_file(dir.join("c.wav"));
    rec.start();
    rt.frame(0.1);
    rec.cancel();
    drop(rec);
    assert!(!dir.join("c.wav").exists());
}

/// Cancelling a start that hasn't opened anything (still waiting on permission, say) leaves
/// an earlier recording at the same path alone.
#[test]
fn cancelling_a_pending_start_keeps_an_earlier_file() {
    let dir = Scratch::new("pending");
    FakeMicrophone::silence().install();
    let path = dir.join("note.wav");
    let rec = Recorder::to_file(&path);
    let mut rt = runtime();
    rec.start();
    rt.frame(0.2);
    rec.stop().unwrap();
    // A device start waiting on a permission answer: Starting, with no input open.
    {
        let mut c = rec.shared.ctl.lock().unwrap();
        c.attempt += 1;
        c.set(RecorderState::Starting);
    }
    rec.cancel();
    assert_eq!(rec.state(), RecorderState::Idle);
    assert!(path.exists(), "the earlier recording stays");
}

/// 60 frames of 1/60 s sum to just under 1 s; that mustn't drop the last block's level, a
/// sample, or move an unplug to a later frame.
#[test]
fn frame_rate_never_moves_a_sample_level_or_unplug() {
    let dir = Scratch::new("rate");
    FakeMicrophone::default().install();
    let mut rt = runtime();
    let one = Recorder::to_file(dir.join("one.wav"));
    one.start();
    rt.frame(1.0);
    let (level, note) = (one.level(), one.stop().unwrap());

    let mut rt = runtime();
    let sixty = Recorder::to_file(dir.join("sixty.wav"));
    sixty.start();
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(sixty.level(), level);
    let other = sixty.stop().unwrap();
    assert_eq!(other.duration, note.duration);
    assert_eq!(fake::read_wav(&other.path).unwrap().1.len(), 48_000);

    FakeMicrophone::silence().unplug_after(0.3).install();
    let mut rt = runtime();
    let rec = Recorder::to_file(dir.join("unplug.wav"));
    rec.start();
    for _ in 0..18 {
        rt.frame(1.0 / 60.0);
    }
    assert!(matches!(rec.state(), RecorderState::Interrupted(_)), "unplugs on the 18th frame: {:?}", rec.state());
    assert_eq!(rec.stop().unwrap().duration, 0.3);
}

/// Restarting a recorder doesn't follow it more than once a frame.
#[test]
fn a_restarted_recorder_is_followed_once() {
    let dir = Scratch::new("restart");
    FakeMicrophone::silence().install();
    let mut rt = runtime();
    let rec = Recorder::to_file(dir.join("note.wav"));
    for _ in 0..3 {
        rec.start();
        rt.frame(0.1);
        rec.stop().unwrap();
    }
    let followed = HEADLESS.with_borrow(|h| h.iter().filter(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(&rec.shared))).count());
    assert_eq!(followed, 1);
}

#[test]
fn levels_pack_into_one_word() {
    for level in [Level::SILENT, Level::new(-6.0, -9.0), Level::new(0.0, -160.0)] {
        assert_eq!(unpack(pack(level)), level);
    }
}

/// A late failure from an earlier attempt leaves the current attempt's session alone.
#[cfg(target_os = "macos")]
#[test]
fn a_stale_failure_leaves_the_current_session() {
    let rec = Recorder::to_file(std::env::temp_dir().join("rune-recorder-stale.m4a"));
    let (session, _commands) = macos::idle_session();
    *rec.shared.native.lock().unwrap() = Some((2, session));
    rec.shared.ctl.lock().unwrap().attempt = 2;
    rec.shared.failed(1, "late".into());
    assert!(rec.shared.native.lock().unwrap().is_some());
    rec.shared.failed(2, "now".into());
    assert!(rec.shared.native.lock().unwrap().is_none());
}

/// Dropping the last handle on the recorder thread itself (the last handle went while it
/// reported) returns at once instead of waiting on itself.
#[cfg(target_os = "macos")]
#[test]
fn finishing_on_the_recorder_thread_doesnt_wait_on_itself() {
    let (session, commands) = macos::idle_session();
    let begun = std::time::Instant::now();
    std::thread::spawn(move || {
        macos::mark_recorder_thread();
        macos::finish(session);
    })
    .join()
    .unwrap();
    assert!(begun.elapsed() < std::time::Duration::from_secs(1));
    assert!(commands.try_recv().is_err(), "nothing was asked of the thread");
}

#[test]
fn cancel_deletes_the_file_of_a_recording_in_progress() {
    let dir = Scratch::new("cancel");
    FakeMicrophone::silence().install();
    let path = dir.join("note.wav");
    let rec = Recorder::to_file(&path);
    let mut rt = runtime();
    rec.start();
    assert!(path.exists(), "the file exists from the start");
    rt.frame(0.3);
    rec.cancel();
    assert!(!path.exists());
    assert_eq!(rec.state(), RecorderState::Idle);
    assert!(rec.stop().is_err());

    rec.start();
    rt.frame(0.3);
    rec.pause();
    rec.cancel();
    assert!(!path.exists(), "paused counts as in progress");

    rec.start();
    rt.frame(0.3);
    rec.stop().unwrap();
    rec.cancel();
    assert!(path.exists(), "a stopped recording is the app's");
    assert_eq!(rec.state(), RecorderState::Stopped);
}

#[test]
fn starting_without_permission_fails_clearly() {
    let dir = Scratch::new("permission");
    let path = dir.join("note.wav");
    FakeMicrophone::silence().permission(MicPermission::Denied).install();
    assert_eq!(Recorder::permission(), MicPermission::Denied);
    let rec = Recorder::to_file(&path);
    rec.start();
    let RecorderState::Failed(message) = rec.state() else { panic!("{:?}", rec.state()) };
    assert!(message.contains("denied"), "{message}");
    assert!(!rec.is_recording());
    assert!(!path.exists(), "nothing is written without permission");
    assert_eq!(rec.stop(), Err(message));

    FakeMicrophone::silence().permission(MicPermission::Restricted).install();
    rec.start();
    assert!(matches!(rec.state(), RecorderState::Failed(m) if m.contains("restricted")));

    // Undetermined: starting asks, and the answer sticks.
    FakeMicrophone::silence().permission(MicPermission::Undetermined).answers(MicPermission::Denied).install();
    rec.start();
    assert!(matches!(rec.state(), RecorderState::Failed(m) if m.contains("denied")));
    assert_eq!(Recorder::permission(), MicPermission::Denied);

    FakeMicrophone::silence().permission(MicPermission::Undetermined).install();
    let answer = Arc::new(Mutex::new(None));
    let heard = answer.clone();
    Recorder::request_permission(move |p| *heard.lock().unwrap() = Some(p));
    assert_eq!(*answer.lock().unwrap(), Some(MicPermission::Granted));
    assert_eq!(Recorder::permission(), MicPermission::Granted);
    rec.start();
    assert_eq!(rec.state(), RecorderState::Recording);

    // A recorder's own fake microphone decides for it.
    let own = Recorder::to_file(dir.join("own.wav")).fake(FakeMicrophone::silence().permission(MicPermission::Undetermined).answers(MicPermission::Granted));
    own.start();
    assert_eq!(own.state(), RecorderState::Recording);
}

#[test]
fn an_unplugged_microphone_keeps_what_was_captured() {
    let dir = Scratch::new("unplug");
    FakeMicrophone::sine(440.0, 0.25).unplug_after(0.5).install();
    let rec = Recorder::to_file(dir.join("note.wav"));
    let changes = Arc::new(AtomicUsize::new(0));
    let count = changes.clone();
    rec.set_waker(Arc::new(move || {
        count.fetch_add(1, SeqCst);
    }));
    let mut rt = runtime();
    rec.start();
    rt.frame(0.3);
    assert_eq!(rec.state(), RecorderState::Recording);
    let before = changes.load(SeqCst);
    rt.frame(0.4);
    assert_eq!(changes.load(SeqCst), before + 1, "the frame that unplugs wakes the app");
    assert_eq!(rec.state(), RecorderState::Interrupted("the microphone went away".into()));
    near(rec.elapsed(), 0.5, 1e-9);
    assert_eq!(rec.level(), Level::SILENT);
    let note = rec.stop().unwrap();
    near(note.duration, 0.5, 1e-9);
    assert_eq!(fake::read_wav(&note.path).unwrap().1.len(), 24_000);
    assert_eq!(rec.state(), RecorderState::Stopped);

    rec.start();
    rt.frame(0.6);
    assert!(matches!(rec.state(), RecorderState::Interrupted(_)));
    rec.cancel();
    assert!(!note.path.exists(), "cancel deletes an interrupted recording");
}

#[test]
fn the_waker_hears_state_changes_not_progress() {
    let dir = Scratch::new("waker");
    FakeMicrophone::silence().install();
    let rec = Recorder::to_file(dir.join("note.wav"));
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    rec.set_waker(Arc::new(move || {
        count.fetch_add(1, SeqCst);
    }));
    assert_eq!(calls.load(SeqCst), 0, "nothing happened yet");
    let mut rt = runtime();
    rec.start();
    assert_eq!(calls.load(SeqCst), 1);
    rt.frame(0.1);
    rt.frame(0.1);
    let _ = (rec.elapsed(), rec.level());
    assert_eq!(calls.load(SeqCst), 1, "elapsed time and levels don't wake");
    rec.pause();
    rec.resume();
    rec.stop().unwrap();
    assert_eq!(calls.load(SeqCst), 4);
}

#[test]
fn wav_and_sample_inputs_record_their_samples() {
    let dir = Scratch::new("wav");
    let ramp: Vec<f32> = (0..800).map(|i| i as f32 / 800.0 - 0.5).collect();
    let rec = Recorder::to_file(dir.join("ramp.wav")).fake(FakeMicrophone::samples(8_000, ramp.clone()));
    let mut rt = runtime();
    rec.start();
    rt.frame(0.25); // 2000 samples: the 800-sample ramp loops.
    let ramp_file = rec.stop().unwrap().path;
    let (rate, got) = fake::read_wav(&ramp_file).unwrap();
    assert_eq!((rate, got.len()), (8_000, 2_000));
    for (i, s) in got.iter().enumerate() {
        near(*s as f64, ramp[i % 800] as f64, 1.0 / 32_000.0);
    }

    let again = Recorder::to_file(dir.join("again.wav")).fake(FakeMicrophone::wav(&ramp_file));
    again.start();
    rt.frame(0.125);
    let (_, copy) = fake::read_wav(&again.stop().unwrap().path).unwrap();
    assert_eq!(copy.len(), 1_000);
    for (a, b) in copy.iter().zip(&got) {
        near(*a as f64, *b as f64, 1.0 / 32_000.0);
    }

    let missing = Recorder::to_file(dir.join("x.wav")).fake(FakeMicrophone::wav(dir.join("missing.wav")));
    missing.start();
    assert!(matches!(missing.state(), RecorderState::Failed(m) if m.contains("couldn't read")));
}

#[test]
fn an_unwritable_path_fails_to_start() {
    FakeMicrophone::silence().install();
    let rec = Recorder::to_file(std::env::temp_dir().join("rune-recorder-missing-dir").join("deeper").join("note.wav"));
    rec.start();
    assert!(matches!(rec.state(), RecorderState::Failed(m) if m.contains("couldn't write")), "{:?}", rec.state());
}

#[test]
fn wav_files_parse_formats_and_reject_others() {
    assert!(fake::parse_wav(b"RIFF\0\0\0\0WAVE").is_err());
    assert!(fake::parse_wav(b"not a wav").is_err());
    // Stereo 32-bit float, mixed to mono.
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&3u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&22_050u32.to_le_bytes());
    b.extend_from_slice(&(22_050u32 * 8).to_le_bytes());
    b.extend_from_slice(&8u16.to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&16u32.to_le_bytes());
    for s in [0.5f32, -0.5, 1.0, 0.0] {
        b.extend_from_slice(&s.to_le_bytes());
    }
    assert_eq!(fake::parse_wav(&b).unwrap(), (22_050, vec![0.0, 0.5]));
}

/// The real backend's recorder object builds for both formats without preparing or starting
/// it, so nothing is captured and no permission prompt appears.
#[cfg(target_os = "macos")]
#[test]
fn the_macos_recorder_builds_without_capturing() {
    use objc2_video::msg_send;
    use objc2_video::runtime::Bool;
    let dir = Scratch::new("macos");
    for (name, format) in [("a.m4a", RecordFormat::Aac), ("b.wav", RecordFormat::Wav)] {
        let opts = Options { format, sample_rate: 44_100, channels: 1, fake: None };
        let recorder = macos::make_recorder(&dir.join(name), &opts).unwrap_or_else(|e| panic!("{name}: {e}"));
        let recording: Bool = unsafe { msg_send![&*recorder, isRecording] };
        assert!(!recording.as_bool());
    }
    assert!(macos::preflight().is_ok(), "an unbundled test binary is attributed to its terminal");
    // Reading the status never prompts.
    let _ = macos::permission();
}
