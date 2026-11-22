# Decisions — MasterCraft

Short architecture decision records. Newest first.

## ADR-007 — Playback position tracking

Date: 2026-11-22

Decision: read the playhead from rodio's `Player::get_pos()` once per frame and
draw it on the waveform; playback itself stays on rodio's audio thread.

Alternatives: an egui-driven sample clock; a shared atomic updated from a
custom CPAL callback.

Reasons: rodio already tracks the position of the active source, including
seeks, so duplicating it would drift and add state to keep in sync.

Consequences: the playhead updates at UI frame rate, which is enough for a
marker. Audio timing remains sample-accurate on the audio thread.

## ADR-006 — Playback backend and system dependency

Date: 2026-11-22

Decision: use rodio (CPAL backend) for playback on the default output device,
initialised lazily on the first Play action. Linux builds need
`libasound2-dev`, installed by both GitHub workflows.

Alternatives: CPAL directly (more callback and position-tracking code); no
playback; a custom audio server owned by the app.

Reasons: rodio gives play/pause/seek with a device-agnostic API and little code,
and a missing device is a recoverable error rather than a crash.

Consequences: changing the buffer stops playback. There is no real-time
monitoring or low-latency loopback in this release.

## ADR-005 — Selection and editing stay in memory

Date: 2026-11-22

Decision: keep the waveform selection, zoom and view offset out of both the
JSON document and the undo history. Only the resulting buffer is a history
step. Selection, trim, fade and silence are pure functions in `src/audio.rs`.

Alternatives: persist selection and zoom in `Document`; record every drag as
an undo step.

Reasons: a selection is transient view state, not a project asset. Recording
drags would flood the bounded history and make undo unpredictable.

Consequences: reopening a project loses the selection; this is intentional.
Undo restores the previous buffer after each committed edit.

## ADR-004 — Export formats and schema compatibility

Date: 2026-11-22

Decision: add `export_format` to the document with `#[serde(default)]` and keep
`SCHEMA_VERSION` at 1, with a test proving v0.1.0 files still load. Support
32-bit float and 16-bit PCM WAV output.

Alternatives: bump to schema 2 and write a migration; keep the format in UI
settings only.

Reasons: the additive field is backward compatible, so a bump would reject
valid existing files for no benefit. The choice belongs to the project and
should round-trip with it.

Consequences: future breaking document changes still require a version bump
and an explicit migration test.

## ADR-003 — Audio state outside the JSON project

Date: 2026-11-22

Decision: keep decoded samples in an `AudioBuffer` owned by the application,
outside the serialized document. A single `History<EditState>` holds both the
persisted settings and the current audio buffer; settings are mirrored into the
`Document` before saving.

Alternatives: serialize the samples in the JSON envelope; keep two independent
undo stacks (one for settings, one for audio).

Reasons: a WAV's samples are large binary data and do not belong in a JSON
project file. Two undo stacks would get out of sync when the user edits a
setting without pressing Apply.

Consequences: opening a project restores settings but not samples — the user
must reload the source WAV (`source_path` is a hint, not an automatic load).
Undo restores the previous buffer together with the settings of that step.
`SCHEMA_VERSION` stays at 1 because the template document in the initial commit
was never released, so no migration is needed.

## ADR-002 — Initial document model

Date: 2026-11-22

Decision: `Document { title, source_path: Option<String>, gain_db: f32,
normalize: bool }`, with gains in decibels and normalisation as a boolean flag.

Alternatives: store a linear gain multiplier; store the processing history in
the project file.

Reasons: decibels match the audio-engineering workflow and remain readable in
the JSON; the last applied settings describe the project without duplicating
audio data.

Consequences: gain values are clamped only when samples are processed, so the
stored value is always the user's exact setting. Normalisation overrides the
gain because peak normalisation already fixes the output level.

## ADR-001 — Built on CraftKit

Date: 2026-11-22

Decision: use the pinned CraftKit crates for the native shell, project
lifecycle, history and background jobs.

Alternatives: build the desktop foundation from scratch; use another UI
framework.

Reasons: consistent behaviour across the portfolio, fewer duplicated bugs,
shared release and testing conventions.

Consequences: CraftKit upgrades are explicit tag changes; domain code stays in
this repository.
