//! WAV decoding/encoding, waveform analysis and sample-level gain processing.
//!
//! Samples are kept as interleaved `f32` values in `[-1.0, 1.0]`, whatever the
//! bit depth of the source file. Everything in this module is independent of
//! the UI and can be tested without a display.

use std::fmt;
use std::path::Path;

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use serde::{Deserialize, Serialize};

/// Number of samples processed between progress/cancellation checks.
const CHUNK_SAMPLES: usize = 16_384;

/// WAV encoding chosen when exporting a buffer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    /// 32-bit IEEE float, lossless round trip for normalised samples.
    #[default]
    Float32,
    /// 16-bit signed PCM, broadly compatible with audio players.
    Pcm16,
}

impl ExportFormat {
    /// Human-readable label used by the UI.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Float32 => "Float 32-bit",
            Self::Pcm16 => "PCM 16-bit",
        }
    }
}

/// A half-open frame range `start..end` selected on the waveform.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Selection {
    /// First selected frame.
    pub start: usize,
    /// One past the last selected frame.
    pub end: usize,
}

impl Selection {
    /// Build a selection, swapping inverted endpoints.
    #[must_use]
    pub fn new(first: usize, second: usize) -> Self {
        Self {
            start: first.min(second),
            end: first.max(second),
        }
    }

    /// Clamp both endpoints to a buffer of `frames` frames.
    #[must_use]
    pub fn clamped(self, frames: usize) -> Self {
        Self {
            start: self.start.min(frames),
            end: self.end.clamp(self.start.min(frames), frames),
        }
    }

    /// Number of selected frames.
    #[must_use]
    pub fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Whether the selection is empty.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// Selection duration in seconds at `sample_rate`.
    #[must_use]
    pub fn duration(self, sample_rate: u32) -> f64 {
        if sample_rate == 0 {
            0.0
        } else {
            self.len() as f64 / f64::from(sample_rate)
        }
    }
}

/// Errors produced while reading or writing WAV files.
#[derive(Debug)]
pub enum AudioError {
    /// The WAV container could not be read or written.
    Hound(hound::Error),
    /// The file could not be opened or its metadata read.
    Io(std::io::Error),
    /// The file uses a sample format that this build cannot decode.
    Unsupported(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hound(error) => write!(formatter, "WAV error: {error}"),
            Self::Io(error) => write!(formatter, "File error: {error}"),
            Self::Unsupported(reason) => write!(formatter, "Unsupported WAV format: {reason}"),
        }
    }
}

impl std::error::Error for AudioError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Hound(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Unsupported(_) => None,
        }
    }
}

impl From<hound::Error> for AudioError {
    fn from(error: hound::Error) -> Self {
        Self::Hound(error)
    }
}

impl From<std::io::Error> for AudioError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Interleaved audio samples and the information needed to play them back.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioBuffer {
    /// Interleaved samples normalised to `[-1.0, 1.0]`.
    pub samples: Vec<f32>,
    /// Samples per second.
    pub sample_rate: u32,
    /// Number of interleaved channels (1 = mono, 2 = stereo).
    pub channels: u16,
}

impl AudioBuffer {
    /// Decode a WAV file into normalised `f32` samples.
    ///
    /// Mono and stereo files are supported. Integer sources (8, 16, 24 and
    /// 32-bit) are scaled to `[-1.0, 1.0]`; 32-bit float sources are read as-is.
    ///
    /// # Errors
    ///
    /// Returns an error for unreadable files, malformed containers or sample
    /// formats that are not supported.
    pub fn load_wav(path: impl AsRef<Path>) -> Result<Self, AudioError> {
        let mut reader = WavReader::open(path)?;
        let spec = reader.spec();
        if spec.channels == 0 {
            return Err(AudioError::Unsupported("zero channels".to_owned()));
        }
        if spec.sample_rate == 0 {
            return Err(AudioError::Unsupported("zero sample rate".to_owned()));
        }

        let mut samples = Vec::with_capacity(reader.len() as usize);
        read_samples(&mut reader, &spec, &mut samples)?;
        Ok(Self {
            samples,
            sample_rate: spec.sample_rate,
            channels: spec.channels,
        })
    }

    /// Encode this buffer as a 32-bit float WAV file.
    ///
    /// Float WAV keeps the round trip lossless. Samples are clamped to
    /// `[-1.0, 1.0]` so an unclamped buffer cannot produce out-of-range data.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be created or written.
    pub fn write_wav(&self, path: impl AsRef<Path>) -> Result<(), AudioError> {
        self.write_wav_with_format(path, ExportFormat::Float32)
    }

