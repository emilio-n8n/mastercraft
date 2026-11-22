//! Audio playback through rodio.
//!
//! Decoding and mixing happen on rodio's own audio thread, so the UI only polls
//! a position. When no output device is available the player reports an error
//! instead of panicking; editing and export stay usable.

use std::num::NonZero;
use std::time::Duration;

use rodio::buffer::SamplesBuffer;
use rodio::{DeviceSinkBuilder, MixerDeviceSink, Player as RodioPlayer};

use crate::audio::AudioBuffer;

/// Visible playhead state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Playhead {
    /// Current position in seconds.
    pub position: f64,
    /// Buffer duration in seconds.
    pub duration: f64,
    /// Whether audio is playing.
    pub playing: bool,
}

/// Owns the device sink and the player, and tracks the visible playhead.
pub struct Playback {
    sink: Option<MixerDeviceSink>,
    player: Option<RodioPlayer>,
    head: Playhead,
    error: Option<String>,
}

impl Default for Playback {
    fn default() -> Self {
        Self::new()
    }
}

impl Playback {
    /// Create an uninitialised player; the device is opened on first play.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sink: None,
            player: None,
            head: Playhead {
                position: 0.0,
                duration: 0.0,
                playing: false,
            },
            error: None,
        }
    }

    /// Last device error, if any.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Record a device error so the UI can show it.
    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    /// Current playhead state.
    #[must_use]
    pub fn playhead(&self) -> Playhead {
        self.head
    }

    /// Current position in seconds.
    #[must_use]
    pub fn position(&self) -> f64 {
        self.head.position
    }

    /// Buffer duration in seconds.
    #[must_use]
    pub fn duration(&self) -> f64 {
        self.head.duration
    }

    /// Whether audio is currently playing.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.head.playing
    }

    /// Stop playback and adopt a new buffer at position zero.
    pub fn set_buffer(&mut self, buffer: &AudioBuffer) {
        self.stop();
        self.head.duration = buffer.duration();
        self.head.position = 0.0;
    }

    /// Start playback of `buffer` from the beginning.
    ///
    /// # Errors
    ///
    /// Returns an error when no output device is available or the buffer has
    /// invalid channel/rate metadata.
    pub fn play(&mut self, buffer: &AudioBuffer) -> Result<(), String> {
        let channels = NonZero::new(buffer.channels).ok_or("invalid channel count")?;
        let rate = NonZero::new(buffer.sample_rate).ok_or("invalid sample rate")?;
        self.ensure_device()?;
        let player = self
            .player
            .as_ref()
            .ok_or_else(|| "audio device is not available".to_owned())?;
        player.clear();
        player.append(SamplesBuffer::new(channels, rate, buffer.samples.clone()));
        self.head.duration = buffer.duration();
        self.head.position = 0.0;
        self.error = None;
        player.play();
        self.head.playing = true;
        Ok(())
    }

    /// Pause or resume playback.
    pub fn toggle(&mut self) {
        let Some(player) = self.player.as_ref() else {
            return;
        };
        if self.head.playing {
            player.pause();
            self.head.playing = false;
        } else {
            player.play();
            self.head.playing = true;
        }
    }

    /// Resume if a source is queued, otherwise start playback of `buffer`.
    pub fn toggle_or_play(&mut self, buffer: &AudioBuffer) -> Result<(), String> {
        let needs_start = self.player.as_ref().is_none_or(|player| player.empty());
        if needs_start {
            self.play(buffer)
        } else {
            self.toggle();
            Ok(())
        }
    }

    /// Stop playback and reset the playhead.
    pub fn stop(&mut self) {
        if let Some(player) = self.player.as_ref() {
            player.stop();
        }
        self.head.playing = false;
        self.head.position = 0.0;
    }

    /// Move the playhead to `seconds`, clamped to the buffer duration.
    pub fn seek(&mut self, seconds: f64) {
        let target = clamp_seek(seconds, self.head.duration);
        if let Some(player) = self.player.as_ref() {
            let _ = player.try_seek(Duration::from_secs_f64(target));
        }
        self.head.position = target;
    }

    /// Refresh the playhead from rodio. Called once per UI frame.
    pub fn tick(&mut self) {
        if !self.head.playing {
            return;
        }
        if let Some(player) = self.player.as_ref() {
            if player.empty() {
                self.head.playing = false;
                self.head.position = self.head.duration;
            } else {
                self.head.position = player.get_pos().as_secs_f64().min(self.head.duration);
            }
        }
    }

    fn ensure_device(&mut self) -> Result<(), String> {
        if self.sink.is_some() {
            return Ok(());
        }
        let sink = DeviceSinkBuilder::open_default_sink()
            .map_err(|error| format!("No audio output device: {error}"))?;
        let player = RodioPlayer::connect_new(sink.mixer());
        self.sink = Some(sink);
        self.player = Some(player);
        Ok(())
    }
}

/// Clamp a seek target to the available duration.
#[must_use]
pub fn clamp_seek(seconds: f64, duration: f64) -> f64 {
    if !seconds.is_finite() {
        return 0.0;
    }
    seconds.clamp(0.0, duration.max(0.0))
}

/// Format seconds as `M:SS.m` for the status bar.
#[must_use]
pub fn format_time(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0:00.0".to_owned();
    }
    let whole = seconds as u64;
    let minutes = whole / 60;
    let secs = whole % 60;
    let tenths = ((seconds - whole as f64) * 10.0) as u64;
    format!("{minutes}:{secs:02}.{tenths}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seek_is_clamped_and_finite() {
        assert_eq!(clamp_seek(-2.0, 10.0), 0.0);
        assert_eq!(clamp_seek(42.0, 10.0), 10.0);
        assert_eq!(clamp_seek(4.5, 10.0), 4.5);
        assert_eq!(clamp_seek(f64::NAN, 10.0), 0.0);
    }

    #[test]
    fn time_is_formatted() {
        assert_eq!(format_time(0.0), "0:00.0");
        assert_eq!(format_time(65.25), "1:05.2");
        assert_eq!(format_time(-1.0), "0:00.0");
    }

    #[test]
    fn playback_without_device_stays_usable() {
        let mut player = Playback::new();
        assert!(!player.is_playing());
        player.toggle();
        player.stop();
        player.seek(5.0);
        player.tick();
        assert_eq!(player.position(), 0.0);
    }
}
