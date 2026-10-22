//! Audio samples: decoding, non-destructive edits and browser listings.
//!
//! Decoding happens on the UI thread when the user opens a sample. The
//! result is a stereo buffer at the engine's sample rate plus the parameters
//! that produced it, so the editor can draw the source and re-render quickly.
//! Nothing here runs on the audio thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

pub use crate::audio::messages::SampleBuffer;

/// Audio extensions the browser lists and the decoder accepts.
pub const SAMPLE_EXTENSIONS: [&str; 3] = ["wav", "flac", "mp3"];

/// Upper bound for one sample. Longer files are almost certainly not clips,
/// and decoding them synchronously would stall the interface.
pub const MAX_SOURCE_SECS: f32 = 120.0;

/// Buckets used by the editor's waveform.
pub const WAVEFORM_BUCKETS: usize = 2048;

/// Delay before an edit is rendered again while the user is dragging.
pub const RENDER_DEBOUNCE: Duration = Duration::from_millis(120);

/// Non-destructive edit parameters. All lengths are in seconds; `trim_end`
/// is measured from the end of the source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleEdits {
    pub trim_start: f32,
    pub trim_end: f32,
    pub gain_db: f32,
    pub fade_in: f32,
    pub fade_out: f32,
    pub reverse: bool,
    pub normalize: bool,
}

impl Default for SampleEdits {
    fn default() -> Self {
        Self {
            trim_start: 0.0,
            trim_end: 0.0,
            gain_db: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            reverse: false,
            normalize: false,
        }
    }
}

/// A rendered sample ready to be assigned to a clip slot.
///
/// The original decoded frames travel with it so the editor can be reopened
/// from a slot without reading the file again.
#[derive(Clone, Debug)]
pub struct SampleClip {
    pub path: PathBuf,
    pub name: String,
    pub source: Arc<Vec<f32>>,
    pub source_rate: u32,
    pub source_frames: usize,
    pub edits: SampleEdits,
    pub buffer: Arc<SampleBuffer>,
    /// Coarse waveform envelope for matrix and arrangement mini-views.
    pub peaks: Vec<f32>,
}

impl SampleClip {
    pub fn duration_secs(&self) -> f32 {
        self.buffer.duration_secs()
    }

    pub fn extension(&self) -> String {
        self.path
            .extension()
            .map(|extension| extension.to_string_lossy().to_ascii_uppercase())
            .unwrap_or_else(|| "AUDIO".to_string())
    }
}

/// An audio file open in the editor.
pub struct DecodedSample {
    pub path: PathBuf,
    pub name: String,
    pub source: Arc<Vec<f32>>,
    pub source_rate: u32,
    pub source_frames: usize,
    pub edits: SampleEdits,
    /// Max absolute amplitude per bucket, for drawing the source waveform.
    pub peaks: Vec<f32>,
    rendered: Option<Arc<SampleBuffer>>,
    /// Last rendered clip, reused while the edits stay unchanged.
    clip: Option<Arc<SampleClip>>,
    /// When an edit last changed, used to debounce expensive renders.
    pub last_edit: Instant,
}

