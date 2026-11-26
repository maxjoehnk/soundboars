//! Drawing: every function here reads [`AppState`] and reports what the user did as [`Action`]s.

mod board;
mod bottom_bar;
mod dialog;
mod hotkey_section;
mod midi_section;
mod settings;
mod sound_editor;
mod top_bar;
mod url_import;

use eframe::egui;

pub use dialog::Dialog;

use super::{Action, AppState};

/// The app's windows: the main one and the dialogs, each an OS window of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum View {
    Main,
    Settings,
    SoundEditor,
    UrlImport,
}

/// Draws the window `view` (a closed dialog draws nothing).
pub fn draw(ui: &mut egui::Ui, state: &AppState, view: View) -> Vec<Action> {
    let mut actions = Vec::new();
    match view {
        View::Main => main_window(ui, state, &mut actions),
        View::Settings => settings::body(ui, state, &mut actions),
        View::SoundEditor => sound_editor::body(ui, state, &mut actions),
        View::UrlImport => url_import::body(ui, state, &mut actions),
    }
    actions
}

/// The dialogs that are open.
pub fn dialogs(state: &AppState) -> Vec<Dialog> {
    [
        settings::dialog(state),
        sound_editor::dialog(state),
        url_import::dialog(state),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn main_window(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    top_bar::show(ui, state, actions);
    error_bar(ui, state, actions);
    bottom_bar::show(ui, state, actions);
    egui::CentralPanel::default().show(ui, |ui| board::show(ui, state, actions));
}

fn error_bar(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    if state.errors.is_empty() {
        return;
    }
    egui::Panel::top("errors").show(ui, |ui| {
        for (index, error) in state.errors.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.button("✖").on_hover_text("Dismiss").clicked() {
                    actions.push(Action::DismissError(index));
                }
                ui.colored_label(ui.visuals().error_fg_color, error);
            });
        }
    });
}
