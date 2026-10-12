//! Tilesets, tile grids and slices (see `PLAN.md`, phase 6).
//!
//! A tilemap layer does not own pixels: each of its cels is a grid of tile numbers, and what
//! is seen is drawn from the tileset. Tile number 0 is "no tile"; the tiles of a set are
//! numbered from 1.

use std::sync::Arc;

use crate::color::Rgba8;
use crate::geom::{Point, Rect};
use crate::image::Image;

/// Largest side of a tile.
pub const MAX_TILE_SIZE: u32 = 256;

/// Identifies a tileset for as long as the sprite exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TilesetId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tileset {
    pub id: TilesetId,
    pub name: String,
    tile_width: u32,
    tile_height: u32,
    /// Tile `n` is `tiles[n - 1]`. Each is exactly one tile in size.
    tiles: Vec<Arc<Image>>,
}

impl Tileset {
    /// An empty tileset. The tile size is clamped to `1..=MAX_TILE_SIZE`.
    pub fn new(id: TilesetId, name: impl Into<String>, tile_width: u32, tile_height: u32) -> Self {
        Self {
            id,
            name: name.into(),
            tile_width: tile_width.clamp(1, MAX_TILE_SIZE),
            tile_height: tile_height.clamp(1, MAX_TILE_SIZE),
            tiles: Vec::new(),
        }
    }

    /// A tileset cut out of a picture of its tiles laid out in rows of `columns`, as
    /// [`Tileset::atlas`] makes it.
    pub fn from_atlas(mut self, atlas: &Image, count: usize, columns: u32) -> Self {
        let columns = columns.max(1) as usize;
        self.tiles = (0..count)
            .map(|index| {
                let x = (index % columns) as i32 * self.tile_width as i32;
                let y = (index / columns) as i32 * self.tile_height as i32;
                Arc::new(atlas.crop(Rect::new(x, y, self.tile_width, self.tile_height)))
            })
            .collect();
        self
    }

    pub fn tile_width(&self) -> u32 {
        self.tile_width
    }

    pub fn tile_height(&self) -> u32 {
        self.tile_height
    }

    /// How many tiles there are; also the highest tile number.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// The picture of a tile; `None` for 0 and for numbers past the end.
    pub fn tile(&self, number: u32) -> Option<&Image> {
        let index = (number as usize).checked_sub(1)?;
        self.tiles.get(index).map(|tile| &**tile)
    }

    /// The number of the tile that looks exactly like `picture`.
    pub fn find(&self, picture: &Image) -> Option<u32> {
        let at = self.tiles.iter().position(|tile| **tile == *picture)?;
        Some(at as u32 + 1)
    }

    /// Adds a tile and returns its number.
    pub(crate) fn push(&mut self, picture: Image) -> u32 {
        self.tiles.push(Arc::new(picture));
        self.tiles.len() as u32
    }

    pub(crate) fn tile_mut(&mut self, number: u32) -> Option<&mut Image> {
        let index = (number as usize).checked_sub(1)?;
        self.tiles.get_mut(index).map(Arc::make_mut)
    }

    /// Takes out tiles that have nothing in them and tiles that look like an earlier one.
    /// Returns what each old tile number has become (index 0 is "no tile" and stays 0).
    pub(crate) fn tidy(&mut self) -> Vec<u32> {
        let mut kept: Vec<Arc<Image>> = Vec::with_capacity(self.tiles.len());
        let mut renumber = vec![0];
        for tile in std::mem::take(&mut self.tiles) {
            let new = if tile.content_bounds().is_empty() {
                0
            } else if let Some(same) = kept.iter().position(|other| **other == *tile) {
                same as u32 + 1
            } else {
                kept.push(tile);
                kept.len() as u32
            };
            renumber.push(new);
        }
        self.tiles = kept;
        renumber
    }

    /// How many tiles are in a row of an atlas that is about square.
    pub fn atlas_columns(&self) -> u32 {
        ((self.tiles.len() as f64).sqrt().ceil() as u32).max(1)
    }

    /// All tiles in one picture, `columns` to a row, in order. A set without tiles gives a
    /// picture of one empty tile.
    pub fn atlas(&self, columns: u32) -> Image {
        let columns = columns.max(1);
        let rows = (self.tiles.len() as u32).div_ceil(columns).max(1);
        let mut image = Image::new(columns * self.tile_width, rows * self.tile_height);
        for (index, tile) in self.tiles.iter().enumerate() {
            let x = (index as u32 % columns * self.tile_width) as i32;
            let y = (index as u32 / columns * self.tile_height) as i32;
            image.blit(tile, Point::new(x, y));
        }
        image
    }
}

/// Which tile is in each cell of a tilemap cel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileGrid {
    width: u32,
    height: u32,
    cells: Vec<u32>,
}

