//! WAV decoding/encoding, waveform analysis and sample-level gain processing.
//!
//! Samples are kept as interleaved `f32` values in `[-1.0, 1.0]`, whatever the
//! bit depth of the source file. Everything in this module is independent of
//! the UI and can be tested without a display.

use std::fmt;
use std::path::Path;

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};

/// Number of samples processed between progress/cancellation checks.
const CHUNK_SAMPLES: usize = 16_384;

/// Errors produced while reading or writing WAV files.
#[derive(Debug)]
pub enum AudioError {
    /// The WAV container could not be read or written.
    Hound(hound::Error),
    /// The file uses a sample format that this build cannot decode.
    Unsupported(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hound(error) => write!(formatter, "WAV error: {error}"),
            Self::Unsupported(reason) => write!(formatter, "Unsupported WAV format: {reason}"),
        }
    }
}

impl std::error::Error for AudioError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Hound(error) => Some(error),
            Self::Unsupported(_) => None,
        }
    }
}

impl From<hound::Error> for AudioError {
    fn from(error: hound::Error) -> Self {
        Self::Hound(error)
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
        let spec = WavSpec {
            channels: self.channels,
            sample_rate: self.sample_rate,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let mut writer = WavWriter::create(path, spec)?;
        for &sample in &self.samples {
            writer.write_sample(clamp_sample(sample))?;
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

    /// Compute `(min, max)` per column of the waveform.
    ///
    /// All channels of a frame contribute to its column. Empty columns return
    /// `(0.0, 0.0)`, which draws as a flat line.
    #[must_use]
    pub fn waveform(&self, columns: usize) -> Vec<(f32, f32)> {
        if columns == 0 {
            return Vec::new();
        }
        let frames = self.frames();
        let channels = self.channels.max(1) as usize;
        let mut peaks = Vec::with_capacity(columns);
        for column in 0..columns {
            let first = frames * column / columns;
            let last = frames * (column + 1) / columns;
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
        mut on_chunk: F,
    ) -> Result<(), String>
    where
        F: FnMut(f32) -> Result<(), String>,
    {
        let multiplier = if normalize {
            normalization_multiplier(&self.samples)
        } else {
            gain_multiplier(gain_db)
        };
        let total = self.samples.len();
        let mut done = 0;
        for chunk in self.samples.chunks_mut(CHUNK_SAMPLES) {
            process_chunk(chunk, multiplier);
            done += chunk.len();
            on_chunk(done as f32 / total as f32)?;
        }
        Ok(())
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
