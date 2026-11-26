//! The grid of sound tiles.

use std::time::Instant;

use eframe::egui::{self, Pos2, Rect, Vec2};

use crate::app::{Action, AppState};
use crate::library::Sound;

const TILE: Vec2 = Vec2::new(170.0, 74.0);
const SPACING: f32 = 8.0;

pub fn show(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    let sounds = state.visible_sounds();
    if sounds.is_empty() {
        ui.centered_and_justified(|ui| ui.weak(empty_message(state)));
        return;
    }
    let columns = ((ui.available_width() + SPACING) / (TILE.x + SPACING))
        .floor()
        .max(1.0) as usize;
    let now = Instant::now();
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("sounds")
            .spacing([SPACING, SPACING])
            .show(ui, |ui| {
                for (index, sound) in sounds.iter().enumerate() {
                    tile(ui, state, sound, now, actions);
                    if (index + 1) % columns == 0 {
                        ui.end_row();
                    }
                }
            });
    });
}

fn empty_message(state: &AppState) -> String {
    if state.is_searching() {
        return "No sounds match the search.".to_owned();
    }
    match state.library.folders().get(state.tab) {
        None => {
            "Add a folder with “➕ Folder”; its audio files become the sounds of a tab.".to_owned()
        }
        Some(folder) => folder.error.clone().unwrap_or_else(|| {
            "No sounds. Drop audio files into this folder or use “Add from URL”.".to_owned()
        }),
    }
}

fn tile(
    ui: &mut egui::Ui,
    state: &AppState,
    sound: &Sound,
    now: Instant,
    actions: &mut Vec<Action>,
) {
    let progress = state.progress(&sound.path, now);
    let label = format!("{}\n{}", state.display_name(sound), state.subtitle(sound));
    let button = egui::Button::new(label).selected(progress.is_some());
    let response = ui
        .add_enabled_ui(state.audio_available, |ui| ui.add_sized(TILE, button))
        .inner;
    if let Some(progress) = progress {
        progress_bar(ui, response.rect, progress);
    }
    if response.clicked() {
        actions.push(Action::Play(sound.path.clone()));
    }
    response.context_menu(|ui| context_menu(ui, sound, actions));
    response.on_hover_text(sound.path.display().to_string());
}

/// A tile's right-click menu.
fn context_menu(ui: &mut egui::Ui, sound: &Sound, actions: &mut Vec<Action>) {
    if ui.button("✏ Edit…").clicked() {
        actions.push(Action::EditSound(sound.path.clone()));
        ui.close();
    }
    if ui.button("Rename…").clicked() {
        actions.push(Action::RenameSound(sound.path.clone()));
        ui.close();
    }
}

fn progress_bar(ui: &egui::Ui, tile: Rect, progress: f32) {
    let bar = Rect::from_min_size(
        Pos2::new(tile.left(), tile.bottom() - 3.0),
        Vec2::new(tile.width() * progress.clamp(0.0, 1.0), 3.0),
    );
    ui.painter()
        .rect_filled(bar, 0.0, ui.visuals().selection.stroke.color);
}
