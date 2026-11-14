//! Ogg (Vorbis, Opus) and WebM/Matroska audio, which AVFoundation can't read: decoded in Rust
//! to 16-bit PCM, streamed to a temporary WAV file that AVPlayer then plays. Only the
//! compressed clip is held in memory while it decodes.

use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_OPUS, CODEC_TYPE_VORBIS, DecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// A WAV's sizes are 32-bit: at most this many 16-bit samples (about 6 hours of 48 kHz stereo).
const MAX_SAMPLES: u64 = (u32::MAX as u64 - 36) / 2;
const TOO_LONG: &str = "too long to decode (a WAV holds about 6 hours of 48 kHz stereo)";

/// What a decode produced.
struct Format {
    rate: u32,
    channels: u16,
    /// Interleaved samples to keep, when the container says where the clip ends.
    keep: Option<u64>,
}

/// An Ogg or EBML (WebM, Matroska) stream, by its first bytes.
pub(super) fn sniff(bytes: &[u8]) -> bool {
    bytes.starts_with(b"OggS") || bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3])
}

/// Decodes the first Vorbis or Opus track, handing `push` interleaved samples as they come.
fn decode(bytes: Vec<u8>, push: &mut dyn FnMut(&[i16]) -> Result<(), String>) -> Result<Format, String> {
    let fail = |e: Error| format!("can't decode this audio: {e}");
    let ogg = bytes.starts_with(b"OggS");
    let stream = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
    let options = FormatOptions { enable_gapless: true, ..Default::default() };
    let mut format = symphonia::default::get_probe()
        .format(&Hint::new(), stream, &options, &MetadataOptions::default())
        .map_err(fail)?
        .format;
    let track = format
        .tracks()
        .iter()
        .find(|t| matches!(t.codec_params.codec, CODEC_TYPE_VORBIS | CODEC_TYPE_OPUS))
        .ok_or("no Vorbis or Opus audio track")?;
    let (id, params) = (track.id, track.codec_params.clone());
    let mut decoder = if params.codec == CODEC_TYPE_OPUS {
        Decoder::Opus(Opus::new(params.extra_data.as_deref().unwrap_or_default())?)
    } else {
        Decoder::Vorbis(symphonia::default::get_codecs().make(&params, &DecoderOptions::default()).map_err(fail)?, None)
    };
    let mut out = Format { rate: params.sample_rate.unwrap_or(48_000), channels: 0, keep: None };
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(fail(e)),
        };
        if packet.track_id() != id {
            continue;
        }
        match &mut decoder {
            Decoder::Vorbis(d, buffer) => match d.decode(&packet) {
                Ok(audio) => {
                    let spec = *audio.spec();
                    out.rate = spec.rate;
                    out.channels = spec.channels.count() as u16;
                    let buffer = buffer.get_or_insert_with(|| SampleBuffer::new(audio.capacity() as u64, spec));
                    if buffer.capacity() < audio.capacity() * spec.channels.count() {
                        *buffer = SampleBuffer::new(audio.capacity() as u64, spec);
                    }
                    buffer.copy_interleaved_ref(audio);
                    push(buffer.samples())?;
                }
                // A damaged packet is skipped, as a player would.
                Err(Error::DecodeError(_)) => {}
                Err(e) => return Err(fail(e)),
            },
            Decoder::Opus(o) => {
                out.rate = 48_000;
                out.channels = o.channels as u16;
                push(o.decode(&packet.data)?)?;
            }
        }
    }
    // Ogg Opus ends at its last granule position, which counts the pre-skip (RFC 7845 §4).
    if let (Decoder::Opus(o), Some(end), true) = (&decoder, params.n_frames, ogg) {
        out.keep = Some(end.saturating_sub(o.pre_skip as u64) * o.channels as u64).filter(|&keep| keep > 0);
    }
    Ok(out)
}

enum Decoder {
    Vorbis(Box<dyn symphonia::core::codecs::Decoder>, Option<SampleBuffer<i16>>),
    Opus(Opus),
}

