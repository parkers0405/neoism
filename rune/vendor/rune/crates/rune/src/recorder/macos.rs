//! AVAudioRecorder on a thread of its own: it encodes AAC (or writes WAV) and meters the
//! input, so Rune never touches samples. The thread creates, drives and releases the recorder;
//! nothing about it needs the main thread. AVCaptureDevice reports and asks for microphone
//! access. CoreAudio's default input device is watched for changes.
use super::meter::Meter;
use super::{MicPermission, Options, RecordFormat, Recorder, Shared};
use objc2_av_foundation::AVMediaTypeAudio;
use objc2_foundation_video::{NSDictionary, NSError, NSString, NSURL};
use objc2_video::rc::{Allocated, Retained, autoreleasepool};
use objc2_video::runtime::{AnyClass, AnyObject, Bool};
use objc2_video::{class, msg_send};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// The default input device is checked this many meter ticks apart (about twice a second).
const DEVICE_CHECK_TICKS: u32 = 15;
/// `stop` and `cancel` give up waiting on the recorder thread after this long.
const REPLY_PATIENCE: Duration = Duration::from_secs(5);

// AVAudioSettings keys (AVFAudio, re-exported by AVFoundation).
#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {
    static AVFormatIDKey: &'static NSString;
    static AVSampleRateKey: &'static NSString;
    static AVNumberOfChannelsKey: &'static NSString;
    static AVEncoderAudioQualityKey: &'static NSString;
    static AVLinearPCMBitDepthKey: &'static NSString;
    static AVLinearPCMIsFloatKey: &'static NSString;
    static AVLinearPCMIsBigEndianKey: &'static NSString;
    static AVLinearPCMIsNonInterleaved: &'static NSString;
}

#[repr(C)]
struct PropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const PropertyAddress,
        qualifier_size: u32,
        qualifier: *const std::ffi::c_void,
        size: *mut u32,
        data: *mut std::ffi::c_void,
    ) -> i32;
}

/// The system's default input device, if there is one.
fn default_input() -> Option<u32> {
    let address = PropertyAddress {
        selector: u32::from_be_bytes(*b"dIn "), // kAudioHardwarePropertyDefaultInputDevice
        scope: u32::from_be_bytes(*b"glob"),
        element: 0,
    };
    let (mut device, mut size) = (0u32, 4u32);
    // kAudioObjectSystemObject is 1.
    let status = unsafe { AudioObjectGetPropertyData(1, &address, 0, std::ptr::null(), &mut size, (&raw mut device).cast()) };
    (status == 0 && device != 0).then_some(device)
}

pub(super) fn permission() -> MicPermission {
    let (Some(audio), Some(device)) = (unsafe { AVMediaTypeAudio }, AnyClass::get(c"AVCaptureDevice")) else {
        return MicPermission::Restricted;
    };
    let status: isize = unsafe { msg_send![device, authorizationStatusForMediaType: audio] };
    match status {
        3 => MicPermission::Granted,
        2 => MicPermission::Denied,
        1 => MicPermission::Restricted,
        _ => MicPermission::Undetermined,
    }
}

pub(super) fn request_permission(callback: Box<dyn FnOnce(MicPermission) + Send>) {
    if let Err(e) = preflight() {
        if cfg!(debug_assertions) {
            eprintln!("rune: {e}");
        }
        return callback(MicPermission::Denied);
    }
    let current = permission();
    let (Some(audio), Some(device)) = (unsafe { AVMediaTypeAudio }, AnyClass::get(c"AVCaptureDevice")) else {
        return callback(current);
    };
    if current != MicPermission::Undetermined {
        return callback(current);
    }
    let callback = Mutex::new(Some(callback));
    let block = block2_metal::RcBlock::new(move |granted: Bool| {
        if let Some(callback) = callback.lock().unwrap().take() {
            callback(if granted.as_bool() { MicPermission::Granted } else { MicPermission::Denied });
        }
    });
    unsafe {
        let _: () = msg_send![device, requestAccessForMediaType: audio, completionHandler: &*block];
    }
}

/// macOS ends a bundled app that touches the microphone without a usage description; refuse
/// instead. An unbundled binary is attributed to the terminal that ran it.
pub(super) fn preflight() -> Result<(), String> {
    unsafe {
        let bundle: Option<Retained<AnyObject>> = msg_send![class!(NSBundle), mainBundle];
        let Some(bundle) = bundle else { return Ok(()) };
        let path: Option<Retained<NSString>> = msg_send![&*bundle, bundlePath];
        if !path.is_some_and(|p| p.to_string().ends_with(".app")) {
            return Ok(());
        }
        let key = NSString::from_str("NSMicrophoneUsageDescription");
        let value: Option<Retained<AnyObject>> = msg_send![&*bundle, objectForInfoDictionaryKey: &*key];
        match value {
            Some(_) => Ok(()),
            None => Err("the app's Info.plist has no NSMicrophoneUsageDescription, so macOS won't let it record".into()),
        }
    }
}

