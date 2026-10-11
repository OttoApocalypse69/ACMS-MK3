//! The dockable panels and their default layout (see `PLAN.md`, section 5).
//!
//! In Phase 0 every panel is a placeholder, except the Log which shows what commands did.

use egui::{Id, RichText, Ui, WidgetText};
use egui_dock::{DockState, NodeIndex, TabViewer};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Panel {
    /// The central area where open sprites will live. Cannot be closed.
    Canvas,
    Tools,
    ToolOptions,
    Colour,
    Palette,
    Layers,
    Timeline,
    Tileset,
    History,
    Preview,
    ScriptConsole,
    Log,
}

impl Panel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Canvas => "Canvas",
            Self::Tools => "Tools",
            Self::ToolOptions => "Tool Options",
            Self::Colour => "Colour",
            Self::Palette => "Palette",
            Self::Layers => "Layers",
            Self::Timeline => "Timeline",
            Self::Tileset => "Tileset",
            Self::History => "History",
            Self::Preview => "Preview",
            Self::ScriptConsole => "Script Console",
            Self::Log => "Log",
        }
    }
}

/// The layout the app starts with, and the one "Reset Panel Layout" restores.
///
/// ```text
/// +-------+----------------------+----------+
/// | Tools |                      | Colour   |
/// +-------+        Canvas        +----------+
/// | Tool  |                      | Layers   |
/// | Opts  |                      +----------+
/// |       |                      | Preview  |
/// +-------+----------------------+----------+
/// | Timeline | Tileset | Script Console | Log |
/// +-----------------------------------------+
/// ```
pub fn default_layout() -> DockState<Panel> {
    use Panel::*;

    let mut dock = DockState::new(vec![Canvas]);
    let tree = dock.main_surface_mut();

    // A fraction is the share of the left / top side of the split, whichever node ends up
    // there (checked against egui_dock 0.21; its `split_left` docs say otherwise).
    let [centre, _bottom] = tree.split_below(
        NodeIndex::root(),
        0.74,
        vec![Timeline, Tileset, ScriptConsole, Log],
    );
    let [centre, tools] = tree.split_left(centre, 0.16, vec![Tools]);
    tree.split_below(tools, 0.45, vec![ToolOptions]);
    let [_centre, colour] = tree.split_right(centre, 0.76, vec![Colour, Palette]);
    let [_colour, layers] = tree.split_below(colour, 0.38, vec![Layers, History]);
    tree.split_below(layers, 0.6, vec![Preview]);

    dock
}

/// Draws panel contents for egui_dock.
pub struct PanelViewer<'a> {
    pub log: &'a [String],
    /// Shown on the empty canvas so the command palette is discoverable.
    pub palette_shortcut: &'a str,
}

impl TabViewer for PanelViewer<'_> {
    type Tab = Panel;

    fn id(&mut self, tab: &mut Panel) -> Id {
        Id::new(*tab)
    }

    fn title(&mut self, tab: &mut Panel) -> WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Panel) {
        match tab {
            Panel::Canvas => {
                ui.heading("No sprite open");
                ui.label(
                    RichText::new(format!(
                        "Press {} to open the command palette.",
                        self.palette_shortcut
                    ))
                    .weak(),
                );
            }
            Panel::Log => {
                if self.log.is_empty() {
                    ui.label(RichText::new("Nothing logged yet.").weak());
                }
                for line in self.log {
                    ui.monospace(line);
                }
            }
            other => {
                ui.label(RichText::new(format!("{} — placeholder", other.title())).weak());
            }
        }
    }

    fn is_closeable(&self, tab: &Panel) -> bool {
        *tab != Panel::Canvas
    }

    fn allowed_in_windows(&self, tab: &mut Panel) -> bool {
        *tab != Panel::Canvas
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_contains_every_panel_once() {
        use Panel::*;
        let all = [
            Canvas,
            Tools,
            ToolOptions,
            Colour,
            Palette,
            Layers,
            Timeline,
            Tileset,
            History,
            Preview,
            ScriptConsole,
            Log,
        ];

        let dock = default_layout();
        let placed: Vec<Panel> = dock.iter_all_tabs().map(|(_, tab)| *tab).collect();
        assert_eq!(placed.len(), all.len());
        for panel in all {
            assert!(placed.contains(&panel), "{panel:?} is missing");
        }
    }
}
