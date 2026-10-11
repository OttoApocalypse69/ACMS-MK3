//! The command palette: fuzzy search over every registered command (see `PLAN.md`, section 5).

use acms_core::command::{Command, CommandRegistry};
use egui::{Align2, Id, Key, Modal, Modifiers, ScrollArea, Sense, TextEdit, TextStyle, Vec2};

use crate::fuzzy;
use crate::keymap::Keymap;

const WIDTH: f32 = 520.0;
const ROW_HEIGHT: f32 = 30.0;
const MAX_LIST_HEIGHT: f32 = ROW_HEIGHT * 10.0;
/// Distance from the top of the window.
const TOP_OFFSET: f32 = 72.0;

#[derive(Default)]
pub struct PaletteState {
    open: bool,
    query: String,
    /// Index into the filtered list.
    selected: usize,
    /// The search field grabs focus on the first frame after opening.
    focus_pending: bool,
}

impl PaletteState {
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.focus_pending = true;
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.open
    }
}

/// Commands matching `query`, best match first. Ties keep registration order.
///
/// Commands that cannot run without being given arguments are left out: the palette has no
/// way to supply them.
fn matching<'a, C>(registry: &'a CommandRegistry<C>, query: &str) -> Vec<&'a Command<C>> {
    let mut scored: Vec<(i32, &Command<C>)> = registry
        .iter()
        .filter(|command| command.runs_without_params())
        .filter_map(|command| {
            let by_name = fuzzy::score(query, command.name());
            let by_id = fuzzy::score(query, command.id());
            by_name.max(by_id).map(|score| (score, command))
        })
        .collect();
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    scored.into_iter().map(|(_, command)| command).collect()
}

/// Shows the palette if it is open. Returns the id of the command the user picked, if any.
pub fn show<C>(
    ctx: &egui::Context,
    state: &mut PaletteState,
    registry: &CommandRegistry<C>,
    keymap: &Keymap,
    command_ctx: &C,
) -> Option<String> {
    if !state.open {
        return None;
    }

    // Taken before the text field sees them, so the arrows move the selection instead of the
    // text cursor and Enter does not just unfocus the field.
    let (up, down, enter) = ctx.input_mut(|input| {
        (
            input.count_and_consume_key(Modifiers::NONE, Key::ArrowUp),
            input.count_and_consume_key(Modifiers::NONE, Key::ArrowDown),
            input.consume_key(Modifiers::NONE, Key::Enter),
        )
    });

    let id = Id::new("command_palette");
    let area = Modal::default_area(id).anchor(Align2::CENTER_TOP, Vec2::new(0.0, TOP_OFFSET));

    let mut picked = None;
    let modal = Modal::new(id).area(area).show(ctx, |ui| {
        ui.set_width(WIDTH);

        let search = ui.add(
            TextEdit::singleline(&mut state.query)
                .hint_text("Type a command…")
                .desired_width(f32::INFINITY),
        );
        if std::mem::take(&mut state.focus_pending) {
            search.request_focus();
        }
        if search.changed() {
            state.selected = 0;
        }

        let commands = matching(registry, &state.query);
        let moved = up + down > 0;
        state.selected = (state.selected + down)
            .saturating_sub(up)
            .min(commands.len().saturating_sub(1));

        ui.separator();
        if commands.is_empty() {
            ui.weak("No matching commands");
            return;
        }

        ScrollArea::vertical()
            .max_height(MAX_LIST_HEIGHT)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (index, command) in commands.iter().enumerate() {
                    let enabled = command.is_enabled(command_ctx);
                    let selected = index == state.selected;
                    let shortcuts: Vec<String> = keymap
                        .shortcuts_for(command.id())
                        .map(|shortcut| ctx.format_shortcut(shortcut))
                        .collect();

                    let row = row(ui, command.name(), &shortcuts.join("  "), selected, enabled);
                    if selected && moved {
                        row.scroll_to_me(None);
                    }
                    if enabled && (row.clicked() || (selected && enter)) {
                        picked = Some(command.id().to_owned());
                    }
                }
            });
    });

    if picked.is_some() || modal.should_close() {
        state.open = false;
    }
    picked
}

/// One line of the list: the command name on the left, its shortcut on the right.
fn row(
    ui: &mut egui::Ui,
    name: &str,
    shortcut: &str,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), sense);

    let visuals = ui.visuals();
    let fill = if selected {
        Some(visuals.selection.bg_fill)
    } else if enabled && response.hovered() {
        Some(visuals.widgets.hovered.bg_fill)
    } else {
        None
    };
    let name_colour = if enabled {
        visuals.strong_text_color()
    } else {
        visuals.weak_text_color()
    };
    let shortcut_colour = visuals.weak_text_color();
    let radius = visuals.widgets.inactive.corner_radius;

    let painter = ui.painter();
    if let Some(fill) = fill {
        painter.rect_filled(rect, radius, fill);
    }
    let inner = rect.shrink2(Vec2::new(10.0, 0.0));
    painter.text(
        inner.left_center(),
        Align2::LEFT_CENTER,
        name,
        TextStyle::Body.resolve(ui.style()),
        name_colour,
    );
    painter.text(
        inner.right_center(),
        Align2::RIGHT_CENTER,
        shortcut,
        TextStyle::Small.resolve(ui.style()),
        shortcut_colour,
    );

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> CommandRegistry<()> {
        let mut registry = CommandRegistry::new();
        for (id, name) in [
            ("view.toggle_theme", "Toggle Light / Dark Theme"),
            ("view.reset_layout", "Reset Panel Layout"),
            ("view.clear_log", "Clear Log"),
        ] {
            registry
                .register(Command::new(id, name, |_, _| Ok(())))
                .unwrap();
        }
        registry
    }

    #[test]
    fn commands_that_need_arguments_are_not_listed() {
        let mut registry = registry();
        let needs_size = Command::new("brush.set_size", "Set Brush Size", |_, _| Ok(()))
            .param("size", acms_core::command::ParamKind::Int);
        registry.register(needs_size).unwrap();
        assert!(ids(&registry, "brush").is_empty());
        assert_eq!(ids(&registry, "").len(), 3);
    }

    fn ids(registry: &CommandRegistry<()>, query: &str) -> Vec<String> {
        matching(registry, query)
            .into_iter()
            .map(|command| command.id().to_owned())
            .collect()
    }

    #[test]
    fn empty_query_lists_everything_in_registration_order() {
        let registry = registry();
        assert_eq!(
            ids(&registry, ""),
            ["view.toggle_theme", "view.reset_layout", "view.clear_log"]
        );
    }

    #[test]
    fn query_filters_and_ranks() {
        let registry = registry();
        assert_eq!(ids(&registry, "reset"), ["view.reset_layout"]);
        assert_eq!(ids(&registry, "clear log")[0], "view.clear_log");
        assert!(ids(&registry, "zzz").is_empty());
    }

    #[test]
    fn commands_can_be_found_by_id() {
        let registry = registry();
        assert_eq!(ids(&registry, "view.reset"), ["view.reset_layout"]);
    }

    #[test]
    fn opening_resets_the_previous_search() {
        let mut state = PaletteState {
            query: "old".to_owned(),
            selected: 3,
            ..Default::default()
        };
        state.open();
        assert!(state.is_open());
        assert!(state.query.is_empty());
        assert_eq!(state.selected, 0);
    }
}
