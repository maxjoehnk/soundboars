//! Tile colors on screen: the fill for the current theme, readable text on it, and the swatch
//! pickers of the tile menu and the editor's "Color" section.

use eframe::egui::{self, Color32, Sense, Stroke, Vec2, WidgetInfo, accesskit::Role};

use crate::color::{self, Preset, TileColor};

const SWATCH: Vec2 = Vec2::new(18.0, 18.0);

/// The tile's fill in the theme of `visuals`.
pub fn fill(color: TileColor, visuals: &egui::Visuals) -> Color32 {
    let [r, g, b] = color.rgb(visuals.dark_mode);
    Color32::from_rgb(r, g, b)
}

/// Near-black or near-white, whichever reads better on `fill`.
pub fn text_on(fill: Color32) -> Color32 {
    if color::wants_dark_text([fill.r(), fill.g(), fill.b()]) {
        Color32::from_gray(20)
    } else {
        Color32::from_gray(245)
    }
}

/// The preset swatches and "None" in a row; returns the color picked this frame.
pub fn swatches(ui: &mut egui::Ui, current: Option<TileColor>) -> Option<Option<TileColor>> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for preset in Preset::ALL {
            let color = TileColor::Preset(preset);
            if swatch(ui, color, preset.label(), current == Some(color)).clicked() {
                picked = Some(Some(color));
            }
        }
        ui.add_space(4.0);
        if ui.selectable_label(current.is_none(), "None").clicked() {
            picked = Some(None);
        }
    });
    picked
}

/// A clickable square of `color`'s fill, outlined while selected or hovered.
fn swatch(ui: &mut egui::Ui, color: TileColor, name: &str, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(SWATCH, Sense::click());
    let enabled = ui.is_enabled();
    response.widget_info(|| WidgetInfo::selected(Role::Button, enabled, selected, name));
    let visuals = ui.visuals();
    let stroke = if selected {
        Stroke::new(2.0, visuals.strong_text_color())
    } else if response.hovered() {
        visuals.widgets.hovered.fg_stroke
    } else {
        visuals.widgets.noninteractive.bg_stroke
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, fill(color, visuals));
    painter.rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Inside);
    response.on_hover_text(name)
}

/// The custom color as sRGB: the current custom color, else the current fill (or gray).
pub fn custom_start(current: Option<TileColor>, visuals: &egui::Visuals) -> [u8; 3] {
    let start = current.unwrap_or(TileColor::Preset(Preset::Gray));
    let fill = fill(start, visuals);
    [fill.r(), fill.g(), fill.b()]
}