    /// Encode this buffer using the requested WAV format.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be created or written.
    pub fn write_wav_with_format(
        &self,
        path: impl AsRef<Path>,
        format: ExportFormat,
    ) -> Result<(), AudioError> {
        let spec = WavSpec {
            channels: self.channels,
            sample_rate: self.sample_rate,
            bits_per_sample: match format {
                ExportFormat::Float32 => 32,
                ExportFormat::Pcm16 => 16,
            },
            sample_format: match format {
                ExportFormat::Float32 => SampleFormat::Float,
                ExportFormat::Pcm16 => SampleFormat::Int,
            },
        };
        let mut writer = WavWriter::create(path, spec)?;
        for &sample in &self.samples {
            let clamped = clamp_sample(sample);
            match format {
                ExportFormat::Float32 => writer.write_sample(clamped)?,
                ExportFormat::Pcm16 => writer.write_sample(sample_to_i16(clamped))?,
            }
        }
        writer.finalize()?;
        Ok(())
    }

    /// Number of audio frames (one frame is one sample per channel).
    #[must_use]
    pub fn frames(&self) -> usize {
        let channels = self.channels.max(1) as usize;
        self.samples.len() / channels
    }

    /// Duration in seconds.
    #[must_use]
    pub fn duration(&self) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.frames() as f64 / f64::from(self.sample_rate)
    }

    /// Compute `(min, max)` per column of the whole waveform.
    #[must_use]
    pub fn waveform(&self, columns: usize) -> Vec<(f32, f32)> {
        let frames = self.frames();
        self.waveform_range(0, frames, columns)
    }

    /// Compute `(min, max)` per column for a frame range (zoom viewport).
    ///
    /// All channels of a frame contribute to its column. Empty columns return
    /// `(0.0, 0.0)`, which draws as a flat line.
    #[must_use]
    pub fn waveform_range(
        &self,
        start_frame: usize,
        end_frame: usize,
        columns: usize,
    ) -> Vec<(f32, f32)> {
        if columns == 0 {
            return Vec::new();
        }
        let (first_frame, last_frame) = self.frame_range(start_frame, end_frame);
        let frames = last_frame - first_frame;
        let channels = self.channels.max(1) as usize;
        let mut peaks = Vec::with_capacity(columns);
        for column in 0..columns {
            let first = first_frame + frames * column / columns;
            let last = first_frame + frames * (column + 1) / columns;
            let mut min = f32::INFINITY;
            let mut max = f32::NEG_INFINITY;
            for frame in first..last {
                let start = frame * channels;
                let end = start + channels;
                for &sample in &self.samples[start..end] {
                    if sample.is_finite() {
                        min = min.min(sample);
                        max = max.max(sample);
                    }
                }
            }
            peaks.push(if min <= max { (min, max) } else { (0.0, 0.0) });
        }
        peaks
    }

    /// Apply a gain in dB, optionally peak-normalising to full scale first.
    ///
    /// When `normalize` is set, the peak of the input is scaled to exactly
    /// `1.0` and `gain_db` is ignored: peak normalisation already fixes the
    /// output level. The result is clamped, so it never clips. Silence stays
    /// silence.
    ///
    /// `on_chunk` is called after each processed chunk with the fraction of
    /// samples handled so far. Returning an error stops the work early; this is
    /// how the UI job observes cancellation.
    ///
    /// # Errors
    ///
    /// Returns the first error produced by `on_chunk`.
    pub fn apply_gain<F>(
        &mut self,
        gain_db: f32,
        normalize: bool,
        on_chunk: F,
    ) -> Result<(), String>
    where
        F: FnMut(f32) -> Result<(), String>,
    {
        let frames = self.frames();
        self.apply_gain_range(gain_db, normalize, 0, frames, on_chunk)
    }

    /// Apply gain or normalisation to a frame range only.
    ///
    /// Normalisation uses the peak of the selected range, not of the whole
    /// buffer. Progress is reported for the processed part of the buffer.
    ///
    /// # Errors
    ///
    /// Returns the first error produced by `on_chunk`.
    pub fn apply_gain_range<F>(
        &mut self,
        gain_db: f32,
        normalize: bool,
        start_frame: usize,
        end_frame: usize,
        mut on_chunk: F,
    ) -> Result<(), String>
    where
        F: FnMut(f32) -> Result<(), String>,
    {
        let (from, to) = self.byte_range(start_frame, end_frame);
        let multiplier = if normalize {
            normalization_multiplier(&self.samples[from..to])
        } else {
            gain_multiplier(gain_db)
        };
        let total = to - from;
        let mut done = 0;
        for chunk in self.samples[from..to].chunks_mut(CHUNK_SAMPLES) {
            process_chunk(chunk, multiplier);
            done += chunk.len();
            on_chunk(done as f32 / total as f32)?;
        }
        Ok(())
    }

    /// Keep only the selected frames, returning a new buffer.
    #[must_use]
    pub fn trim(&self, start_frame: usize, end_frame: usize) -> Self {
        let (from, to) = self.byte_range(start_frame, end_frame);
        Self {
            samples: self.samples[from..to].to_vec(),
            sample_rate: self.sample_rate,
            channels: self.channels,
        }
    }

    /// Replace a frame range with silence.
    pub fn silence_range(&mut self, start_frame: usize, end_frame: usize) {
        let (from, to) = self.byte_range(start_frame, end_frame);
        for sample in &mut self.samples[from..to] {
            *sample = 0.0;
        }
    }

    /// Apply a linear fade-in or fade-out across a frame range.
    ///
    /// `fade_frames` is clamped to the range length. A fade-in ramps from 0 to
    /// 1 over the first frames; a fade-out ramps from 1 to 0 over the last.
    /// The rest of the range is left untouched.
    pub fn fade_range(
        &mut self,
        start_frame: usize,
        end_frame: usize,
        fade_frames: usize,
        fade_in: bool,
    ) {
        let channels = self.channels.max(1) as usize;
        let (from_frame, to_frame) = self.frame_range(start_frame, end_frame);
        let length = to_frame - from_frame;
        let fade = fade_frames.min(length);
        for index in 0..fade {
            let frame = if fade_in {
                from_frame + index
            } else {
                to_frame - fade + index
            };
            let factor = if fade_in {
                (index + 1) as f32 / fade as f32
            } else {
                (fade - index) as f32 / fade as f32
            };
            let first = frame * channels;
            for sample in &mut self.samples[first..first + channels] {
                *sample *= factor;
            }
        }
    }

    fn frame_range(&self, start_frame: usize, end_frame: usize) -> (usize, usize) {
        let frames = self.frames();
        let start = start_frame.min(frames);
        (start, end_frame.clamp(start, frames))
    }

    fn byte_range(&self, start_frame: usize, end_frame: usize) -> (usize, usize) {
        let channels = self.channels.max(1) as usize;
        let (start, end) = self.frame_range(start_frame, end_frame);
        (start * channels, end * channels)
    }

    /// A deterministic sine wave for the `--sample` startup mode.
    ///
    /// Two seconds of a 440 Hz mono sine at half amplitude, 44.1 kHz.
    #[must_use]
    pub fn sample_sine() -> Self {
        let sample_rate = 44_100;
        let frames = sample_rate * 2;
        let frequency = 440.0;
        let samples = (0..frames)
            .map(|frame| {
                let phase =
                    2.0 * std::f64::consts::PI * frequency * frame as f64 / sample_rate as f64;
                (0.5 * phase.sin()) as f32
            })
            .collect();
        Self {
            samples,
            sample_rate,
            channels: 1,
        }
    }
}