struct Opus {
    decoder: opuscule::Decoder,
    channels: usize,
    pre_skip: usize,
    /// Decoded frames still to drop for the pre-skip.
    skip: usize,
    gain: f32,
    buffer: Vec<f32>,
    out: Vec<i16>,
}

impl Opus {
    /// From the `OpusHead` identification header (RFC 7845 §5.1).
    fn new(head: &[u8]) -> Result<Self, String> {
        if head.len() < 19 || &head[..8] != b"OpusHead" {
            return Err("Opus track without an OpusHead header".into());
        }
        let channels = head[9] as usize;
        if head[18] != 0 || !(1..=2).contains(&channels) {
            return Err(format!("Opus with {channels} channels isn't supported (mono and stereo are)"));
        }
        let pre_skip = u16::from_le_bytes([head[10], head[11]]) as usize;
        let gain_db = i16::from_le_bytes([head[16], head[17]]) as f32 / 256.0;
        let layout = if channels == 1 { opuscule::Channels::Mono } else { opuscule::Channels::Stereo };
        let decoder = opuscule::Decoder::new(opuscule::SampleRate::Hz48000, layout);
        // 120 ms, the longest packet.
        let buffer = vec![0.0; 5760 * channels];
        Ok(Self { decoder, channels, pre_skip, skip: pre_skip, gain: 10f32.powf(gain_db / 20.0), buffer, out: Vec::new() })
    }

    fn decode(&mut self, packet: &[u8]) -> Result<&[i16], String> {
        self.out.clear();
        let frames = match self.decoder.decode(Some(packet), &mut self.buffer, false) {
            Ok(frames) => frames,
            // A damaged packet is skipped, as a player would.
            Err(opuscule::Error::InvalidPacket) => return Ok(&self.out),
            Err(e) => return Err(format!("Opus: {e}")),
        };
        let start = self.skip.min(frames);
        self.skip -= start;
        let gain = self.gain;
        let samples = &self.buffer[start * self.channels..frames * self.channels];
        self.out.extend(samples.iter().map(|s| ((s * gain).clamp(-1.0, 1.0) * 32767.0) as i16));
        Ok(&self.out)
    }
}

/// The 44-byte header of a 16-bit PCM WAV holding `samples` interleaved samples.
fn header(rate: u32, channels: u16, samples: u64) -> Result<[u8; 44], String> {
    let too_long = || TOO_LONG.to_string();
    let data = u32::try_from(samples.checked_mul(2).ok_or_else(too_long)?).map_err(|_| too_long())?;
    let riff = data.checked_add(36).ok_or_else(too_long)?;
    let block = channels.checked_mul(2).ok_or("too many channels")?;
    let byte_rate = rate.checked_mul(block as u32).ok_or("sample rate too high")?;
    let mut h = [0u8; 44];
    let fields: [&[u8]; 12] = [
        b"RIFF",
        &riff.to_le_bytes(),
        b"WAVEfmt ",
        &16u32.to_le_bytes(),
        &1u16.to_le_bytes(),
        &channels.to_le_bytes(),
        &rate.to_le_bytes(),
        &byte_rate.to_le_bytes(),
        &block.to_le_bytes(),
        &16u16.to_le_bytes(),
        b"data",
        &data.to_le_bytes(),
    ];
    let mut at = 0;
    for field in fields {
        h[at..at + field.len()].copy_from_slice(field);
        at += field.len();
    }
    Ok(h)
}

/// A decoded clip on disk for AVPlayer, removed when dropped.
pub(super) struct Wav(PathBuf);

