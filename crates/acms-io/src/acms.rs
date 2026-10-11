//! The `.acms` project file (see `PLAN.md`, section 4.7).
//!
//! A zip archive:
//!
//! ```text
//! manifest.json     format version and the structure of the sprite
//! cels/<id>.png     the pixels of each cel
//! thumb.png         the first frame, flattened, for file browsers
//! ```

use std::fs::File;
use std::io::{BufReader, Read, Seek, Write};
use std::path::Path;

use acms_core::color::Rgba8;
use acms_core::compositor;
use acms_core::geom::Point;
use acms_core::sprite::{
    BlendMode, CelSource, Frame, GroupLayer, ImageLayer, Layer, LayerId, LayerKind, Palette,
    PaletteEntry, Sprite,
};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::png::{decode_png, encode_png};
use crate::{IoError, write_atomically};

/// The version this build writes. Raise it whenever the manifest changes in a way older
/// builds cannot read, and teach [`migrate`] to upgrade the previous version.
///
/// - **1**: a flat list of image layers with an id and a name.
/// - **2**: layers have visibility, opacity, blend mode, lock and alpha lock, and can be
///   groups with children.
pub const FORMAT_VERSION: u32 = 2;

const MANIFEST: &str = "manifest.json";
const THUMBNAIL: &str = "thumb.png";

fn cel_path(image: u32) -> String {
    format!("cels/{image}.png")
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    format_version: u32,
    width: u32,
    height: u32,
    palette: Vec<PaletteEntryData>,
    /// Bottom to top.
    layers: Vec<LayerData>,
    frames: Vec<FrameData>,
    cels: Vec<CelData>,
}

#[derive(Serialize, Deserialize)]
struct PaletteEntryData {
    /// `#RRGGBB` or `#RRGGBBAA`.
    color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum LayerData {
    Image {
        id: u32,
        name: String,
        visible: bool,
        /// 0 to 255.
        opacity: u8,
        /// One of [`BlendMode::id`].
        blend: String,
        locked: bool,
        alpha_lock: bool,
    },
    Group {
        id: u32,
        name: String,
        visible: bool,
        collapsed: bool,
        /// Bottom to top.
        children: Vec<LayerData>,
    },
}

impl LayerData {
    fn from_layer(layer: &Layer) -> Self {
        let (id, name, visible) = (layer.id.0, layer.name.clone(), layer.visible);
        match &layer.kind {
            LayerKind::Image(image) => Self::Image {
                id,
                name,
                visible,
                opacity: image.opacity,
                blend: image.blend.id().to_owned(),
                locked: image.locked,
                alpha_lock: image.alpha_lock,
            },
            LayerKind::Group(group) => Self::Group {
                id,
                name,
                visible,
                collapsed: group.collapsed,
                children: group.children.iter().map(Self::from_layer).collect(),
            },
        }
    }

    fn into_layer(self) -> Result<Layer, IoError> {
        Ok(match self {
            Self::Image {
                id,
                name,
                visible,
                opacity,
                blend,
                locked,
                alpha_lock,
            } => {
                let blend = BlendMode::from_id(&blend)
                    .ok_or_else(|| IoError::Invalid(format!("`{blend}` is not a blend mode")))?;
                Layer {
                    id: LayerId(id),
                    name,
                    visible,
                    kind: LayerKind::Image(ImageLayer {
                        opacity,
                        blend,
                        locked,
                        alpha_lock,
                    }),
                }
            }
            Self::Group {
                id,
                name,
                visible,
                collapsed,
                children,
            } => {
                let children = children
                    .into_iter()
                    .map(Self::into_layer)
                    .collect::<Result<_, _>>()?;
                Layer {
                    id: LayerId(id),
                    name,
                    visible,
                    kind: LayerKind::Group(GroupLayer {
                        children,
                        collapsed,
                    }),
                }
            }
        })
    }
}

#[derive(Serialize, Deserialize)]
struct FrameData {
    duration_ms: u32,
}

#[derive(Serialize, Deserialize)]
struct CelData {
    layer: u32,
    frame: usize,
    /// Where the image's top-left pixel sits on the canvas.
    x: i32,
    y: i32,
    /// The image is stored as `cels/<image>.png`.
    image: u32,
}

/// Writes a sprite as an `.acms` archive.
pub fn write_acms<W: Write + Seek>(sprite: &Sprite, writer: W) -> Result<(), IoError> {
    // Layer by layer, frame by frame, so that the same sprite always gives the same file.
    let mut cels: Vec<_> = sprite.cels().collect();
    let painted: Vec<LayerId> = sprite.walk().iter().map(|entry| entry.layer.id).collect();
    let layer_order = |id: LayerId| painted.iter().position(|&layer| layer == id);
    cels.sort_by_key(|&(layer, frame, _)| (layer_order(layer), frame));

    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        width: sprite.width(),
        height: sprite.height(),
        palette: sprite
            .palette()
            .entries()
            .iter()
            .map(|entry| PaletteEntryData {
                color: entry.color.to_hex(),
                name: entry.name.clone(),
            })
            .collect(),
        layers: sprite.layers().iter().map(LayerData::from_layer).collect(),
        frames: sprite
            .frames()
            .iter()
            .map(|frame| FrameData {
                duration_ms: frame.duration_ms,
            })
            .collect(),
        cels: cels
            .iter()
            .zip(1..)
            .map(|(&(layer, frame, cel), image)| CelData {
                layer: layer.0,
                frame,
                x: cel.origin().x,
                y: cel.origin().y,
                image,
            })
            .collect(),
    };