impl TileGrid {
    /// A grid without tiles.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            cells: vec![0; width as usize * height as usize],
        }
    }

    /// A grid from its cells, row by row. `None` if there are not `width × height` of them.
    pub fn from_cells(width: u32, height: u32, cells: Vec<u32>) -> Option<Self> {
        (cells.len() == width as usize * height as usize).then_some(Self {
            width,
            height,
            cells,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The tile numbers, row by row.
    pub fn cells(&self) -> &[u32] {
        &self.cells
    }

    /// The tile in a cell; 0 outside the grid.
    pub fn get(&self, x: u32, y: u32) -> u32 {
        if x < self.width && y < self.height {
            self.cells[(y * self.width + x) as usize]
        } else {
            0
        }
    }

    pub(crate) fn set(&mut self, x: u32, y: u32, tile: u32) {
        if x < self.width && y < self.height {
            self.cells[(y * self.width + x) as usize] = tile;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.cells.iter().all(|tile| *tile == 0)
    }

    pub(crate) fn renumber(&mut self, renumber: &[u32]) {
        for cell in &mut self.cells {
            *cell = renumber.get(*cell as usize).copied().unwrap_or(0);
        }
    }

    /// What the grid looks like with the tiles of `tileset`: an image of
    /// `width × height` tiles.
    pub fn render(&self, tileset: &Tileset) -> Image {
        let (tw, th) = (tileset.tile_width(), tileset.tile_height());
        let mut image = Image::new(self.width * tw, self.height * th);
        for y in 0..self.height {
            for x in 0..self.width {
                if let Some(tile) = tileset.tile(self.get(x, y)) {
                    image.blit(tile, Point::new((x * tw) as i32, (y * th) as i32));
                }
            }
        }
        image
    }
}

/// A named rectangle of the sprite, for a game engine to cut out: a button, a hitbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slice {
    pub name: String,
    pub rect: Rect,
    /// For a 9-slice: the part that stretches, relative to `rect`'s corner.
    pub center: Option<Rect>,
    /// A point to hold the slice by, relative to `rect`'s corner.
    pub pivot: Option<Point>,
    pub color: Rgba8,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgba8 = Rgba8::rgb(255, 0, 0);
    const BLUE: Rgba8 = Rgba8::rgb(0, 0, 255);

    fn solid(color: Rgba8) -> Image {
        Image::from_pixels(2, 2, vec![color; 4]).unwrap()
    }

    fn set() -> Tileset {
        let mut tileset = Tileset::new(TilesetId(1), "Test", 2, 2);
        tileset.push(solid(RED));
        tileset.push(solid(BLUE));
        tileset
    }

    #[test]
    fn tiles_are_numbered_from_one() {
        let tileset = set();
        assert_eq!(tileset.len(), 2);
        assert_eq!(tileset.tile(0), None);
        assert_eq!(tileset.tile(1), Some(&solid(RED)));
        assert_eq!(tileset.tile(3), None);
        assert_eq!(tileset.find(&solid(BLUE)), Some(2));
        assert_eq!(tileset.find(&Image::new(2, 2)), None);
        assert_eq!(
            Tileset::new(TilesetId(1), "x", 0, 9999).tile_height(),
            MAX_TILE_SIZE
        );
    }

    #[test]
    fn tidying_merges_duplicates_and_drops_empty_tiles() {
        let mut tileset = set();
        tileset.push(Image::new(2, 2));
        tileset.push(solid(RED));
        tileset.push(solid(Rgba8::WHITE));
        assert_eq!(tileset.tidy(), [0, 1, 2, 0, 1, 3]);
        assert_eq!(tileset.len(), 3);
        assert_eq!(tileset.tile(3), Some(&solid(Rgba8::WHITE)));

        let mut grid = TileGrid::from_cells(3, 1, vec![4, 3, 5]).unwrap();
        grid.renumber(&[0, 1, 2, 0, 1, 3]);
        assert_eq!(grid.cells(), [1, 0, 3]);
    }

    #[test]
    fn a_grid_renders_its_tiles_and_an_atlas_round_trips() {
        let tileset = set();
        let mut grid = TileGrid::new(3, 2);
        assert!(grid.is_empty());
        grid.set(0, 0, 1);
        grid.set(2, 1, 2);
        grid.set(1, 1, 9);
        grid.set(7, 7, 1);
        let image = grid.render(&tileset);
        assert_eq!((image.width(), image.height()), (6, 4));
        assert_eq!(image.get(1, 1), RED);
        assert_eq!(image.get(5, 3), BLUE);
        // A tile number the set does not have shows nothing.
        assert_eq!(image.get(2, 2), Rgba8::TRANSPARENT);
        assert_eq!((grid.get(2, 1), grid.get(9, 9)), (2, 0));
        assert_eq!(TileGrid::from_cells(2, 2, vec![0; 3]), None);

        assert_eq!(tileset.atlas_columns(), 2);
        let atlas = tileset.atlas(1);
        assert_eq!((atlas.width(), atlas.height()), (2, 4));
        let back = Tileset::new(TilesetId(1), "Test", 2, 2).from_atlas(&atlas, 2, 1);
        assert_eq!(back, tileset);
        let empty = Tileset::new(TilesetId(2), "Empty", 4, 4);
        assert_eq!(empty.atlas(3).width(), 12);
    }
}
