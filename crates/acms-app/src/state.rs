//! Everything commands act on: the editor from `acms-core` plus the app's own UI state.
//!
//! UI code reads this freely but changes the document only by running commands.

use std::collections::HashMap;
use std::path::PathBuf;

use acms_core::color::Rgba8;
use acms_core::command::Params;
use acms_core::compositor::OnionSkin;
use acms_core::document::Document;
use acms_core::editor::{DocId, Editor};
use acms_core::playback::Playback;
use acms_core::sprite::{Direction, LayerId};
use acms_io::export::SheetOptions;
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
#[derive(Clone)]
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

/// The cels selected in the timeline: a run of frames on one layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CelRange {
    pub layer: LayerId,
    /// The frame the selection was started on.
    pub anchor: usize,
    /// The frame it was extended to; before or after the anchor.
    pub extent: usize,
}

impl CelRange {
    pub fn first(&self) -> usize {
        self.anchor.min(self.extent)
    }

    pub fn last(&self) -> usize {
        self.anchor.max(self.extent)
    }

    pub fn contains(&self, frame: usize) -> bool {
        (self.first()..=self.last()).contains(&frame)
    }
}

/// What an export dialog is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportKind {
    Gif,
    Sheet,
    Sequence,
}

/// How the Preview panel shows the sprite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewSettings {
    /// Screen pixels per sprite pixel.
    pub scale: u32,
    /// The sprite is repeated 3 × 3, to see how it tiles.
    pub tiled: bool,
}

/// Largest scale of the Preview panel.
pub const MAX_PREVIEW_SCALE: u32 = 16;

impl Default for PreviewSettings {
    fn default() -> Self {
        Self {
            scale: 2,
            tiled: false,
        }
    }
}

/// An animation being played on the canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Playing {
    pub playback: Playback,
    /// The frame that was shown when playing began, and is shown again when it stops.
    pub return_to: usize,
    /// The time up to which the animation has been advanced, in seconds; `None` until the
    /// first frame after playing began.
    clock: Option<f64>,
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
    /// Asks for a width and height, then runs the command for `purpose` with them.
    Size {
        purpose: SizePurpose,
        width: u32,
        height: u32,
        /// For a canvas resize: which corner or edge the existing picture stays against.
        anchor: Anchor,
    },
    /// Asks how many colours a ramp from the primary to the secondary colour should have.
    Ramp {
        steps: u32,
    },
    /// A new tag, or with `index` the tag being changed. Frames count from 0 here.
    Tag {
        index: Option<usize>,
        name: String,
        from: usize,
        to: usize,
        direction: Direction,
        color: Rgba8,
    },
    /// How long the selected frames are shown.
    FrameDuration {
        ms: u32,
    },
    /// The selected slice being changed.
    Slice {
        name: String,
        rect: acms_core::geom::Rect,
        /// The stretching part of a 9-slice, relative to the slice's corner.
        center: Option<acms_core::geom::Rect>,
        pivot: Option<acms_core::geom::Point>,
    },
    /// A dialog a script built, with the values as the user has set them so far.
    Script(acms_script::DialogSpec),
    /// The settings of an export; only those the kind uses are shown.
    Export {
        kind: ExportKind,
        scale: u32,
        /// The tag to export, or every frame.
        tag: Option<usize>,
        sheet: SheetOptions,
        json: bool,
    },
}

/// What a [`Dialog::Size`] is asking the size for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizePurpose {
    CanvasSize,
    SpriteSize,
    ScaleSelection,
    /// The size of the tiles of a new tilemap layer.
    NewTilemap,
}

/// Where the existing picture sits when the canvas changes size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Anchor {
    #[default]
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    pub const ALL: [Self; 9] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Left,
        Self::Center,
        Self::Right,
        Self::BottomLeft,
        Self::Bottom,
        Self::BottomRight,
    ];

    /// Stable name, used as a command argument.
    pub fn id(self) -> &'static str {
        match self {
            Self::TopLeft => "top-left",
            Self::Top => "top",
            Self::TopRight => "top-right",
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
            Self::BottomLeft => "bottom-left",
            Self::Bottom => "bottom",
            Self::BottomRight => "bottom-right",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|anchor| anchor.id() == id)
    }

    /// How far the picture moves when the canvas grows by `(grow_w, grow_h)` pixels.
    pub fn offset(self, grow_w: i32, grow_h: i32) -> (i32, i32) {
        let index = Self::ALL.iter().position(|a| *a == self).unwrap_or(0) as i32;
        let (column, row) = (index % 3, index / 3);
        (grow_w * column / 2, grow_h * row / 2)
    }
}

