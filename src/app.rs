//! MasterCraft UI, actions and project lifecycle.

use std::path::PathBuf;

use craftkit_core::History;
use craftkit_io::DocumentHost;
use craftkit_jobs::JobRunner;
use craftkit_ui::cli::CliOptions;
use craftkit_ui::eframe::{self, egui};
use craftkit_ui::egui::{Align, Layout, RichText, Stroke, Vec2, pos2};
use craftkit_ui::{AppIdentity, AppShell, Tone, dialogs, theme, widgets};

use crate::audio::AudioBuffer;
use crate::document::Document;

const FORMAT: &str = "mastercraft";
const SCHEMA_VERSION: u32 = 1;
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const PROCESS_JOB: &str = "Apply gain";

/// Everything the user can edit, in one undo stack.
///
/// Audio samples are deliberately excluded from the JSON document, so they are
/// kept here instead of in [`Document`]. Persisted fields are mirrored into the
/// document before every save.
#[derive(Clone, PartialEq)]
struct EditState {
    title: String,
    source_path: Option<String>,
    gain_db: f32,
    normalize: bool,
    audio: Option<AudioBuffer>,
}

impl EditState {
    fn from_document(document: &Document, audio: Option<AudioBuffer>) -> Self {
        Self {
            title: document.title.clone(),
            source_path: document.source_path.clone(),
            gain_db: document.gain_db,
            normalize: document.normalize,
            audio,
        }
    }
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
    jobs: JobRunner<AudioBuffer>,
    pending: Option<PendingAction>,
    gesture_active: bool,
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
            pending: None,
            gesture_active: false,
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
    }

    fn finish_gesture(&mut self) {
        if self.gesture_active {
            self.edits.end_gesture();
            self.gesture_active = false;
        }
    }

    fn load_project(&mut self, path: PathBuf) {
        match DocumentHost::<Document>::from_file(path.clone(), FORMAT, SCHEMA_VERSION, APP_VERSION)
        {
            Ok(host) => {
                let document = host.document().clone();
                self.host = host;
                // Samples are not part of the JSON envelope: reload the WAV.
                self.edits = History::new(EditState::from_document(&document, None));
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
        match AudioBuffer::load_wav(&path) {
            Ok(audio) => {
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
                self.sync_document();
                self.shell
                    .set_status(format!("Loaded {} ({seconds:.2} s)", path.display()));
            }
            Err(error) => {
                self.shell
                    .notify(Tone::Error, "Could not load WAV", error.to_string());
            }
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
        let default_name = format!("{}.wav", slugify(&self.edits.current().title));
        let Some(path) = dialogs::save_project("Export WAV", &default_name, "wav") else {
            return;
        };
        match audio.write_wav(&path) {
            Ok(()) => self
                .shell
                .set_status(format!("Exported {}", path.display())),
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
        if self.jobs.jobs().iter().any(|job| job.label == PROCESS_JOB) {
            self.shell
                .set_status("A processing job is already running.");
            return;
        }
        let gain_db = self.edits.current().gain_db;
        let normalize = self.edits.current().normalize;
        let result = self.jobs.spawn(PROCESS_JOB, move |context| {
            let mut output = audio;
            output.apply_gain(gain_db, normalize, |fraction| {
                context
                    .check_cancelled()
                    .map_err(|error| error.to_string())?;
                context
                    .set_progress_message(fraction, format!("Processed {:.0}%", fraction * 100.0));
                Ok(())
            })?;
            Ok(output)
        });
        if let Err(error) = result {
            self.shell
                .notify(Tone::Error, "Could not start job", error.to_string());
        }
    }

    fn poll_jobs(&mut self) {
        for (_, result) in self.jobs.poll() {
            match result {
                Ok(audio) => {
                    self.finish_gesture();
                    self.edits.apply(|state| state.audio = Some(audio));
                    self.shell
                        .set_status("Processing applied — undo to restore");
                }
                Err(error) if error == "job cancelled" => {
                    self.shell.set_status("Processing cancelled");
                }
                Err(error) => {
                    self.shell.notify(Tone::Error, "Processing failed", error);
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
                    self.shell.set_status("Undo");
                }
            }
            Action::Redo => {
                self.finish_gesture();
                if self.edits.redo() {
                    self.sync_document();
                    self.shell.set_status("Redo");
                }
            }
            Action::LoadWav => self.load_wav(),
            Action::ExportWav => self.export_wav(),
            Action::About => self.shell.open_about(),
            Action::Settings => self.shell.open_settings(),
        }
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
        self.shell.status_bar_with(ui, |ui| {
            for job in running {
                widgets::job_row(ui, job);
            }
            if let Some(audio) = &audio {
                ui.label(RichText::new(format!("{:.2} s", audio.duration())).size(10.5));
                ui.label(RichText::new(format!("{} Hz", audio.sample_rate)).size(10.5));
                ui.label(RichText::new(format!("{} ch", audio.channels)).size(10.5));
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
        let source = self.edits.current().source_path.clone();
        let has_audio = self.edits.current().audio.is_some();
        let busy = self.jobs.jobs().iter().any(|job| job.label == PROCESS_JOB);
        let mut load = false;
        let mut export = false;
        let mut apply = false;

        egui::Panel::left("controls-panel")
            .resizable(true)
            .default_size(250.0)
            .min_size(200.0)
            .show(ui, |ui| {
                widgets::section_label(ui, "Source");
                ui.add_space(4.0);
                if ui.button("Load WAV...").clicked() {
                    load = true;
                }
                if ui.button("Export WAV...").clicked() {
                    export = true;
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
                ui.add_space(6.0);
                if ui
                    .add_enabled(has_audio && !busy, egui::Button::new("Apply"))
                    .on_hover_text("Process the audio in a cancellable background job")
                    .clicked()
                {
                    apply = true;
                }
            });

        if load {
            self.load_wav();
        }
        if export {
            self.export_wav();
        }
        if apply {
            self.start_processing();
        }
    }

    fn central_panel(&mut self, ui: &mut egui::Ui) {
        let mut title = self.edits.current().title.clone();
        let mut load_wav = false;
        let mut export_wav = false;

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Waveform");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::toolbar_button(ui, "Export WAV", "Export the processed buffer")
                        .clicked()
                    {
                        export_wav = true;
                    }
                    if widgets::toolbar_button(ui, "Load WAV", "Decode a WAV file").clicked() {
                        load_wav = true;
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
            } else {
                widgets::empty_state(
                    ui,
                    "No audio loaded",
                    "Click 'Load WAV', or start MasterCraft with --sample to see a sine wave.",
                );
            }
        });

        if load_wav {
            self.load_wav();
        }
        if export_wav {
            self.export_wav();
        }
    }

    fn draw_waveform(&self, ui: &mut egui::Ui) {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 220.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, egui::CornerRadius::same(6), theme::SUNKEN);
        let middle = rect.center().y;
        painter.line_segment(
            [pos2(rect.left(), middle), pos2(rect.right(), middle)],
            Stroke::new(1.0, theme::BORDER),
        );
        let Some(audio) = self.edits.current().audio.as_ref() else {
            return;
        };
        let columns = rect.width().max(1.0) as usize;
        let peaks = audio.waveform(columns);
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
