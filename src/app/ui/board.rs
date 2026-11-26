//! The grid of sound tiles.

use std::time::Instant;

use eframe::egui::{self, Pos2, Rect, Vec2};

use crate::app::{Action, AppState, ClipId};

const TILE: Vec2 = Vec2::new(170.0, 74.0);
const SPACING: f32 = 8.0;

pub fn show(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    let clips = state.visible_clips();
    if clips.is_empty() {
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
                for (index, clip) in clips.iter().enumerate() {
                    tile(ui, state, clip, now, actions);
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
    clip: &ClipId,
    now: Instant,
    actions: &mut Vec<Action>,
) {
    let progress = state.progress(clip, now);
    let label = format!("{}\n{}", state.display_name(clip), state.subtitle(clip));
    let button = egui::Button::new(label).selected(progress.is_some());
    let response = ui
        .add_enabled_ui(state.audio_available, |ui| ui.add_sized(TILE, button))
        .inner;
    if let Some(progress) = progress {
        progress_bar(ui, response.rect, progress);
    }
    if response.clicked() {
        actions.push(Action::Play(clip.clone()));
    }
    response.context_menu(|ui| context_menu(ui, clip, actions));
    response.on_hover_text(clip.path.display().to_string());
}

/// A tile's right-click menu.
fn context_menu(ui: &mut egui::Ui, clip: &ClipId, actions: &mut Vec<Action>) {
    let mut item = |label: &str, action: fn(ClipId) -> Action| {
        if ui.button(label).clicked() {
            actions.push(action(clip.clone()));
            ui.close();
        }
    };
    item("✏ Edit…", Action::EditSound);
    item("Rename…", Action::RenameSound);
    item("Duplicate", Action::DuplicateClip);
    // The file's own clip goes away with the file.
    if clip.clip != 0 {
        item("Delete clip", Action::DeleteClip);
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