/// A command needs a file path from the user. The UI shows a native file dialog and then runs
/// the command again with the path.
#[derive(Clone, Debug, PartialEq)]
pub enum FileRequest {
    Open,
    SaveAs,
    ExportPng {
        scale: u32,
    },
    ImportPalette,
    ExportPalette,
    ReferenceImage,
    /// Asks where to save, then runs `command` with `params` and the path.
    Export {
        command: &'static str,
        params: Params,
        /// What the file type is called, and its extension.
        file_type: (&'static str, &'static str),
    },
}

/// What the app knows about an open sprite besides the sprite itself.
#[derive(Clone, Default)]
struct DocState {
    path: Option<PathBuf>,
    saved_revision: u64,
    view: ViewState,
}

pub struct AppState {
    pub editor: Editor,
    /// Where the active sprite was loaded from or last saved to.
    pub file_path: Option<PathBuf>,
    /// The revision of the active sprite that matches what is on disk (or the blank sprite it
    /// began as).
    pub saved_revision: u64,
    /// How the active sprite is viewed.
    pub view: ViewState,
    /// The document the three fields above belong to.
    shown: Option<DocId>,
    /// The same three things for the open sprites that are not the active one.
    parked: HashMap<DocId, DocState>,
    /// The palette positions selected in the Palette panel, in the order they were picked.
    pub swatch_selection: Vec<usize>,
    /// The animation playing on the canvas, if one is.
    pub playing: Option<Playing>,
    /// The tag that is played, here and in the Preview panel; `None` plays every frame.
    pub play_tag: Option<usize>,
    pub onion_enabled: bool,
    pub onion: OnionSkin,
    /// The cels selected in the timeline.
    pub cel_range: Option<CelRange>,
    pub preview: PreviewSettings,
    /// How many copies a radial symmetry makes when it is switched on.
    pub radial_count: u8,
    /// Copy and paste also go through the system clipboard. Off in tests.
    pub system_clipboard: bool,
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
    /// The scripts: the Lua state, what they registered and the console's lines.
    pub scripts: crate::scripting::ScriptState,
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
            shown: None,
            parked: HashMap::new(),
            swatch_selection: Vec::new(),
            playing: None,
            play_tag: None,
            onion_enabled: false,
            onion: OnionSkin::default(),
            cel_range: None,
            preview: PreviewSettings::default(),
            radial_count: 6,
            system_clipboard: false,
            dock: panels::default_layout(),
            layout_generation: 0,
            theme: ThemeKind::default(),
            palette: PaletteState::default(),
            dialog: None,
            file_request: None,
            rename_layer_request: None,
            log: Vec::new(),
            scripts: crate::scripting::ScriptState::default(),
            quit_requested: false,
        }
    }
}

impl AppState {
    /// Makes `file_path`, `saved_revision` and `view` those of the editor's active document,
    /// putting the previous document's away. Called whenever the active document may have
    /// changed.
    pub fn sync_active_document(&mut self) {
        let active = self.editor.active_document();
        if active == self.shown {
            return;
        }
        // The selected swatches were positions in the other sprite's palette; the same goes
        // for everything about its frames.
        self.swatch_selection.clear();
        self.playing = None;
        self.play_tag = None;
        self.cel_range = None;
        let previous = DocState {
            path: self.file_path.take(),
            saved_revision: self.saved_revision,
            view: std::mem::take(&mut self.view),
        };
        if let Some(id) = self.shown
            && self.editor.document_by_id(id).is_some()
        {
            self.parked.insert(id, previous);
        }
        let next = active
            .and_then(|id| self.parked.remove(&id))
            .unwrap_or_default();
        self.file_path = next.path;
        self.saved_revision = next.saved_revision;
        self.view = next.view;
        self.shown = active;
        self.parked
            .retain(|id, _| self.editor.document_by_id(*id).is_some());
    }

