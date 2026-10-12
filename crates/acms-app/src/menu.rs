//! The menu bar. Every item is a command from the registry, shown with its name, its shortcut
//! and whether it can run right now.

use acms_core::command::{CommandRegistry, Params};
use egui::{Button, MenuBar, Ui};

use crate::commands;
use crate::keymap::Keymap;
use crate::state::AppState;

enum Entry {
    Command(&'static str),
    Separator,
}

use Entry::{Command, Separator};

const MENUS: &[(&str, &[Entry])] = &[
    (
        "File",
        &[
            Command("file.new"),
            Command("file.open"),
            Separator,
            Command("file.save"),
            Command("file.save_as"),
            Command("file.export_png"),
            Separator,
            Command("file.close"),
            Command("app.quit"),
        ],
    ),
    (
        "Edit",
        &[
            Command("edit.undo"),
            Command("edit.redo"),
            Separator,
            Command("edit.cut"),
            Command("edit.copy"),
            Command("edit.paste"),
            Command("edit.clear"),
        ],
    ),
    (
        "Select",
        &[
            Command("select.all"),
            Command("select.none"),
            Command("select.invert"),
            Separator,
            Command("selection.flip_horizontal"),
            Command("selection.flip_vertical"),
            Command("selection.rotate_cw"),
            Command("selection.rotate_ccw"),
            Command("selection.scale"),
            Separator,
            Command("brush.from_selection"),
            Command("pattern.from_selection"),
        ],
    ),
    (
        "Sprite",
        &[
            Command("sprite.canvas_size"),
            Command("sprite.size"),
            Command("sprite.crop"),
            Separator,
            Command("symmetry.off"),
            Command("symmetry.horizontal"),
            Command("symmetry.vertical"),
            Command("symmetry.both"),
            Command("symmetry.radial"),
        ],
    ),
    (
        "Palette",
        &[
            Command("palette.import"),
            Command("palette.export"),
            Separator,
            Command("palette.add_color"),
            Command("palette.remove_colors"),
            Command("palette.sort_hue"),
            Command("palette.sort_brightness"),
            Separator,
            Command("palette.new_ramp"),
            Command("palette.generate_ramp"),
            Separator,
            Command("palette.replace_color"),
            Command("palette.reduce"),
        ],
    ),
    (
        "Layer",
        &[
            Command("layer.new"),
            Command("layer.new_group"),
            Command("layer.add_reference"),
            Command("layer.duplicate"),
            Command("layer.delete"),
            Command("layer.rename"),
            Separator,
            Command("layer.move_up"),
            Command("layer.move_down"),
            Separator,
            Command("layer.merge_down"),
            Command("layer.flatten"),
            Separator,
            Command("layer.set_visible"),
            Command("layer.set_locked"),
            Command("layer.set_alpha_lock"),
        ],
    ),
    (
        "View",
        &[
            Command("view.zoom_in"),
            Command("view.zoom_out"),
            Command("view.zoom_fit"),
            Command("view.toggle_grid"),
            Command("view.toggle_tiled"),
            Command("tool.set_pixel_perfect"),
            Separator,
            Command("doc.next"),
            Command("doc.previous"),
            Separator,
            Command("view.command_palette"),
            Command("view.toggle_theme"),
            Command("view.reset_layout"),
            Command("view.clear_log"),
        ],
    ),
];

pub fn show(
    ui: &mut Ui,
    state: &mut AppState,
    registry: &CommandRegistry<AppState>,
    keymap: &Keymap,
) {
    MenuBar::new().ui(ui, |ui| {
        for (title, entries) in MENUS {
            ui.menu_button(*title, |ui| {
                for entry in *entries {
                    let Command(id) = entry else {
                        ui.separator();
                        continue;
                    };
                    let Some(command) = registry.get(id) else {
                        continue;
                    };
                    let shortcuts: Vec<String> = keymap
                        .shortcuts_for(id)
                        .map(|shortcut| ui.ctx().format_shortcut(shortcut))
                        .collect();
                    let button = Button::new(command.name()).shortcut_text(shortcuts.join("  "));
                    if ui.add_enabled(command.is_enabled(state), button).clicked() {
                        commands::run(registry, state, id, &Params::new());
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::register_builtin;

    #[test]
    fn every_menu_item_is_a_command_that_runs_without_arguments() {
        let mut registry = CommandRegistry::new();
        register_builtin(&mut registry).unwrap();
        for (title, entries) in MENUS {
            for entry in *entries {
                if let Command(id) = entry {
                    let command = registry.get(id);
                    assert!(command.is_some(), "{title}: no command `{id}`");
                    assert!(
                        command.is_some_and(|command| command.runs_without_params()),
                        "{title}: `{id}` needs arguments"
                    );
                }
            }
        }
    }

    #[test]
    fn no_command_is_listed_twice() {
        let mut ids: Vec<&str> = MENUS
            .iter()
            .flat_map(|(_, entries)| entries.iter())
            .filter_map(|entry| match entry {
                Command(id) => Some(*id),
                Separator => None,
            })
            .collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total);
    }
}
