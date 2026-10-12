//! The `.acms` project file (see `PLAN.md`, section 4.7).
//!
//! A zip archive:
//!
//! ```text
//! manifest.json     format version and the structure of the sprite
//! cels/<id>.png     the pixels of each cel
//! refs/<id>.png     the picture of each reference layer
//! thumb.png         the first frame, flattened, for file browsers
//! ```

use std::fs::File;
use std::io::{BufReader, Read, Seek, Write};
use std::path::Path;

use acms_core::color::Rgba8;
use acms_core::compositor;
use acms_core::geom::{Point, Rect};
use acms_core::image::Image;
use acms_core::sprite::{
    BlendMode, CelSource, Frame, GroupLayer, ImageLayer, Layer, LayerId, LayerKind, Palette,
    PaletteEntry, ReferenceLayer, Sprite, Symmetry,
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
/// - **3**: the palette has ramps, the sprite has a symmetry setting, and there are reference
///   layers, whose pictures are stored under `refs/`.
pub const FORMAT_VERSION: u32 = 3;

const MANIFEST: &str = "manifest.json";
const THUMBNAIL: &str = "thumb.png";

fn cel_path(image: u32) -> String {
    format!("cels/{image}.png")
}

fn reference_path(image: u32) -> String {
    format!("refs/{image}.png")
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    format_version: u32,
    width: u32,
    height: u32,
    palette: Vec<PaletteEntryData>,
    /// Each ramp is a list of colours, written like the palette's.
    ramps: Vec<Vec<String>>,
    symmetry: SymmetryData,
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
struct SymmetryData {
    /// One of [`Symmetry::id`].
    mode: String,
    /// How many copies a radial symmetry makes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    count: Option<u8>,
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
    Reference {
        id: u32,
        name: String,
        visible: bool,
        /// 0 to 255.
        opacity: u8,
        /// The canvas area the picture is stretched over.
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        /// The picture is stored as `refs/<image>.png`.
        image: u32,
    },
}

impl LayerData {
    /// `pictures` collects the reference pictures met on the way; each is stored under its
    /// position in that list, counted from 1.
    fn from_layer<'a>(layer: &'a Layer, pictures: &mut Vec<&'a Image>) -> Self {
        let (id, name, visible) = (layer.id.0, layer.name.clone(), layer.visible);
        match &layer.kind {
            LayerKind::Reference(reference) => {
                pictures.push(&reference.image);
                Self::Reference {
                    id,
                    name,
                    visible,
                    opacity: reference.opacity,
                    x: reference.rect.x,
                    y: reference.rect.y,
                    width: reference.rect.w,
                    height: reference.rect.h,
                    image: pictures.len() as u32,
                }
            }
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
                children: group
                    .children
                    .iter()
                    .map(|child| Self::from_layer(child, pictures))
                    .collect(),
            },
        }
    }

    /// `picture` loads the reference picture stored under a number.
    fn into_layer(
        self,
        picture: &mut impl FnMut(u32) -> Result<Image, IoError>,
    ) -> Result<Layer, IoError> {
        Ok(match self {
            Self::Reference {
                id,
                name,
                visible,
                opacity,
                x,
                y,
                width,
                height,
                image,
            } => Layer {
                id: LayerId(id),
                name,
                visible,
                kind: LayerKind::Reference(ReferenceLayer {
                    opacity,
                    image: std::sync::Arc::new(picture(image)?),
                    rect: Rect::new(x, y, width, height),
                }),
            },
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
                    .map(|child| child.into_layer(picture))
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

    let mut pictures = Vec::new();
    let layers = sprite
        .layers()
        .iter()
        .map(|layer| LayerData::from_layer(layer, &mut pictures))
        .collect();
    let symmetry = sprite.symmetry();

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
        ramps: sprite
            .palette()
            .ramps()
            .iter()
            .map(|ramp| ramp.iter().map(|color| color.to_hex()).collect())
            .collect(),
        symmetry: SymmetryData {
            mode: symmetry.id().to_owned(),
            count: match symmetry {
                Symmetry::Radial(count) => Some(count),
                _ => None,
            },
        },
        layers,
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
    for (picture, id) in pictures.into_iter().zip(1..) {
        zip.start_file(reference_path(id), image)?;
        encode_png(picture, &mut zip)?;
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
    if version <= 2 {
        upgrade_2_to_3(&mut manifest);
    }
    Ok(manifest)
}

/// Version 2 had neither ramps nor a symmetry setting: none, and off.
fn upgrade_2_to_3(manifest: &mut serde_json::Value) {
    manifest["ramps"] = serde_json::json!([]);
    manifest["symmetry"] = serde_json::json!({ "mode": Symmetry::Off.id() });
    manifest["format_version"] = 3.into();
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

    let color = |text: &str| {
        Rgba8::from_hex(text).ok_or_else(|| IoError::Invalid(format!("`{text}` is not a colour")))
    };
    let palette = manifest
        .palette
        .into_iter()
        .map(|entry| {
            Ok(PaletteEntry {
                color: color(&entry.color)?,
                name: entry.name,
            })
        })
        .collect::<Result<Vec<_>, IoError>>()?;
    let ramps = manifest
        .ramps
        .iter()
        .map(|ramp| ramp.iter().map(|text| color(text)).collect())
        .collect::<Result<Vec<_>, IoError>>()?;
    let symmetry = Symmetry::from_id(
        &manifest.symmetry.mode,
        manifest.symmetry.count.unwrap_or(0),
    )
    .ok_or_else(|| IoError::Invalid(format!("`{}` is not a symmetry", manifest.symmetry.mode)))?;

    let mut picture = |id: u32| decode_png(&read_entry(&mut zip, &reference_path(id))?);
    let layers = manifest
        .layers
        .into_iter()
        .map(|layer| layer.into_layer(&mut picture))
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

    let sprite = Sprite::from_parts(
        manifest.width,
        manifest.height,
        Palette::new(palette).with_ramps(ramps),
        layers,
        frames,
        cels,
    )?;
    Ok(sprite.with_symmetry(symmetry))
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
        assert_eq!(manifest_json(&sprite)["format_version"], 3);
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
        assert_eq!(manifest["format_version"], 3);
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
                supported: 3
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
    /// The layered sprite with everything format version 3 added: ramps, a symmetry setting
    /// and a reference layer inside the group.
    fn phase_4_sprite() -> Sprite {
        use acms_core::document::Document;
        use acms_core::image::Image;
        use acms_core::sprite::Slot;

        let mut document = Document::new(layered_sprite());
        let (red, blue) = (Rgba8::rgb(200, 30, 40), Rgba8::new(40, 90, 220, 128));
        let mut picture = Image::new(6, 3);
        picture.set(0, 0, red);
        picture.set(5, 2, blue);
        document
            .transact("Setup", |tx| {
                let palette = Palette::from_colors([red, blue, Rgba8::WHITE])
                    .with_ramps(vec![vec![red, Rgba8::WHITE], vec![blue, red, Rgba8::WHITE]]);
                tx.set_palette(palette);
                let id =
                    tx.add_reference_layer(Slot::new(Some(LayerId(10)), 1), "Sketch", picture)?;
                tx.update_layer(id, |props| {
                    if let Some(reference) = &mut props.reference {
                        reference.opacity = 77;
                    }
                })
            })
            .unwrap();
        document.set_symmetry(Symmetry::Radial(6));
        document.sprite().clone()
    }

    #[test]
    fn ramps_symmetry_and_reference_layers_round_trip() {
        let sprite = phase_4_sprite();
        let bytes = archive(&sprite);
        let loaded = read_acms(Cursor::new(bytes.clone())).unwrap();
        assert_eq!(loaded, sprite);

        // Spot checks, in case both sides agreed on losing something.
        assert_eq!(loaded.symmetry(), Symmetry::Radial(6));
        assert_eq!(loaded.palette().ramps().len(), 2);
        assert_eq!(loaded.palette().ramps()[1].len(), 3);
        let reference = loaded.walk()[4].layer.as_reference().unwrap().clone();
        assert_eq!(reference.opacity, 77);
        assert_eq!(reference.rect, Rect::new(0, 2, 16, 8));
        assert_eq!(reference.image.get(5, 2), Rgba8::new(40, 90, 220, 128));

        let manifest = manifest_json(&sprite);
        assert_eq!(manifest["symmetry"]["mode"], "radial");
        assert_eq!(manifest["symmetry"]["count"], 6);
        assert_eq!(manifest["ramps"][0][1], "#FFFFFF");
        assert_eq!(manifest["layers"][2]["children"][1]["kind"], "reference");
        assert_eq!(manifest["layers"][2]["children"][1]["image"], 1);

        let mut zip = ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert!(zip.by_name("refs/1.png").is_ok());
        // The reference is not part of the picture, so not of the thumbnail either.
        let thumb = decode_png(&read_entry(&mut zip, THUMBNAIL).unwrap()).unwrap();
        assert_eq!(thumb, compositor::compose(&layered_sprite(), 0));
    }

    #[test]
    fn symmetry_modes_round_trip() {
        for symmetry in [
            Symmetry::Off,
            Symmetry::Horizontal,
            Symmetry::Vertical,
            Symmetry::Both,
            Symmetry::Radial(3),
        ] {
            let sprite = Sprite::new(4, 4).unwrap().with_symmetry(symmetry);
            let loaded = read_acms(Cursor::new(archive(&sprite))).unwrap();
            assert_eq!(loaded.symmetry(), symmetry);
        }
    }

    #[test]
    fn a_version_2_file_is_upgraded_on_load() {
        let sprite = layered_sprite();
        let bytes = archive(&sprite);
        // What the Phase 3 build wrote: the same manifest without the version 3 fields.
        let mut manifest = manifest_json(&sprite);
        let fields = manifest.as_object_mut().unwrap();
        fields.remove("ramps");
        fields.remove("symmetry");
        fields.insert("format_version".to_owned(), 2.into());

        let mut zip = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let cels: Vec<(String, Vec<u8>)> = (1..=4)
            .map(|id| (cel_path(id), read_entry(&mut zip, &cel_path(id)).unwrap()))
            .collect();
        let manifest = manifest.to_string();
        let mut files: Vec<(&str, &[u8])> = vec![(MANIFEST, manifest.as_bytes())];
        files.extend(cels.iter().map(|(name, bytes)| (name.as_str(), &bytes[..])));

        let loaded = read_acms(Cursor::new(archive_with(&files))).unwrap();
        assert_eq!(loaded, sprite);
        assert_eq!(loaded.symmetry(), Symmetry::Off);
        assert!(loaded.palette().ramps().is_empty());
    }

    #[test]
    fn a_file_saved_by_the_phase_3_build_still_opens() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/samples/phase3-v2.acms");
        assert_eq!(load_acms(&path).unwrap(), layered_sprite());
    }

    #[test]
    fn damaged_version_3_parts_are_errors_not_panics() {
        let sprite = phase_4_sprite();
        let good = manifest_json(&sprite);
        let mut zip = ZipArchive::new(Cursor::new(archive(&sprite))).unwrap();
        let cels: Vec<(String, Vec<u8>)> = (1..=4)
            .map(|id| (cel_path(id), read_entry(&mut zip, &cel_path(id)).unwrap()))
            .collect();
        let read = |manifest: &serde_json::Value, extra: &[(&str, &[u8])]| {
            let manifest = manifest.to_string();
            let mut files: Vec<(&str, &[u8])> = vec![(MANIFEST, manifest.as_bytes())];
            files.extend(cels.iter().map(|(name, bytes)| (name.as_str(), &bytes[..])));
            files.extend_from_slice(extra);
            read_acms(Cursor::new(archive_with(&files)))
        };

        // The reference picture is missing, or is not a picture.
        assert!(matches!(read(&good, &[]), Err(IoError::Invalid(_))));
        assert!(matches!(
            read(&good, &[("refs/1.png", b"not a png")]),
            Err(IoError::PngDecode(_))
        ));
        let picture = read_entry(&mut zip, "refs/1.png").unwrap();
        let with_picture: [(&str, &[u8]); 1] = [("refs/1.png", &picture)];
        assert!(read(&good, &with_picture).is_ok());

        let mut odd_symmetry = good.clone();
        odd_symmetry["symmetry"]["mode"] = "kaleidoscope".into();
        assert!(matches!(
            read(&odd_symmetry, &with_picture),
            Err(IoError::Invalid(_))
        ));
        let mut bad_ramp = good.clone();
        bad_ramp["ramps"][0][0] = "reddish".into();
        assert!(matches!(
            read(&bad_ramp, &with_picture),
            Err(IoError::Invalid(_))
        ));
    }
}