    /// Starts playing from the frame being shown, or stops and goes back to it.
    pub fn toggle_playback(&mut self) {
        match self.playing.take() {
            Some(playing) => {
                if let Some(document) = self.editor.document_mut() {
                    document.set_active_frame(playing.return_to);
                }
            }
            None => {
                if let Some(document) = self.editor.document() {
                    let frame = document.active_frame();
                    self.playing = Some(Playing {
                        playback: Playback::starting_at(frame),
                        return_to: frame,
                        clock: None,
                    });
                }
            }
        }
    }

    /// Stops playing without going anywhere, e.g. because the user picked a frame.
    pub fn stop_playback(&mut self) {
        self.playing = None;
    }

    /// Moves a playing animation on to the time `now` (in seconds) and shows the frame it is
    /// at. Returns how many milliseconds the frame stays, or `None` if nothing is playing.
    pub fn tick_playback(&mut self, now: f64) -> Option<u32> {
        let mut playing = self.playing?;
        let Some(document) = self.editor.document() else {
            self.playing = None;
            return None;
        };
        let last = playing.clock.unwrap_or(now);
        let elapsed_ms = ((now - last) * 1000.0).floor().clamp(0.0, 1e9);
        // Whole milliseconds are used up; what is left of one stays for the next tick.
        playing.clock = Some(last + elapsed_ms / 1000.0);

        // A stroke in progress stays on its frame: time stands still while it is drawn.
        let changed = !self.editor.is_interacting()
            && playing
                .playback
                .advance(document.sprite(), self.play_tag, elapsed_ms as u32);
        let wait = playing.playback.until_next(document.sprite());
        if changed && let Some(document) = self.editor.document_mut() {
            document.set_active_frame(playing.playback.frame);
        }
        self.playing = Some(playing);
        Some(wait)
    }

    /// Opens `document` in a new tab and switches to it. `path` is the file it came from.
    pub fn open_document(&mut self, document: Document, path: Option<PathBuf>) {
        let saved_revision = document.revision();
        self.editor.add_document(document);
        self.sync_active_document();
        self.saved_revision = saved_revision;
        self.file_path = path;
        self.view.fit_pending = true;
    }

    /// Switches to another open sprite. Returns `false` if it is not open.
    pub fn activate_document(&mut self, id: DocId) -> bool {
        let found = self.editor.set_active_document(id);
        self.sync_active_document();
        found
    }

    /// Closes the active sprite without asking.
    pub fn close_active_document(&mut self) {
        if let Some(id) = self.editor.active_document() {
            self.editor.close_document(id);
            self.sync_active_document();
        }
    }

    /// The path and saved revision of any open document.
    fn doc_state(&self, id: DocId) -> (Option<&PathBuf>, u64) {
        if self.shown == Some(id) {
            (self.file_path.as_ref(), self.saved_revision)
        } else {
            let parked = self.parked.get(&id);
            (
                parked.and_then(|doc| doc.path.as_ref()),
                parked.map_or(0, |doc| doc.saved_revision),
            )
        }
    }

    /// The open document that came from `path`, if there is one.
    pub fn document_at(&self, path: &std::path::Path) -> Option<DocId> {
        let mut ids = self.editor.documents().map(|(id, _)| id);
        ids.find(|id| self.doc_state(*id).0.is_some_and(|p| p == path))
    }

    /// Whether an open sprite differs from what is on disk.
    pub fn is_unsaved(&self, id: DocId) -> bool {
        self.editor
            .document_by_id(id)
            .is_some_and(|document| document.revision() != self.doc_state(id).1)
    }

    /// Whether the active sprite differs from what is on disk.
    pub fn has_unsaved_changes(&self) -> bool {
        self.editor
            .active_document()
            .is_some_and(|id| self.is_unsaved(id))
    }

    /// Whether any open sprite has unsaved changes.
    pub fn any_unsaved_changes(&self) -> bool {
        let mut ids = self.editor.documents().map(|(id, _)| id);
        ids.any(|id| self.is_unsaved(id))
    }

    /// The file name of an open sprite, with a mark when it has unsaved changes.
    pub fn title_of(&self, id: DocId) -> String {
        let name = self
            .doc_state(id)
            .0
            .and_then(|path| path.file_name())
            .map_or_else(
                || "Untitled".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            );
        if self.is_unsaved(id) {
            format!("{name} •")
        } else {
            name
        }
    }

    /// The title of the active sprite, or of the empty canvas.
    pub fn document_title(&self) -> String {
        match self.editor.active_document() {
            Some(id) => self.title_of(id),
            None => "Canvas".to_owned(),
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
