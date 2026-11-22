# MasterCraft

Audio restoration with waveform editing and WAV export

**MasterCraft is an independent, community open-source project.** It is not an
official product of, endorsed by, or affiliated with any third-party vendor it
may interoperate with.

Built on [CraftKit](https://github.com/emilio-n8n/craftkit), a small native Rust
desktop foundation.

## Features

- Native Rust desktop UI (egui/eframe), no web runtime.
- Load mono/stereo WAV files (8/16/24/32-bit integer and 32-bit float), with
  decoding in a background job so large files never freeze the UI.
- Playback on the default output device through rodio: play/pause, stop, seek,
  click-to-seek and a moving playhead. No device available? The editor keeps
  working and shows a clear message.
- Waveform drawn with min/max peaks per column, with drag selection, markers,
  selection duration and 1x–64x zoom (buttons and mouse wheel).
- Editing on the selection (or the whole buffer when nothing is selected):
  gain in dB, peak normalisation, trim, silence and linear fade-in/out. Each
  edit is one undo step.
- Export to 32-bit float or 16-bit PCM WAV; the format is saved in the project.
- Batch processing: queue several WAV files and apply one preset
  (gain/normalize/fade in/out) with progress and cancellation; each result is
  written next to its source as `<name>-processed.wav`.
- Versioned JSON project files (schema 1) with atomic saves and a backward
  migration test for files written by v0.1.0.
- Bounded undo/redo through the shared document history.
- Background jobs with progress and cooperative cancellation.
- Offline-first: no account, no telemetry, no network required.
- No real-time effects, spectral editing or machine-learning denoise: this is a
  precise offline editor for level and time-domain cleanup.

## Run

```bash
cargo run --release -- --sample
cargo run --release -- --open project.json
```

Without arguments the application opens an empty document.

- `--sample` generates a two-second 440 Hz sine buffer (no external asset).
- `--open <file>` opens a project. Audio samples are not stored in the JSON
  project, so the source WAV must be loaded again after opening.
- `--version` prints the version.
- `--help` prints usage.

Load a WAV, drag on the waveform to select a range, adjust gain or enable
**Normalize to 0 dBFS**, then press **Apply gain** to process the selection in a
cancellable job. **Trim**, **Silence** and the fades act on the selection too.
Progress and a **Cancel** button appear in the status bar. Normalisation scales
the selection peak to exactly 1.0 and overrides the gain slider; the output is
always clamped to avoid clipping.

## Build and test

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python3 tools/launcher_check.py .
```

On Linux, playback needs ALSA headers: `libasound2-dev` (Debian/Ubuntu) or the
equivalent on other distributions. Both GitHub workflows install it.

## Project layout

```text
src/main.rs       CLI and window startup
src/app.rs        UI, menus, actions and lifecycle
src/audio.rs      WAV I/O, waveform, DSP primitives (pure logic)
src/batch.rs      Batch preset and per-file processing
src/document.rs   Persisted document model (schema 1)
src/playback.rs   rodio playback and pure playhead helpers
```

The application depends on pinned CraftKit crates from Git. Keep the tag in
`Cargo.toml` and `Cargo.lock` consistent when upgrading the foundation.

## File format

Projects are JSON envelopes:

```json
{
  "format": "mastercraft",
  "schema_version": 1,
  "app_version": "0.1.0",
  "document": { }
}
```

Only `title`, `source_path`, `gain_db`, `normalize` and `export_format` are
persisted. New fields added after v0.1.0 use `#[serde(default)]`, so schema 1
files from the first release still load without a version bump. Audio samples
stay in memory and are never written to the JSON envelope.

## Licence

Licensed under either of:

- Apache License, Version 2.0 (`LICENSE-APACHE`)
- MIT license (`LICENSE-MIT`)

at your option.

## Third-party notices

See [THIRD_PARTY.md](THIRD_PARTY.md) and [LAUNCHER.md](LAUNCHER.md).
