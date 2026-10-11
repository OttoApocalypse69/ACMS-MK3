//! The document model (see `PLAN.md`, section 4.1).
//!
//! A [`Sprite`] can be read freely, but it can only be changed through a transaction on the
//! [`Document`](crate::document::Document) that owns it: the mutating methods here are private
//! to this crate.

use std::collections::HashMap;
use std::sync::Arc;

use thiserror::Error;

use crate::color::Rgba8;
use crate::geom::{Point, Rect};
use crate::image::Image;
use crate::selection::Mask;

/// Largest width or height of a sprite. The canvas is shown as a single texture, and this is
/// a size every GPU the app targets can hold.
pub const MAX_SPRITE_SIZE: u32 = 4096;

/// How long a new frame is shown.
pub const DEFAULT_FRAME_MS: u32 = 100;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SpriteError {
    #[error("a sprite must be between 1 and {MAX_SPRITE_SIZE} pixels on each side, not {0}×{1}")]
    InvalidSize(u32, u32),
    #[error("a sprite needs at least one layer")]
    NoLayers,
    #[error("a sprite needs at least one frame")]
    NoFrames,
    #[error("layer id {0} is used twice")]
    DuplicateLayer(u32),
    #[error("there is no layer with id {0}")]
    UnknownLayer(u32),
    #[error("there is no frame {0}")]
    UnknownFrame(usize),
    #[error("layer {0} is a group and has no pixels of its own")]
    NotAnImageLayer(u32),
    #[error("layer {0} is not a group")]
    NotAGroup(u32),
    #[error("the layer is locked")]
    LayerLocked(u32),
    #[error("the layer is hidden")]
    LayerHidden(u32),
    #[error("the last layer cannot be deleted")]
    LastLayer,
    #[error("there is no position {index} among those layers")]
    InvalidSlot { index: usize },
    #[error("a group cannot be moved into itself")]
    IntoItself,
    #[error("there is no image layer directly below to merge into")]
    NothingBelow,
    #[error("pixel data is {got_w}×{got_h} but the area to fill is {want_w}×{want_h}")]
    SizeMismatch {
        want_w: u32,
        want_h: u32,
        got_w: u32,
        got_h: u32,
    },
}

/// Identifies a layer for as long as the sprite exists, however layers are reordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LayerId(pub u32);

/// How a layer's pixels combine with what is below them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Add,
}

impl BlendMode {
    pub const ALL: [Self; 5] = [
        Self::Normal,
        Self::Multiply,
        Self::Screen,
        Self::Overlay,
        Self::Add,
    ];

    /// Stable lower-case name, used in files and command arguments.
    pub fn id(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Multiply => "multiply",
            Self::Screen => "screen",
            Self::Overlay => "overlay",
            Self::Add => "add",
        }
    }

    /// Name shown in the UI.
    pub fn name(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Multiply => "Multiply",
            Self::Screen => "Screen",
            Self::Overlay => "Overlay",
            Self::Add => "Add",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.id() == id)
    }
}

/// What only image layers have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageLayer {
    /// 0 is invisible, 255 fully opaque.
    pub opacity: u8,
    pub blend: BlendMode,
    /// A locked layer cannot be drawn on.
    pub locked: bool,
    /// Drawing only recolours pixels that already exist; transparency stays as it is.
    pub alpha_lock: bool,
}

impl Default for ImageLayer {
    fn default() -> Self {
        Self {
            opacity: 255,
            blend: BlendMode::Normal,
            locked: false,
            alpha_lock: false,
        }
    }
}

/// What only groups have.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupLayer {
    /// Bottom to top.
    pub children: Vec<Layer>,
    /// Whether the Layers panel hides the children. Not part of undo.
    pub collapsed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayerKind {
    Image(ImageLayer),
    Group(GroupLayer),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    /// A hidden group hides everything inside it.
    pub visible: bool,
    pub kind: LayerKind,
}

impl Layer {
    /// A visible image layer with default properties.
    pub fn image(id: LayerId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            visible: true,
            kind: LayerKind::Image(ImageLayer::default()),
        }
    }

    /// A visible, expanded group holding `children` (bottom to top).
    pub fn group(id: LayerId, name: impl Into<String>, children: Vec<Layer>) -> Self {
        let group = GroupLayer {
            children,
            collapsed: false,
        };
        Self {
            id,
            name: name.into(),
            visible: true,
            kind: LayerKind::Group(group),
        }
    }

    pub fn as_image(&self) -> Option<&ImageLayer> {
        match &self.kind {
            LayerKind::Image(image) => Some(image),
            LayerKind::Group(_) => None,
        }
    }

    pub fn as_group(&self) -> Option<&GroupLayer> {
        match &self.kind {
            LayerKind::Group(group) => Some(group),
            LayerKind::Image(_) => None,
        }
    }

    pub fn is_group(&self) -> bool {
        self.as_group().is_some()
    }

    /// The layers inside a group, bottom to top; empty for an image layer.
    pub fn children(&self) -> &[Layer] {
        self.as_group()
            .map_or(&[][..], |group| group.children.as_slice())
    }

    /// The properties that can change without the layer tree changing shape.
    pub fn props(&self) -> LayerProps {
        LayerProps {
            name: self.name.clone(),
            visible: self.visible,
            image: self.as_image().copied(),
        }
    }

    /// The ids of this layer and everything inside it.
    fn subtree_ids(&self, ids: &mut Vec<LayerId>) {
        ids.push(self.id);
        for child in self.children() {
            child.subtree_ids(ids);
        }
    }
}