fn read_samples<R: std::io::Read>(
    reader: &mut WavReader<R>,
    spec: &WavSpec,
    samples: &mut Vec<f32>,
) -> Result<(), AudioError> {
    match (spec.sample_format, spec.bits_per_sample) {
        (SampleFormat::Float, 32) => {
            for sample in reader.samples::<f32>() {
                samples.push(sample?);
            }
        }
        (SampleFormat::Int, 8) => {
            for sample in reader.samples::<i8>() {
                samples.push(f32::from(sample?) / 128.0);
            }
        }
        (SampleFormat::Int, 16) => {
            for sample in reader.samples::<i16>() {
                samples.push(f32::from(sample?) / 32_768.0);
            }
        }
        (SampleFormat::Int, 24 | 32) => {
            let divisor = 2.0_f32.powi(i32::from(spec.bits_per_sample) - 1);
            for sample in reader.samples::<i32>() {
                samples.push(sample? as f32 / divisor);
            }
        }
        (format, bits) => {
            return Err(AudioError::Unsupported(format!(
                "{format:?} at {bits} bits"
            )));
        }
    }
    Ok(())
}

/// Linear multiplier for a gain expressed in decibels.
#[must_use]
pub fn gain_multiplier(gain_db: f32) -> f32 {
    if gain_db.is_finite() {
        10.0_f32.powf(gain_db / 20.0)
    } else {
        1.0
    }
}

