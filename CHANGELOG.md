# Changelog — MasterCraft

All notable changes to MasterCraft are documented here.

## [Unreleased]

### Added

- Playback on the default output device with rodio: play/pause, stop, seek,
  click-to-seek and a live playhead in the waveform.
- Waveform selection by dragging, zoom from 1x to 64x, selection markers,
  selection duration and view scrolling.
- Editing tools on the selection: trim, silence and linear fade-in/out.
- Batch processing of multiple WAV files with a gain/normalize/fade preset,
  progress and cancellation; output files are suffixed `-processed`.
- 16-bit PCM WAV export alongside 32-bit float; the choice is persisted.
- Asynchronous WAV decoding with progress reporting for large files.
- Pure DSP tests for selection, trim, fade, range gain, PCM16, batch and
  playback helpers.

### Changed

- Document keeps the `mastercraft` format and schema 1. The new `export_format`
  field uses `#[serde(default)]`; a test confirms v0.1.0 schema 1 files load.
- Linux CI and release workflows install `libasound2-dev` for rodio/CPAL.

### Notes

- Audio samples are still memory-only: opening a JSON project requires loading
  the source WAV again. No spectral editing, real-time monitoring or ML denoise.

## [0.1.0] - 2026-11-22

### Added

- Initial CraftKit-based application shell.
- Versioned project files with atomic save.
- Undo/redo through the shared document history.
- Background jobs with progress and cancellation.
- Application-specific document model and UI.
- WAV loading, min/max waveform, gain and peak normalisation, float WAV export.
