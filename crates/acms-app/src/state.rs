//! Everything commands act on: the editor from `acms-core` plus the app's own UI state.
//!
//! UI code reads this freely but changes the document only by running commands.

use std::path::PathBuf;

use acms_core::command::Params;
use acms_core::document::Document;
use acms_core::editor::Editor;
use acms_core::sprite::LayerId;
use egui::Vec2;
use egui_dock::DockState;

use crate::palette::PaletteState;
use crate::panels::{self, Panel};
use crate::theme::ThemeKind;

/// Screen pixels per sprite pixel. Whole numbers when zoomed in, so every sprite pixel is the
/// same size on screen; simple fractions when zoomed out.
pub const ZOOM_LEVELS: [f32; 20] = [
    1.0 / 8.0,
    1.0 / 6.0,
    1.0 / 4.0,
    1.0 / 3.0,
    1.0 / 2.0,
    1.0,
    2.0,
    3.0,
    4.0,
    5.0,
    6.0,
    8.0,
    10.0,
    12.0,
    16.0,
    20.0,
    24.0,
    32.0,
    48.0,
    64.0,
];

const ZOOM_100: usize = 5;

/// Space kept free around the sprite when it is zoomed to fit, in screen pixels.
const FIT_MARGIN: f32 = 24.0;

/// How the canvas is shown. Lengths are in physical screen pixels, not egui points, so that
/// sprite pixels stay crisp on scaled displays.
pub struct ViewState {
    zoom_index: usize,
    /// Offset of the sprite's centre from the centre of the viewport.
    pub pan: Vec2,
    pub show_grid: bool,
    /// Size of the canvas area, as of the last frame it was shown.
    pub viewport: Vec2,
    /// Set when a sprite is opened; the canvas zooms to fit once it knows its size.
    pub fit_pending: bool,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            zoom_index: ZOOM_100,
            pan: Vec2::ZERO,
            show_grid: true,
            viewport: Vec2::ZERO,
            fit_pending: false,
        }
    }
}

impl ViewState {
    /// Screen pixels per sprite pixel.
    pub fn zoom(&self) -> f32 {
        ZOOM_LEVELS[self.zoom_index]
    }

    /// Moves `steps` zoom levels in (positive) or out (negative), keeping the sprite point
    /// under `anchor` where it is. `anchor` is relative to the centre of the viewport.
    pub fn zoom_by(&mut self, steps: i32, anchor: Vec2) {
        let old = self.zoom();
        let last = ZOOM_LEVELS.len() as i32 - 1;
        self.zoom_index = (self.zoom_index as i32 + steps).clamp(0, last) as usize;
        let ratio = self.zoom() / old;
        self.pan = (anchor - (anchor - self.pan) * ratio).round();
    }

    /// Centres the sprite at the largest zoom level at which all of it is visible.
    pub fn fit(&mut self, sprite_size: Vec2) {
        let room = (self.viewport - Vec2::splat(2.0 * FIT_MARGIN)).max(Vec2::splat(1.0));
        self.zoom_index = ZOOM_LEVELS
            .iter()
            .rposition(|zoom| sprite_size.x * zoom <= room.x && sprite_size.y * zoom <= room.y)
            .unwrap_or(0);
        self.pan = Vec2::ZERO;
    }
}

/// A command to run later, once the user has confirmed or supplied something.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingCommand {
    pub id: String,
    pub params: Params,
}

/// The modal dialog that is open, with the contents of its form.
#[derive(Clone, Debug, PartialEq)]
pub enum Dialog {
    NewSprite {
        width: u32,
        height: u32,
    },
    ExportPng {
        scale: u32,
    },
    /// The open sprite has unsaved changes that running `then` would throw away.
    ConfirmDiscard {
        then: PendingCommand,
    },
    Message {
        title: String,
        text: String,
    },
}

/// A command needs a file path from the user. The UI shows a native file dialog and then runs
/// the command again with the path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileRequest {
    Open {
        /// The user has already agreed to lose unsaved changes.
        discard: bool,
    },
    SaveAs,
    ExportPng {
        scale: u32,
    },
}

