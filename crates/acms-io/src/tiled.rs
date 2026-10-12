//! Exporting tilesets and tilemaps: a tileset as a PNG, a tilemap as a Tiled map (`.tmj`)
//! or as CSV.

use std::io::Write;
use std::path::{Path, PathBuf};

use acms_core::image::Image;
use acms_core::sprite::Sprite;
use acms_core::tiles::{TileGrid, Tileset};
use serde_json::json;

use crate::png::encode_png;
use crate::{IoError, write_atomically};

fn unsupported(message: impl Into<String>) -> IoError {
    IoError::Unsupported(message.into())
}

/// A name that is safe as part of a file name on every system.
fn file_part(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unnamed".to_owned()
    } else {
        cleaned
    }
}

fn stem(path: &Path) -> String {
    path.file_stem().map_or_else(
        || "map".to_owned(),
        |stem| stem.to_string_lossy().into_owned(),
    )
}

/// The picture of a tileset as it is exported: its tiles in rows, about square, without
/// margin or spacing. Returns the picture and how many tiles are in a row.
pub fn tileset_image(tileset: &Tileset) -> (Image, u32) {
    let columns = tileset.atlas_columns();
    (tileset.atlas(columns), columns)
}

/// Writes a tileset's picture to a PNG file.
pub fn export_tileset(tileset: &Tileset, path: &Path) -> Result<(), IoError> {
    let (image, _) = tileset_image(tileset);
    write_atomically(path, |writer| encode_png(&image, writer))
}