impl Wav {
    /// Decodes an Ogg or WebM clip into a new WAV file, streaming its samples to disk.
    pub fn decode(bytes: Vec<u8>) -> Result<Self, String> {
        let io = |e: std::io::Error| format!("can't write decoded audio: {e}");
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join("rune-audio");
        if NEXT.load(Ordering::Relaxed) == 0 {
            sweep(&dir);
        }
        std::fs::create_dir_all(&dir).map_err(io)?;
        // Removes the file again on any failure below.
        let wav = Self(dir.join(format!("{}-{}.wav", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))));
        let mut out = BufWriter::new(std::fs::File::create(&wav.0).map_err(io)?);
        out.write_all(&[0; 44]).map_err(io)?;
        let (mut samples, mut bytes_out) = (0u64, Vec::new());
        let format = decode(bytes, &mut |chunk| {
            samples += chunk.len() as u64;
            if samples > MAX_SAMPLES {
                return Err(TOO_LONG.into());
            }
            bytes_out.clear();
            bytes_out.extend(chunk.iter().flat_map(|s| s.to_le_bytes()));
            out.write_all(&bytes_out).map_err(io)
        })?;
        let samples = format.keep.map_or(samples, |keep| keep.min(samples));
        if samples == 0 || format.channels == 0 {
            return Err("no audio decoded".into());
        }
        let header = header(format.rate, format.channels, samples)?;
        let mut file = out.into_inner().map_err(|e| io(e.into_error()))?;
        file.set_len(44 + samples * 2).map_err(io)?;
        file.seek(SeekFrom::Start(0)).and_then(|_| file.write_all(&header)).map_err(io)?;
        Ok(wav)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Wav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Removes files a process that didn't exit cleanly left a day or more ago.
fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let day = std::time::Duration::from_secs(24 * 60 * 60);
    for entry in entries.flatten() {
        let old = entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|age| age > day));
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
    }

    /// A WAV's rate, channels and samples, checking its sizes agree with the file.
    fn read(path: &Path) -> (u32, u16, Vec<i16>) {
        let b = std::fs::read(path).unwrap();
        let u32_at = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        assert_eq!((&b[..4], &b[8..16], &b[36..40]), (&b"RIFF"[..], &b"WAVEfmt "[..], &b"data"[..]));
        assert_eq!((u32_at(4) as usize, u32_at(40) as usize), (b.len() - 8, b.len() - 44));
        let samples = b[44..].chunks(2).map(|s| i16::from_le_bytes([s[0], s[1]])).collect();
        (u32_at(24), u16::from_le_bytes([b[22], b[23]]), samples)
    }

    /// Each fixture is 0.5 s of a 440 Hz sine.
    #[test]
    fn decodes_ogg_vorbis_ogg_opus_and_webm_opus() {
        for (name, rate, channels) in [("sine-vorbis.ogg", 22_050, 2), ("sine-opus.ogg", 48_000, 1), ("sine-opus.webm", 48_000, 1)] {
            let bytes = fixture(name);
            assert!(sniff(&bytes), "{name}");
            let wav = Wav::decode(bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
            let (got_rate, got_channels, samples) = read(wav.path());
            assert_eq!((got_rate, got_channels), (rate, channels), "{name}");
            let duration = samples.len() as f64 / channels as f64 / rate as f64;
            assert!((duration - 0.5).abs() < 0.03, "{name}: {duration}");
            if name == "sine-opus.ogg" {
                assert_eq!(samples.len(), 24_000, "Ogg's pre-skip and end trim, each applied once");
            }
            // A sine, not silence: 1/8 amplitude (lavfi's default), RMS ~0.09.
            let mid = &samples[samples.len() / 4..samples.len() * 3 / 4];
            let rms = (mid.iter().map(|&s| (s as f64 / 32768.0).powi(2)).sum::<f64>() / mid.len() as f64).sqrt();
            assert!((0.05..0.6).contains(&rms), "{name}: rms {rms}");
            let path = wav.path().to_owned();
            drop(wav);
            assert!(!path.exists(), "{name}: removed on drop");
        }
    }

    #[test]
    fn a_clip_too_long_for_a_wav_fails_clearly() {
        assert!(header(48_000, 2, MAX_SAMPLES).is_ok());
        let e = header(48_000, 2, MAX_SAMPLES + 1).unwrap_err();
        assert!(e.contains("too long"), "{e}");
        assert!(header(48_000, 2, u64::MAX).is_err());
    }

    #[test]
    fn rejects_what_isnt_ogg_or_webm() {
        assert!(!sniff(b"RIFF\0\0\0\0WAVE"));
        assert!(Wav::decode(b"OggS but not really".to_vec()).is_err());
    }
}