fn number_u32(value: u32) -> Retained<AnyObject> {
    unsafe { msg_send![class!(NSNumber), numberWithUnsignedInt: value] }
}
fn number_f64(value: f64) -> Retained<AnyObject> {
    unsafe { msg_send![class!(NSNumber), numberWithDouble: value] }
}
fn number_bool(value: bool) -> Retained<AnyObject> {
    unsafe { msg_send![class!(NSNumber), numberWithBool: Bool::new(value)] }
}

/// An AVAudioRecorder for `path`, not yet prepared or recording: no file and no capture.
pub(super) fn make_recorder(path: &Path, opts: &Options) -> Result<Retained<AnyObject>, String> {
    let class = AnyClass::get(c"AVAudioRecorder").ok_or("the system recorder (AVAudioRecorder) is unavailable")?;
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let mut entries: Vec<(&NSString, Retained<AnyObject>)> = unsafe {
        vec![(AVSampleRateKey, number_f64(opts.sample_rate as f64)), (AVNumberOfChannelsKey, number_u32(opts.channels as u32))]
    };
    unsafe {
        match opts.format {
            RecordFormat::Aac => {
                entries.push((AVFormatIDKey, number_u32(u32::from_be_bytes(*b"aac "))));
                entries.push((AVEncoderAudioQualityKey, number_u32(0x60))); // AVAudioQualityHigh
            }
            RecordFormat::Wav => {
                entries.push((AVFormatIDKey, number_u32(u32::from_be_bytes(*b"lpcm"))));
                entries.push((AVLinearPCMBitDepthKey, number_u32(16)));
                entries.push((AVLinearPCMIsFloatKey, number_bool(false)));
                entries.push((AVLinearPCMIsBigEndianKey, number_bool(false)));
                entries.push((AVLinearPCMIsNonInterleaved, number_bool(false)));
            }
        }
    }
    let keys: Vec<&NSString> = entries.iter().map(|(k, _)| *k).collect();
    let values: Vec<&AnyObject> = entries.iter().map(|(_, v)| &**v).collect();
    let settings = NSDictionary::<NSString, AnyObject>::from_slices(&keys, &values);
    unsafe {
        let recorder: Allocated<AnyObject> = msg_send![class, alloc];
        let recorder: Result<Retained<AnyObject>, Retained<NSError>> = msg_send![recorder, initWithURL: &*url, settings: &*settings, error: _];
        recorder.map_err(|e| format!("couldn't record to {}: {}", path.display(), e.localizedDescription()))
    }
}

thread_local! {
    static ON_RECORDER_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) struct Session {
    commands: Sender<Command>,
}

pub(super) enum Command {
    Pause,
    Resume,
    Stop(Sender<Result<f64, String>>),
    Cancel(Sender<()>),
}

pub(super) fn start(shared: &Arc<Shared>, attempt: u64, path: &Path, opts: &Options) -> Result<Session, String> {
    let (commands, rx) = mpsc::channel();
    let (weak, path, opts) = (Arc::downgrade(shared), path.to_path_buf(), opts.clone());
    std::thread::Builder::new()
        .name("rune-recorder".into())
        .spawn(move || run(weak, attempt, path, opts, rx))
        .map_err(|e| format!("couldn't start the recorder thread: {e}"))?;
    Ok(Session { commands })
}

pub(super) fn pause(session: &Session) {
    let _ = session.commands.send(Command::Pause);
}

pub(super) fn resume(session: &Session) {
    let _ = session.commands.send(Command::Resume);
}

pub(super) fn stop(session: Session) -> Result<f64, String> {
    let (reply, done) = mpsc::channel();
    session.commands.send(Command::Stop(reply)).map_err(|_| "the recording already ended".to_string())?;
    done.recv_timeout(REPLY_PATIENCE).unwrap_or_else(|_| Err("the recorder didn't stop".into()))
}

/// The last handle dropped: stop and finalize the file (an m4a is unplayable until the
/// recorder writes its index), waiting a bounded time. On the recorder thread itself (the last
/// handle dropped while it reported) waiting would deadlock; dropping the session disconnects
/// it instead, and the thread stops the recorder as soon as its report returns.
pub(super) fn finish(session: Session) {
    if !ON_RECORDER_THREAD.try_with(|on| on.get()).unwrap_or(true) {
        let _ = stop(session);
    }
}