/// A snapshot of a layer's changeable properties. `image` is `None` for a group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerProps {
    pub name: String,
    pub visible: bool,
    pub image: Option<ImageLayer>,
}

/// A position in the layer tree: `index` counts from the bottom among the children of
/// `parent`, or among the top-level layers when `parent` is `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub parent: Option<LayerId>,
    pub index: usize,
}

impl Slot {
    pub const fn new(parent: Option<LayerId>, index: usize) -> Self {
        Self { parent, index }
    }
}

/// One layer met while walking the tree.
#[derive(Clone, Copy, Debug)]
pub struct LayerEntry<'a> {
    pub layer: &'a Layer,
    /// 0 for top-level layers, 1 inside a group, and so on.
    pub depth: usize,
    pub slot: Slot,
    /// Whether the layer is drawn: it and every group around it are visible.
    pub shown: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub duration_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteEntry {
    pub color: Rgba8,
    pub name: Option<String>,
}

/// An ordered list of colours.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Palette {
    entries: Vec<PaletteEntry>,
}

impl Palette {
    pub fn new(entries: Vec<PaletteEntry>) -> Self {
        Self { entries }
    }

    pub fn from_colors(colors: impl IntoIterator<Item = Rgba8>) -> Self {
        let entries = colors
            .into_iter()
            .map(|color| PaletteEntry { color, name: None })
            .collect();
        Self { entries }
    }

    /// The palette new sprites start with: seven neutrals, then three shades each of eight
    /// hues, then a brown.
    pub fn default_palette() -> Self {
        const COLORS: [u32; 32] = [
            0x000000, 0x1D1F27, 0x3B3F4C, 0x666C7A, 0x9AA1AE, 0xCDD2DA, 0xFFFFFF, //
            0x6E1A2B, 0xC2293A, 0xF0605D, // red
            0x9C4A1A, 0xE8852B, 0xF7B76B, // orange
            0xB58A12, 0xF2CE3B, 0xFAE89A, // yellow
            0x1F6B3A, 0x3FA34D, 0x9BD86B, // green
            0x14596B, 0x2A9D9C, 0x7FD8C4, // teal
            0x1E3A8A, 0x3B6FE0, 0x7FB2F5, // blue
            0x4A2A7A, 0x8A4FD0, 0xC49BF0, // purple
            0x8A2A6B, 0xD9529E, 0xF5A3CC, // pink
            0x4A2E1F, // brown
        ];
        Self::from_colors(
            COLORS
                .into_iter()
                .map(|rgb| Rgba8::rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)),
        )
    }

    pub fn entries(&self) -> &[PaletteEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The pixels of one image layer in one frame.
///
/// A cel only stores the rectangle that has something in it, so a small drawing on a large
/// canvas stays small. A stored cel is never empty, has no fully transparent border, and its
/// fully transparent pixels are all [`Rgba8::TRANSPARENT`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cel {
    origin: Point,
    /// Shared so that linked cels can point at the same pixels.
    image: Arc<Image>,
}

impl Cel {
    /// Makes a cel from an image whose top-left pixel sits at `origin` on the canvas, cropped
    /// to its content. `None` if the image has nothing visible in it.
    pub(crate) fn cropped(image: &Image, origin: Point) -> Option<Self> {
        let content = image.content_bounds();
        if content.is_empty() {
            return None;
        }
        let pixels = image
            .crop(content)
            .pixels()
            .iter()
            .map(|px| px.normalized())
            .collect();
        let image = Image::from_pixels(content.w, content.h, pixels)?;
        Some(Self {
            origin: Point::new(origin.x + content.x, origin.y + content.y),
            image: Arc::new(image),
        })
    }

    /// Where the top-left pixel of [`Cel::image`] sits on the canvas.
    pub fn origin(&self) -> Point {
        self.origin
    }

    pub fn image(&self) -> &Image {
        &self.image
    }

    /// The canvas rectangle this cel covers.
    pub fn rect(&self) -> Rect {
        Rect::new(
            self.origin.x,
            self.origin.y,
            self.image.width(),
            self.image.height(),
        )
    }

    /// The pixel at a canvas position; transparent outside the cel.
    pub fn pixel(&self, x: i32, y: i32) -> Rgba8 {
        self.image.get(x - self.origin.x, y - self.origin.y)
    }
}

/// One cel as handed to [`Sprite::from_parts`]. It may be any size and sit anywhere; the sprite
/// clips and crops it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CelSource {
    pub layer: LayerId,
    pub frame: usize,
    pub origin: Point,
    pub image: Image,
}

/// A cel together with the layer and frame it belongs to.
pub(crate) type PlacedCel = (LayerId, usize, Cel);

fn find(layers: &[Layer], id: LayerId) -> Option<&Layer> {
    layers.iter().find_map(|layer| {
        if layer.id == id {
            Some(layer)
        } else {
            find(layer.children(), id)
        }
    })
}

