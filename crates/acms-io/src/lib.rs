//! ACMS IO: the `.acms` project format and import/export.

pub mod acms;
pub mod palette;
pub mod png;

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

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

/// Writes a file so that it is either complete or not there at all: the data goes to a
/// temporary file next to `path`, which replaces `path` only once everything is written.
/// A crash halfway never leaves a half-written file under the real name.
pub(crate) fn write_atomically(
    path: &Path,
    write: impl FnOnce(&mut BufWriter<File>) -> Result<(), IoError>,
) -> Result<(), IoError> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);

    let result = (|| {
        let mut writer = BufWriter::new(File::create(&temp)?);
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
