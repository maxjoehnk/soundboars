//! The "Add sound from URL" window. Closing it (Cancel or the window's close button) cancels a
//! running download.

use eframe::egui;

use super::View;
use super::dialog::{Dialog, close_requested};
use crate::app::url_import::{check_url, stage_fraction, stage_text};
use crate::app::{Action, AppState, ImportStatus, UrlImport, UrlImportAction};

pub fn dialog(state: &AppState) -> Option<Dialog> {
    state.url_import.as_ref().map(|_| Dialog {
        view: View::UrlImport,
        title: "Add sound from URL".to_owned(),
        size: [480.0, 200.0],
        resizable: false,
    })
}

pub fn body(ui: &mut egui::Ui, state: &AppState, actions: &mut Vec<Action>) {
    let Some(import) = &state.url_import else {
        return;
    };
    contents(ui, state, import, actions);
    if close_requested(ui) {
        actions.push(Action::UrlImport(UrlImportAction::Close));
    }
}

fn contents(ui: &mut egui::Ui, state: &AppState, import: &UrlImport, actions: &mut Vec<Action>) {
    if state.config.tabs.is_empty() {
        no_tab(ui, actions);
        return;
    }
    ui.add_enabled_ui(!import.is_running(), |ui| {
        url_field(ui, import, actions);
        tab_picker(ui, state, import, actions);
    });
    ui.add_space(6.0);
    status(ui, import);
    ui.separator();
    buttons(ui, state, import, actions);
}

/// Downloads go into a tab's folder, so there must be one.
fn no_tab(ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    ui.label("Sounds are saved into a tab's folder. Add a folder as a tab first.");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("➕ Add folder").clicked() {
            actions.push(Action::AddFolder);
        }
        if ui.button("Cancel").clicked() {
            actions.push(Action::UrlImport(UrlImportAction::Close));
        }
    });
}

fn url_field(ui: &mut egui::Ui, import: &UrlImport, actions: &mut Vec<Action>) {
    let caption = ui.label("Paste a YouTube (or any yt-dlp supported) URL:");
    let mut url = import.url.clone();
    let response = ui
        .add(
            egui::TextEdit::singleline(&mut url)
                .hint_text("https://www.youtube.com/watch?v=…")
                .desired_width(f32::INFINITY),
        )
        .labelled_by(caption.id);
    if response.changed() {
        actions.push(Action::UrlImport(UrlImportAction::Url(url)));
    }
}

fn tab_picker(ui: &mut egui::Ui, state: &AppState, import: &UrlImport, actions: &mut Vec<Action>) {
    let tabs = &state.config.tabs;
    let selected = tabs
        .get(import.tab)
        .map(|tab| tab.display_name())
        .unwrap_or_default();
    ui.horizontal(|ui| {
        let caption = ui.label("Save to tab:");
        egui::ComboBox::from_id_salt("url import tab")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for (index, tab) in tabs.iter().enumerate() {
                    if ui
                        .selectable_label(index == import.tab, tab.display_name())
                        .clicked()
                    {
                        actions.push(Action::UrlImport(UrlImportAction::Tab(index)));
                    }
                }
            })
            .response
            .labelled_by(caption.id);
    });
}

/// The progress, the error, or what's wrong with the URL.
fn status(ui: &mut egui::Ui, import: &UrlImport) {
    match &import.status {
        ImportStatus::Running(stage) => {
            let bar = match stage_fraction(*stage) {
                Some(fraction) => egui::ProgressBar::new(fraction),
                None => egui::ProgressBar::new(0.0).animate(true),
            };
            ui.add(bar.text(stage_text(*stage)));
        }
        ImportStatus::Failed(error) => {
            ui.colored_label(ui.visuals().error_fg_color, format!("Failed: {error}"));
        }
        ImportStatus::Idle => {
            if let Err(problem) = check_url(&import.url)
                && !import.url.trim().is_empty()
            {
                ui.weak(problem);
            }
        }
    }
}

fn buttons(ui: &mut egui::Ui, state: &AppState, import: &UrlImport, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        let label = match import.status {
            ImportStatus::Failed(_) => "⟳ Retry",
            _ => "⬇ Download",
        };
        let download = egui::Button::new(label);
        if ui
            .add_enabled(import.can_download(&state.config.tabs), download)
            .clicked()
        {
            actions.push(Action::UrlImport(UrlImportAction::Download));
        }
        if ui.button("Cancel").clicked() {
            actions.push(Action::UrlImport(UrlImportAction::Close));
        }
    });
}