impl DecodedSample {
    /// Decode WAV, FLAC or MP3 into stereo frames.
    pub fn load(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path)
            .map_err(|error| format!("could not open {}: {error}", path.display()))?;
        let stream = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
            hint.with_extension(&extension.to_ascii_lowercase());
        }

        let mut format = symphonia::default::get_probe()
            .probe(
                &hint,
                stream,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|error| format!("unsupported audio file: {error}"))?;

        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| "the file has no audio track".to_string())?;
        let track_id = track.id;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|params| params.audio())
            .ok_or_else(|| "the audio track has no codec parameters".to_string())?;
        let mut source_rate = params.sample_rate.unwrap_or(48_000).max(1);

        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(|error| format!("unsupported codec: {error}"))?;

        let mut source: Vec<f32> = Vec::new();
        let mut scratch: Vec<f32> = Vec::new();

        loop {
            let packet = match format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => break,
                Err(SymphoniaError::ResetRequired) => break,
                Err(SymphoniaError::IoError(_)) => break,
                Err(error) => return Err(format!("read failed: {error}")),
            };
            if packet.track_id != track_id {
                continue;
            }

            let decoded = match decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(SymphoniaError::DecodeError(_)) | Err(SymphoniaError::IoError(_)) => continue,
                Err(error) => return Err(format!("decode failed: {error}")),
            };

            let spec = decoded.spec();
            source_rate = spec.rate().max(1);
            let channels = spec.channels().count().max(1);
            let sample_count = decoded.samples_interleaved();
            if sample_count == 0 {
                continue;
            }
            scratch.resize(sample_count, 0.0);
            decoded.copy_to_slice_interleaved(&mut scratch);

            let frames = sample_count / channels;
            if ((source.len() / 2 + frames) as f32 / source_rate as f32) > MAX_SOURCE_SECS {
                return Err(format!(
                    "sample is longer than {} minutes",
                    MAX_SOURCE_SECS as u32 / 60
                ));
            }

            // Normalize every layout to stereo: duplicate mono, take the
            // first two channels of a multichannel file.
            source.reserve(frames * 2);
            for frame in scratch.chunks_exact(channels) {
                let left = frame[0];
                let right = if channels > 1 { frame[1] } else { frame[0] };
                source.push(left);
                source.push(right);
            }
        }

        if source.is_empty() {
            return Err("the file decoded to no audio".to_string());
        }

        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| "Sample".to_string());
        let source_frames = source.len() / 2;
        let peaks = compute_peaks(&source, WAVEFORM_BUCKETS);

        Ok(Self {
            path: path.to_path_buf(),
            name,
            source: Arc::new(source),
            source_rate,
            source_frames,
            edits: SampleEdits::default(),
            peaks,
            rendered: None,
            clip: None,
            last_edit: Instant::now(),
        })
    }

    /// Rebuild an editor around a sample that is already assigned to a slot.
    pub fn from_clip(clip: &SampleClip) -> Self {
        let peaks = compute_peaks(&clip.source, WAVEFORM_BUCKETS);
        Self {
            path: clip.path.clone(),
            name: clip.name.clone(),
            source: Arc::clone(&clip.source),
            source_rate: clip.source_rate,
            source_frames: clip.source_frames,
            edits: clip.edits,
            peaks,
            rendered: Some(Arc::clone(&clip.buffer)),
            clip: Some(Arc::new(clip.clone())),
            last_edit: Instant::now(),
        }
    }

    pub fn source_duration_secs(&self) -> f32 {
        self.source_frames as f32 / self.source_rate.max(1) as f32
    }

    /// Seconds of usable audio after trimming.
    pub fn edited_duration_secs(&self) -> f32 {
        let start = self
            .edits
            .trim_start
            .clamp(0.0, self.source_duration_secs());
        let end = (self.source_duration_secs() - self.edits.trim_end).max(start);
        (end - start).max(0.0)
    }

    pub fn mark_dirty(&mut self) {
        self.rendered = None;
        self.clip = None;
        self.last_edit = Instant::now();
    }

    pub fn is_dirty(&self) -> bool {
        self.rendered.is_none()
    }

    /// Render now if the debounce window has passed (or `force` is set).
    pub fn refresh(&mut self, target_rate: u32, force: bool) {
        if !self.is_dirty() {
            return;
        }
        if !force && self.last_edit.elapsed() < RENDER_DEBOUNCE {
            return;
        }
        self.rendered = Some(Arc::new(render_edit(
            &self.source,
            self.source_rate,
            self.source_frames,
            self.edits,
            target_rate,
        )));
    }

    /// Render and wrap the result for a clip slot.
    pub fn to_clip(&mut self, target_rate: u32) -> Arc<SampleClip> {
        if let Some(clip) = &self.clip {
            return Arc::clone(clip);
        }
        self.refresh(target_rate, true);
        let buffer = self.rendered.as_ref().map(Arc::clone).unwrap_or_else(|| {
            Arc::new(render_edit(
                &self.source,
                self.source_rate,
                self.source_frames,
                self.edits,
                target_rate,
            ))
        });
        let clip = Arc::new(SampleClip {
            path: self.path.clone(),
            name: self.name.clone(),
            source: Arc::clone(&self.source),
            source_rate: self.source_rate,
            source_frames: self.source_frames,
            edits: self.edits,
            peaks: compute_peaks(&buffer.frames, 160),
            buffer,
        });
        self.clip = Some(Arc::clone(&clip));
        clip
    }
}

