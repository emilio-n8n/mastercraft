# Changelog — MasterCraft

All notable changes to MasterCraft are documented here.

## [Unreleased]

### Added

- WAV loading (mono/stereo, integer and float bit depths) and float-WAV export.
- Min/max waveform display drawn with an egui `Painter`.
- Gain (dB) and peak normalisation, processed in a cancellable CraftKit job
  with progress and undo/redo.
- Status bar with duration, sample rate and channel count.
- Audio, waveform, gain, WAV round-trip and document JSON tests.

### Changed

- Document model now holds `title`, `source_path`, `gain_db` and `normalize`.
  The initial template model was never released, so the schema version stays 1.

## [0.1.0] - 2026-11-22

### Added

- Initial CraftKit-based application shell.
- Versioned project files with atomic save.
- Undo/redo through the shared document history.
- Background jobs with progress and cancellation.
- Application-specific document model and UI.
