//! Drawing: every function here reads [`AppState`] and reports what the user did as [`Action`]s.

mod board;
mod bottom_bar;
mod dialog;
mod settings;
mod sound_editor;
mod top_bar;
mod url_import;

use eframe::egui;

use super::{Action, AppState};

pub fn draw(ui: &mut egui::Ui, state: &AppState) -> Vec<Action> {
    let mut actions = Vec::new();
    top_bar::show(ui, state, &mut actions);
    error_bar(ui, state, &mut actions);
    bottom_bar::show(ui, state, &mut actions);
    egui::CentralPanel::default().show(ui, |ui| board::show(ui, state, &mut actions));
    settings::show(ui.ctx(), state, &mut actions);
    sound_editor::show(ui.ctx(), state, &mut actions);
    url_import::show(ui.ctx(), state, &mut actions);
    actions
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
