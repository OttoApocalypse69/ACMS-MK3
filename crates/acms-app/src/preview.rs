//! The Preview panel: the sprite at a small scale, always playing, optionally tiled 3 × 3 to
//! show how it repeats.

use acms_core::command::{CommandRegistry, Params};
use acms_core::compositor;
use acms_core::playback::Playback;
use egui::{
    Color32, ColorImage, DragValue, Pos2, Rect, RichText, Sense, TextureHandle, TextureOptions, Ui,
    Vec2,
};

use crate::commands;
use crate::state::{AppState, MAX_PREVIEW_SCALE};

/// What the panel keeps between frames. None of it is part of the document.
#[derive(Default)]
pub struct PreviewUi {
    playback: Option<Playback>,
    /// The time up to which the animation has been advanced, in seconds.
    clock: Option<f64>,
    /// The picture being shown, and what it is a picture of: the editor generation, the
    /// document's revision and the frame.
    texture: Option<((u64, u64, usize), TextureHandle)>,
}

pub fn show(
    ui: &mut Ui,
    state: &mut AppState,
    registry: &CommandRegistry<AppState>,
    me: &mut PreviewUi,
) {
    let Some(document) = state.editor.document() else {
        *me = PreviewUi::default();
        ui.label(RichText::new("No sprite open").weak());
        return;
    };
    let sprite = document.sprite();
    let settings = state.preview;

    // The animation runs on its own clock, whatever the canvas shows.
    let now = ui.input(|input| input.time);
    let playback = me
        .playback
        .get_or_insert_with(|| Playback::starting_at(document.active_frame()));
    let last = me.clock.unwrap_or(now);
    let elapsed_ms = ((now - last) * 1000.0).floor().clamp(0.0, 1e9);
    me.clock = Some(last + elapsed_ms / 1000.0);
    if sprite.frames().len() > 1 {
        playback.advance(sprite, state.play_tag, elapsed_ms as u32);
        let wait = u64::from(playback.until_next(sprite));
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(wait));
    } else {
        playback.frame = 0;
    }
    let frame = playback.frame.min(sprite.frames().len() - 1);

    let key = (state.editor.generation(), document.revision(), frame);
    if me
        .texture
        .as_ref()
        .is_none_or(|(made_for, _)| *made_for != key)
    {
        let flat = compositor::compose(sprite, frame);
        let size = [flat.width() as usize, flat.height() as usize];
        let image = ColorImage::from_rgba_unmultiplied(size, &flat.to_rgba_bytes());
        let texture = ui
            .ctx()
            .load_texture("preview", image, TextureOptions::NEAREST);
        me.texture = Some((key, texture));
    }
    let size = Vec2::new(sprite.width() as f32, sprite.height() as f32);

    ui.horizontal(|ui| {
        let mut scale = settings.scale;
        let field = DragValue::new(&mut scale)
            .range(1..=MAX_PREVIEW_SCALE)
            .suffix("×");
        if ui.add(field).on_hover_text("Preview scale").changed() {
            let params = Params::new().with("scale", scale);
            commands::run(registry, state, "preview.set", &params);
        }
        let mut tiled = settings.tiled;
        if ui
            .checkbox(&mut tiled, "Tiled")
            .on_hover_text("Repeat the sprite 3 × 3")
            .changed()
        {
            let params = Params::new().with("tiled", tiled);
            commands::run(registry, state, "preview.set", &params);
        }
    });

    let Some((_, texture)) = &me.texture else {
        return;
    };
    // In screen pixels, so that every sprite pixel is the same size.
    let pixels_per_point = ui.ctx().pixels_per_point();
    let one = size * settings.scale as f32 / pixels_per_point;
    let copies = if settings.tiled { 3 } else { 1 };
    let (area, _) = ui.allocate_exact_size(one * copies as f32, Sense::hover());
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 0.0, state.theme.tokens().surface);
    let whole = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    for y in 0..copies {
        for x in 0..copies {
            let min = area.min + Vec2::new(x as f32 * one.x, y as f32 * one.y);
            painter.image(
                texture.id(),
                Rect::from_min_size(min, one),
                whole,
                Color32::WHITE,
            );
        }
    }
}
