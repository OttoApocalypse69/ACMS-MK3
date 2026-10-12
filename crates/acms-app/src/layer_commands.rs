//! The commands that work on layers.
//!
//! A command that takes a `layer` argument acts on that layer; without it, on the selected
//! one. That is what lets the Layers panel toggle any row while shortcuts and menus act on
//! the selection.

use acms_core::command::{Command, CommandError, CommandRegistry, ParamKind, Params};
use acms_core::document::{Document, Transaction};
use acms_core::sprite::{BlendMode, ImageLayer, Layer, LayerId, Slot, Sprite, SpriteError};

use crate::commands::sentence;
use crate::state::AppState;

type CommandResult = Result<(), CommandError>;

fn document(state: &AppState) -> Result<&Document, CommandError> {
    state
        .editor
        .document()
        .ok_or_else(|| CommandError::failed("No sprite is open."))
}

fn failed(err: SpriteError) -> CommandError {
    CommandError::failed(sentence(err))
}

/// The layer a command is about: the `layer` argument if given, otherwise the selected one.
fn target(state: &AppState, params: &Params) -> Result<LayerId, CommandError> {
    let document = document(state)?;
    let id = match params.int("layer") {
        // An id that does not fit is simply a layer that does not exist.
        Some(id) => LayerId(u32::try_from(id).unwrap_or(u32::MAX)),
        None => document.active_layer(),
    };
    if document.sprite().layer(id).is_none() {
        return Err(failed(SpriteError::UnknownLayer(id.0)));
    }
    Ok(id)
}

/// Runs `edit` on the open sprite as one undo step called `name`.
pub(crate) fn transact<T>(
    state: &mut AppState,
    name: &str,
    edit: impl FnOnce(&mut Transaction<'_>) -> Result<T, SpriteError>,
) -> Result<T, CommandError> {
    let Some(document) = state.editor.document_mut() else {
        return Err(CommandError::failed("No sprite is open."));
    };
    document.transact(name, edit).map_err(failed)
}

fn select(state: &mut AppState, id: LayerId) {
    if let Some(document) = state.editor.document_mut() {
        document.set_active_layer(id);
    }
}

fn new_layer(state: &mut AppState, group: bool) -> CommandResult {
    let document = document(state)?;
    let sprite = document.sprite();
    // Directly above the selected layer, next to it.
    let slot = match sprite.slot_of(document.active_layer()) {
        Some(slot) => Slot::new(slot.parent, slot.index + 1),
        None => Slot::new(None, sprite.layers().len()),
    };
    let number = sprite.layer_count() + 1;
    let (kind, undo_name) = if group {
        ("Group", "New Group")
    } else {
        ("Layer", "New Layer")
    };
    let name = format!("{kind} {number}");
    let id = transact(state, undo_name, |tx| tx.add_layer(slot, &name, group))?;
    select(state, id);
    Ok(())
}

fn delete(state: &mut AppState, params: &Params) -> CommandResult {
    let id = target(state, params)?;
    // Afterwards the layer below is selected, or the one above, or the group it was in.
    let sprite = document(state)?.sprite();
    let next = sprite.slot_of(id).and_then(|slot| {
        let siblings = sprite.siblings(slot.parent).ok()?;
        let below = slot.index.checked_sub(1).and_then(|i| siblings.get(i));
        let neighbour = below.or_else(|| siblings.get(slot.index + 1));
        neighbour.map(|layer| layer.id).or(slot.parent)
    });
    transact(state, "Delete Layer", |tx| tx.remove_layer(id))?;
    if let Some(next) = next {
        select(state, next);
    }
    Ok(())
}

fn duplicate(state: &mut AppState, params: &Params) -> CommandResult {
    let id = target(state, params)?;
    let copy = transact(state, "Duplicate Layer", |tx| tx.duplicate_layer(id))?;
    select(state, copy);
    Ok(())
}

fn rename(state: &mut AppState, params: &Params) -> CommandResult {
    let id = target(state, params)?;
    let Some(name) = params.text("name") else {
        // No name given: the Layers panel lets the user type one.
        state.rename_layer_request = Some(id);
        return Ok(());
    };
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err(CommandError::failed("A layer needs a name."));
    }
    transact(state, "Rename Layer", |tx| {
        tx.update_layer(id, |props| props.name = name)
    })
}

