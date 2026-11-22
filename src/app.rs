//! MasterCraft UI, actions and project lifecycle.

use std::path::PathBuf;

use craftkit_core::History;
use craftkit_io::DocumentHost;
use craftkit_jobs::JobRunner;
use craftkit_ui::cli::CliOptions;
use craftkit_ui::eframe::{self, egui};
use craftkit_ui::egui::{Align, Layout, Rect, RichText, Stroke, Vec2, pos2};
use craftkit_ui::{AppIdentity, AppShell, Tone, dialogs, theme, widgets};

use crate::audio::{AudioBuffer, ExportFormat, Selection, load_wav_with_progress};
use crate::batch::{self, BatchPreset, BatchReport};
use crate::document::Document;
use crate::playback::{Playback, format_time};

const FORMAT: &str = "mastercraft";
const SCHEMA_VERSION: u32 = 1;
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const LOAD_JOB: &str = "Load WAV";
const PROCESS_JOB: &str = "Process audio";
const BATCH_JOB: &str = "Batch process";

/// Undoable editing state. Audio samples live here, not in the JSON project.
#[derive(Clone, PartialEq)]
struct EditState {
    title: String,
    source_path: Option<String>,
    gain_db: f32,
    normalize: bool,
    export_format: ExportFormat,
    audio: Option<AudioBuffer>,
}

impl EditState {
    fn from_document(document: &Document, audio: Option<AudioBuffer>) -> Self {
        Self {
            title: document.title.clone(),
            source_path: document.source_path.clone(),
            gain_db: document.gain_db,
            normalize: document.normalize,
            export_format: document.export_format,
            audio,
        }
    }
}

/// Results delivered by background jobs.
enum JobResult {
    Loaded { path: PathBuf, audio: AudioBuffer },
    Processed(AudioBuffer),
    Batch(BatchReport),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    New,
    Open,
    Save,
    SaveAs,
    Undo,
    Redo,
    LoadWav,
    ExportWav,
    PlayPause,
    Stop,
    SelectAll,
    ClearSelection,
    Trim,
    Silence,
    FadeIn,
    FadeOut,
    BatchAdd,
    BatchRun,
    About,
    Settings,
}