/// A tilemap cel as CSV, the way Tiled writes it: one line per row, the number of each tile
/// counted from 0, and -1 where there is none.
pub fn tilemap_csv(grid: Option<&TileGrid>, width: u32, height: u32) -> String {
    let mut out = String::new();
    for y in 0..height {
        let row: Vec<String> = (0..width)
            .map(|x| (i64::from(grid.map_or(0, |grid| grid.get(x, y))) - 1).to_string())
            .collect();
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

/// The tilemap layers of a sprite, bottom to top, each with its tileset.
fn tilemap_layers(sprite: &Sprite) -> Vec<(&acms_core::sprite::Layer, &Tileset)> {
    sprite
        .walk()
        .iter()
        .filter_map(|entry| Some((entry.layer, sprite.tileset_of(entry.layer.id)?)))
        .collect()
}

/// One frame of a sprite's tilemap layers as a Tiled map in JSON (`.tmj`). `image_of` gives
/// the file name of a tileset's picture, relative to the map.
///
/// A Tiled map has one tile size, so every tilemap layer must use tiles of the same size.
/// Layers that are not tilemaps are not part of the map.
pub fn tiled_map(
    sprite: &Sprite,
    frame: usize,
    image_of: &dyn Fn(&Tileset) -> String,
) -> Result<String, IoError> {
    let layers = tilemap_layers(sprite);
    let Some((_, first)) = layers.first() else {
        return Err(unsupported("the sprite has no tilemap layers"));
    };
    let size = (first.tile_width(), first.tile_height());
    if layers
        .iter()
        .any(|(_, set)| (set.tile_width(), set.tile_height()) != size)
    {
        return Err(unsupported(
            "the tilemap layers use tiles of different sizes; a Tiled map has one tile size",
        ));
    }
    if frame >= sprite.frames().len() {
        return Err(unsupported(format!("there is no frame {}", frame + 1)));
    }
    let (width, height) = sprite.grid_size(first);

    // The tilesets in use, each with the global number of its first tile.
    let mut first_gids: Vec<(&Tileset, u32)> = Vec::new();
    let mut next = 1;
    for set in sprite.tilesets() {
        let used = layers.iter().any(|(_, used)| used.id == set.id);
        if used && !set.is_empty() {
            first_gids.push((set, next));
            next += set.len() as u32;
        }
    }
    let gid = |set: &Tileset, tile: u32| {
        let first = first_gids.iter().find(|(other, _)| other.id == set.id);
        match (tile, first) {
            (0, _) | (_, None) => 0,
            (tile, Some((_, first))) => first + tile - 1,
        }
    };

    let layer_values: Vec<serde_json::Value> = layers
        .iter()
        .zip(1..)
        .map(|((layer, set), id)| {
            let grid = sprite.grid(layer.id, frame);
            let data: Vec<u32> = (0..height)
                .flat_map(|y| (0..width).map(move |x| (x, y)))
                .map(|(x, y)| gid(set, grid.map_or(0, |grid| grid.get(x, y))))
                .collect();
            let opacity = layer.as_image().map_or(255, |image| image.opacity);
            json!({
                "id": id,
                "name": layer.name,
                "type": "tilelayer",
                "width": width,
                "height": height,
                "x": 0,
                "y": 0,
                "opacity": f64::from(opacity) / 255.0,
                "visible": layer.visible,
                "data": data,
            })
        })
        .collect();
    let tileset_values: Vec<serde_json::Value> = first_gids
        .iter()
        .map(|(set, first)| {
            let (image, columns) = tileset_image(set);
            json!({
                "firstgid": first,
                "name": set.name,
                "image": image_of(set),
                "imagewidth": image.width(),
                "imageheight": image.height(),
                "tilewidth": set.tile_width(),
                "tileheight": set.tile_height(),
                "tilecount": set.len(),
                "columns": columns,
                "margin": 0,
                "spacing": 0,
            })
        })
        .collect();

    let map = json!({
        "type": "map",
        "version": "1.10",
        "tiledversion": "1.10.2",
        "orientation": "orthogonal",
        "renderorder": "right-down",
        "infinite": false,
        "compressionlevel": -1,
        "width": width,
        "height": height,
        "tilewidth": size.0,
        "tileheight": size.1,
        "nextlayerid": layer_values.len() + 1,
        "nextobjectid": 1,
        "layers": layer_values,
        "tilesets": tileset_values,
    });
    Ok(serde_json::to_string_pretty(&map)?)
}

/// Writes one frame's tilemap as a Tiled map to `path`, and next to it the picture of each
/// tileset it uses, named `<map>_<tileset>.png`. Returns every file written, the map first.
pub fn export_tiled(sprite: &Sprite, frame: usize, path: &Path) -> Result<Vec<PathBuf>, IoError> {
    let stem = stem(path);
    let image_name = |set: &Tileset| format!("{stem}_{}_{}.png", file_part(&set.name), set.id.0);
    let map = tiled_map(sprite, frame, &image_name)?;
    write_atomically(path, |writer| Ok(writer.write_all(map.as_bytes())?))?;

    let mut written = vec![path.to_owned()];
    let layers = tilemap_layers(sprite);
    for set in sprite.tilesets() {
        let used = layers.iter().any(|(_, used)| used.id == set.id);
        if used && !set.is_empty() {
            let file = path.with_file_name(image_name(set));
            export_tileset(set, &file)?;
            written.push(file);
        }
    }
    Ok(written)
}

/// Writes one frame's tilemap layers as CSV: the only layer to `path` itself, or with
/// several layers one file each, named `<file>_<layer>.csv`. Returns the files written.
pub fn export_csv(sprite: &Sprite, frame: usize, path: &Path) -> Result<Vec<PathBuf>, IoError> {
    let layers = tilemap_layers(sprite);
    if layers.is_empty() {
        return Err(unsupported("the sprite has no tilemap layers"));
    }
    if frame >= sprite.frames().len() {
        return Err(unsupported(format!("there is no frame {}", frame + 1)));
    }
    let stem = stem(path);
    let mut written = Vec::new();
    for (layer, set) in &layers {
        let (width, height) = sprite.grid_size(set);
        let csv = tilemap_csv(sprite.grid(layer.id, frame), width, height);
        let file = if layers.len() == 1 {
            path.to_owned()
        } else {
            path.with_file_name(format!(
                "{stem}_{}_{}.csv",
                file_part(&layer.name),
                layer.id.0
            ))
        };
        write_atomically(&file, |writer| Ok(writer.write_all(csv.as_bytes())?))?;
        written.push(file);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::png::decode_png;
    use crate::test_util::{TempPath, drawn_sprite, tiled_sprite};
    use acms_core::geom::Rect;
    use acms_core::sprite::LayerId;

    #[test]
    fn a_tileset_is_exported_as_a_picture_of_its_tiles() {
        let sprite = tiled_sprite();
        let ground = &sprite.tilesets()[0];
        assert_eq!(ground.len(), 3);
        let (image, columns) = tileset_image(ground);
        assert_eq!(columns, 2);
        assert_eq!((image.width(), image.height()), (8, 8));
        for number in 1..=3_u32 {
            let at = Rect::new(
                ((number - 1) % 2 * 4) as i32,
                ((number - 1) / 2 * 4) as i32,
                4,
                4,
            );
            assert_eq!(image.crop(at), *ground.tile(number).unwrap());
        }
        let target = TempPath::new("tileset.png");
        export_tileset(ground, target.path()).unwrap();
        let written = decode_png(&std::fs::read(target.path()).unwrap()).unwrap();
        assert_eq!(written, image);
    }

    #[test]
    fn a_tilemap_is_written_as_csv() {
        let sprite = tiled_sprite();
        let ground = sprite.grid(LayerId(2), 0);
        assert_eq!(
            tilemap_csv(ground, 4, 3),
            "0,1,1,0\n-1,-1,2,-1\n-1,-1,-1,-1\n"
        );
        assert_eq!(tilemap_csv(None, 2, 1), "-1,-1\n");
    }

    #[test]
    fn the_tiled_map_has_what_tiled_needs() {
        let sprite = tiled_sprite();
        let text = tiled_map(&sprite, 0, &|set| format!("{}.png", set.name)).unwrap();
        let map: serde_json::Value = serde_json::from_str(&text).unwrap();
        // The fields Tiled's JSON map format requires.
        assert_eq!(map["type"], "map");
        assert_eq!(map["orientation"], "orthogonal");
        assert_eq!(map["renderorder"], "right-down");
        assert_eq!(map["infinite"], false);
        assert!(map["version"].is_string() && map["tiledversion"].is_string());
        assert_eq!((&map["width"], &map["height"]), (&json!(4), &json!(3)));
        assert_eq!(
            (&map["tilewidth"], &map["tileheight"]),
            (&json!(4), &json!(4))
        );
        assert_eq!(map["nextlayerid"], 3);
        assert_eq!(map["nextobjectid"], 1);

        // Two tilemap layers, bottom first; the plain image layer is not part of the map.
        let layers = map["layers"].as_array().unwrap();
        assert_eq!(layers.len(), 2);
        let ground = &layers[0];
        assert_eq!(ground["type"], "tilelayer");
        assert_eq!(ground["name"], "Ground");
        assert_eq!(
            (&ground["width"], &ground["height"]),
            (&json!(4), &json!(3))
        );
        assert_eq!((&ground["x"], &ground["y"]), (&json!(0), &json!(0)));
        assert_eq!(ground["opacity"], 1.0);
        assert_eq!(ground["visible"], true);
        // Global tile numbers: 0 is empty, the first tileset starts at 1.
        assert_eq!(ground["data"], json!([1, 2, 2, 1, 0, 0, 3, 0, 0, 0, 0, 0]));
        // The second layer uses the second tileset, whose tiles follow the first's three.
        let props = &layers[1];
        assert_eq!(props["name"], "Props");
        assert_eq!(props["data"], json!([0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0]));
        assert!((props["opacity"].as_f64().unwrap() - 0.5).abs() < 0.01);

        let tilesets = map["tilesets"].as_array().unwrap();
        assert_eq!(tilesets.len(), 2);
        assert_eq!(
            tilesets[0],
            json!({
                "firstgid": 1, "name": "Ground", "image": "Ground.png",
                "imagewidth": 8, "imageheight": 8, "tilewidth": 4, "tileheight": 4,
                "tilecount": 3, "columns": 2, "margin": 0, "spacing": 0
            })
        );
        assert_eq!(tilesets[1]["firstgid"], 4);
        assert_eq!(tilesets[1]["tilecount"], 1);
        // Every number in the data points at a tile that exists.
        let last = 4;
        for layer in layers {
            for gid in layer["data"].as_array().unwrap() {
                assert!(gid.as_u64().unwrap() <= last);
            }
        }
    }

    #[test]
    fn a_map_and_its_tilesets_are_written_side_by_side() {
        let sprite = tiled_sprite();
        let target = TempPath::new("level.tmj");
        let files = export_tiled(&sprite, 0, target.path()).unwrap();
        assert_eq!(files.len(), 3);
        let map: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&files[0]).unwrap()).unwrap();
        // Each tileset's picture is where the map says, and is that tileset.
        for (index, file) in files[1..].iter().enumerate() {
            let named = map["tilesets"][index]["image"].as_str().unwrap();
            assert_eq!(file.file_name().unwrap().to_string_lossy(), named);
            let image = decode_png(&std::fs::read(file).unwrap()).unwrap();
            assert_eq!(image, tileset_image(&sprite.tilesets()[index]).0);
            std::fs::remove_file(file).unwrap();
        }

        let csv = TempPath::new("level.csv");
        let files = export_csv(&sprite, 0, csv.path()).unwrap();
        assert_eq!(files.len(), 2);
        assert!(!csv.path().exists());
        let ground = std::fs::read_to_string(&files[0]).unwrap();
        assert!(ground.starts_with("0,1,1,0\n"));
        for file in files {
            std::fs::remove_file(file).unwrap();
        }
    }

    #[test]
    fn maps_that_cannot_be_written_are_refused() {
        let plain = drawn_sprite();
        let target = TempPath::new("none.tmj");
        assert!(matches!(
            export_tiled(&plain, 0, target.path()),
            Err(IoError::Unsupported(_))
        ));
        assert!(matches!(
            export_csv(&plain, 0, target.path()),
            Err(IoError::Unsupported(_))
        ));
        assert!(!target.path().exists());
        let sprite = tiled_sprite();
        assert!(matches!(
            tiled_map(&sprite, 5, &|_| String::new()),
            Err(IoError::Unsupported(_))
        ));
        assert_eq!(file_part("My tiles/1"), "My_tiles_1");
        assert_eq!(file_part(""), "unnamed");
    }
}