    let text = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    // PNG data is already compressed.
    let image = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);

    let mut zip = ZipWriter::new(writer);
    zip.start_file(MANIFEST, text)?;
    serde_json::to_writer_pretty(&mut zip, &manifest)?;

    for (&(_, _, cel), id) in cels.iter().zip(1..) {
        zip.start_file(cel_path(id), image)?;
        encode_png(cel.image(), &mut zip)?;
    }

    zip.start_file(THUMBNAIL, image)?;
    encode_png(&compositor::compose(sprite, 0), &mut zip)?;

    zip.finish()?;
    Ok(())
}

/// Brings a manifest written by an older version up to [`FORMAT_VERSION`], one version step
/// at a time.
fn migrate(mut manifest: serde_json::Value, version: u64) -> Result<serde_json::Value, IoError> {
    if version > u64::from(FORMAT_VERSION) {
        return Err(IoError::TooNew {
            found: version,
            supported: FORMAT_VERSION,
        });
    }
    if version == 0 {
        return Err(IoError::Invalid(
            "format version 0 does not exist".to_owned(),
        ));
    }
    if version == 1 {
        upgrade_1_to_2(&mut manifest);
    }
    Ok(manifest)
}

/// Version 1 only knew plain image layers. In version 2 terms they are all visible, fully
/// opaque, normally blended and unlocked.
fn upgrade_1_to_2(manifest: &mut serde_json::Value) {
    let layers = manifest
        .get_mut("layers")
        .and_then(serde_json::Value::as_array_mut);
    for layer in layers.into_iter().flatten() {
        if let Some(layer) = layer.as_object_mut() {
            layer.insert("visible".to_owned(), true.into());
            layer.insert("opacity".to_owned(), 255.into());
            layer.insert("blend".to_owned(), BlendMode::Normal.id().into());
            layer.insert("locked".to_owned(), false.into());
            layer.insert("alpha_lock".to_owned(), false.into());
        }
    }
    manifest["format_version"] = 2.into();
}
fn read_entry<R: Read + Seek>(zip: &mut ZipArchive<R>, name: &str) -> Result<Vec<u8>, IoError> {
    let mut entry = zip
        .by_name(name)
        .map_err(|_| IoError::Invalid(format!("`{name}` is missing")))?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Reads a sprite from an `.acms` archive.
pub fn read_acms<R: Read + Seek>(reader: R) -> Result<Sprite, IoError> {
    let mut zip = ZipArchive::new(reader)?;

    let manifest: serde_json::Value = serde_json::from_slice(&read_entry(&mut zip, MANIFEST)?)?;
    let version = manifest
        .get("format_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| IoError::Invalid("the manifest has no format version".to_owned()))?;
    let manifest: Manifest = serde_json::from_value(migrate(manifest, version)?)?;

    let palette = manifest
        .palette
        .into_iter()
        .map(|entry| {
            let color = Rgba8::from_hex(&entry.color)
                .ok_or_else(|| IoError::Invalid(format!("`{}` is not a colour", entry.color)))?;
            Ok(PaletteEntry {
                color,
                name: entry.name,
            })
        })
        .collect::<Result<Vec<_>, IoError>>()?;

    let layers = manifest
        .layers
        .into_iter()
        .map(LayerData::into_layer)
        .collect::<Result<Vec<_>, IoError>>()?;

    let frames = manifest
        .frames
        .into_iter()
        .map(|frame| Frame {
            duration_ms: frame.duration_ms,
        })
        .collect();

    let cels = manifest
        .cels
        .into_iter()
        .map(|cel| {
            let bytes = read_entry(&mut zip, &cel_path(cel.image))?;
            Ok(CelSource {
                layer: LayerId(cel.layer),
                frame: cel.frame,
                origin: Point::new(cel.x, cel.y),
                image: decode_png(&bytes)?,
            })
        })
        .collect::<Result<Vec<_>, IoError>>()?;

    Ok(Sprite::from_parts(
        manifest.width,
        manifest.height,
        Palette::new(palette),
        layers,
        frames,
        cels,
    )?)
}

/// Saves a sprite to a file. An existing file is replaced only once the new one is complete.
pub fn save_acms(sprite: &Sprite, path: &Path) -> Result<(), IoError> {
    write_atomically(path, |writer| write_acms(sprite, writer))
}

/// Loads a sprite from a file.
pub fn load_acms(path: &Path) -> Result<Sprite, IoError> {
    read_acms(BufReader::new(File::open(path)?))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::test_util::{TempPath, drawn_sprite, layered_sprite};

    fn archive(sprite: &Sprite) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        write_acms(sprite, &mut bytes).unwrap();
        bytes.into_inner()
    }

    /// Builds an archive by hand, to test how damaged or foreign files are handled.
    fn archive_with(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in files {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn manifest_json(sprite: &Sprite) -> serde_json::Value {
        let mut zip = ZipArchive::new(Cursor::new(archive(sprite))).unwrap();
        serde_json::from_slice(&read_entry(&mut zip, MANIFEST).unwrap()).unwrap()
    }

    #[test]
    fn save_then_load_returns_the_same_sprite() {
        let sprite = drawn_sprite();
        let loaded = read_acms(Cursor::new(archive(&sprite))).unwrap();
        assert_eq!(loaded, sprite);
    }

    #[test]
    fn layers_groups_and_their_properties_round_trip() {
        let sprite = layered_sprite();
        let loaded = read_acms(Cursor::new(archive(&sprite))).unwrap();
        assert_eq!(loaded, sprite);
        // Spot checks, in case both sides agreed on losing something.
        let names: Vec<&str> = loaded
            .walk()
            .iter()
            .map(|entry| entry.layer.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["Background", "Shading", "Details", "Line", "Glow", "Notes"]
        );
        let glow = loaded.walk()[4].layer.as_image().copied().unwrap();
        assert_eq!((glow.opacity, glow.blend), (140, BlendMode::Add));
        assert!(glow.alpha_lock && !glow.locked);
        assert!(!loaded.walk()[5].layer.visible);
        assert!(loaded.walk()[2].layer.as_group().unwrap().collapsed);
        assert_eq!(loaded.cels().count(), 4);

        let manifest = manifest_json(&sprite);
        assert_eq!(manifest["layers"][2]["kind"], "group");
        assert_eq!(manifest["layers"][2]["children"][1]["blend"], "add");
    }

    /// The manifest of a Phase 1 file, with the given cels.
    fn version_1_manifest() -> serde_json::Value {
        serde_json::json!({
            "format_version": 1,
            "width": 8,
            "height": 8,
            "palette": [{ "color": "#FF0000" }, { "color": "#00FF0080", "name": "glass" }],
            "layers": [
                { "id": 1, "name": "Layer 1", "kind": "image" },
                { "id": 5, "name": "Ink", "kind": "image" }
            ],
            "frames": [{ "duration_ms": 80 }],
            "cels": [{ "layer": 5, "frame": 0, "x": 2, "y": 3, "image": 1 }]
        })
    }

    #[test]
    fn a_version_1_file_is_upgraded_on_load() {
        let mut cel = Vec::new();
        let mut pixels = acms_core::image::Image::new(2, 1);
        pixels.set(0, 0, Rgba8::rgb(1, 2, 3));
        pixels.set(1, 0, Rgba8::rgb(4, 5, 6));
        encode_png(&pixels, &mut cel).unwrap();
        let bytes = archive_with(&[
            (MANIFEST, version_1_manifest().to_string().as_bytes()),
            ("cels/1.png", &cel),
        ]);

        let sprite = read_acms(Cursor::new(bytes)).unwrap();
        assert_eq!(sprite.layers().len(), 2);
        for layer in sprite.layers() {
            assert!(layer.visible);
            assert_eq!(layer.as_image(), Some(&ImageLayer::default()));
        }
        assert_eq!(sprite.layers()[1].name, "Ink");
        assert_eq!(sprite.pixel(LayerId(5), 0, 3, 3), Rgba8::rgb(4, 5, 6));
        assert_eq!(sprite.frames()[0].duration_ms, 80);
        assert_eq!(sprite.palette().entries()[1].name.as_deref(), Some("glass"));

        // Saving writes the current version.
        assert_eq!(manifest_json(&sprite)["format_version"], 2);
    }

    #[test]
    fn a_file_saved_by_the_phase_1_build_still_opens() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/samples/phase1-v1.acms");
        let sprite = load_acms(&path).unwrap();
        assert_eq!((sprite.width(), sprite.height()), (32, 32));
        assert_eq!(sprite.layers().len(), 1);
        assert_eq!(sprite.layers()[0].name, "Layer 1");
        assert_eq!(sprite.layers()[0].as_image(), Some(&ImageLayer::default()));
        assert_eq!(sprite.palette().len(), 32);
        // The file holds one stroke from (3, 3) to (20, 9).
        let stroke = Rgba8::rgb(200, 30, 40);
        assert_eq!(sprite.pixel(LayerId(1), 0, 3, 3), stroke);
        assert_eq!(sprite.pixel(LayerId(1), 0, 20, 9), stroke);
        assert_eq!(sprite.pixel(LayerId(1), 0, 20, 3), Rgba8::TRANSPARENT);
    }

    #[test]
    fn an_empty_sprite_round_trips() {
        let sprite = Sprite::new(7, 3).unwrap();
        let loaded = read_acms(Cursor::new(archive(&sprite))).unwrap();
        assert_eq!(loaded, sprite);
        assert_eq!(loaded.cels().count(), 0);
    }

    #[test]
    fn the_same_sprite_always_gives_the_same_bytes() {
        let sprite = drawn_sprite();
        assert_eq!(archive(&sprite), archive(&sprite));
    }

    #[test]
    fn the_archive_has_the_documented_layout() {
        let sprite = drawn_sprite();
        let mut zip = ZipArchive::new(Cursor::new(archive(&sprite))).unwrap();
        let mut names: Vec<String> = zip
            .file_names()
            .map(|name| name.unwrap().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["cels/1.png", "manifest.json", "thumb.png"]);

        let manifest = manifest_json(&sprite);
        assert_eq!(manifest["format_version"], 2);
        assert_eq!(manifest["width"], 24);
        assert_eq!(manifest["layers"][0]["kind"], "image");
        assert_eq!(manifest["layers"][0]["opacity"], 255);
        assert_eq!(manifest["layers"][0]["blend"], "normal");
        assert_eq!(manifest["palette"][0]["color"], "#000000");

        let thumb = decode_png(&read_entry(&mut zip, THUMBNAIL).unwrap()).unwrap();
        assert_eq!(thumb, compositor::compose(&sprite, 0));
    }

    #[test]
    fn a_file_from_a_newer_version_is_refused_with_a_clear_message() {
        let mut manifest = manifest_json(&drawn_sprite());
        manifest["format_version"] = 99.into();
        manifest["something_new"] = "from the future".into();
        let bytes = archive_with(&[(MANIFEST, manifest.to_string().as_bytes())]);

        let err = read_acms(Cursor::new(bytes)).unwrap_err();
        assert!(matches!(
            err,
            IoError::TooNew {
                found: 99,
                supported: 2
            }
        ));
        assert!(err.to_string().contains("format version 99"));
    }

    #[test]
    fn damaged_files_are_errors_not_panics() {
        let sprite = drawn_sprite();
        let good = manifest_json(&sprite);
        let read = |files: &[(&str, &[u8])]| read_acms(Cursor::new(archive_with(files)));

        // Not a zip at all.
        assert!(matches!(
            read_acms(Cursor::new(b"hello".to_vec())),
            Err(IoError::Archive(_))
        ));
        // A zip without a manifest.
        assert!(matches!(
            read(&[("readme.txt", b"hi")]),
            Err(IoError::Invalid(_))
        ));
        // A manifest that is not JSON.
        assert!(matches!(
            read(&[(MANIFEST, b"{ nope")]),
            Err(IoError::Manifest(_))
        ));
        // No version, or one that never existed.
        assert!(matches!(
            read(&[(MANIFEST, b"{}")]),
            Err(IoError::Invalid(_))
        ));
        let mut zero = good.clone();
        zero["format_version"] = 0.into();
        assert!(matches!(
            read(&[(MANIFEST, zero.to_string().as_bytes())]),
            Err(IoError::Invalid(_))
        ));
        // The manifest points at a cel image that is not in the archive.
        assert!(matches!(
            read(&[(MANIFEST, good.to_string().as_bytes())]),
            Err(IoError::Invalid(_))
        ));
        // The cel image is not a PNG.
        assert!(matches!(
            read(&[
                (MANIFEST, good.to_string().as_bytes()),
                ("cels/1.png", b"not a png")
            ]),
            Err(IoError::PngDecode(_))
        ));
        // A cel on a layer that does not exist.
        let mut stray = good.clone();
        stray["cels"][0]["layer"] = 42.into();
        let cel_png = {
            let mut zip = ZipArchive::new(Cursor::new(archive(&sprite))).unwrap();
            read_entry(&mut zip, "cels/1.png").unwrap()
        };
        assert!(matches!(
            read(&[
                (MANIFEST, stray.to_string().as_bytes()),
                ("cels/1.png", &cel_png)
            ]),
            Err(IoError::Sprite(_))
        ));
        // An impossible canvas size.
        let mut huge = good.clone();
        huge["width"] = 1_000_000.into();
        assert!(matches!(
            read(&[
                (MANIFEST, huge.to_string().as_bytes()),
                ("cels/1.png", &cel_png)
            ]),
            Err(IoError::Sprite(_))
        ));
        // A blend mode this version does not know.
        let mut odd_blend = good.clone();
        odd_blend["layers"][0]["blend"] = "dissolve".into();
        assert!(matches!(
            read(&[(MANIFEST, odd_blend.to_string().as_bytes())]),
            Err(IoError::Invalid(_))
        ));
        // A layer kind this version does not know.
        let mut odd_kind = good.clone();
        odd_kind["layers"][0]["kind"] = "tilemap".into();
        assert!(matches!(
            read(&[(MANIFEST, odd_kind.to_string().as_bytes())]),
            Err(IoError::Manifest(_))
        ));
        // A palette entry that is not a colour.
        let mut bad_color = good.clone();
        bad_color["palette"][0]["color"] = "blue-ish".into();
        assert!(matches!(
            read(&[(MANIFEST, bad_color.to_string().as_bytes())]),
            Err(IoError::Invalid(_))
        ));
    }

    #[test]
    fn saving_to_a_path_and_loading_it_back() {
        let target = TempPath::new("roundtrip.acms");
        let sprite = drawn_sprite();
        save_acms(&sprite, target.path()).unwrap();
        assert_eq!(load_acms(target.path()).unwrap(), sprite);

        // Saving again over the existing file works and leaves no temporary file.
        let other = Sprite::new(3, 3).unwrap();
        save_acms(&other, target.path()).unwrap();
        assert_eq!(load_acms(target.path()).unwrap(), other);
        let mut temp = target.path().as_os_str().to_owned();
        temp.push(".tmp");
        assert!(!Path::new(&temp).exists());
    }

    #[test]
    fn loading_a_missing_file_is_an_io_error() {
        let target = TempPath::new("missing.acms");
        assert!(matches!(load_acms(target.path()), Err(IoError::Io(_))));
    }
}