/// Compute a max-absolute-amplitude envelope from interleaved stereo frames.
pub fn compute_peaks(source: &[f32], buckets: usize) -> Vec<f32> {
    let frames = source.len() / 2;
    if frames == 0 || buckets == 0 {
        return vec![0.0; buckets];
    }
    let mut peaks = vec![0.0_f32; buckets];
    for (index, frame) in source.chunks_exact(2).enumerate() {
        let bucket = (index * buckets / frames).min(buckets - 1);
        let value = frame[0].abs().max(frame[1].abs());
        if value > peaks[bucket] {
            peaks[bucket] = value;
        }
    }
    peaks
}

/// Apply trim, reverse, normalize, gain and fades, then resample.
fn render_edit(
    source: &[f32],
    source_rate: u32,
    source_frames: usize,
    edits: SampleEdits,
    target_rate: u32,
) -> SampleBuffer {
    let source_rate = source_rate.max(1);
    let start = (edits.trim_start.max(0.0) * source_rate as f32) as usize;
    let end = ((source_frames as f32 - edits.trim_end.max(0.0) * source_rate as f32).ceil()
        as usize)
        .min(source_frames);
    let start = start.min(source_frames.saturating_sub(1));
    let end = end.max(start + 1).min(source_frames);
    let count = end - start;

    let gain = if edits.gain_db.is_finite() {
        10.0_f32.powf(edits.gain_db.clamp(-48.0, 24.0) / 20.0)
    } else {
        1.0
    };

    let normalize_gain = if edits.normalize {
        let mut peak = 0.0_f32;
        for frame in source[start * 2..end * 2].chunks_exact(2) {
            peak = peak.max(frame[0].abs()).max(frame[1].abs());
        }
        if peak > 1.0e-6 {
            (1.0 / peak).min(32.0)
        } else {
            1.0
        }
    } else {
        1.0
    };

    let fade_in = (edits.fade_in.max(0.0) * source_rate as f32) as usize;
    let fade_out = (edits.fade_out.max(0.0) * source_rate as f32) as usize;

    let mut edited = Vec::with_capacity(count * 2);
    for index in 0..count {
        let source_index = if edits.reverse {
            end - 1 - index
        } else {
            start + index
        };
        let left = source[source_index * 2];
        let right = source[source_index * 2 + 1];

        let fade_in_gain = if fade_in > 0 {
            (index as f32 / fade_in as f32).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let fade_out_gain = if fade_out > 0 {
            ((count - 1 - index) as f32 / fade_out as f32).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let envelope = gain * normalize_gain * fade_in_gain.min(fade_out_gain);
        edited.push(left * envelope);
        edited.push(right * envelope);
    }

    let target_rate = target_rate.max(8_000);
    if source_rate == target_rate {
        return SampleBuffer {
            frames: edited,
            sample_rate: target_rate,
        };
    }

    // Linear resampling: predictable and fast enough for interactive edits.
    let ratio = target_rate as f64 / source_rate as f64;
    let out_frames = ((count as f64) * ratio).round().max(1.0) as usize;
    let mut frames = Vec::with_capacity(out_frames * 2);
    for index in 0..out_frames {
        let position = index as f64 / ratio;
        let base = position.floor() as usize;
        let frac = (position - base as f64) as f32;
        let next = (base + 1).min(count - 1);
        for channel in 0..2 {
            let a = edited[base * 2 + channel];
            let b = edited[next * 2 + channel];
            frames.push(a + (b - a) * frac);
        }
    }
    SampleBuffer {
        frames,
        sample_rate: target_rate,
    }
}

/// One row in the browser.
#[derive(Clone, Debug)]
pub struct SampleEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// List child folders and audio files. Directories come first; hidden
/// entries are skipped so a home directory does not drown the list.
pub fn list_directory(dir: &Path) -> Result<Vec<SampleEntry>, String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|error| format!("could not read {}: {error}", dir.display()))?;
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let is_dir = path.is_dir();
        if is_dir {
            dirs.push(SampleEntry {
                name,
                path,
                is_dir: true,
            });
        } else if is_sample_path(&path) {
            files.push(SampleEntry {
                name,
                path,
                is_dir: false,
            });
        }
    }
    dirs.sort_by_key(|entry| entry.name.to_lowercase());
    files.sort_by_key(|entry| entry.name.to_lowercase());
    dirs.extend(files);
    Ok(dirs)
}