fn move_to(state: &mut AppState, params: &Params) -> CommandResult {
    let id = target(state, params)?;
    let parent = params
        .int("parent")
        .map(|parent| LayerId(u32::try_from(parent).unwrap_or(u32::MAX)));
    let index = usize::try_from(params.int("index").unwrap_or(0)).unwrap_or(usize::MAX);
    transact(state, "Move Layer", |tx| {
        tx.move_layer(id, Slot::new(parent, index))
    })
}

/// Where the selected layer sits and how many layers share that list.
fn selected_slot(state: &AppState) -> Option<(Slot, usize)> {
    let document = state.editor.document()?;
    let sprite = document.sprite();
    let slot = sprite.slot_of(document.active_layer())?;
    Some((slot, sprite.siblings(slot.parent).ok()?.len()))
}

/// The slot one step up (`+1`) or down (`-1`) from the selected layer, if there is one.
fn neighbour_slot(state: &AppState, step: isize) -> Option<Slot> {
    let (slot, count) = selected_slot(state)?;
    let index = slot.index.checked_add_signed(step)?;
    (index < count).then_some(Slot::new(slot.parent, index))
}

fn move_by(state: &mut AppState, step: isize) -> CommandResult {
    let id = document(state)?.active_layer();
    let Some(to) = neighbour_slot(state, step) else {
        return Ok(());
    };
    transact(state, "Move Layer", |tx| tx.move_layer(id, to))
}

fn selected_layer(state: &AppState) -> Option<(&Sprite, &Layer)> {
    let document = state.editor.document()?;
    let sprite = document.sprite();
    Some((sprite, sprite.layer(document.active_layer())?))
}

fn image_layer_selected(state: &AppState) -> bool {
    selected_layer(state).is_some_and(|(_, layer)| !layer.is_group())
}

fn can_delete(state: &AppState) -> bool {
    selected_slot(state).is_some_and(|(slot, count)| slot.parent.is_some() || count > 1)
}

fn can_merge_down(state: &AppState) -> bool {
    let Some((sprite, layer)) = selected_layer(state) else {
        return false;
    };
    let below = sprite.slot_of(layer.id).and_then(|slot| {
        let siblings = sprite.siblings(slot.parent).ok()?;
        siblings.get(slot.index.checked_sub(1)?)
    });
    !layer.is_group() && below.is_some_and(|below| !below.is_group())
}

fn merge_down(state: &mut AppState, params: &Params) -> CommandResult {
    let id = target(state, params)?;
    let below = transact(state, "Merge Down", |tx| tx.merge_down(id))?;
    select(state, below);
    Ok(())
}

fn flatten(state: &mut AppState, _: &Params) -> CommandResult {
    let flat = transact(state, "Flatten", |tx| tx.flatten())?;
    select(state, flat);
    Ok(())
}

fn select_layer(state: &mut AppState, params: &Params) -> CommandResult {
    let id = target(state, params)?;
    select(state, id);
    Ok(())
}

fn set_visible(state: &mut AppState, params: &Params) -> CommandResult {
    let id = target(state, params)?;
    let shown = document(state)?
        .sprite()
        .layer(id)
        .is_some_and(|l| l.visible);
    let visible = params.bool("visible").unwrap_or(!shown);
    transact(state, "Layer Visibility", |tx| {
        tx.update_layer(id, |props| props.visible = visible)
    })
}

/// Changes one of an image layer's properties as an undo step called `name`. `merge` joins
/// the step with the previous one carrying the same token.
fn set_image_property(
    state: &mut AppState,
    params: &Params,
    name: &str,
    merge: Option<u64>,
    change: impl FnOnce(&mut ImageLayer),
) -> CommandResult {
    let id = target(state, params)?;
    if document(state)?
        .sprite()
        .layer(id)
        .is_some_and(Layer::is_group)
    {
        return Err(failed(SpriteError::NotAnImageLayer(id.0)));
    }
    let Some(document) = state.editor.document_mut() else {
        return Ok(());
    };
    document
        .transact_merging(name, merge, |tx| {
            tx.update_layer(id, |props| {
                if let Some(image) = &mut props.image {
                    change(image);
                }
            })
        })
        .map_err(failed)
}

/// The current value of an image-layer flag of the target layer, to toggle it.
fn image_flag(state: &AppState, params: &Params, flag: fn(&ImageLayer) -> bool) -> bool {
    target(state, params)
        .ok()
        .and_then(|id| {
            state
                .editor
                .document()?
                .sprite()
                .layer(id)?
                .as_image()
                .map(flag)
        })
        .unwrap_or(false)
}