fn find_mut(layers: &mut [Layer], id: LayerId) -> Option<&mut Layer> {
    for layer in layers {
        if layer.id == id {
            return Some(layer);
        }
        if let LayerKind::Group(group) = &mut layer.kind
            && let Some(found) = find_mut(&mut group.children, id)
        {
            return Some(found);
        }
    }
    None
}

fn find_slot(layers: &[Layer], parent: Option<LayerId>, id: LayerId) -> Option<Slot> {
    layers.iter().enumerate().find_map(|(index, layer)| {
        if layer.id == id {
            Some(Slot::new(parent, index))
        } else {
            find_slot(layer.children(), Some(layer.id), id)
        }
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sprite {
    width: u32,
    height: u32,
    palette: Palette,
    /// The top-level layers, bottom to top. Groups hold their own children.
    layers: Vec<Layer>,
    frames: Vec<Frame>,
    /// A missing entry is an empty cel.
    cels: HashMap<(LayerId, usize), Cel>,
    /// The pixels tools may change; `None` means all of them. Never an empty mask.
    selection: Option<Arc<Mask>>,
}

/// The size of the canvas and everything whose coordinates depend on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CanvasState {
    pub width: u32,
    pub height: u32,
    pub cels: HashMap<(LayerId, usize), Cel>,
    pub selection: Option<Arc<Mask>>,
}

impl Sprite {
    /// A blank sprite with one layer, one frame and the default palette.
    pub fn new(width: u32, height: u32) -> Result<Self, SpriteError> {
        let frame = Frame {
            duration_ms: DEFAULT_FRAME_MS,
        };
        Self::from_parts(
            width,
            height,
            Palette::default_palette(),
            vec![Layer::image(LayerId(1), "Layer 1")],
            vec![frame],
            Vec::new(),
        )
    }

    /// Assembles a sprite, for example from a file. Checks that the pieces fit together.
    /// `layers` are the top-level layers, bottom to top.
    pub fn from_parts(
        width: u32,
        height: u32,
        palette: Palette,
        layers: Vec<Layer>,
        frames: Vec<Frame>,
        cels: Vec<CelSource>,
    ) -> Result<Self, SpriteError> {
        let valid = 1..=MAX_SPRITE_SIZE;
        if !valid.contains(&width) || !valid.contains(&height) {
            return Err(SpriteError::InvalidSize(width, height));
        }
        if layers.is_empty() {
            return Err(SpriteError::NoLayers);
        }
        if frames.is_empty() {
            return Err(SpriteError::NoFrames);
        }

        let mut sprite = Self {
            width,
            height,
            palette,
            layers,
            frames,
            cels: HashMap::new(),
            selection: None,
        };
        let mut ids: Vec<LayerId> = sprite.walk().iter().map(|entry| entry.layer.id).collect();
        ids.sort();
        if let Some(pair) = ids.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(SpriteError::DuplicateLayer(pair[0].0));
        }

        for cel in cels {
            sprite.check_cel(cel.layer, cel.frame)?;
            let rect = cel.image.bounds().translated(cel.origin.x, cel.origin.y);
            sprite.write_region(cel.layer, cel.frame, rect, &cel.image);
        }
        Ok(sprite)
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The canvas rectangle.
    pub fn bounds(&self) -> Rect {
        Rect::from_size(self.width, self.height)
    }

    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// The top-level layers, bottom to top.
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// Any layer in the tree.
    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        find(&self.layers, id)
    }

    /// Where a layer sits in the tree.
    pub fn slot_of(&self, id: LayerId) -> Option<Slot> {
        find_slot(&self.layers, None, id)
    }

    /// The layers in the given slot's list: the children of a group, or the top level.
    pub fn siblings(&self, parent: Option<LayerId>) -> Result<&[Layer], SpriteError> {
        match parent {
            None => Ok(&self.layers),
            Some(id) => {
                let layer = self.layer(id).ok_or(SpriteError::UnknownLayer(id.0))?;
                let group = layer.as_group().ok_or(SpriteError::NotAGroup(id.0))?;
                Ok(&group.children)
            }
        }
    }

    /// Every layer in the order they are painted: bottom to top, a group just before what
    /// it contains.
    pub fn walk(&self) -> Vec<LayerEntry<'_>> {
        let mut entries = Vec::new();
        collect(&self.layers, None, 0, true, false, &mut entries);
        entries
    }

    /// Every layer in the order a layer list shows them: top to bottom, a group just above
    /// what it contains.
    pub fn walk_top_down(&self) -> Vec<LayerEntry<'_>> {
        let mut entries = Vec::new();
        collect(&self.layers, None, 0, true, true, &mut entries);
        entries
    }

    /// How many layers there are, groups and their contents included.
    pub fn layer_count(&self) -> usize {
        self.walk().len()
    }

    /// Whether `id` is `ancestor` itself or somewhere inside it.
    pub fn is_within(&self, id: LayerId, ancestor: LayerId) -> bool {
        self.layer(ancestor)
            .is_some_and(|layer| layer.id == id || find(layer.children(), id).is_some())
    }

    /// Whether a layer is drawn: it and every group around it are visible.
    pub fn is_shown(&self, id: LayerId) -> bool {
        self.walk()
            .iter()
            .any(|entry| entry.layer.id == id && entry.shown)
    }

    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    /// The selection, or `None` when nothing is selected and every pixel may be changed.
    pub fn selection(&self) -> Option<&Arc<Mask>> {
        self.selection.as_ref()
    }

    /// Whether tools may change this pixel as far as the selection is concerned.
    pub fn is_selected(&self, x: i32, y: i32) -> bool {
        self.selection
            .as_ref()
            .is_none_or(|mask| mask.contains(x, y))
    }

    pub(crate) fn set_selection(&mut self, selection: Option<Arc<Mask>>) {
        self.selection = selection.filter(|mask| !mask.is_empty());
    }

    pub(crate) fn canvas_state(&self) -> CanvasState {
        CanvasState {
            width: self.width,
            height: self.height,
            cels: self.cels.clone(),
            selection: self.selection.clone(),
        }
    }

    pub(crate) fn set_canvas_state(&mut self, state: CanvasState) {
        self.width = state.width;
        self.height = state.height;
        self.cels = state.cels;
        self.selection = state.selection;
    }

    /// Every cel as a full image positioned on the canvas, to rebuild them for another size.
    pub(crate) fn cel_sources(&self) -> Vec<CelSource> {
        let mut sources: Vec<CelSource> = self
            .cels
            .iter()
            .map(|(&(layer, frame), cel)| CelSource {
                layer,
                frame,
                origin: cel.origin,
                image: (*cel.image).clone(),
            })
            .collect();
        sources.sort_by_key(|cel| (cel.layer, cel.frame));
        sources
    }

    /// Replaces the canvas size and every cel. The cels are clipped and cropped as usual.
    pub(crate) fn rebuild_canvas(&mut self, width: u32, height: u32, cels: Vec<CelSource>) {
        self.width = width;
        self.height = height;
        self.cels.clear();
        self.selection = None;
        for cel in cels {
            let rect = cel.image.bounds().translated(cel.origin.x, cel.origin.y);
            self.write_region(cel.layer, cel.frame, rect, &cel.image);
        }
    }

    /// The cel of a layer in a frame, or `None` if nothing is drawn there.
    pub fn cel(&self, layer: LayerId, frame: usize) -> Option<&Cel> {
        self.cels.get(&(layer, frame))
    }

    /// Every non-empty cel, in no particular order.
    pub fn cels(&self) -> impl Iterator<Item = (LayerId, usize, &Cel)> {
        self.cels
            .iter()
            .map(|(&(layer, frame), cel)| (layer, frame, cel))
    }

    /// `Ok` if `layer` is an image layer and the frame exists, so a cel can live there.
    pub fn check_cel(&self, layer: LayerId, frame: usize) -> Result<ImageLayer, SpriteError> {
        let found = self
            .layer(layer)
            .ok_or(SpriteError::UnknownLayer(layer.0))?;
        let image = found
            .as_image()
            .ok_or(SpriteError::NotAnImageLayer(layer.0))?;
        if frame >= self.frames.len() {
            return Err(SpriteError::UnknownFrame(frame));
        }
        Ok(*image)
    }

    /// Like [`Sprite::check_cel`], and also that a tool may draw there: the layer is neither
    /// locked nor hidden.
    pub fn check_drawable(&self, layer: LayerId, frame: usize) -> Result<ImageLayer, SpriteError> {
        let image = self.check_cel(layer, frame)?;
        if image.locked {
            return Err(SpriteError::LayerLocked(layer.0));
        }
        if !self.is_shown(layer) {
            return Err(SpriteError::LayerHidden(layer.0));
        }
        Ok(image)
    }

    /// The pixel of one layer at a canvas position; transparent where nothing is drawn.
    pub fn pixel(&self, layer: LayerId, frame: usize, x: i32, y: i32) -> Rgba8 {
        self.cel(layer, frame)
            .map_or(Rgba8::TRANSPARENT, |cel| cel.pixel(x, y))
    }

    /// A copy of a canvas rectangle of one layer.
    pub fn read_region(&self, layer: LayerId, frame: usize, rect: Rect) -> Image {
        match self.cel(layer, frame) {
            Some(cel) => cel
                .image
                .crop(rect.translated(-cel.origin.x, -cel.origin.y)),
            None => Image::new(rect.w, rect.h),
        }
    }

    /// The canvas area covered by a layer (and everything inside it, for a group) in any
    /// frame.
    pub fn layer_bounds(&self, id: LayerId) -> Rect {
        let mut ids = Vec::new();
        if let Some(layer) = self.layer(id) {
            layer.subtree_ids(&mut ids);
        }
        self.cels
            .iter()
            .filter(|((layer, _), _)| ids.contains(layer))
            .fold(Rect::EMPTY, |bounds, (_, cel)| bounds.union(cel.rect()))
    }

    /// Replaces the pixels of `rect` with `pixels` (which must be `rect`-sized), then re-crops
    /// the cel to its content. Anything outside the canvas is dropped.
    ///
    /// The caller has checked that the layer and frame exist.
    pub(crate) fn write_region(
        &mut self,
        layer: LayerId,
        frame: usize,
        rect: Rect,
        pixels: &Image,
    ) {
        debug_assert_eq!((rect.w, rect.h), (pixels.width(), pixels.height()));
        let target = rect.intersect(self.bounds());
        if target.is_empty() {
            return;
        }

        let old = self.cels.remove(&(layer, frame));
        let old_rect = old.as_ref().map_or(Rect::EMPTY, Cel::rect);
        let grown = old_rect.union(target);

        let mut image = match old {
            Some(cel) if grown == old_rect => Arc::unwrap_or_clone(cel.image),
            Some(cel) => {
                let mut image = Image::new(grown.w, grown.h);
                let at = Point::new(old_rect.x - grown.x, old_rect.y - grown.y);
                image.blit(&cel.image, at);
                image
            }
            None => Image::new(grown.w, grown.h),
        };

        // Only erasing can leave the cel with an empty border to crop away.
        let mut erased = false;
        for y in target.y..target.bottom() {
            for x in target.x..target.right() {
                let color = pixels.get(x - rect.x, y - rect.y).normalized();
                erased |= color.a == 0;
                image.set(x - grown.x, y - grown.y, color);
            }
        }

        let mut cel_rect = grown;
        if erased {
            let content = image.content_bounds();
            if content.is_empty() {
                return;
            }
            if content != image.bounds() {
                image = image.crop(content);
                cel_rect = content.translated(grown.x, grown.y);
            }
        }
        self.cels.insert(
            (layer, frame),
            Cel {
                origin: cel_rect.origin(),
                image: Arc::new(image),
            },
        );
    }

    fn children_mut(&mut self, parent: Option<LayerId>) -> Option<&mut Vec<Layer>> {
        match parent {
            None => Some(&mut self.layers),
            Some(id) => match &mut find_mut(&mut self.layers, id)?.kind {
                LayerKind::Group(group) => Some(&mut group.children),
                LayerKind::Image(_) => None,
            },
        }
    }

    /// Puts a layer (with everything inside it) and its cels into the tree. Does nothing if
    /// the slot's parent is not a group; an index past the end appends.
    pub(crate) fn insert_layer(&mut self, slot: Slot, layer: Layer, cels: Vec<PlacedCel>) {
        let Some(siblings) = self.children_mut(slot.parent) else {
            return;
        };
        siblings.insert(slot.index.min(siblings.len()), layer);
        for (layer, frame, cel) in cels {
            self.cels.insert((layer, frame), cel);
        }
    }

    /// Takes a layer (with everything inside it) and its cels out of the tree.
    pub(crate) fn remove_layer(&mut self, id: LayerId) -> Option<(Slot, Layer, Vec<PlacedCel>)> {
        let slot = self.slot_of(id)?;
        let layer = self.children_mut(slot.parent)?.remove(slot.index);

        let mut ids = Vec::new();
        layer.subtree_ids(&mut ids);
        let mut keys: Vec<(LayerId, usize)> = self
            .cels
            .keys()
            .filter(|(layer, _)| ids.contains(layer))
            .copied()
            .collect();
        keys.sort();
        let cels = keys
            .into_iter()
            .filter_map(|key| self.cels.remove(&key).map(|cel| (key.0, key.1, cel)))
            .collect();
        Some((slot, layer, cels))
    }

    /// Moves a layer to `to`, where `to.index` is its position once it has left its old
    /// place.
    pub(crate) fn move_layer(&mut self, id: LayerId, to: Slot) {
        if let Some((_, layer, cels)) = self.remove_layer(id) {
            self.insert_layer(to, layer, cels);
        }
    }

    pub(crate) fn set_layer_props(&mut self, id: LayerId, props: &LayerProps) {
        let Some(layer) = find_mut(&mut self.layers, id) else {
            return;
        };
        layer.name.clone_from(&props.name);
        layer.visible = props.visible;
        if let (LayerKind::Image(image), Some(new)) = (&mut layer.kind, props.image) {
            *image = new;
        }
    }

    /// Returns `false` if `id` is not a group.
    pub(crate) fn set_collapsed(&mut self, id: LayerId, collapsed: bool) -> bool {
        match find_mut(&mut self.layers, id).map(|layer| &mut layer.kind) {
            Some(LayerKind::Group(group)) => {
                group.collapsed = collapsed;
                true
            }
            _ => false,
        }
    }
}

