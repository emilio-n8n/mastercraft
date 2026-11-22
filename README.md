# MasterCraft

Audio restoration with waveform editing and WAV export

**MasterCraft is an independent, community open-source project.** It is not an
official product of, endorsed by, or affiliated with any third-party vendor it
may interoperate with.

Built on [CraftKit](https://github.com/emilio-n8n/craftkit), a small native Rust
desktop foundation.

## Features

- Native Rust desktop UI (egui/eframe), no web runtime.
- Load mono/stereo WAV files (8/16/24/32-bit integer and 32-bit float).
- Waveform display drawn with min/max peaks per screen column.
- Gain in dB and peak normalisation, processed in a cancellable background job.
- Export to 32-bit float WAV, with a lossless round trip for exported files.
- Bounded undo/redo of editing settings **and** processed audio.
- Versioned JSON project files with atomic saves.
- Background jobs with progress and cooperative cancellation.
- Recent files and persisted preferences.
- Offline-first: no account, no telemetry, no network required.

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

Use **Load WAV...** to decode a file, adjust the gain slider or enable
**Normalize to 0 dBFS**, then press **Apply**. Progress and a **Cancel** button
appear in the status bar. Normalisation scales the peak to exactly 1.0 and
overrides the gain slider; the output is always clamped to avoid clipping.

## Build and test

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Linux windowing development packages are installed by the CI workflow.

## Project layout

```text
src/main.rs       CLI and window startup
src/app.rs        UI, menus, actions and lifecycle
src/audio.rs      WAV I/O, waveform, gain and normalisation (pure logic)
src/document.rs   persisted document model
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

Only `title`, `source_path`, `gain_db` and `normalize` are persisted. Audio
samples stay in memory and are never written to the JSON envelope. The
application owns the `document` schema and its migrations; a newer
`schema_version` is rejected instead of being silently misread.

## Licence

Licensed under either of:

- Apache License, Version 2.0 (`LICENSE-APACHE`)
- MIT license (`LICENSE-MIT`)

at your option.

## Third-party notices

See [THIRD_PARTY.md](THIRD_PARTY.md).