fn set_locked(state: &mut AppState, params: &Params) -> CommandResult {
    let current = image_flag(state, params, |image| image.locked);
    let locked = params.bool("locked").unwrap_or(!current);
    set_image_property(state, params, "Layer Lock", None, |image| {
        image.locked = locked
    })
}

fn set_alpha_lock(state: &mut AppState, params: &Params) -> CommandResult {
    let current = image_flag(state, params, |image| image.alpha_lock);
    let alpha_lock = params.bool("alpha_lock").unwrap_or(!current);
    set_image_property(state, params, "Alpha Lock", None, |image| {
        image.alpha_lock = alpha_lock;
    })
}

fn set_opacity(state: &mut AppState, params: &Params) -> CommandResult {
    let opacity = params.int("opacity").unwrap_or(255).clamp(0, 255) as u8;
    let merge = params.int("merge").map(|token| token as u64);
    set_image_property(state, params, "Layer Opacity", merge, |image| {
        image.opacity = opacity;
    })
}

fn set_blend(state: &mut AppState, params: &Params) -> CommandResult {
    let id = params.text("blend").unwrap_or_default();
    let Some(blend) = BlendMode::from_id(id) else {
        let known: Vec<&str> = BlendMode::ALL.iter().map(|mode| mode.id()).collect();
        return Err(CommandError::failed(format!(
            "\"{id}\" is not a blend mode. The blend modes are: {}.",
            known.join(", ")
        )));
    };
    set_image_property(state, params, "Blend Mode", None, |image| {
        image.blend = blend
    })
}

fn set_collapsed(state: &mut AppState, params: &Params) -> CommandResult {
    let id = target(state, params)?;
    let collapsed = params.bool("collapsed").unwrap_or(false);
    let is_group = state
        .editor
        .document_mut()
        .is_some_and(|document| document.set_group_collapsed(id, collapsed));
    if is_group {
        Ok(())
    } else {
        Err(failed(SpriteError::NotAGroup(id.0)))
    }
}