#[cfg(test)]
pub(super) fn mark_recorder_thread() {
    ON_RECORDER_THREAD.set(true);
}

#[cfg(test)]
pub(super) fn idle_session() -> (Session, Receiver<Command>) {
    let (commands, rx) = mpsc::channel();
    (Session { commands }, rx)
}

pub(super) fn cancel(session: Session) {
    let (reply, done) = mpsc::channel();
    if session.commands.send(Command::Cancel(reply)).is_ok() {
        let _ = done.recv_timeout(REPLY_PATIENCE);
    }
}

/// Prepares and starts the recorder: (recorder, the input device it started on).
fn open(path: &Path, opts: &Options) -> Result<(Retained<AnyObject>, Option<u32>), String> {
    let device = default_input().ok_or("no microphone is connected")?;
    let recorder = make_recorder(path, opts)?;
    unsafe {
        let _: () = msg_send![&*recorder, setMeteringEnabled: Bool::YES];
        let prepared: Bool = msg_send![&*recorder, prepareToRecord];
        if !prepared.as_bool() {
            return Err(format!("couldn't create {}", path.display()));
        }
        let started: Bool = msg_send![&*recorder, record];
        if !started.as_bool() {
            let _: Bool = msg_send![&*recorder, deleteRecording];
            return Err("couldn't start recording from the microphone".into());
        }
    }
    Ok((recorder, Some(device)))
}

fn run(weak: Weak<Shared>, attempt: u64, path: PathBuf, opts: Options, commands: Receiver<Command>) {
    ON_RECORDER_THREAD.set(true);
    let opened = autoreleasepool(|_| open(&path, &opts));
    let (recorder, device) = match opened {
        Ok(opened) => opened,
        Err(e) => {
            if let Some(shared) = weak.upgrade() {
                shared.failed(attempt, e);
            }
            return;
        }
    };
    if let Some(shared) = weak.upgrade() {
        shared.began(attempt);
    }
    let recorder = &*recorder;
    let current_time = || -> f64 { unsafe { msg_send![recorder, currentTime] } };
    let stop = || unsafe {
        let _: () = msg_send![recorder, stop];
    };
    let (mut meter, mut last, mut ticks, mut paused, mut recorded) = (Meter::default(), Instant::now(), 0u32, false, 0f64);
    loop {
        let command = commands.recv_timeout(Recorder::METER_INTERVAL);
        let done = autoreleasepool(|_| match command {
            Ok(Command::Pause) => {
                recorded = recorded.max(current_time());
                unsafe {
                    let _: () = msg_send![recorder, pause];
                }
                paused = true;
                false
            }
            Ok(Command::Resume) => {
                let _: Bool = unsafe { msg_send![recorder, record] };
                (paused, last) = (false, Instant::now());
                false
            }
            Ok(Command::Stop(reply)) => {
                // currentTime reads 0 once stopped.
                recorded = recorded.max(current_time());
                stop();
                let _ = reply.send(Ok(recorded));
                true
            }
            Ok(Command::Cancel(reply)) => {
                stop();
                let _: Bool = unsafe { msg_send![recorder, deleteRecording] };
                let _ = reply.send(());
                true
            }
            Err(RecvTimeoutError::Timeout) if paused => false,
            Err(RecvTimeoutError::Timeout) => {
                let Some(shared) = weak.upgrade() else {
                    stop();
                    return true;
                };
                let (peak, average, recording): (f32, f32, Bool) = unsafe {
                    let _: () = msg_send![recorder, updateMeters];
                    (msg_send![recorder, peakPowerForChannel: 0usize], msg_send![recorder, averagePowerForChannel: 0usize], msg_send![recorder, isRecording])
                };
                recorded = recorded.max(current_time());
                let now = Instant::now();
                meter.push_db(peak, average, (now - last).as_secs_f64());
                last = now;
                shared.metered(attempt, meter.level());
                ticks += 1;
                let lost = if !recording.as_bool() {
                    Some("recording stopped: the input device went away")
                } else if ticks % DEVICE_CHECK_TICKS == 0 && default_input() != device {
                    Some("the input device changed")
                } else {
                    None
                };
                match lost {
                    Some(reason) => {
                        stop();
                        shared.interrupted(attempt, reason.into(), recorded);
                        true
                    }
                    None => false,
                }
            }
            // Every handle is gone: keep the file.
            Err(RecvTimeoutError::Disconnected) => {
                stop();
                true
            }
        });
        if done {
            break;
        }
    }
}