#[derive(Debug, Clone)]
enum PendingAction {
    New,
    Open(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingChoice {
    Save,
    Discard,
    Cancel,
}

/// The MasterCraft application state.
pub struct MasterCraftApp {
    shell: AppShell,
    host: DocumentHost<Document>,
    edits: History<EditState>,
    jobs: JobRunner<JobResult>,
    playback: Playback,
    pending: Option<PendingAction>,
    gesture_active: bool,
    selection: Option<Selection>,
    selection_anchor: Option<usize>,
    zoom: f32,
    view_start: f64,
    batch_paths: Vec<PathBuf>,
    batch_preset: BatchPreset,
    fade_ms: u32,
}

impl MasterCraftApp {
    /// Build the application, honouring `--sample` and `--open`.
    pub fn new(context: &eframe::CreationContext<'_>, options: CliOptions) -> Self {
        let identity = AppIdentity {
            id: "mastercraft",
            name: "MasterCraft",
            version: APP_VERSION,
            tagline: "Audio restoration with waveform editing and WAV export",
            repository: Some("https://github.com/emilio-n8n/mastercraft"),
        };
        let shell = AppShell::new(identity);
        let document = if options.sample {
            Document::sample()
        } else {
            Document::default()
        };
        let audio = options.sample.then(AudioBuffer::sample_sine);
        let edits = History::new(EditState::from_document(&document, audio));
        let mut app = Self {
            shell,
            host: DocumentHost::new(document, FORMAT, SCHEMA_VERSION, APP_VERSION),
            edits,
            jobs: JobRunner::new(),
            playback: Playback::new(),
            pending: None,
            gesture_active: false,
            selection: None,
            selection_anchor: None,
            zoom: 1.0,
            view_start: 0.0,
            batch_paths: Vec::new(),
            batch_preset: BatchPreset::default(),
            fade_ms: 100,
        };
        app.shell.install_theme(&context.egui_ctx);
        if let Some(path) = options.open {
            app.load_project(path);
        }
        app
    }

    fn sync_document(&mut self) {
        let state = self.edits.current();
        let document = self.host.history_mut().current_mut();
        document.title = state.title.clone();
        document.source_path = state.source_path.clone();
        document.gain_db = state.gain_db;
        document.normalize = state.normalize;
        document.export_format = state.export_format;
    }

    fn finish_gesture(&mut self) {
        if self.gesture_active {
            self.edits.end_gesture();
            self.gesture_active = false;
        }
    }

    fn frames(&self) -> usize {
        self.edits
            .current()
            .audio
            .as_ref()
            .map_or(0, AudioBuffer::frames)
    }

    fn selected(&self) -> Option<Selection> {
        self.selection
            .map(|selection| selection.clamped(self.frames()))
    }

    fn apply_audio(&mut self, audio: AudioBuffer) {
        self.finish_gesture();
        self.playback.set_buffer(&audio);
        self.edits.apply(|state| state.audio = Some(audio));
        let frames = self.frames();
        self.selection = self
            .selection
            .map(|selection| selection.clamped(frames))
            .filter(|selection| !selection.is_empty());
    }

    fn load_project(&mut self, path: PathBuf) {
        match DocumentHost::<Document>::from_file(path.clone(), FORMAT, SCHEMA_VERSION, APP_VERSION)
        {
            Ok(host) => {
                let document = host.document().clone();
                self.host = host;
                self.edits = History::new(EditState::from_document(&document, None));
                self.playback.stop();
                self.selection = None;
                self.zoom = 1.0;
                self.view_start = 0.0;
                self.pending = None;
                self.shell.set_status(format!(
                    "Opened {} — reload the WAV to restore audio",
                    path.display()
                ));
            }
            Err(error) => {
                self.shell
                    .notify(Tone::Error, "Could not open project", error.to_string())
            }
        }
    }

    fn new_document(&mut self) {
        let document = Document::default();
        self.edits = History::new(EditState::from_document(&document, None));
        self.host.reset(document);
        self.playback.stop();
        self.selection = None;
        self.zoom = 1.0;
        self.view_start = 0.0;
        self.pending = None;
        self.shell.set_status("New document");
    }

    fn save(&mut self) -> bool {
        self.finish_gesture();
        self.sync_document();
        if self.host.path().is_none() {
            return self.save_as();
        }
        match self.host.save() {
            Ok(path) => {
                self.edits.mark_saved();
                self.shell.set_status(format!("Saved {}", path.display()));
                true
            }
            Err(error) => {
                self.shell
                    .notify(Tone::Error, "Could not save project", error.to_string());
                false
            }
        }
    }

    fn save_as(&mut self) -> bool {
        self.finish_gesture();
        self.sync_document();
        let default_name = format!("{}.json", slugify(&self.edits.current().title));
        let Some(path) = dialogs::save_project("Save project", &default_name, "json") else {
            return false;
        };
        match self.host.save_as(path) {
            Ok(path) => {
                self.edits.mark_saved();
                self.shell.set_status(format!("Saved {}", path.display()));
                true
            }
            Err(error) => {
                self.shell
                    .notify(Tone::Error, "Could not save project", error.to_string());
                false
            }
        }
    }

    fn request_new(&mut self) {
        self.finish_gesture();
        if self.edits.is_dirty() {
            self.pending = Some(PendingAction::New);
        } else {
            self.new_document();
        }
    }

    fn request_open(&mut self) {
        self.finish_gesture();
        let Some(path) = dialogs::open_project("Open project", &["json"]) else {
            return;
        };
        if self.edits.is_dirty() {
            self.pending = Some(PendingAction::Open(path));
        } else {
            self.load_project(path);
        }
    }

    fn resolve_pending(&mut self, choice: PendingChoice) {
        let Some(action) = self.pending.take() else {
            return;
        };
        match choice {
            PendingChoice::Cancel => {}
            PendingChoice::Discard => match action {
                PendingAction::New => self.new_document(),
                PendingAction::Open(path) => self.load_project(path),
            },
            PendingChoice::Save => {
                if self.save() {
                    match action {
                        PendingAction::New => self.new_document(),
                        PendingAction::Open(path) => self.load_project(path),
                    }
                } else {
                    self.pending = Some(action);
                }
            }
        }
    }

    fn load_wav(&mut self) {
        let Some(path) = dialogs::open_project("Load WAV", &["wav"]) else {
            return;
        };
        if self.job_running(LOAD_JOB) {
            return;
        }
        let result = self.jobs.spawn(LOAD_JOB, move |context| {
            let audio = load_wav_with_progress(&path, |fraction| {
                context
                    .check_cancelled()
                    .map_err(|error| error.to_string())?;
                context
                    .set_progress_message(fraction, format!("Decoding {:.0}%", fraction * 100.0));
                Ok(())
            })
            .map_err(|error| error.to_string())?;
            Ok(JobResult::Loaded { path, audio })
        });
        if let Err(error) = result {
            self.shell
                .notify(Tone::Error, "Could not start load job", error.to_string());
        }
    }

    fn export_wav(&mut self) {
        let Some(audio) = self.edits.current().audio.clone() else {
            self.shell.notify(
                Tone::Warning,
                "Nothing to export",
                "Load or generate audio first.",
            );
            return;
        };
        let format = self.edits.current().export_format;
        let default_name = format!("{}.wav", slugify(&self.edits.current().title));
        let Some(path) = dialogs::save_project("Export WAV", &default_name, "wav") else {
            return;
        };
        let written = match format {
            ExportFormat::Float32 => audio.write_wav(&path),
            ExportFormat::Pcm16 => audio.write_wav_with_format(&path, format),
        };
        match written {
            Ok(()) => {
                self.shell
                    .set_status(format!("Exported {} ({})", path.display(), format.label()))
            }
            Err(error) => {
                self.shell
                    .notify(Tone::Error, "Could not export WAV", error.to_string());
            }
        }
    }

    fn start_processing(&mut self) {
        let Some(audio) = self.edits.current().audio.clone() else {
            self.shell.notify(
                Tone::Warning,
                "Nothing to process",
                "Load or generate audio first.",
            );
            return;
        };
        if audio.samples.is_empty() {
            self.shell.notify(
                Tone::Warning,
                "Empty audio",
                "The loaded buffer has no samples.",
            );
            return;
        }
        if self.job_running(PROCESS_JOB) {
            self.shell
                .set_status("A processing job is already running.");
            return;
        }
        let gain_db = self.edits.current().gain_db;
        let normalize = self.edits.current().normalize;
        let range = self.selected().unwrap_or(Selection {
            start: 0,
            end: audio.frames(),
        });
        let result = self.jobs.spawn(PROCESS_JOB, move |context| {
            let mut output = audio;
            output.apply_gain_range(gain_db, normalize, range.start, range.end, |fraction| {
                context
                    .check_cancelled()
                    .map_err(|error| error.to_string())?;
                context
                    .set_progress_message(fraction, format!("Processed {:.0}%", fraction * 100.0));
                Ok(())
            })?;
            Ok(JobResult::Processed(output))
        });
        if let Err(error) = result {
            self.shell
                .notify(Tone::Error, "Could not start job", error.to_string());
        }
    }

    fn batch_add(&mut self) {
        let Some(paths) = rfd::FileDialog::new()
            .set_title("Add WAV files to batch")
            .add_filter("WAV audio", &["wav"])
            .pick_files()
        else {
            return;
        };
        self.batch_paths.extend(paths);
        self.shell.set_status(format!(
            "{} file(s) queued for batch",
            self.batch_paths.len()
        ));
    }

    fn batch_run(&mut self) {
        if self.batch_paths.is_empty() {
            self.shell.notify(
                Tone::Warning,
                "Empty batch",
                "Add at least one WAV file first.",
            );
            return;
        }
        if self.job_running(BATCH_JOB) {
            self.shell.set_status("A batch job is already running.");
            return;
        }
        let paths = self.batch_paths.clone();
        let preset = self.batch_preset;
        let format = self.edits.current().export_format;
        let result = self.jobs.spawn(BATCH_JOB, move |context| {
            let mut report = BatchReport {
                processed: 0,
                failed: Vec::new(),
            };
            let total = paths.len();
            for (index, path) in paths.iter().enumerate() {
                context
                    .check_cancelled()
                    .map_err(|error| error.to_string())?;
                context.set_progress_message(
                    index as f32 / total as f32,
                    format!("File {}/{}: {}", index + 1, total, path.display()),
                );
                match batch::process_file(path, &preset, format) {
                    Ok(_) => report.processed += 1,
                    Err(error) => report.failed.push((
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("unknown")
                            .to_owned(),
                        error,
                    )),
                }
            }
            context.set_progress(1.0);
            Ok(JobResult::Batch(report))
        });
        if let Err(error) = result {
            self.shell
                .notify(Tone::Error, "Could not start batch", error.to_string());
        }
    }

    fn job_running(&self, label: &str) -> bool {
        self.jobs.jobs().iter().any(|job| job.label == label)
    }

    fn poll_jobs(&mut self) {
        for (_, result) in self.jobs.poll() {
            match result {
                Ok(JobResult::Loaded { path, audio }) => {
                    let source = path.to_string_lossy().into_owned();
                    let title = path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .map_or_else(|| "Imported audio".to_owned(), str::to_owned);
                    let seconds = audio.duration();
                    self.finish_gesture();
                    self.edits.apply(|state| {
                        state.audio = Some(audio);
                        state.source_path = Some(source);
                        state.title = title;
                    });
                    self.playback.stop();
                    self.selection = None;
                    self.zoom = 1.0;
                    self.view_start = 0.0;
                    self.sync_document();
                    self.shell
                        .set_status(format!("Loaded {} ({seconds:.2} s)", path.display()));
                }
                Ok(JobResult::Processed(audio)) => {
                    self.apply_audio(audio);
                    self.shell
                        .set_status("Processing applied — undo to restore");
                }
                Ok(JobResult::Batch(report)) => {
                    let summary = report.summary();
                    if report.failed.is_empty() {
                        self.shell.set_status(summary);
                    } else {
                        self.shell
                            .notify(Tone::Warning, "Batch finished with errors", summary);
                    }
                }
                Err(error) => {
                    self.shell.notify(Tone::Error, "Job failed", error);
                }
            }
        }
    }

    fn handle_action(&mut self, action: Action) {
        match action {
            Action::New => self.request_new(),
            Action::Open => self.request_open(),
            Action::Save => {
                self.save();
            }
            Action::SaveAs => {
                self.save_as();
            }
            Action::Undo => {
                self.finish_gesture();
                if self.edits.undo() {
                    self.sync_document();
                    self.playback.stop();
                    self.clamp_selection();
                    self.shell.set_status("Undo");
                }
            }
            Action::Redo => {
                self.finish_gesture();
                if self.edits.redo() {
                    self.sync_document();
                    self.playback.stop();
                    self.clamp_selection();
                    self.shell.set_status("Redo");
                }
            }
            Action::LoadWav => self.load_wav(),
            Action::ExportWav => self.export_wav(),
            Action::PlayPause => {
                let Some(audio) = self.edits.current().audio.clone() else {
                    self.shell
                        .notify(Tone::Warning, "Nothing to play", "Load audio first.");
                    return;
                };
                if let Err(error) = self.playback.toggle_or_play(&audio) {
                    self.playback.set_error(error.clone());
                    self.shell
                        .notify(Tone::Warning, "Playback unavailable", error);
                }
            }
            Action::Stop => self.playback.stop(),
            Action::SelectAll => {
                let frames = self.frames();
                if frames > 0 {
                    self.selection = Some(Selection {
                        start: 0,
                        end: frames,
                    });
                }
            }
            Action::ClearSelection => self.selection = None,
            Action::Trim => {
                let Some(audio) = self.edits.current().audio.clone() else {
                    return;
                };
                let Some(selection) = self.selected() else {
                    self.shell.set_status("Select a range first.");
                    return;
                };
                if selection.is_empty() {
                    return;
                }
                self.apply_audio(audio.trim(selection.start, selection.end));
                self.selection = None;
                self.shell
                    .set_status("Trimmed to selection — undo to restore");
            }
            Action::Silence => {
                let Some(selection) = self.selected() else {
                    self.shell.set_status("Select a range first.");
                    return;
                };
                if selection.is_empty() {
                    return;
                }
                self.finish_gesture();
                self.playback.stop();
                self.edits.apply(|state| {
                    if let Some(audio) = &mut state.audio {
                        audio.silence_range(selection.start, selection.end);
                    }
                });
                self.shell
                    .set_status("Selection silenced — undo to restore");
            }
            Action::FadeIn | Action::FadeOut => {
                let Some(selection) = self.selected() else {
                    self.shell.set_status("Select a range first.");
                    return;
                };
                if selection.is_empty() {
                    return;
                }
                let fade_in = action == Action::FadeIn;
                let frames = self.fade_frames();
                self.finish_gesture();
                self.playback.stop();
                self.edits.apply(|state| {
                    if let Some(audio) = &mut state.audio {
                        audio.fade_range(selection.start, selection.end, frames, fade_in);
                    }
                });
                self.shell.set_status("Fade applied — undo to restore");
            }
            Action::BatchAdd => self.batch_add(),
            Action::BatchRun => self.batch_run(),
            Action::About => self.shell.open_about(),
            Action::Settings => self.shell.open_settings(),
        }
    }

    fn clamp_selection(&mut self) {
        let frames = self.frames();
        self.selection = self
            .selection
            .map(|selection| selection.clamped(frames))
            .filter(|selection| !selection.is_empty());
    }

    fn fade_frames(&self) -> usize {
        let rate = self
            .edits
            .current()
            .audio
            .as_ref()
            .map_or(0, |audio| audio.sample_rate);
        batch::fade_frames(self.fade_ms, rate)
    }

    fn zoom_by(&mut self, factor: f32) {
        let frames = self.frames();
        let visible = (frames as f32 / self.zoom).max(1.0);
        let center =
            self.view_start * frames.saturating_sub(visible as usize) as f64 + visible as f64 / 2.0;
        self.zoom = (self.zoom * factor).clamp(1.0, 64.0);
        let new_visible = (frames as f32 / self.zoom).max(1.0);
        let max_start = frames.saturating_sub(new_visible as usize);
        let start = (center - new_visible as f64 / 2.0).max(0.0) as usize;
        self.view_start = if max_start == 0 {
            0.0
        } else {
            start.min(max_start) as f64 / max_start as f64
        };
    }

    fn handle_shortcuts(&mut self, ui: &egui::Ui) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let action = ui.input_mut(|input| {
            if input.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::S,
            ) {
                Some(Action::SaveAs)
            } else if input.consume_key(egui::Modifiers::COMMAND, egui::Key::S) {
                Some(Action::Save)
            } else if input.consume_key(egui::Modifiers::COMMAND, egui::Key::O) {
                Some(Action::Open)
            } else if input.consume_key(egui::Modifiers::COMMAND, egui::Key::N) {
                Some(Action::New)
            } else if input.consume_key(egui::Modifiers::COMMAND, egui::Key::Z) {
                Some(Action::Undo)
            } else if input.consume_key(egui::Modifiers::COMMAND, egui::Key::Y) {
                Some(Action::Redo)
            } else if input.consume_key(egui::Modifiers::NONE, egui::Key::Space) {
                Some(Action::PlayPause)
            } else {
                None
            }
        });
        if let Some(action) = action {
            self.handle_action(action);
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let title = self.host.display_name();
        let dirty = self.edits.is_dirty();
        let can_undo = self.edits.can_undo();
        let can_redo = self.edits.can_redo();
        let mut action = None;
        self.shell.top_bar(ui, &title, dirty, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New").clicked() {
                    action = Some(Action::New);
                    ui.close();
                }
                if ui.button("Open project...").clicked() {
                    action = Some(Action::Open);
                    ui.close();
                }
                if ui.button("Save project").clicked() {
                    action = Some(Action::Save);
                    ui.close();
                }
                if ui.button("Save project as...").clicked() {
                    action = Some(Action::SaveAs);
                    ui.close();
                }
                ui.separator();
                if ui.button("Load WAV...").clicked() {
                    action = Some(Action::LoadWav);
                    ui.close();
                }
                if ui.button("Export WAV...").clicked() {
                    action = Some(Action::ExportWav);
                    ui.close();
                }
                if ui.button("Add batch files...").clicked() {
                    action = Some(Action::BatchAdd);
                    ui.close();
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui
                    .add_enabled(can_undo, egui::Button::new("Undo"))
                    .clicked()
                {
                    action = Some(Action::Undo);
                    ui.close();
                }
                if ui
                    .add_enabled(can_redo, egui::Button::new("Redo"))
                    .clicked()
                {
                    action = Some(Action::Redo);
                    ui.close();
                }
                if ui.button("Select all").clicked() {
                    action = Some(Action::SelectAll);
                    ui.close();
                }
                if ui.button("Clear selection").clicked() {
                    action = Some(Action::ClearSelection);
                    ui.close();
                }
            });
            if ui.button("About").clicked() {
                action = Some(Action::About);
            }
            if ui.button("Settings").clicked() {
                action = Some(Action::Settings);
            }
        });
        if let Some(action) = action {
            self.handle_action(action);
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let running = self.jobs.jobs();
        let audio = self.edits.current().audio.clone();
        let selection = self.selected();
        let head = self.playback.playhead();
        self.shell.status_bar_with(ui, |ui| {
            for job in running {
                widgets::job_row(ui, job);
            }
            if let Some(audio) = &audio {
                ui.label(RichText::new(format!("{:.2} s", audio.duration())).size(10.5));
                ui.label(RichText::new(format!("{} Hz", audio.sample_rate)).size(10.5));
                ui.label(RichText::new(format!("{} ch", audio.channels)).size(10.5));
                ui.label(
                    RichText::new(format!(
                        "{} / {}",
                        format_time(head.position),
                        format_time(head.duration)
                    ))
                    .size(10.5)
                    .color(theme::INFO),
                );
                if let Some(selection) = selection {
                    ui.label(
                        RichText::new(format!(
                            "sel {:.3} s",
                            selection.duration(audio.sample_rate)
                        ))
                        .size(10.5)
                        .color(theme::ACCENT),
                    );
                }
            } else {
                ui.label(
                    RichText::new("No audio loaded")
                        .size(10.5)
                        .color(theme::FAINT),
                );
            }
        });
    }

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        let mut gain = self.edits.current().gain_db;
        let mut normalize = self.edits.current().normalize;
        let mut format = self.edits.current().export_format;
        let source = self.edits.current().source_path.clone();
        let has_audio = self.edits.current().audio.is_some();
        let has_selection = self
            .selected()
            .is_some_and(|selection| !selection.is_empty());
        let busy = self.job_running(PROCESS_JOB);
        let mut actions = Vec::new();

        egui::Panel::left("controls-panel")
            .resizable(true)
            .default_size(260.0)
            .min_size(220.0)
            .show(ui, |ui| {
                widgets::section_label(ui, "Source");
                ui.add_space(4.0);
                if ui.button("Load WAV...").clicked() {
                    actions.push(Action::LoadWav);
                }
                if ui.button("Export WAV...").clicked() {
                    actions.push(Action::ExportWav);
                }
                ui.horizontal(|ui| {
                    ui.label("Export:");
                    if ui
                        .radio(format == ExportFormat::Float32, "Float 32")
                        .clicked()
                    {
                        format = ExportFormat::Float32;
                    }
                    if ui.radio(format == ExportFormat::Pcm16, "PCM 16").clicked() {
                        format = ExportFormat::Pcm16;
                    }
                });
                if format != self.edits.current().export_format {
                    self.edits.apply(|state| state.export_format = format);
                }
                ui.add_space(4.0);
                if let Some(source) = &source {
                    ui.label(
                        RichText::new(source)
                            .size(10.0)
                            .color(theme::MUTED)
                            .monospace(),
                    );
                } else {
                    ui.label(
                        RichText::new("No source file")
                            .size(10.0)
                            .color(theme::FAINT),
                    );
                }

                ui.separator();
                widgets::section_label(ui, "Playback");
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let label = if self.playback.is_playing() {
                        "Pause"
                    } else {
                        "Play"
                    };
                    if ui.button(label).clicked() {
                        actions.push(Action::PlayPause);
                    }
                    if ui.button("Stop").clicked() {
                        actions.push(Action::Stop);
                    }
                });
                if let Some(error) = self.playback.error() {
                    ui.label(RichText::new(error).size(10.0).color(theme::ERROR));
                }
                let duration = self.playback.duration() as f32;
                if duration > 0.0 {
                    let mut position = self.playback.position() as f32;
                    let response = ui.add(
                        egui::Slider::new(&mut position, 0.0..=duration)
                            .text("Position")
                            .show_value(false),
                    );
                    if response.changed() {
                        self.playback.seek(f64::from(position));
                    }
                }

                ui.separator();
                widgets::section_label(ui, "Selection");
                ui.add_space(4.0);
                if let Some(selection) = self.selected() {
                    let rate = self
                        .edits
                        .current()
                        .audio
                        .as_ref()
                        .map_or(0, |audio| audio.sample_rate);
                    ui.label(
                        RichText::new(format!(
                            "frames {}..{} · {:.3} s",
                            selection.start,
                            selection.end,
                            selection.duration(rate)
                        ))
                        .size(10.5),
                    );
                } else {
                    ui.label(
                        RichText::new("Drag on the waveform to select")
                            .size(10.0)
                            .color(theme::FAINT),
                    );
                }
                ui.horizontal(|ui| {
                    if ui.button("All").clicked() {
                        actions.push(Action::SelectAll);
                    }
                    if ui.button("Clear").clicked() {
                        actions.push(Action::ClearSelection);
                    }
                });
                if ui
                    .add_enabled(has_selection, egui::Button::new("Trim to selection"))
                    .clicked()
                {
                    actions.push(Action::Trim);
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(has_selection, egui::Button::new("Silence"))
                        .clicked()
                    {
                        actions.push(Action::Silence);
                    }
                    if ui
                        .add_enabled(has_selection, egui::Button::new("Fade in"))
                        .clicked()
                    {
                        actions.push(Action::FadeIn);
                    }
                    if ui
                        .add_enabled(has_selection, egui::Button::new("Fade out"))
                        .clicked()
                    {
                        actions.push(Action::FadeOut);
                    }
                });
                ui.add(
                    egui::Slider::new(&mut self.fade_ms, 1..=2_000)
                        .suffix(" ms")
                        .text("Fade"),
                );

                ui.separator();
                widgets::section_label(ui, "Processing");
                ui.add_space(4.0);
                let response = ui.add(
                    egui::Slider::new(&mut gain, -60.0..=20.0)
                        .suffix(" dB")
                        .text("Gain"),
                );
                if response.changed() {
                    if !self.gesture_active {
                        self.edits.begin_gesture();
                        self.gesture_active = true;
                    }
                    self.edits.current_mut().gain_db = gain;
                }
                if response.drag_stopped() || response.lost_focus() {
                    self.finish_gesture();
                }
                if ui.checkbox(&mut normalize, "Normalize to 0 dBFS").changed() {
                    self.finish_gesture();
                    self.edits.apply(|state| state.normalize = normalize);
                }
                ui.label(
                    RichText::new("Normalization overrides the gain slider.")
                        .size(10.0)
                        .color(theme::FAINT),
                );
                if ui
                    .add_enabled(has_audio && !busy, egui::Button::new("Apply gain"))
                    .clicked()
                {
                    self.start_processing();
                }

                ui.separator();
                widgets::section_label(ui, "Batch");
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!("{} file(s) queued", self.batch_paths.len()))
                        .size(10.5)
                        .color(theme::MUTED),
                );
                if ui.button("Add WAV files...").clicked() {
                    actions.push(Action::BatchAdd);
                }
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut self.batch_preset.gain_db)
                            .speed(0.5)
                            .suffix(" dB"),
                    );
                    ui.checkbox(&mut self.batch_preset.normalize, "Normalize");
                });
                ui.add(
                    egui::Slider::new(&mut self.batch_preset.fade_in_ms, 0..=2_000)
                        .text("Fade in ms"),
                );
                ui.add(
                    egui::Slider::new(&mut self.batch_preset.fade_out_ms, 0..=2_000)
                        .text("Fade out ms"),
                );
                if ui.button("Run batch").clicked() {
                    actions.push(Action::BatchRun);
                }
            });

        for action in actions {
            self.handle_action(action);
        }
    }

    fn central_panel(&mut self, ui: &mut egui::Ui) {
        let mut title = self.edits.current().title.clone();
        let mut actions = Vec::new();

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Waveform");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::toolbar_button(ui, "Export WAV", "Export the processed buffer")
                        .clicked()
                    {
                        actions.push(Action::ExportWav);
                    }
                    if widgets::toolbar_button(ui, "Load WAV", "Decode a WAV file").clicked() {
                        actions.push(Action::LoadWav);
                    }
                    if widgets::toolbar_button(ui, "Zoom -", "Zoom out").clicked() {
                        self.zoom_by(0.8);
                    }
                    if widgets::toolbar_button(ui, "Zoom +", "Zoom in").clicked() {
                        self.zoom_by(1.25);
                    }
                    if widgets::toolbar_button(ui, "1:1", "Reset zoom").clicked() {
                        self.zoom = 1.0;
                        self.view_start = 0.0;
                    }
                });
            });
            ui.add_space(4.0);
            let response =
                ui.add(egui::TextEdit::singleline(&mut title).hint_text("Document title"));
            if response.changed() {
                if !self.gesture_active {
                    self.edits.begin_gesture();
                    self.gesture_active = true;
                }
                self.edits.current_mut().title = title;
            }
            if response.lost_focus() {
                self.finish_gesture();
            }
            ui.add_space(6.0);

            if self.edits.current().audio.is_some() {
                self.draw_waveform(ui);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("Zoom {:.1}x", self.zoom))
                            .size(10.0)
                            .color(theme::FAINT),
                    );
                    ui.label(
                        RichText::new("Drag to select · click to seek · scroll to zoom")
                            .size(10.0)
                            .color(theme::FAINT),
                    );
                });
            } else {
                widgets::empty_state(
                    ui,
                    "No audio loaded",
                    "Click 'Load WAV', or start MasterCraft with --sample to see a sine wave.",
                );
            }
        });

        for action in actions {
            self.handle_action(action);
        }
    }

    fn draw_waveform(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), 220.0),
            egui::Sense::click_and_drag(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, egui::CornerRadius::same(6), theme::SUNKEN);
        let middle = rect.center().y;

        let frames = self.frames();
        if frames == 0 {
            return;
        }
        let visible = (frames as f32 / self.zoom).max(1.0) as usize;
        let max_start = frames.saturating_sub(visible);
        let start = (self.view_start * max_start as f64).round() as usize;
        let (start, visible) = if max_start == 0 {
            (0, frames)
        } else {
            (start.min(max_start), visible)
        };

        let Some(audio) = self.edits.current().audio.as_ref() else {
            return;
        };
        let columns = rect.width().max(1.0) as usize;
        let peaks = if self.zoom <= 1.0 {
            audio.waveform(columns)
        } else {
            audio.waveform_range(start, start + visible, columns)
        };
        let half_height = (rect.height() / 2.0 - 6.0).max(0.0);
        let step = rect.width() / columns as f32;
        for (column, &(min, max)) in peaks.iter().enumerate() {
            let x = rect.left() + column as f32 * step + step * 0.5;
            painter.line_segment(
                [
                    pos2(x, middle - max * half_height),
                    pos2(x, middle - min * half_height),
                ],
                Stroke::new(1.0, theme::ACCENT),
            );
        }
        painter.line_segment(
            [pos2(rect.left(), middle), pos2(rect.right(), middle)],
            Stroke::new(1.0, theme::BORDER),
        );

        let frame_at_x = |x: f32| -> usize {
            let fraction = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            start + (fraction * visible as f32) as usize
        };

        if let Some(pointer) = response.interact_pointer_pos() {
            let frame = frame_at_x(pointer.x);
            if response.drag_started() {
                self.selection_anchor = Some(frame);
                self.selection = Some(Selection::new(frame, frame));
            } else if let Some(anchor) = self.selection_anchor {
                self.selection = Some(Selection::new(anchor, frame));
            } else if response.clicked() {
                self.playback
                    .seek(frame as f64 / f64::from(audio.sample_rate));
            }
        }
        if response.drag_stopped() {
            self.selection_anchor = None;
        }

        if let Some(selection) = self.selected() {
            let from = selection.start.clamp(start, start + visible);
            let to = selection.end.clamp(start, start + visible);
            let left = rect.left() + (from - start) as f32 / visible as f32 * rect.width();
            let right = rect.left() + (to - start) as f32 / visible as f32 * rect.width();
            painter.rect_filled(
                Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.bottom())),
                0.0,
                egui::Color32::from_rgba_unmultiplied(232, 87, 63, 48),
            );
            for x in [left, right] {
                painter.line_segment(
                    [pos2(x, rect.top()), pos2(x, rect.bottom())],
                    Stroke::new(1.5, theme::ACCENT_LIGHT),
                );
                painter.text(
                    pos2(x, rect.bottom() - 10.0),
                    egui::Align2::CENTER_BOTTOM,
                    format!("{:.2}s", x as f64 / f64::from(audio.sample_rate)),
                    egui::FontId::proportional(9.0),
                    theme::MUTED,
                );
            }
        }

        let position = self.playback.position();
        if position > 0.0 {
            let frame = (position * f64::from(audio.sample_rate)) as usize;
            if (start..start + visible).contains(&frame) {
                let x = rect.left() + (frame - start) as f32 / visible as f32 * rect.width();
                painter.line_segment(
                    [pos2(x, rect.top()), pos2(x, rect.bottom())],
                    Stroke::new(1.5, theme::SUCCESS),
                );
            }
        }

        if response.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > 0.0 {
                self.zoom_by(if scroll > 0.0 { 1.1 } else { 0.9 });
            }
        }
    }

    fn unsaved_window(&mut self, ctx: &egui::Context) {
        if self.pending.is_none() {
            return;
        }
        let mut open = true;
        let mut choice = None;
        egui::Window::new("Unsaved changes")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.label("The current project has unsaved changes.");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Save and continue").clicked() {
                        choice = Some(PendingChoice::Save);
                    }
                    if ui.button("Discard").clicked() {
                        choice = Some(PendingChoice::Discard);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(PendingChoice::Cancel);
                    }
                });
            });
        if let Some(choice) = choice {
            self.resolve_pending(choice);
        } else if !open {
            self.pending = None;
        }
    }
}