/// Registers the layer commands.
pub fn register(registry: &mut CommandRegistry<AppState>) -> Result<(), CommandError> {
    use ParamKind::{Bool, Int, Text};
    let has_document = |state: &AppState| state.editor.document().is_some();

    registry.register(
        Command::new("layer.new", "New Layer", |state: &mut AppState, _| {
            new_layer(state, false)
        })
        .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("layer.new_group", "New Group", |state: &mut AppState, _| {
            new_layer(state, true)
        })
        .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("layer.duplicate", "Duplicate Layer", duplicate)
            .optional_param("layer", Int)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("layer.delete", "Delete Layer", delete)
            .optional_param("layer", Int)
            .enabled_when(can_delete),
    )?;
    registry.register(
        Command::new("layer.rename", "Rename Layer…", rename)
            .optional_param("layer", Int)
            .optional_param("name", Text)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("layer.move", "Move Layer", move_to)
            .optional_param("layer", Int)
            .optional_param("parent", Int)
            .param("index", Int)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new(
            "layer.move_up",
            "Move Layer Up",
            |state: &mut AppState, _| move_by(state, 1),
        )
        .enabled_when(|state| neighbour_slot(state, 1).is_some()),
    )?;
    registry.register(
        Command::new(
            "layer.move_down",
            "Move Layer Down",
            |state: &mut AppState, _| move_by(state, -1),
        )
        .enabled_when(|state| neighbour_slot(state, -1).is_some()),
    )?;
    registry.register(
        Command::new("layer.merge_down", "Merge Layer Down", merge_down)
            .optional_param("layer", Int)
            .enabled_when(can_merge_down),
    )?;
    registry.register(
        Command::new("layer.flatten", "Flatten Layers", flatten).enabled_when(has_document),
    )?;
    registry.register(
        Command::new("layer.select", "Select Layer", select_layer)
            .param("layer", Int)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("layer.set_visible", "Show / Hide Layer", set_visible)
            .optional_param("layer", Int)
            .optional_param("visible", Bool)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("layer.set_locked", "Lock / Unlock Layer", set_locked)
            .optional_param("layer", Int)
            .optional_param("locked", Bool)
            .enabled_when(image_layer_selected),
    )?;
    registry.register(
        Command::new("layer.set_alpha_lock", "Toggle Alpha Lock", set_alpha_lock)
            .optional_param("layer", Int)
            .optional_param("alpha_lock", Bool)
            .enabled_when(image_layer_selected),
    )?;
    registry.register(
        Command::new("layer.set_opacity", "Set Layer Opacity", set_opacity)
            .optional_param("layer", Int)
            .param("opacity", Int)
            .optional_param("merge", Int)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new("layer.set_blend", "Set Blend Mode", set_blend)
            .optional_param("layer", Int)
            .param("blend", Text)
            .enabled_when(has_document),
    )?;
    registry.register(
        Command::new(
            "layer.set_collapsed",
            "Collapse or Expand Group",
            set_collapsed,
        )
        .param("layer", Int)
        .param("collapsed", Bool)
        .enabled_when(has_document),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::testing::{Fixture, RED};
    use crate::state::Dialog;
    use acms_core::color::Rgba8;

    const L1: LayerId = LayerId(1);

    fn order(fixture: &Fixture) -> Vec<u32> {
        let entries = fixture.document().sprite().walk();
        entries.iter().map(|entry| entry.layer.id.0).collect()
    }

    fn active(fixture: &Fixture) -> LayerId {
        fixture.document().active_layer()
    }

    fn layer(fixture: &Fixture, id: LayerId) -> Layer {
        fixture.document().sprite().layer(id).unwrap().clone()
    }

    fn image(fixture: &Fixture, id: LayerId) -> ImageLayer {
        *layer(fixture, id).as_image().unwrap()
    }

    fn on(id: LayerId) -> Params {
        Params::new().with("layer", id.0)
    }

    fn history(fixture: &Fixture) -> Vec<String> {
        fixture.document().history().map(str::to_owned).collect()
    }

    /// Takes the error message a failed command left behind.
    fn message(fixture: &mut Fixture) -> String {
        match fixture.state.dialog.take() {
            Some(Dialog::Message { text, .. }) => text,
            other => panic!("expected a message, found {other:?}"),
        }
    }

    #[test]
    fn new_layers_go_above_the_selection_and_become_selected() {
        let mut f = Fixture::with_sprite();
        f.run("layer.new");
        let a = active(&f);
        assert_ne!(a, L1);
        assert_eq!(order(&f), [1, a.0]);
        assert_eq!(layer(&f, a).name, "Layer 2");

        // With the bottom layer selected, the new one lands between the two.
        f.run_with("layer.select", on(L1));
        f.run("layer.new");
        let b = active(&f);
        assert_eq!(order(&f), [1, b.0, a.0]);

        f.run("layer.new_group");
        let group = active(&f);
        assert!(layer(&f, group).is_group());
        assert_eq!(layer(&f, group).name, "Group 4");
        assert_eq!(history(&f), ["New Layer", "New Layer", "New Group"]);

        // Drawing goes to the selected layer.
        f.run_with("layer.select", on(b));
        f.draw_line(3);
        assert_eq!(f.pixel_on(b, 5, 3), RED);
        assert_eq!(f.pixel(5, 3), Rgba8::TRANSPARENT);
    }

    #[test]
    fn deleting_selects_a_neighbour_and_the_last_layer_stays() {
        let mut f = Fixture::with_sprite();
        assert!(!f.is_enabled("layer.delete"));
        f.run("layer.new");
        let upper = active(&f);
        f.draw_line(3);
        assert!(f.is_enabled("layer.delete"));

        f.run("layer.delete");
        assert_eq!(order(&f), [1]);
        assert_eq!(active(&f), L1);
        assert!(!f.is_enabled("layer.delete"));

        f.run("edit.undo");
        assert_eq!(order(&f), [1, upper.0]);
        assert_eq!(f.pixel_on(upper, 5, 3), RED);

        // Deleting by id rather than the selection.
        f.run_with("layer.select", on(upper));
        f.run_with("layer.delete", on(L1));
        assert_eq!(order(&f), [upper.0]);
        assert_eq!(active(&f), upper);
        assert!(!f.is_enabled("layer.delete"));
    }

    #[test]
    fn the_only_top_level_layer_is_refused_with_a_reason() {
        // A group holding the only image layer: the selection could be deleted, the group not.
        let mut f = Fixture::with_sprite();
        f.run("layer.new_group");
        let group = active(&f);
        f.run_with(
            "layer.move",
            on(L1).with("parent", group.0).with("index", 0_i64),
        );
        f.run_with("layer.select", on(L1));
        assert!(f.is_enabled("layer.delete"));

        f.run_with("layer.delete", on(group));
        assert_eq!(message(&mut f), "The last layer cannot be deleted.");
        assert_eq!(order(&f), [group.0, 1]);

        // Deleting the selection leaves the group, which becomes selected.
        f.run("layer.delete");
        assert_eq!(order(&f), [group.0]);
        assert_eq!(active(&f), group);
    }

    #[test]
    fn duplicate_rename_and_move() {
        let mut f = Fixture::with_sprite();
        f.draw_line(3);
        f.run("layer.duplicate");
        let copy = active(&f);
        assert_eq!(order(&f), [1, copy.0]);
        assert_eq!(layer(&f, copy).name, "Layer 1 copy");
        assert_eq!(f.pixel_on(copy, 5, 3), RED);

        f.run_with("layer.rename", Params::new().with("name", "  Outline  "));
        assert_eq!(layer(&f, copy).name, "Outline");
        f.run_with("layer.rename", on(L1).with("name", "Fill"));
        assert_eq!(layer(&f, L1).name, "Fill");
        f.run_with("layer.rename", Params::new().with("name", "   "));
        assert_eq!(message(&mut f), "A layer needs a name.");
        // Without a name the Layers panel is asked to edit it in place.
        f.run("layer.rename");
        assert_eq!(f.state.rename_layer_request, Some(copy));

        assert!(f.is_enabled("layer.move_down"));
        assert!(!f.is_enabled("layer.move_up"));
        f.run("layer.move_down");
        assert_eq!(order(&f), [copy.0, 1]);
        assert!(!f.is_enabled("layer.move_down"));
        f.run("layer.move_up");
        assert_eq!(order(&f), [1, copy.0]);

        // Into a group and out again, by position.
        f.run("layer.new_group");
        let group = active(&f);
        f.run_with(
            "layer.move",
            on(L1).with("parent", group.0).with("index", 0_i64),
        );
        assert_eq!(order(&f), [copy.0, group.0, 1]);
        f.run_with("layer.move", on(L1).with("index", 2_i64));
        assert_eq!(order(&f), [copy.0, group.0, 1]);
        assert_eq!(f.document().sprite().slot_of(L1), Some(Slot::new(None, 2)));

        f.run_with(
            "layer.move",
            on(group).with("parent", group.0).with("index", 0_i64),
        );
        assert_eq!(message(&mut f), "A group cannot be moved into itself.");
        f.run_with("layer.move", on(L1).with("index", 9_i64));
        assert!(message(&mut f).contains("no position 9"));
    }

    #[test]
    fn merge_down_and_flatten() {
        let mut f = Fixture::with_sprite();
        f.draw_line(3);
        assert!(!f.is_enabled("layer.merge_down"));
        f.run("layer.new");
        let upper = active(&f);
        f.draw_line(6);
        assert!(f.is_enabled("layer.merge_down"));

        f.run("layer.merge_down");
        assert_eq!(order(&f), [1]);
        assert_eq!(active(&f), L1);
        assert_eq!((f.pixel(5, 3), f.pixel(5, 6)), (RED, RED));
        f.run("edit.undo");
        assert_eq!(order(&f), [1, upper.0]);
        assert_eq!(f.pixel(5, 6), Rgba8::TRANSPARENT);

        // A group directly below is not something to merge into.
        f.run_with("layer.select", on(L1));
        f.run("layer.new_group");
        f.run_with("layer.select", on(upper));
        assert!(!f.is_enabled("layer.merge_down"));

        f.run("layer.flatten");
        let flat = active(&f);
        assert_eq!(order(&f), [flat.0]);
        assert_eq!((f.pixel_on(flat, 5, 3), f.pixel_on(flat, 5, 6)), (RED, RED));
        assert_eq!(history(&f).last().map(String::as_str), Some("Flatten"));
        f.run("edit.undo");
        assert_eq!(order(&f).len(), 3);
    }

    #[test]
    fn visibility_lock_and_alpha_lock_toggle_and_stop_the_pencil() {
        let mut f = Fixture::with_sprite();
        f.run("layer.set_visible");
        assert!(!layer(&f, L1).visible);
        f.draw_line(3);
        assert_eq!(f.pixel(5, 3), Rgba8::TRANSPARENT);
        f.run("layer.set_visible");
        assert!(layer(&f, L1).visible);
        f.run_with("layer.set_visible", Params::new().with("visible", true));
        assert_eq!(history(&f), ["Layer Visibility", "Layer Visibility"]);

        f.run("layer.set_locked");
        assert!(image(&f, L1).locked);
        f.draw_line(3);
        assert_eq!(f.pixel(5, 3), Rgba8::TRANSPARENT);
        f.run("layer.set_locked");
        f.draw_line(3);
        assert_eq!(f.pixel(5, 3), RED);

        // With alpha lock, an empty row stays empty and no undo step appears.
        f.run("layer.set_alpha_lock");
        assert!(image(&f, L1).alpha_lock);
        let steps = history(&f).len();
        f.draw_line(4);
        assert_eq!(f.pixel(5, 4), Rgba8::TRANSPARENT);
        assert_eq!(history(&f).len(), steps);
        f.run_with(
            "layer.set_alpha_lock",
            Params::new().with("alpha_lock", false),
        );
        assert!(!image(&f, L1).alpha_lock);

        // These three only make sense for image layers.
        f.run("layer.new_group");
        for id in ["layer.set_locked", "layer.set_alpha_lock"] {
            assert!(!f.is_enabled(id));
        }
        assert!(f.is_enabled("layer.set_visible"));
        let group = active(&f);
        f.run_with("layer.select", on(L1));
        f.run_with("layer.set_locked", on(group));
        assert!(message(&mut f).contains("is not an image layer"));
    }

    #[test]
    fn opacity_and_blend_mode() {
        let mut f = Fixture::with_sprite();
        f.run_with("layer.set_opacity", Params::new().with("opacity", 128_i64));
        assert_eq!(image(&f, L1).opacity, 128);
        f.run_with("layer.set_opacity", Params::new().with("opacity", 999_i64));
        assert_eq!(image(&f, L1).opacity, 255);
        f.run_with("layer.set_opacity", Params::new().with("opacity", -5_i64));
        assert_eq!(image(&f, L1).opacity, 0);
        assert_eq!(history(&f).len(), 3);

        // A drag sends many values with one token: a single undo step.
        for opacity in [200_i64, 150, 100] {
            let params = Params::new().with("opacity", opacity).with("merge", 42_i64);
            f.run_with("layer.set_opacity", params);
        }
        assert_eq!(history(&f).len(), 4);
        f.run("edit.undo");
        assert_eq!(image(&f, L1).opacity, 0);

        for mode in BlendMode::ALL {
            f.run_with("layer.set_blend", Params::new().with("blend", mode.id()));
            assert_eq!(image(&f, L1).blend, mode);
        }
        f.run_with("layer.set_blend", Params::new().with("blend", "dissolve"));
        assert!(message(&mut f).starts_with("\"dissolve\" is not a blend mode."));

        f.run("layer.new_group");
        let group = active(&f);
        f.run_with("layer.set_opacity", on(group).with("opacity", 10_i64));
        assert!(message(&mut f).contains("is not an image layer"));
    }

    #[test]
    fn collapsing_a_group_and_selecting_by_id() {
        let mut f = Fixture::with_sprite();
        f.run("layer.new_group");
        let group = active(&f);
        let (steps, revision) = (history(&f).len(), f.document().revision());

        // Folding a group away is neither an undo step nor a change to save.
        f.run_with("layer.set_collapsed", on(group).with("collapsed", true));
        assert!(layer(&f, group).as_group().unwrap().collapsed);
        assert_eq!(history(&f).len(), steps);
        assert_eq!(f.document().revision(), revision);
        f.run_with("layer.set_collapsed", on(L1).with("collapsed", true));
        assert!(message(&mut f).contains("is not a group"));

        f.run_with("layer.select", on(L1));
        assert_eq!(active(&f), L1);
        f.run_with("layer.select", on(LayerId(77)));
        assert_eq!(message(&mut f), "There is no layer with id 77.");
        assert_eq!(active(&f), L1);
    }

    #[test]
    fn layer_commands_need_a_sprite() {
        let f = Fixture::new();
        for id in [
            "layer.new",
            "layer.new_group",
            "layer.duplicate",
            "layer.delete",
            "layer.rename",
            "layer.move_up",
            "layer.move_down",
            "layer.merge_down",
            "layer.flatten",
            "layer.set_visible",
            "layer.set_locked",
            "layer.set_alpha_lock",
        ] {
            assert!(!f.is_enabled(id), "{id}");
        }
    }
}
