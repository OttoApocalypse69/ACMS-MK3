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
            Command("file.export_gif"),
            Command("file.export_sheet"),
            Command("file.export_sequence"),
            Command("file.export_tileset"),
            Command("file.export_tilemap"),
            Command("file.export_tilemap_csv"),
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
            Separator,
            Command("slice.edit"),
            Command("slice.delete"),
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
            Command("layer.new_tilemap"),
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
        "Frame",
        &[
            Command("frame.new"),
            Command("frame.duplicate"),
            Command("frame.delete"),
            Command("frame.set_duration"),
            Separator,
            Command("frame.next"),
            Command("frame.previous"),
            Command("frame.first"),
            Command("frame.last"),
            Separator,
            Command("cel.link"),
            Command("cel.unlink"),
            Command("tag.new"),
            Separator,
            Command("playback.toggle"),
            Command("onion.toggle"),
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
            Command("tile.toggle_mode"),
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

/// The fixed part of the Scripts menu; the commands of the scripts follow it.
const SCRIPT_MENU: [&str; 2] = ["script.reload", "script.clear_console"];

pub fn show(
    ui: &mut Ui,
    state: &mut AppState,
    registry: &CommandRegistry<AppState>,
    keymap: &Keymap,
) {
    let item = |ui: &mut Ui, state: &mut AppState, id: &str| {
        let Some(command) = registry.get(id) else {
            return;
        };
        let shortcuts: Vec<String> = keymap
            .shortcuts_for(id)
            .map(|shortcut| ui.ctx().format_shortcut(shortcut))
            .collect();
        let button = Button::new(command.name()).shortcut_text(shortcuts.join("  "));
        if ui.add_enabled(command.is_enabled(state), button).clicked() {
            commands::run(registry, state, id, &Params::new());
        }
    };
    MenuBar::new().ui(ui, |ui| {
        for (title, entries) in MENUS {
            ui.menu_button(*title, |ui| {
                for entry in *entries {
                    match entry {
                        Command(id) => item(ui, state, id),
                        Separator => {
                            ui.separator();
                        }
                    }
                }
            });
        }
        // The commands scripts registered come and go, so this menu is built as it is shown.
        ui.menu_button("Scripts", |ui| {
            for id in SCRIPT_MENU {
                item(ui, state, id);
            }
            let ids: Vec<String> = state
                .scripts
                .commands()
                .iter()
                .map(|command| command.id.clone())
                .collect();
            if !ids.is_empty() {
                ui.separator();
            }
            for id in ids {
                item(ui, state, &id);
            }
        });
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
        let scripts = SCRIPT_MENU.map(Command);
        let menus = MENUS.iter().copied().chain([("Scripts", &scripts[..])]);
        for (title, entries) in menus {
            for entry in entries {
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
