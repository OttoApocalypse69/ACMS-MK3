//! The status bar: what is under the pointer, and how the sprite is being viewed.

use acms_core::color::Rgba8;
use acms_core::geom::Point;
use acms_core::sprite::SpriteError;
use egui::color_picker::show_color_at;
use egui::{Align, Color32, Layout, RichText, Sense, Ui, Vec2};

use crate::state::AppState;

/// Why drawing has no effect right now, in words for the status bar.
pub fn draw_block_message(block: &SpriteError) -> &'static str {
    match block {
        SpriteError::LayerLocked(_) => "The selected layer is locked",
        SpriteError::LayerHidden(_) => "The selected layer is hidden",
        SpriteError::NotAnImageLayer(_) => "A group is selected: select a layer to draw on",
        _ => "Nothing can be drawn here",
    }
}

/// The zoom as a percentage: `"800%"`, `"12.5%"`.
pub fn zoom_text(zoom: f32) -> String {
    let percent = zoom * 100.0;
    if percent.fract() == 0.0 {
        format!("{percent:.0}%")
    } else {
        format!("{percent:.1}%")
    }
}

/// `hover` is the sprite pixel under the pointer and the colour seen there, if the pointer is
/// over the canvas.
pub fn show(ui: &mut Ui, state: &AppState, hover: Option<(Point, Rgba8)>) {
    let Some(document) = state.editor.document() else {
        ui.label(RichText::new("No sprite open").weak());
        return;
    };
    let sprite = document.sprite();

    ui.horizontal(|ui| {
        // Left: what is under the pointer.
        if let Some(block) = state.editor.draw_block() {
            let warning = ui.visuals().warn_fg_color;
            ui.label(RichText::new(draw_block_message(&block)).color(warning));
            ui.separator();
        }
        match hover.filter(|(pixel, _)| sprite.bounds().contains(*pixel)) {
            Some((pixel, color)) => {
                ui.monospace(format!("{}, {}", pixel.x, pixel.y));
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                let swatch = Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a);
                show_color_at(ui.painter(), swatch, rect);
                let name = if color.a == 0 {
                    "transparent".to_owned()
                } else {
                    color.to_hex()
                };
                ui.monospace(name);
            }
            None => {
                ui.label(RichText::new("—").weak());
            }
        }

        // Right: the sprite and the view.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let frame = document.active_frame() + 1;
            ui.label(format!("Frame {frame} / {}", sprite.frames().len()));
            ui.separator();
            ui.label(zoom_text(state.view.zoom()));
            ui.separator();
            ui.label(format!("{} × {} px", sprite.width(), sprite.height()));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_is_shown_as_a_percentage() {
        assert_eq!(zoom_text(1.0), "100%");
        assert_eq!(zoom_text(8.0), "800%");
        assert_eq!(zoom_text(0.5), "50%");
        assert_eq!(zoom_text(0.125), "12.5%");
        assert_eq!(zoom_text(1.0 / 3.0), "33.3%");
    }

    #[test]
    fn each_reason_for_not_drawing_has_its_own_words() {
        let messages = [
            draw_block_message(&SpriteError::LayerLocked(1)),
            draw_block_message(&SpriteError::LayerHidden(1)),
            draw_block_message(&SpriteError::NotAnImageLayer(1)),
        ];
        assert!(messages[0].contains("locked"));
        assert!(messages[1].contains("hidden"));
        assert!(messages[2].contains("group"));
    }
}
