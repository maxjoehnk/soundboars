//! The app's look: egui's default colors and widgets with more room and lighter text fields.

use eframe::egui::{self, Color32, CornerRadius, Margin, Stroke, Style, TextStyle, Vec2};

/// Inside the main window's top and bottom bars.
const BAR_MARGIN: Margin = Margin::symmetric(12, 8);
/// Around the board and inside dialogs.
pub const CONTENT_MARGIN: Margin = Margin::same(12);
const BODY_SIZE: f32 = 13.5;
const CORNER_RADIUS: u8 = 3;

/// Applies the look to both themes; the theme setting only picks one of them.
pub fn apply(ctx: &egui::Context) {
    ctx.all_styles_mut(roomy);
}

/// The frame of a top or bottom bar of the main window.
pub fn bar_frame(style: &Style) -> egui::Frame {
    egui::Frame::side_top_panel(style).inner_margin(BAR_MARGIN)
}

/// The frame of the board, or of a dialog's contents.
pub fn content_frame(style: &Style) -> egui::Frame {
    egui::Frame::central_panel(style).inner_margin(CONTENT_MARGIN)
}

fn roomy(style: &mut Style) {
    let spacing = &mut style.spacing;
    spacing.item_spacing = Vec2::new(10.0, 8.0);
    spacing.button_padding = Vec2::new(10.0, 5.0);
    spacing.interact_size.y = 26.0;
    if let Some(body) = style.text_styles.get_mut(&TextStyle::Body) {
        body.size = BODY_SIZE;
    }
    if let Some(button) = style.text_styles.get_mut(&TextStyle::Button) {
        button.size = BODY_SIZE;
    }
    lighten(&mut style.visuals);
}

/// Text fields a step lighter than egui's near-black, with a faint outline at rest so they read as
/// fields; labels a bit brighter. The outline uses the widget fill in dark mode, so buttons (which
/// share it) don't change there.
fn lighten(visuals: &mut egui::Visuals) {
    let widgets = &mut visuals.widgets;
    for state in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        state.corner_radius = CornerRadius::same(CORNER_RADIUS);
    }
    if visuals.dark_mode {
        visuals.text_edit_bg_color = Some(Color32::from_gray(35));
        widgets.inactive.bg_stroke = Stroke::new(1.0, widgets.inactive.weak_bg_fill);
        widgets.noninteractive.fg_stroke.color = Color32::from_gray(160);
    } else {
        widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_gray(210));
        widgets.noninteractive.fg_stroke.color = Color32::from_gray(74);
    }
}
