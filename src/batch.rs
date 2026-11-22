//! Batch processing of WAV files through a shared preset.
//!
//! The DSP here is pure and testable; the job runner in the UI loops over the
//! files, reports progress and observes cancellation between files.

use std::path::{Path, PathBuf};

use crate::audio::{AudioBuffer, ExportFormat};

/// Processing settings applied to each file in a batch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatchPreset {
    /// Gain in decibels when normalisation is off.
    pub gain_db: f32,
    /// Peak-normalise each file instead of applying the gain.
    pub normalize: bool,
    /// Fade-in length in milliseconds (0 disables it).
    pub fade_in_ms: u32,
    /// Fade-out length in milliseconds (0 disables it).
    pub fade_out_ms: u32,
}

impl Default for BatchPreset {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            normalize: false,
            fade_in_ms: 0,
            fade_out_ms: 0,
        }
    }
}

/// Outcome of a batch run.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchReport {
    /// Number of files written successfully.
    pub processed: usize,
    /// `(file name, reason)` for each failure.
    pub failed: Vec<(String, String)>,
}

impl BatchReport {
    /// Short summary for the status bar.
    #[must_use]
    pub fn summary(&self) -> String {
        if self.failed.is_empty() {
            format!("{} file(s) processed", self.processed)
        } else {
            format!(
                "{} file(s) processed, {} failed",
                self.processed,
                self.failed.len()
            )
        }
    }
}

/// Convert milliseconds to frames at `sample_rate`, rounding up.
#[must_use]
pub fn fade_frames(milliseconds: u32, sample_rate: u32) -> usize {
    if sample_rate == 0 {
        return 0;
    }
    ((f64::from(milliseconds) * f64::from(sample_rate)) / 1000.0).ceil() as usize
}

/// Apply a preset to an in-memory buffer.
///
/// # Errors
///
/// Returns the first error produced by `on_progress`.
pub fn apply_preset<F>(
    buffer: &mut AudioBuffer,
    preset: &BatchPreset,
    mut on_progress: F,
) -> Result<(), String>
where
    F: FnMut(f32) -> Result<(), String>,
{
    let frames = buffer.frames();
    buffer.apply_gain(preset.gain_db, preset.normalize, |fraction| {
        on_progress(fraction * 0.6)
    })?;

    let fade_in = fade_frames(preset.fade_in_ms, buffer.sample_rate).min(frames);
    let fade_out = fade_frames(preset.fade_out_ms, buffer.sample_rate).min(frames);
    if fade_in > 0 {
        buffer.fade_range(0, frames, fade_in, true);
    }
    on_progress(0.8)?;
    if fade_out > 0 {
        buffer.fade_range(0, frames, fade_out, false);
    }
    on_progress(1.0)
}

/// Destination path for a processed file: `<stem>-processed.wav` next to it.
#[must_use]
pub fn output_path_for(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("audio");
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("wav");
    path.with_file_name(format!("{stem}-processed.{extension}"))
}

/// Load, process and write one WAV file.
///
/// # Errors
///
/// Returns a readable message when decoding, processing or writing fails.
pub fn process_file(
    path: &Path,
    preset: &BatchPreset,
    format: ExportFormat,
) -> Result<PathBuf, String> {
    let mut buffer =
        AudioBuffer::load_wav(path).map_err(|error| format!("{}: {error}", path.display()))?;
    apply_preset(&mut buffer, preset, |_| Ok(()))?;
    let output = output_path_for(path);
    buffer
        .write_wav_with_format(&output, format)
        .map_err(|error| format!("{}: {error}", output.display()))?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_input(dir: &Path, name: &str, samples: &[f32]) -> PathBuf {
        let path = dir.join(name);
        let buffer = AudioBuffer {
            samples: samples.to_vec(),
            sample_rate: 48_000,
            channels: 1,
        };
        buffer.write_wav(&path).expect("write fixture");
        path
    }

    #[test]
    fn fade_frames_rounds_up() {
        assert_eq!(fade_frames(0, 48_000), 0);
        assert_eq!(fade_frames(10, 48_000), 480);
        assert_eq!(fade_frames(1, 44_100), 45);
        assert_eq!(fade_frames(10, 0), 0);
    }

    #[test]
    fn preset_applies_gain_and_fades() {
        let mut buffer = AudioBuffer {
            samples: vec![1.0; 16],
            sample_rate: 1_000,
            channels: 1,
        };
        let preset = BatchPreset {
            gain_db: -6.0,
            normalize: false,
            fade_in_ms: 2,
            fade_out_ms: 2,
        };
        apply_preset(&mut buffer, &preset, |_| Ok(())).expect("preset");
        let expected = 10.0_f32.powf(-6.0 / 20.0);
        assert!(buffer.samples[0] < expected);
        assert!((buffer.samples[2] - expected).abs() < 1e-6);
        assert!(buffer.samples[15] < expected);
    }

    #[test]
    fn preset_normalizes_each_buffer() {
        let mut buffer = AudioBuffer {
            samples: vec![0.25, -0.125],
            sample_rate: 1_000,
            channels: 1,
        };
        let preset = BatchPreset {
            normalize: true,
            ..BatchPreset::default()
        };
        apply_preset(&mut buffer, &preset, |_| Ok(())).expect("preset");
        assert_eq!(buffer.samples[0], 1.0);
        assert_eq!(buffer.samples[1], -0.5);
    }

    #[test]
    fn preset_can_be_cancelled() {
        let mut buffer = AudioBuffer {
            samples: vec![1.0; 4],
            sample_rate: 1_000,
            channels: 1,
        };
        let error = apply_preset(&mut buffer, &BatchPreset::default(), |_| {
            Err("cancelled".to_owned())
        })
        .expect_err("cancel");
        assert_eq!(error, "cancelled");
    }

    #[test]
    fn processes_file_to_output_path() {
        let dir = tempdir().expect("tempdir");
        let input = write_input(dir.path(), "take.wav", &[0.5, -0.5, 0.25]);
        let preset = BatchPreset {
            gain_db: 6.0,
            ..BatchPreset::default()
        };
        let output = process_file(&input, &preset, ExportFormat::Pcm16).expect("process");
        assert_eq!(output, dir.path().join("take-processed.wav"));
        assert!(output.is_file());
        let loaded = AudioBuffer::load_wav(&output).expect("load output");
        let expected = 0.5 * 10.0_f32.powf(6.0 / 20.0);
        assert!((loaded.samples[0] - expected).abs() < 1e-4);
        assert!(fs::metadata(&output).expect("metadata").len() > 44);
    }
}
