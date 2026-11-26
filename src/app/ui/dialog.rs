//! Dialogs as OS windows of their own (egui deferred viewports). Each one is drawn in a pass of its
//! own, so it keeps working while the main window gets no frames (Wayland compositors don't draw
//! hidden windows). Where the backend can't open more windows (e.g. `egui_kittest`), egui embeds
//! them as windows inside the main one.

use std::sync::{Arc, OnceLock};

use eframe::egui::{self, IconData, ViewportBuilder, ViewportClass, ViewportId};

use super::View;
use crate::app::{APP_ID, ICON_PNG};

/// An open dialog window; the main window shows it every frame while it's open.
pub struct Dialog {
    pub view: View,
    pub title: String,
    /// Inner size in points when it opens.
    pub size: [f32; 2],
}

impl Dialog {
    pub fn viewport_id(&self) -> ViewportId {
        ViewportId::from_hash_of(self.view)
    }

    /// Keeps the dialog's window open; `contents` draws it, in a pass of the window's own.
    pub fn show(
        &self,
        ctx: &egui::Context,
        contents: impl Fn(&mut egui::Ui) + Send + Sync + 'static,
    ) {
        let mut builder = ViewportBuilder::default()
            .with_title(&self.title)
            .with_app_id(APP_ID)
            .with_inner_size(self.size);
        if let Some(icon) = icon() {
            builder = builder.with_icon(icon);
        }
        ctx.show_viewport_deferred(self.viewport_id(), builder, move |ui, class| match class {
            ViewportClass::EmbeddedWindow => contents(ui),
            _ => {
                egui::CentralPanel::default().show(ui, |ui| contents(ui));
            }
        });
    }
}

/// Whether the user closes the dialog drawn in `ui` (its close button, or the compositor's close
/// shortcut): treat it as Cancel.
pub fn close_requested(ui: &egui::Ui) -> bool {
    ui.input(|input| input.viewport().close_requested())
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
