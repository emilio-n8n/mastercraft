# Decisions — MasterCraft

Short architecture decision records. Newest first.

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
