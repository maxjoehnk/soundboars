//! The settings window.

use eframe::egui;

use crate::app::{Action, AppState, Setting};
use crate::audio::Retrigger;
use crate::config::{Settings, Theme};

pub fn show(ctx: &egui::Context, state: &AppState, actions: &mut Vec<Action>) {
    let mut open = state.settings_open;
    egui::Window::new("Settings")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::Grid::new("settings")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| rows(ui, &state.config.settings, actions));
        });
    if open != state.settings_open {
        actions.push(Action::ShowSettings(open));
    }
}

fn rows(ui: &mut egui::Ui, settings: &Settings, actions: &mut Vec<Action>) {
    let retrigger_options = [
        (Retrigger::Overlap, "Overlap"),
        (Retrigger::Restart, "Restart"),
        (Retrigger::Stop, "Stop"),
    ];
    let caption = "Re-trigger playing sound:";
    if let Some(retrigger) = choice_row(ui, caption, settings.retrigger, retrigger_options) {
        actions.push(Action::ChangeSetting(Setting::Retrigger(retrigger)));
    }
    let theme_options = [(Theme::Dark, "Dark"), (Theme::Light, "Light")];
    if let Some(theme) = choice_row(ui, "Theme:", settings.theme, theme_options) {
        actions.push(Action::ChangeSetting(Setting::Theme(theme)));
    }
}

/// A grid row of radio buttons; returns the option the user clicked.
fn choice_row<T: PartialEq + Copy, const N: usize>(
    ui: &mut egui::Ui,
    caption: &str,
    current: T,
    options: [(T, &str); N],
) -> Option<T> {
    ui.label(caption);
    let mut chosen = None;
    ui.horizontal(|ui| {
        for (option, name) in options {
            if ui.radio(current == option, name).clicked() {
                chosen = Some(option);
            }
        }
    });
    ui.end_row();
    chosen
}
