//! ACMS IO: the `.acms` project format and import/export.

pub mod acms;
pub mod aseprite;
pub mod cmg;
pub mod export;
pub mod netpbm;
pub mod palette;
pub mod png;
pub mod tiled;

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use acms_core::sprite::SpriteError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IoError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("not a readable .acms archive ({0})")]
    Archive(#[from] zip::result::ZipError),
    #[error("the manifest is not valid ({0})")]
    Manifest(#[from] serde_json::Error),
    #[error("could not read a PNG image ({0})")]
    PngDecode(#[from] ::png::DecodingError),
    #[error("could not write a PNG image ({0})")]
    PngEncode(#[from] ::png::EncodingError),
    #[error("could not write a GIF ({0})")]
    GifEncode(#[from] gif::EncodingError),
    #[error(
        "this file uses format version {found}, but this version of ACMS only reads up to \
         version {supported}"
    )]
    TooNew { found: u64, supported: u32 },
    #[error("the file is damaged: {0}")]
    Invalid(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("the file does not describe a valid sprite: {0}")]
    Sprite(#[from] SpriteError),
}

/// Most bytes a file that is opened may have. Larger files are refused before anything is
/// read, so that a huge (or endless, like a device) file cannot use up the memory.
pub const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// Reads a whole file of at most [`MAX_FILE_BYTES`].
pub fn read_file(path: &Path) -> Result<Vec<u8>, IoError> {
    read_file_limited(path, MAX_FILE_BYTES)
}

/// Reads a whole file of at most `limit` bytes; only ordinary files are read.
pub fn read_file_limited(path: &Path, limit: u64) -> Result<Vec<u8>, IoError> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(IoError::Unsupported("this is not a file".to_owned()));
    }
    if metadata.len() > limit {
        return Err(too_large(limit));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    // The file may grow while it is read: never more than the limit is taken.
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(too_large(limit));
    }
    Ok(bytes)
}

fn too_large(limit: u64) -> IoError {
    IoError::Unsupported(format!(
        "the file is larger than {} MB, the most ACMS opens",
        limit / (1024 * 1024)
    ))
}

/// A temporary file next to `path`, made new: an existing file (or a link) of that name is
/// never written through, and two writers never share one.
fn create_temp(path: &Path) -> io::Result<(PathBuf, File)> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .map_or_else(|| "file".into(), |name| name.to_string_lossy().into_owned());
    let mut last = None;
    for _ in 0..16 {
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp = path.with_file_name(format!(".{name}.{}-{count}.tmp", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => return Ok((temp, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => last = Some(err),
            Err(err) => return Err(err),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("no temporary file could be made")))
}

/// Writes a file so that it is either complete or not there at all: the data goes to a
/// new temporary file next to `path`, which replaces `path` only once everything is written.
/// A crash halfway never leaves a half-written file under the real name, and a link at `path`
/// is replaced rather than written through.
pub fn write_atomically(
    path: &Path,
    write: impl FnOnce(&mut BufWriter<File>) -> Result<(), IoError>,
) -> Result<(), IoError> {
    let (temp, file) = create_temp(path)?;
    let result = (|| {
        let mut writer = BufWriter::new(file);
        write(&mut writer)?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        std::fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempPath;

    #[test]
    fn writing_leaves_no_temporary_file_and_replaces_the_old_one() {
        let target = TempPath::new("atomic-write.txt");
        std::fs::write(target.path(), "old").unwrap();
        write_atomically(target.path(), |file| Ok(file.write_all(b"new")?)).unwrap();
        assert_eq!(std::fs::read(target.path()).unwrap(), b"new");
        let folder = target.path().parent().unwrap();
        let name = target
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let left = std::fs::read_dir(folder)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                let other = entry.file_name().to_string_lossy().into_owned();
                other.starts_with(&format!(".{name}.")) && other.ends_with(".tmp")
            });
        assert!(!left);
    }

    #[test]
    fn a_failed_write_leaves_the_old_file_alone() {
        let target = TempPath::new("atomic-fail.txt");
        std::fs::write(target.path(), "old").unwrap();
        let result = write_atomically(target.path(), |_| {
            Err(IoError::Invalid("stopped halfway".to_owned()))
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(target.path()).unwrap(), b"old");
    }

    #[test]
    fn the_temporary_file_never_takes_the_place_of_an_existing_one() {
        let target = TempPath::new("atomic-taken.txt");
        // A file where the old temporary name was is not touched.
        let old_temp = TempPath::new("atomic-taken.txt.tmp");
        std::fs::write(old_temp.path(), "keep").unwrap();
        write_atomically(target.path(), |file| Ok(file.write_all(b"data")?)).unwrap();
        assert_eq!(std::fs::read(old_temp.path()).unwrap(), b"keep");
        assert_eq!(std::fs::read(target.path()).unwrap(), b"data");
    }

    #[test]
    fn files_larger_than_the_limit_are_refused() {
        let file = TempPath::new("read-limit.bin");
        std::fs::write(file.path(), [7_u8; 100]).unwrap();
        assert_eq!(read_file_limited(file.path(), 100).unwrap().len(), 100);
        assert!(matches!(
            read_file_limited(file.path(), 99),
            Err(IoError::Unsupported(_))
        ));
        // A folder is not a file.
        let folder = std::env::temp_dir();
        assert!(read_file(&folder).is_err());
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use std::path::{Path, PathBuf};

    use acms_core::color::Rgba8;
    use acms_core::document::Document;
    use acms_core::geom::Point;
    use acms_core::sprite::{LayerId, Sprite};
    use acms_core::stroke::{Brush, BrushShape, Ink, Stroke, StrokeBuffer};

    /// A file path in the OS temp directory that is cleaned up when dropped.
    pub struct TempPath(PathBuf);

    impl TempPath {
        /// `name` must be unique among the tests, which run in parallel.
        pub fn new(name: &str) -> Self {
            let file_name = format!("acms-io-test-{}-{name}", std::process::id());
            let path = std::env::temp_dir().join(file_name);
            let _ = std::fs::remove_file(&path);
            Self(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// A sprite that uses every layer feature: a group, each property, and a few cels.
    ///
    /// ```text
    /// Notes        hidden
    /// Details      group, collapsed
    ///     Glow     add, opacity 140, alpha lock
    ///     Line     locked, no pixels
    /// Shading      multiply, opacity 200
    /// Background
    /// ```
    pub fn layered_sprite() -> Sprite {
        use acms_core::image::Image;
        use acms_core::sprite::{
            BlendMode, CelSource, Frame, GroupLayer, ImageLayer, Layer, LayerKind, Palette,
        };

        let image = |id, name: &str, visible, style: ImageLayer| Layer {
            id: LayerId(id),
            name: name.to_owned(),
            visible,
            kind: LayerKind::Image(style),
        };
        let plain = ImageLayer::default();
        let details = Layer {
            id: LayerId(10),
            name: "Details".to_owned(),
            visible: true,
            kind: LayerKind::Group(GroupLayer {
                collapsed: true,
                children: vec![
                    image(
                        4,
                        "Line",
                        true,
                        ImageLayer {
                            locked: true,
                            ..plain
                        },
                    ),
                    image(
                        7,
                        "Glow",
                        true,
                        ImageLayer {
                            opacity: 140,
                            blend: BlendMode::Add,
                            alpha_lock: true,
                            ..plain
                        },
                    ),
                ],
            }),
        };
        let layers = vec![
            image(1, "Background", true, plain),
            image(
                2,
                "Shading",
                true,
                ImageLayer {
                    opacity: 200,
                    blend: BlendMode::Multiply,
                    ..plain
                },
            ),
            details,
            image(12, "Notes", false, plain),
        ];

        let block = |layer, x, y, w, h, color: Rgba8| CelSource {
            layer: LayerId(layer),
            frame: 0,
            origin: Point::new(x, y),
            image: Image::from_pixels(w, h, vec![color; (w * h) as usize]).unwrap(),
        };
        let cels = vec![
            block(1, 0, 0, 16, 12, Rgba8::rgb(90, 140, 200)),
            block(2, 4, 2, 8, 8, Rgba8::rgb(180, 120, 120)),
            block(7, 6, 5, 6, 4, Rgba8::new(250, 220, 90, 200)),
            block(12, 1, 1, 3, 3, Rgba8::rgb(255, 0, 255)),
        ];
        let frames = vec![Frame { duration_ms: 100 }];
        Sprite::from_parts(16, 12, Palette::default_palette(), layers, frames, cels).unwrap()
    }

    /// A multi-layer, tagged animation: four frames of 16×12.
    ///
    /// ```text
    ///            frame 0   1        2        3
    /// Body       block at x = 1, 4, 7, 10 (each its own cel)
    /// Ground     one strip, the cels of frames 0, 1 and 3 linked; frame 2 empty
    /// tags       Idle [0]   Walk [1..3] ping-pong
    /// durations  100, 80, 120, 60 ms
    /// ```
    ///
    /// Every pixel is opaque, so a GIF can hold it exactly.
    pub fn animated_sprite() -> Sprite {
        use acms_core::geom::Rect;
        use acms_core::image::Image;
        use acms_core::sprite::{Direction, Slot, SpriteError, Tag};

        let block = |w: u32, h: u32, color: Rgba8| {
            Image::from_pixels(w, h, vec![color; (w * h) as usize]).unwrap()
        };
        let mut document = Document::new(Sprite::new(16, 12).unwrap());
        let ground = LayerId(1);
        document
            .transact("Build", |tx| {
                tx.update_layer(ground, |props| props.name = "Ground".to_owned())?;
                let body = tx.add_layer(Slot::new(None, 1), "Body", false)?;
                for (frame, ms) in [100, 80, 120, 60].into_iter().enumerate() {
                    if frame > 0 {
                        tx.add_frame(frame, None)?;
                    }
                    tx.set_frame_duration(frame, ms)?;
                    let at = Rect::new(1 + 3 * frame as i32, 3, 4, 5);
                    let shade = 60 * frame as u8;
                    tx.set_pixels(body, frame, at, &block(4, 5, Rgba8::rgb(220, shade, 40)))?;
                }
                tx.link_cels(ground, 0, &[1, 3])?;
                let strip = block(16, 2, Rgba8::rgb(40, 110, 60));
                tx.set_pixels(ground, 3, Rect::new(0, 9, 16, 2), &strip)?;
                tx.add_tag(Tag {
                    name: "Idle".to_owned(),
                    from: 0,
                    to: 0,
                    direction: Direction::Forward,
                    color: Rgba8::rgb(90, 90, 200),
                })?;
                tx.add_tag(Tag {
                    name: "Walk".to_owned(),
                    from: 1,
                    to: 3,
                    direction: Direction::PingPong,
                    color: Rgba8::new(200, 90, 90, 128),
                })?;
                Ok::<_, SpriteError>(())
            })
            .unwrap();
        document.sprite().clone()
    }

    /// A sprite with tilemaps and a slice: 16×12, tiles of 4×4, so maps of 4 × 3 tiles.
    ///
    /// ```text
    /// Props      tilemap, half transparent, tileset "Props": one tile, in cell (1, 1)
    /// Ground     tilemap, tileset "Ground": tiles 1 2 2 1 / . . 3 . / . . . .
    /// Backdrop   an ordinary layer with a block on it
    /// slice      "Door": a rectangle with a 9-slice centre and a pivot
    /// ```
    pub fn tiled_sprite() -> Sprite {
        use acms_core::geom::Rect;
        use acms_core::image::Image;
        use acms_core::sprite::{Slot, SpriteError};
        use acms_core::tiles::Slice;

        let block = |w: u32, h: u32, color: Rgba8| {
            Image::from_pixels(w, h, vec![color; (w * h) as usize]).unwrap()
        };
        let mut document = Document::new(Sprite::new(16, 12).unwrap());
        let (grass, earth) = (Rgba8::rgb(60, 160, 70), Rgba8::rgb(120, 80, 50));
        document
            .transact("Build", |tx| {
                let backdrop = LayerId(1);
                tx.update_layer(backdrop, |props| props.name = "Backdrop".to_owned())?;
                tx.set_pixels(
                    backdrop,
                    0,
                    Rect::new(2, 9, 5, 3),
                    &block(5, 3, Rgba8::rgb(40, 60, 120)),
                )?;

                let ground_set = tx.add_tileset("Ground", 4, 4)?;
                let ground = tx.add_tilemap_layer(Slot::new(None, 1), "Ground", ground_set)?;
                // Drawn cell by cell, so the tiles are numbered in this order.
                for (cell, color) in [(0, grass), (1, earth), (2, earth), (3, grass)] {
                    tx.set_pixels(ground, 0, Rect::new(cell * 4, 0, 4, 4), &block(4, 4, color))?;
                }
                tx.set_pixels(ground, 0, Rect::new(8, 4, 4, 2), &block(4, 2, grass))?;

                let props_set = tx.add_tileset("Props", 4, 4)?;
                let props = tx.add_tilemap_layer(Slot::new(None, 2), "Props", props_set)?;
                tx.set_pixels(
                    props,
                    0,
                    Rect::new(5, 5, 2, 3),
                    &block(2, 3, Rgba8::rgb(230, 200, 60)),
                )?;
                tx.update_layer(props, |layer| {
                    if let Some(image) = &mut layer.image {
                        image.opacity = 128;
                    }
                })?;

                tx.add_slice(Slice {
                    name: "Door".to_owned(),
                    rect: Rect::new(4, 4, 8, 8),
                    center: Some(Rect::new(2, 2, 4, 4)),
                    pivot: Some(Point::new(4, 7)),
                    color: Rgba8::rgb(60, 130, 240),
                })?;
                tx.add_slice(Slice {
                    name: "Hitbox".to_owned(),
                    rect: Rect::new(0, 0, 3, 2),
                    center: None,
                    pivot: None,
                    color: Rgba8::new(255, 0, 0, 128),
                })?;
                Ok::<_, SpriteError>(())
            })
            .unwrap();
        document.sprite().clone()
    }

    /// A sprite with a few strokes on it, including translucent pixels.
    pub fn drawn_sprite() -> Sprite {
        let mut document = Document::new(Sprite::new(24, 16).unwrap());
        let strokes = [
            (Rgba8::rgb(200, 30, 40), 1, vec![(2, 2), (20, 4), (5, 12)]),
            (Rgba8::new(40, 90, 220, 128), 3, vec![(12, 1), (12, 14)]),
            (Rgba8::rgb(250, 250, 250), 2, vec![(22, 14)]),
        ];
        for (color, size, path) in strokes {
            let brush = Brush::new(BrushShape::Round, size);
            let mut stroke = Stroke::begin(
                document.sprite(),
                LayerId(1),
                0,
                brush,
                Ink::Simple(color),
                StrokeBuffer::new(),
            )
            .unwrap();
            for (x, y) in path {
                stroke.extend_to(document.sprite(), Point::new(x, y));
            }
            stroke.commit(&mut document, "Pencil").unwrap();
        }
        document.sprite().clone()
    }
}