impl eframe::App for MasterCraftApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_jobs();
        self.playback.tick();
        self.handle_shortcuts(ui);
        self.top_bar(ui);
        self.status_bar(ui);
        self.left_panel(ui);
        self.central_panel(ui);
        self.sync_document();
        self.unsaved_window(ui.ctx());
        self.shell.show_windows(ui.ctx());
        self.shell
            .update_title(ui.ctx(), &self.edits.current().title, self.edits.is_dirty());
    }
}

fn slugify(title: &str) -> String {
    let mut slug = String::new();
    for character in title.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "untitled".to_owned()
    } else {
        slug.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(gain_db: f32, normalize: bool, audio: Option<AudioBuffer>) -> EditState {
        EditState {
            title: "Test".to_owned(),
            source_path: None,
            gain_db,
            normalize,
            export_format: ExportFormat::Float32,
            audio,
        }
    }

    #[test]
    fn gain_and_normalization_changes_are_undoable() {
        let mut edits = History::new(state(0.0, false, None));
        edits.apply(|edit| edit.gain_db = -6.0);
        edits.apply(|edit| edit.normalize = true);

        assert!(edits.undo());
        assert!(!edits.current().normalize);
        assert_eq!(edits.current().gain_db, -6.0);
        assert!(edits.undo());
        assert_eq!(edits.current().gain_db, 0.0);
        assert!(edits.redo());
        assert_eq!(edits.current().gain_db, -6.0);
    }

    #[test]
    fn trim_and_fade_are_one_undo_step_each() {
        let audio = AudioBuffer {
            samples: vec![1.0, 1.0, 1.0, 1.0],
            sample_rate: 48_000,
            channels: 1,
        };
        let mut edits = History::new(state(0.0, false, Some(audio.clone())));
        let trimmed = audio.trim(1, 3);
        edits.apply(|edit| edit.audio = Some(trimmed));
        assert_eq!(
            edits.current().audio.as_ref().expect("audio").samples,
            vec![1.0, 1.0]
        );

        let mut faded = audio.clone();
        faded.fade_range(0, 4, 2, true);
        edits.apply(|edit| edit.audio = Some(faded));
        assert!(edits.undo());
        assert_eq!(
            edits.current().audio.as_ref().expect("audio").samples,
            vec![1.0, 1.0]
        );
        assert!(edits.undo());
        assert_eq!(
            edits.current().audio.as_ref().expect("audio").samples.len(),
            4
        );
    }

    #[test]
    fn processed_audio_is_restored_by_undo() {
        let original = AudioBuffer {
            samples: vec![0.25],
            sample_rate: 48_000,
            channels: 1,
        };
        let mut edits = History::new(state(0.0, false, Some(original.clone())));
        let mut processed = original.clone();
        processed
            .apply_gain(6.0, false, |_| Ok(()))
            .expect("apply gain");
        edits.apply(|edit| edit.audio = Some(processed));
        assert_ne!(edits.current().audio, Some(original.clone()));

        assert!(edits.undo());
        assert_eq!(edits.current().audio, Some(original));
    }

    #[test]
    fn a_gain_drag_is_one_undo_step() {
        let mut edits = History::new(state(0.0, false, None));
        edits.begin_gesture();
        edits.current_mut().gain_db = -1.0;
        edits.current_mut().gain_db = -2.0;
        edits.current_mut().gain_db = -3.0;
        edits.end_gesture();

        assert_eq!(edits.undo_depth(), 1);
        assert!(edits.undo());
        assert_eq!(edits.current().gain_db, 0.0);
    }
}