/// Multiplier that scales the peak of `samples` to exactly `1.0`.
///
/// Returns `1.0` for silence so that no division by zero can occur.
#[must_use]
pub fn normalization_multiplier(samples: &[f32]) -> f32 {
    let peak = samples
        .iter()
        .filter(|sample| sample.is_finite())
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    if peak > 0.0 && peak.is_finite() {
        1.0 / peak
    } else {
        1.0
    }
}

/// Multiply a slice of samples and clamp the result to `[-1.0, 1.0]`.
///
/// Non-finite inputs are replaced by silence. The job processor calls this on
/// chunks so it can report progress and check for cancellation between chunks.
pub fn process_chunk(samples: &mut [f32], multiplier: f32) {
    let factor = if multiplier.is_finite() {
        multiplier
    } else {
        1.0
    };
    for sample in samples.iter_mut() {
        *sample = if sample.is_finite() {
            clamp_sample(*sample * factor)
        } else {
            0.0
        };
    }
}

fn clamp_sample(sample: f32) -> f32 {
    sample.clamp(-1.0, 1.0)
}

fn sample_to_i16(sample: f32) -> i16 {
    let scaled = (clamp_sample(sample) * 32_768.0).round();
    if scaled >= 32_767.0 {
        32_767
    } else if scaled <= -32_768.0 {
        -32_768
    } else {
        scaled as i16
    }
}

/// Decode a WAV file while reporting byte-based progress.
///
/// Used by the background loading job so large files never block the UI. The
/// callback may return an error to cancel the read.
///
/// # Errors
///
/// Returns an error for unreadable files, malformed containers or when the
/// callback cancels.
pub fn load_wav_with_progress<F>(
    path: impl AsRef<Path>,
    mut on_progress: F,
) -> Result<AudioBuffer, AudioError>
where
    F: FnMut(f32) -> Result<(), String>,
{
    let file = std::fs::File::open(path)?;
    let total = file.metadata()?.len().max(1);
    let mut reader = ProgressReader {
        inner: std::io::BufReader::new(file),
        bytes_read: 0,
        total,
        on_progress: &mut on_progress,
        last_report: 0,
    };
    let mut wav = WavReader::new(&mut reader)?;
    let spec = wav.spec();
    if spec.channels == 0 {
        return Err(AudioError::Unsupported("zero channels".to_owned()));
    }
    if spec.sample_rate == 0 {
        return Err(AudioError::Unsupported("zero sample rate".to_owned()));
    }
    let mut samples = Vec::with_capacity(wav.len() as usize);
    read_samples(&mut wav, &spec, &mut samples)?;
    on_progress(1.0).map_err(AudioError::Unsupported)?;
    Ok(AudioBuffer {
        samples,
        sample_rate: spec.sample_rate,
        channels: spec.channels,
    })
}

struct ProgressReader<'a, F> {
    inner: std::io::BufReader<std::fs::File>,
    bytes_read: u64,
    total: u64,
    on_progress: &'a mut F,
    last_report: u64,
}