fn collect<'a>(
    layers: &'a [Layer],
    parent: Option<LayerId>,
    depth: usize,
    parent_shown: bool,
    top_down: bool,
    entries: &mut Vec<LayerEntry<'a>>,
) {
    let mut visit = |(index, layer): (usize, &'a Layer)| {
        let shown = parent_shown && layer.visible;
        entries.push(LayerEntry {
            layer,
            depth,
            slot: Slot::new(parent, index),
            shown,
        });
        let children = layer.children();
        collect(
            children,
            Some(layer.id),
            depth + 1,
            shown,
            top_down,
            entries,
        );
    };
    if top_down {
        layers.iter().enumerate().rev().for_each(&mut visit);
    } else {
        layers.iter().enumerate().for_each(&mut visit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgba8 = Rgba8::rgb(255, 0, 0);
    const L1: LayerId = LayerId(1);

    fn filled(w: u32, h: u32, color: Rgba8) -> Image {
        Image::from_pixels(w, h, vec![color; (w * h) as usize]).unwrap()
    }

    fn frame() -> Frame {
        Frame { duration_ms: 100 }
    }

    /// ```text
    /// 5 Top
    /// 4 Group
    ///     3 Inner top
    ///     2 Inner bottom
    /// 1 Bottom
    /// ```
    fn tree() -> Sprite {
        let layers = vec![
            Layer::image(LayerId(1), "Bottom"),
            Layer::group(
                LayerId(4),
                "Group",
                vec![
                    Layer::image(LayerId(2), "Inner bottom"),
                    Layer::image(LayerId(3), "Inner top"),
                ],
            ),
            Layer::image(LayerId(5), "Top"),
        ];
        let cel = |layer| CelSource {
            layer: LayerId(layer),
            frame: 0,
            origin: Point::new(layer as i32, 0),
            image: filled(1, 1, RED),
        };
        let cels = vec![cel(1), cel(2), cel(3), cel(5)];
        Sprite::from_parts(8, 8, Palette::default(), layers, vec![frame()], cels).unwrap()
    }

    fn ids(entries: &[LayerEntry<'_>]) -> Vec<u32> {
        entries.iter().map(|entry| entry.layer.id.0).collect()
    }

    #[test]
    fn new_sprite_has_one_layer_one_frame_and_no_pixels() {
        let sprite = Sprite::new(16, 8).unwrap();
        assert_eq!((sprite.width(), sprite.height()), (16, 8));
        assert_eq!(sprite.layers().len(), 1);
        assert_eq!(sprite.layers()[0].as_image(), Some(&ImageLayer::default()));
        assert!(sprite.layers()[0].visible);
        assert_eq!(sprite.frames().len(), 1);
        assert_eq!(sprite.cels().count(), 0);
        assert_eq!(sprite.palette().len(), 32);
        assert_eq!(sprite.pixel(L1, 0, 3, 3), Rgba8::TRANSPARENT);
    }

    #[test]
    fn sizes_outside_the_limits_are_rejected() {
        assert_eq!(Sprite::new(0, 8), Err(SpriteError::InvalidSize(0, 8)));
        assert_eq!(
            Sprite::new(8, MAX_SPRITE_SIZE + 1),
            Err(SpriteError::InvalidSize(8, MAX_SPRITE_SIZE + 1))
        );
        assert!(Sprite::new(MAX_SPRITE_SIZE, 1).is_ok());
    }

    #[test]
    fn writing_grows_the_cel_just_enough() {
        let mut sprite = Sprite::new(32, 32).unwrap();
        sprite.write_region(L1, 0, Rect::new(10, 10, 2, 2), &filled(2, 2, RED));
        assert_eq!(sprite.cel(L1, 0).unwrap().rect(), Rect::new(10, 10, 2, 2));

        sprite.write_region(L1, 0, Rect::new(20, 5, 1, 1), &filled(1, 1, RED));
        assert_eq!(sprite.cel(L1, 0).unwrap().rect(), Rect::new(10, 5, 11, 7));
        assert_eq!(sprite.pixel(L1, 0, 20, 5), RED);
        assert_eq!(sprite.pixel(L1, 0, 11, 11), RED);
        assert_eq!(sprite.pixel(L1, 0, 15, 8), Rgba8::TRANSPARENT);
    }

    #[test]
    fn erasing_crops_the_cel_and_removes_it_when_nothing_is_left() {
        let mut sprite = Sprite::new(32, 32).unwrap();
        sprite.write_region(L1, 0, Rect::new(4, 4, 6, 1), &filled(6, 1, RED));
        sprite.write_region(L1, 0, Rect::new(7, 4, 3, 1), &Image::new(3, 1));
        assert_eq!(sprite.cel(L1, 0).unwrap().rect(), Rect::new(4, 4, 3, 1));

        sprite.write_region(L1, 0, Rect::new(0, 0, 32, 32), &Image::new(32, 32));
        assert!(sprite.cel(L1, 0).is_none());
    }

    #[test]
    fn writes_are_clipped_to_the_canvas() {
        let mut sprite = Sprite::new(4, 4).unwrap();
        sprite.write_region(L1, 0, Rect::new(-2, -2, 4, 4), &filled(4, 4, RED));
        assert_eq!(sprite.cel(L1, 0).unwrap().rect(), Rect::new(0, 0, 2, 2));

        sprite.write_region(L1, 0, Rect::new(10, 10, 4, 4), &filled(4, 4, RED));
        assert_eq!(sprite.cel(L1, 0).unwrap().rect(), Rect::new(0, 0, 2, 2));
    }

    #[test]
    fn invisible_colour_is_stored_as_plain_transparency() {
        let mut sprite = Sprite::new(4, 4).unwrap();
        let ghost = Rgba8::new(200, 100, 50, 0);
        sprite.write_region(L1, 0, Rect::new(0, 0, 2, 1), &filled(2, 1, RED));
        sprite.write_region(L1, 0, Rect::new(1, 0, 1, 1), &filled(1, 1, ghost));
        assert_eq!(sprite.cel(L1, 0).unwrap().rect(), Rect::new(0, 0, 1, 1));
        assert_eq!(sprite.pixel(L1, 0, 1, 0), Rgba8::TRANSPARENT);
    }

    #[test]
    fn read_region_pads_with_transparency() {
        let mut sprite = Sprite::new(8, 8).unwrap();
        sprite.write_region(L1, 0, Rect::new(2, 2, 1, 1), &filled(1, 1, RED));
        let region = sprite.read_region(L1, 0, Rect::new(1, 1, 3, 3));
        assert_eq!(region.get(1, 1), RED);
        assert_eq!(region.content_bounds(), Rect::new(1, 1, 1, 1));
        assert!(
            sprite
                .read_region(L1, 0, Rect::new(5, 5, 2, 2))
                .content_bounds()
                .is_empty()
        );
    }

    #[test]
    fn from_parts_checks_its_pieces() {
        let layer = |id| Layer::image(LayerId(id), format!("Layer {id}"));
        let build = |layers, frames, cels| {
            Sprite::from_parts(8, 8, Palette::default(), layers, frames, cels)
        };

        assert_eq!(
            build(vec![], vec![frame()], vec![]),
            Err(SpriteError::NoLayers)
        );
        assert_eq!(
            build(vec![layer(1)], vec![], vec![]),
            Err(SpriteError::NoFrames)
        );
        assert_eq!(
            build(vec![layer(1), layer(1)], vec![frame()], vec![]),
            Err(SpriteError::DuplicateLayer(1))
        );
        // The same id deep inside a group is caught too.
        let nested = Layer::group(LayerId(2), "Group", vec![layer(1)]);
        assert_eq!(
            build(vec![layer(1), nested], vec![frame()], vec![]),
            Err(SpriteError::DuplicateLayer(1))
        );

        let cel = |layer, frame| CelSource {
            layer: LayerId(layer),
            frame,
            origin: Point::new(6, 6),
            image: filled(4, 4, RED),
        };
        assert_eq!(
            build(vec![layer(1)], vec![frame()], vec![cel(2, 0)]),
            Err(SpriteError::UnknownLayer(2))
        );
        assert_eq!(
            build(vec![layer(1)], vec![frame()], vec![cel(1, 1)]),
            Err(SpriteError::UnknownFrame(1))
        );
        // A group has no pixels of its own.
        let group = Layer::group(LayerId(2), "Group", vec![]);
        assert_eq!(
            build(vec![layer(1), group], vec![frame()], vec![cel(2, 0)]),
            Err(SpriteError::NotAnImageLayer(2))
        );

        // A cel hanging over the edge is clipped to the canvas.
        let sprite = build(vec![layer(1)], vec![frame()], vec![cel(1, 0)]).unwrap();
        assert_eq!(sprite.cel(L1, 0).unwrap().rect(), Rect::new(6, 6, 2, 2));
    }

    #[test]
    fn walking_visits_groups_and_their_children() {
        let sprite = tree();
        assert_eq!(ids(&sprite.walk()), [1, 4, 2, 3, 5]);
        assert_eq!(ids(&sprite.walk_top_down()), [5, 4, 3, 2, 1]);
        assert_eq!(sprite.layer_count(), 5);

        let entries = sprite.walk();
        let inner = entries.iter().find(|e| e.layer.id == LayerId(3)).unwrap();
        assert_eq!(inner.depth, 1);
        assert_eq!(inner.slot, Slot::new(Some(LayerId(4)), 1));
        let top = entries.iter().find(|e| e.layer.id == LayerId(5)).unwrap();
        assert_eq!((top.depth, top.slot), (0, Slot::new(None, 2)));
    }

    #[test]
    fn finding_layers_and_their_places() {
        let sprite = tree();
        assert_eq!(sprite.layer(LayerId(3)).unwrap().name, "Inner top");
        assert!(sprite.layer(LayerId(9)).is_none());
        assert_eq!(
            sprite.slot_of(LayerId(2)),
            Some(Slot::new(Some(LayerId(4)), 0))
        );
        assert_eq!(sprite.slot_of(LayerId(9)), None);

        assert_eq!(sprite.siblings(None).unwrap().len(), 3);
        assert_eq!(sprite.siblings(Some(LayerId(4))).unwrap().len(), 2);
        assert_eq!(
            sprite.siblings(Some(LayerId(1))).err(),
            Some(SpriteError::NotAGroup(1))
        );
        assert_eq!(
            sprite.siblings(Some(LayerId(9))).err(),
            Some(SpriteError::UnknownLayer(9))
        );

        assert!(sprite.is_within(LayerId(3), LayerId(4)));
        assert!(sprite.is_within(LayerId(4), LayerId(4)));
        assert!(!sprite.is_within(LayerId(5), LayerId(4)));
        assert!(!sprite.is_within(LayerId(4), LayerId(3)));
    }

    #[test]
    fn a_hidden_group_hides_what_is_inside_it() {
        let mut sprite = tree();
        assert!(sprite.is_shown(LayerId(3)));

        let mut props = sprite.layer(LayerId(4)).unwrap().props();
        props.visible = false;
        sprite.set_layer_props(LayerId(4), &props);
        assert!(!sprite.is_shown(LayerId(4)));
        assert!(!sprite.is_shown(LayerId(3)));
        assert!(sprite.is_shown(LayerId(5)));
        // The child's own flag is untouched.
        assert!(sprite.layer(LayerId(3)).unwrap().visible);
    }

    #[test]
    fn removing_and_reinserting_a_group_restores_the_sprite() {
        let mut sprite = tree();
        let original = sprite.clone();

        let (slot, layer, cels) = sprite.remove_layer(LayerId(4)).unwrap();
        assert_eq!(slot, Slot::new(None, 1));
        assert_eq!(cels.len(), 2);
        assert_eq!(ids(&sprite.walk()), [1, 5]);
        assert!(sprite.cel(LayerId(2), 0).is_none());
        assert_eq!(sprite.cels().count(), 2);

        sprite.insert_layer(slot, layer, cels);
        assert_eq!(sprite, original);
        assert!(sprite.remove_layer(LayerId(9)).is_none());
    }

    #[test]
    fn moving_a_layer_keeps_its_pixels() {
        let mut sprite = tree();
        // Out of the group, to the very top.
        sprite.move_layer(LayerId(2), Slot::new(None, 3));
        assert_eq!(ids(&sprite.walk()), [1, 4, 3, 5, 2]);
        assert_eq!(sprite.pixel(LayerId(2), 0, 2, 0), RED);

        // Into the group, at the bottom.
        sprite.move_layer(LayerId(5), Slot::new(Some(LayerId(4)), 0));
        assert_eq!(ids(&sprite.walk()), [1, 4, 5, 3, 2]);
        assert_eq!(
            sprite.slot_of(LayerId(5)),
            Some(Slot::new(Some(LayerId(4)), 0))
        );
    }

    #[test]
    fn properties_apply_to_the_right_kind_of_layer() {
        let mut sprite = tree();
        let props = LayerProps {
            name: "Renamed".to_owned(),
            visible: false,
            image: Some(ImageLayer {
                opacity: 100,
                blend: BlendMode::Multiply,
                locked: true,
                alpha_lock: true,
            }),
        };
        sprite.set_layer_props(LayerId(1), &props);
        assert_eq!(sprite.layer(LayerId(1)).unwrap().props(), props);

        // A group takes the name and visibility and ignores the rest.
        sprite.set_layer_props(LayerId(4), &props);
        let group = sprite.layer(LayerId(4)).unwrap();
        assert_eq!((group.name.as_str(), group.visible), ("Renamed", false));
        assert_eq!(group.props().image, None);
        assert_eq!(group.children().len(), 2);

        assert!(sprite.set_collapsed(LayerId(4), true));
        assert!(
            sprite
                .layer(LayerId(4))
                .unwrap()
                .as_group()
                .unwrap()
                .collapsed
        );
        assert!(!sprite.set_collapsed(LayerId(1), true));
    }

    #[test]
    fn drawable_means_an_unlocked_shown_image_layer() {
        let mut sprite = tree();
        assert!(sprite.check_drawable(LayerId(3), 0).is_ok());
        assert_eq!(
            sprite.check_drawable(LayerId(4), 0),
            Err(SpriteError::NotAnImageLayer(4))
        );
        assert_eq!(
            sprite.check_drawable(LayerId(3), 7),
            Err(SpriteError::UnknownFrame(7))
        );

        let mut props = sprite.layer(LayerId(3)).unwrap().props();
        props.image = Some(ImageLayer {
            locked: true,
            ..ImageLayer::default()
        });
        sprite.set_layer_props(LayerId(3), &props);
        assert_eq!(
            sprite.check_drawable(LayerId(3), 0),
            Err(SpriteError::LayerLocked(3))
        );

        let mut group = sprite.layer(LayerId(4)).unwrap().props();
        group.visible = false;
        sprite.set_layer_props(LayerId(4), &group);
        assert_eq!(
            sprite.check_drawable(LayerId(2), 0),
            Err(SpriteError::LayerHidden(2))
        );
    }

    #[test]
    fn layer_bounds_cover_a_whole_group() {
        let sprite = tree();
        assert_eq!(sprite.layer_bounds(LayerId(1)), Rect::new(1, 0, 1, 1));
        assert_eq!(sprite.layer_bounds(LayerId(4)), Rect::new(2, 0, 2, 1));
        assert_eq!(sprite.layer_bounds(LayerId(9)), Rect::EMPTY);
    }

    #[test]
    fn cropped_cels_drop_the_empty_border() {
        let mut image = Image::new(6, 6);
        image.set(2, 3, RED);
        image.set(4, 3, Rgba8::new(9, 9, 9, 0));
        let cel = Cel::cropped(&image, Point::new(10, 20)).unwrap();
        assert_eq!(cel.rect(), Rect::new(12, 23, 1, 1));
        assert!(Cel::cropped(&Image::new(4, 4), Point::new(0, 0)).is_none());
    }

    #[test]
    fn blend_modes_have_stable_ids() {
        for mode in BlendMode::ALL {
            assert_eq!(BlendMode::from_id(mode.id()), Some(mode));
        }
        assert_eq!(BlendMode::from_id("dissolve"), None);
        assert_eq!(BlendMode::default(), BlendMode::Normal);
    }
}
