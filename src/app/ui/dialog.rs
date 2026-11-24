//! Dialogs as OS windows of their own (egui immediate viewports). Where the backend can't open
//! more windows (e.g. `egui_kittest`), egui embeds them as windows inside the main one.

use std::sync::{Arc, OnceLock};

use eframe::egui::{self, IconData, ViewportBuilder, ViewportClass, ViewportId};

use crate::app::{APP_ID, ICON_PNG};

/// A dialog window; show it every frame while it's open.
pub struct Dialog<'a> {
    /// Identifies the window across frames; unique per dialog.
    pub id: &'a str,
    pub title: &'a str,
    /// Inner size in points when it opens.
    pub size: [f32; 2],
}

impl Dialog<'_> {
    /// Shows the dialog with `contents` for this frame. Returns `true` when the user closes the
    /// window (its close button, or the compositor's close shortcut): treat that as Cancel.
    pub fn show(self, ctx: &egui::Context, mut contents: impl FnMut(&mut egui::Ui)) -> bool {
        let mut builder = ViewportBuilder::default()
            .with_title(self.title)
            .with_app_id(APP_ID)
            .with_inner_size(self.size);
        if let Some(icon) = icon() {
            builder = builder.with_icon(icon);
        }
        ctx.show_viewport_immediate(ViewportId::from_hash_of(self.id), builder, |ui, class| {
            match class {
                ViewportClass::EmbeddedWindow => contents(ui),
                _ => {
                    egui::CentralPanel::default().show(ui, |ui| contents(ui));
                }
            }
            ui.input(|input| input.viewport().close_requested())
        })
    }
}

/// The app icon, decoded once.
fn icon() -> Option<Arc<IconData>> {
    static ICON: OnceLock<Option<Arc<IconData>>> = OnceLock::new();
    ICON.get_or_init(|| match eframe::icon_data::from_png_bytes(ICON_PNG) {
        Ok(icon) => Some(Arc::new(icon)),
        Err(error) => {
            tracing::warn!("invalid window icon: {error}");
            None
        }
    })
    .clone()
}