impl<F> std::io::Read for ProgressReader<'_, F>
where
    F: FnMut(f32) -> Result<(), String>,
{
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.bytes_read += read as u64;
        if self.bytes_read - self.last_report >= 64 * 1024 {
            self.last_report = self.bytes_read;
            let fraction = (self.bytes_read as f32 / self.total as f32).min(1.0);
            (self.on_progress)(fraction).map_err(std::io::Error::other)?;
        }
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(samples: &[f32], channels: u16) -> AudioBuffer {
        AudioBuffer {
            samples: samples.to_vec(),
            sample_rate: 48_000,
            channels,
        }
    }

    #[test]
    fn wav_round_trip() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("roundtrip.wav");
        let original = buffer(&[0.0, 0.5, -0.5, 1.0, -1.0, 0.25], 2);

        original.write_wav(&path).expect("write WAV");
        let loaded = AudioBuffer::load_wav(&path).expect("read WAV");

        assert_eq!(loaded, original);
        assert_eq!(loaded.channels, 2);
        assert_eq!(loaded.sample_rate, 48_000);
    }

    #[test]
    fn reads_sixteen_bit_pcm() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("pcm16.wav");
        let spec = WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::create(&path, spec).expect("writer");
        for sample in [0_i16, 16_384, -16_384] {
            writer.write_sample(sample).expect("write sample");
        }
        writer.finalize().expect("finalize");

        let loaded = AudioBuffer::load_wav(&path).expect("read PCM");
        assert_eq!(loaded.channels, 1);
        assert_eq!(loaded.sample_rate, 44_100);
        assert!((loaded.samples[0] - 0.0).abs() < 1e-6);
        assert!((loaded.samples[1] - 0.5).abs() < 1e-4);
        assert!((loaded.samples[2] + 0.5).abs() < 1e-4);
    }

    #[test]
    fn normalize_reaches_full_scale_without_clipping() {
        let mut audio = buffer(&[-0.5, 0.25, 0.0, -0.1], 1);
        audio.apply_gain(12.0, true, |_| Ok(())).expect("apply");

        let peak = audio
            .samples
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max);
        assert!((peak - 1.0).abs() < 1e-6, "peak was {peak}");
        assert!(audio.samples.iter().all(|sample| sample.is_finite()));
        assert!(audio.samples.iter().all(|sample| sample.abs() <= 1.0));
        assert_eq!(audio.samples[0], -1.0);
        assert!((audio.samples[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn gain_positive_negative_and_silence() {
        let mut positive = buffer(&[0.25], 1);
        positive.apply_gain(6.0, false, |_| Ok(())).expect("apply");
        let expected_positive = 0.25 * 10.0_f32.powf(6.0 / 20.0);
        assert!((positive.samples[0] - expected_positive).abs() < 1e-6);

        let mut negative = buffer(&[0.25], 1);
        negative.apply_gain(-6.0, false, |_| Ok(())).expect("apply");
        let expected_negative = 0.25 * 10.0_f32.powf(-6.0 / 20.0);
        assert!((negative.samples[0] - expected_negative).abs() < 1e-6);

        let mut silence = buffer(&[0.0, 0.0], 1);
        silence.apply_gain(24.0, false, |_| Ok(())).expect("apply");
        assert_eq!(silence.samples, vec![0.0, 0.0]);

        let mut clipping = buffer(&[0.9], 1);
        clipping.apply_gain(20.0, false, |_| Ok(())).expect("apply");
        assert_eq!(clipping.samples[0], 1.0);
    }

    #[test]
    fn apply_gain_reports_progress_and_can_stop_early() {
        let samples = vec![0.5_f32; 40_000];
        let mut audio = buffer(&samples, 1);
        let mut fractions = Vec::new();
        audio
            .apply_gain(0.0, false, |fraction| {
                fractions.push(fraction);
                Ok(())
            })
            .expect("apply");
        assert!(fractions.len() > 1, "expected more than one chunk");
        assert_eq!(fractions.last(), Some(&1.0));

        let mut stopped = buffer(&samples, 1);
        let error = stopped
            .apply_gain(0.0, false, |_| Err("cancelled".to_owned()))
            .expect_err("stop");
        assert_eq!(error, "cancelled");
    }

    #[test]
    fn waveform_min_max_per_column() {
        let audio = buffer(&[-1.0, -0.5, 0.0, 0.5, 1.0, 0.5, -0.5, -1.0], 1);
        let peaks = audio.waveform(4);
        assert_eq!(
            peaks,
            vec![(-1.0, -0.5), (0.0, 0.5), (0.5, 1.0), (-1.0, -0.5)]
        );
    }

    #[test]
    fn waveform_handles_stereo_and_empty_columns() {
        let audio = buffer(&[0.2, -0.4, 0.8, -0.6], 2);
        assert_eq!(audio.frames(), 2);
        assert_eq!(audio.waveform(2), vec![(-0.4, 0.2), (-0.6, 0.8)]);

        let many = audio.waveform(5);
        assert_eq!(
            many,
            vec![(0.0, 0.0), (0.0, 0.0), (-0.4, 0.2), (0.0, 0.0), (-0.6, 0.8)]
        );
        assert!(audio.waveform(0).is_empty());
    }

    #[test]
    fn sample_sine_is_bounded_and_deterministic() {
        let a = AudioBuffer::sample_sine();
        let b = AudioBuffer::sample_sine();
        assert_eq!(a, b);
        assert_eq!(a.sample_rate, 44_100);
        assert_eq!(a.channels, 1);
        assert_eq!(a.frames(), 88_200);
        assert!((a.duration() - 2.0).abs() < 1e-6);
        assert!(a.samples.iter().all(|sample| sample.abs() <= 0.5 + 1e-6));
    }

    #[test]
    fn selection_normalizes_and_clamps() {
        assert_eq!(Selection::new(7, 3), Selection { start: 3, end: 7 });
        assert_eq!(
            Selection { start: 4, end: 99 }.clamped(10),
            Selection { start: 4, end: 10 }
        );
        assert_eq!(Selection { start: 2, end: 6 }.len(), 4);
        assert!(Selection { start: 2, end: 2 }.is_empty());
        assert!(
            (Selection {
                start: 0,
                end: 22_050
            }
            .duration(44_100)
                - 0.5)
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn trim_keeps_only_selected_frames() {
        let audio = buffer(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 2);
        let trimmed = audio.trim(1, 3);
        assert_eq!(trimmed.samples, vec![3.0, 4.0, 5.0, 6.0]);
        assert_eq!(trimmed.channels, 2);
        assert_eq!(trimmed.sample_rate, 48_000);
        assert_eq!(audio.trim(0, 99).samples, audio.samples);
    }

    #[test]
    fn fade_in_and_out_ramp_linearly() {
        let mut audio = buffer(&[1.0, 1.0, 1.0, 1.0, 1.0], 1);
        audio.fade_range(0, 5, 3, true);
        assert_eq!(audio.samples[0], 1.0 / 3.0);
        assert_eq!(audio.samples[1], 2.0 / 3.0);
        assert_eq!(audio.samples[2], 1.0);
        assert_eq!(audio.samples[3], 1.0);

        let mut audio = buffer(&[1.0, 1.0, 1.0, 1.0, 1.0], 1);
        audio.fade_range(0, 5, 2, false);
        assert_eq!(audio.samples[2], 1.0);
        assert_eq!(audio.samples[3], 1.0);
        assert_eq!(audio.samples[4], 0.5);
    }

    #[test]
    fn gain_and_normalization_apply_only_to_selection() {
        let mut audio = buffer(&[0.25, 0.5, 0.25, 0.5], 1);
        audio
            .apply_gain_range(6.0, false, 1, 3, |_| Ok(()))
            .expect("range gain");
        let multiplier = 10.0_f32.powf(6.0 / 20.0);
        assert_eq!(audio.samples[0], 0.25);
        assert!((audio.samples[1] - 0.5 * multiplier).abs() < 1e-6);
        assert!((audio.samples[2] - 0.25 * multiplier).abs() < 1e-6);
        assert_eq!(audio.samples[3], 0.5);

        let mut audio = buffer(&[1.0, 0.25, 0.25, 1.0], 1);
        audio
            .apply_gain_range(0.0, true, 1, 3, |_| Ok(()))
            .expect("range normalize");
        assert_eq!(audio.samples[0], 1.0);
        assert_eq!(audio.samples[1], 1.0);
        assert_eq!(audio.samples[2], 1.0);
        assert_eq!(audio.samples[3], 1.0);
    }

    #[test]
    fn silence_range_zeroes_only_selection() {
        let mut audio = buffer(&[0.5, 0.5, 0.5], 1);
        audio.silence_range(1, 2);
        assert_eq!(audio.samples, vec![0.5, 0.0, 0.5]);
    }

    #[test]
    fn pcm16_export_round_trip() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("pcm16-export.wav");
        let original = buffer(&[0.0, 0.5, -0.5, 1.0, -1.0], 1);

        original
            .write_wav_with_format(&path, ExportFormat::Pcm16)
            .expect("write PCM16");
        let spec = hound::WavReader::open(&path).expect("open").spec();
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);

        let loaded = AudioBuffer::load_wav(&path).expect("read PCM16");
        assert_eq!(loaded.sample_rate, original.sample_rate);
        for (expected, actual) in original.samples.iter().zip(&loaded.samples) {
            assert!((expected - actual).abs() < 1e-4, "{expected} vs {actual}");
        }
    }

    #[test]
    fn duration_of_empty_buffer_is_zero() {
        let audio = AudioBuffer {
            samples: Vec::new(),
            sample_rate: 0,
            channels: 0,
        };
        assert_eq!(audio.duration(), 0.0);
        assert!(audio.waveform(3).iter().all(|peak| *peak == (0.0, 0.0)));
    }
}