pub fn is_sample_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            SAMPLE_EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Where the browser starts: an explicit override, then a few conventional
/// music folders, then the home directory.
pub fn initial_directory() -> PathBuf {
    if let Some(dir) = std::env::var_os("REPLICANT_DAW_SAMPLES") {
        let dir = PathBuf::from(dir);
        if dir.is_dir() {
            return dir;
        }
    }
    let home = home_dir();
    for candidate in ["Samples", "Music", "Downloads", "Documents"] {
        let dir = home.join(candidate);
        if dir.is_dir() {
            return dir;
        }
    }
    if home.is_dir() {
        home
    } else {
        PathBuf::from(".")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a minimal 16-bit mono PCM WAV file.
    fn write_wav(path: &Path, rate: u32, samples: &[i16]) {
        let data_len = (samples.len() * 2) as u32;
        let mut bytes = Vec::with_capacity(44 + data_len as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(path, bytes).expect("write wav");
    }

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("replicant-daw-{name}-{}.wav", std::process::id()))
    }

    #[test]
    fn decodes_wav_to_stereo_at_source_rate() {
        let path = temp_path("decode");
        let samples: Vec<i16> = (0..800)
            .map(|i| ((i as f32 / 800.0) * 8000.0) as i16)
            .collect();
        write_wav(&path, 8_000, &samples);
        let decoded = DecodedSample::load(&path).expect("decode");
        assert_eq!(decoded.source_rate, 8_000);
        assert_eq!(decoded.source_frames, 800);
        assert_eq!(decoded.source.len(), 1600);
        // Mono is duplicated to both channels.
        assert_eq!(decoded.source[0], decoded.source[1]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn trims_gains_and_resamples() {
        let path = temp_path("edit");
        let samples: Vec<i16> = vec![1000; 800];
        write_wav(&path, 8_000, &samples);
        let mut decoded = DecodedSample::load(&path).expect("decode");
        decoded.edits = SampleEdits {
            trim_start: 0.01, // 80 frames
            trim_end: 0.01,   // 80 frames
            gain_db: 6.0,
            fade_in: 0.0,
            fade_out: 0.0,
            reverse: false,
            normalize: false,
        };
        let clip = decoded.to_clip(16_000);
        // 800 - 160 = 640 frames at 8 kHz, doubled to 1280 at 16 kHz.
        assert_eq!(clip.buffer.frame_count(), 1_280);
        assert_eq!(clip.buffer.sample_rate, 16_000);
        // +6 dB is ~1.995x.
        let expected = (1000.0 / 32768.0) * 1.995_262;
        assert!((clip.buffer.frames[0] - expected).abs() < 1.0e-4);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reverse_flips_the_trimmed_region() {
        let path = temp_path("reverse");
        let samples: Vec<i16> = (0..400).map(|i| i as i16).collect();
        write_wav(&path, 8_000, &samples);
        let mut decoded = DecodedSample::load(&path).expect("decode");
        decoded.edits = SampleEdits {
            trim_start: 0.0,
            trim_end: 0.0,
            gain_db: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            reverse: true,
            normalize: false,
        };
        let clip = decoded.to_clip(8_000);
        let first = clip.buffer.frames[0] * 32768.0;
        assert!((first - 399.0).abs() < 0.01, "first sample {first}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn lists_audio_files_and_directories() {
        let dir = std::env::temp_dir().join(format!("replicant-daw-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Kicks")).expect("dir");
        std::fs::write(dir.join("a.wav"), b"x").expect("wav");
        std::fs::write(dir.join("b.FLAC"), b"x").expect("flac");
        std::fs::write(dir.join("c.mp3"), b"x").expect("mp3");
        std::fs::write(dir.join("notes.txt"), b"x").expect("txt");
        let entries = list_directory(&dir).expect("list");
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].name, "Kicks");
        let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["Kicks", "a.wav", "b.FLAC", "c.mp3"]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