pub struct AppState {
    pub editor: Editor,
    /// Where the open sprite was loaded from or last saved to.
    pub file_path: Option<PathBuf>,
    /// The document revision that matches what is on disk (or the blank sprite it began as).
    pub saved_revision: u64,
    pub view: ViewState,
    pub dock: DockState<Panel>,
    /// Goes up when a command replaces `dock` as a whole.
    pub layout_generation: u64,
    pub theme: ThemeKind,
    pub palette: PaletteState,
    pub dialog: Option<Dialog>,
    pub file_request: Option<FileRequest>,
    /// `layer.rename` was run without a name: the Layers panel should let the user type one.
    pub rename_layer_request: Option<LayerId>,
    /// Lines shown in the Log panel.
    pub log: Vec<String>,
    /// Set by `app.quit`; the app closes the window at the end of the frame.
    pub quit_requested: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            editor: Editor::new(),
            file_path: None,
            saved_revision: 0,
            view: ViewState::default(),
            dock: panels::default_layout(),
            layout_generation: 0,
            theme: ThemeKind::default(),
            palette: PaletteState::default(),
            dialog: None,
            file_request: None,
            rename_layer_request: None,
            log: Vec::new(),
            quit_requested: false,
        }
    }
}

impl AppState {
    /// Makes `document` the open sprite. `path` is the file it came from, if any.
    pub fn open_document(&mut self, document: Document, path: Option<PathBuf>) {
        self.saved_revision = document.revision();
        self.editor.set_document(Some(document));
        self.file_path = path;
        self.view.fit_pending = true;
    }

    /// Whether the open sprite differs from what is on disk.
    pub fn has_unsaved_changes(&self) -> bool {
        self.editor
            .document()
            .is_some_and(|document| document.revision() != self.saved_revision)
    }

    /// The file name of the open sprite, with a mark when it has unsaved changes.
    pub fn document_title(&self) -> String {
        if self.editor.document().is_none() {
            return "Canvas".to_owned();
        }
        let name = self
            .file_path
            .as_ref()
            .and_then(|path| path.file_name())
            .map_or_else(
                || "Untitled".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            );
        if self.has_unsaved_changes() {
            format!("{name} •")
        } else {
            name
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(viewport: Vec2) -> ViewState {
        ViewState {
            viewport,
            ..ViewState::default()
        }
    }

    #[test]
    fn zoom_levels_ascend_and_are_whole_when_zoomed_in() {
        assert!(ZOOM_LEVELS.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(ZOOM_LEVELS[ZOOM_100], 1.0);
        assert!(
            ZOOM_LEVELS[ZOOM_100..]
                .iter()
                .all(|zoom| zoom.fract() == 0.0)
        );
    }

    #[test]
    fn zooming_stops_at_both_ends() {
        let mut view = view(Vec2::new(800.0, 600.0));
        view.zoom_by(1000, Vec2::ZERO);
        assert_eq!(view.zoom(), 64.0);
        view.zoom_by(-1000, Vec2::ZERO);
        assert_eq!(view.zoom(), 1.0 / 8.0);
    }

    #[test]
    fn zooming_keeps_the_point_under_the_cursor_still() {
        let mut view = view(Vec2::new(800.0, 600.0));
        view.pan = Vec2::new(30.0, -10.0);
        let anchor = Vec2::new(120.0, 80.0);
        // The sprite point under the anchor, relative to the sprite's centre.
        let under = |view: &ViewState| (anchor - view.pan) / view.zoom();

        let before = under(&view);
        view.zoom_by(3, anchor);
        assert_eq!(view.zoom(), 4.0);
        let after = under(&view);
        assert!((before - after).length() < 0.5, "{before:?} vs {after:?}");

        view.zoom_by(-2, anchor);
        assert!((before - under(&view)).length() < 0.5);
    }

    #[test]
    fn fit_picks_the_largest_level_that_shows_everything() {
        let mut view = view(Vec2::new(848.0, 648.0));
        view.pan = Vec2::new(99.0, 99.0);
        view.fit(Vec2::new(32.0, 32.0));
        // 600 pixels of room: 16× fits (512), 20× does not (640).
        assert_eq!(view.zoom(), 16.0);
        assert_eq!(view.pan, Vec2::ZERO);

        view.fit(Vec2::new(1600.0, 100.0));
        assert_eq!(view.zoom(), 0.5);

        // Nothing fits: the smallest level is used.
        view.fit(Vec2::new(100_000.0, 100_000.0));
        assert_eq!(view.zoom(), 1.0 / 8.0);
    }

    #[test]
    fn title_shows_the_file_name_and_unsaved_changes() {
        let mut state = AppState::default();
        assert_eq!(state.document_title(), "Canvas");

        let sprite = acms_core::sprite::Sprite::new(8, 8).unwrap();
        state.open_document(Document::new(sprite), None);
        assert_eq!(state.document_title(), "Untitled");
        assert!(!state.has_unsaved_changes());

        state.file_path = Some(PathBuf::from("art").join("hero.acms"));
        state.saved_revision = 12345;
        assert_eq!(state.document_title(), "hero.acms •");
        assert!(state.has_unsaved_changes());
    }
}
