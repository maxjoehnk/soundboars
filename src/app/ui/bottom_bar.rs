//! Stop all, what's playing, and the volumes.

use eframe::egui;

use crate::app::{Action, AppState, Setting};

pub fn show(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    egui::Panel::bottom("bottom")
        .frame(super::style::bar_frame(ui.style()))
        .show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let stop = egui::Button::new("⏹ Stop all");
                if ui.add_enabled(!state.playing.is_empty(), stop).clicked() {
                    actions.push(Action::StopAll);
                }
                ui.separator();
                now_playing(ui, state);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    volumes(ui, state, actions);
                });
            });
            ui.add_space(4.0);
        });
}

fn now_playing(ui: &mut egui::Ui, state: &AppState) {
    let names = state.now_playing();
    if names.is_empty() {
        ui.weak("Nothing playing");
    } else {
        ui.label(format!("▶ {}", names.join(", ")));
    }
}

/// Laid out right to left: call volume rightmost.
fn volumes(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    let settings = &state.config.settings;
    if let Some(volume) = volume_slider(ui, settings.call_volume, true, "Call volume") {
        actions.push(Action::ChangeSetting(Setting::CallVolume(volume)));
    }
    ui.label("📞 Call");
    ui.separator();
    let local = volume_slider(
        ui,
        settings.local_volume,
        settings.local_enabled,
        "Local volume",
    );
    if let Some(volume) = local {
        actions.push(Action::ChangeSetting(Setting::LocalVolume(volume)));
    }
    let mut local_enabled = settings.local_enabled;
    if ui.checkbox(&mut local_enabled, "🔊 Local").changed() {
        actions.push(Action::ChangeSetting(Setting::LocalEnabled(local_enabled)));
    }
}

/// Returns the new volume when it changed.
fn volume_slider(ui: &mut egui::Ui, volume: f32, enabled: bool, name: &str) -> Option<f32> {
    let mut volume = volume;
    let slider = egui::Slider::new(&mut volume, 0.0..=1.0).show_value(false);
    let response = ui.add_enabled(enabled, slider).accessible_name(name);
    response.changed().then_some(volume)
}
