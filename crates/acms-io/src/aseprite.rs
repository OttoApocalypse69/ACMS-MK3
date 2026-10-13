//! Reading Aseprite files (`.aseprite`, `.ase`).
//!
//! The file is parsed by the `aseprite-loader` crate; this module turns what it finds into a
//! [`Sprite`]. Whatever ACMS has no place for is left out and named in the notes that come
//! back with the sprite, so that nothing goes missing quietly.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use acms_core::color::Rgba8;
use acms_core::geom::{Point, Rect};
use acms_core::image::Image;
use acms_core::palette::MAX_PALETTE_COLORS;
use acms_core::sprite::{
    BlendMode, CelSource, Direction, Frame, ImageLayer, Layer, LayerId, LayerKind, MAX_FRAME_MS,
    MAX_SPRITE_SIZE, Palette, Sprite, Tag,
};
use acms_core::tiles::Slice;
use aseprite_loader::binary::blend_mode::BlendMode as AseBlend;
use aseprite_loader::binary::chunks::cel::CelContent;
use aseprite_loader::binary::chunks::layer::{LayerChunk, LayerFlags, LayerType};
use aseprite_loader::binary::chunks::tags::AnimationDirection;
use aseprite_loader::binary::color_depth::ColorDepth;
use aseprite_loader::binary::file::{File, parse_file};
use aseprite_loader::binary::image::Image as AseImage;
use aseprite_loader::loader::decompress;

use crate::IoError;

/// The colour slices are given; Aseprite's own default.
const SLICE_COLOR: Rgba8 = Rgba8::rgb(0, 0, 255);

/// A sprite read from another program's file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Imported {
    pub sprite: Sprite,
    /// What was left out or changed on the way in, one sentence each.
    pub notes: Vec<String>,
}

fn damaged(why: impl std::fmt::Display) -> IoError {
    IoError::Invalid(why.to_string())
}

/// Whether a file name looks like an Aseprite file.
pub fn is_aseprite_path(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("aseprite") || extension.eq_ignore_ascii_case("ase")
    })
}

/// The pixels of one cel as RGBA, whatever the colour mode of the file.
fn decode(file: &File<'_>, image: &AseImage<'_>, background: bool) -> Result<Image, IoError> {
    let (width, height) = (u32::from(image.width), u32::from(image.height));
    // A cel may reach past the canvas, but not by much in any real file.
    if width > 2 * MAX_SPRITE_SIZE || height > 2 * MAX_SPRITE_SIZE {
        return Err(damaged(format!("a cel of {width}×{height} pixels")));
    }
    let count = width as usize * height as usize;
    let depth = file.header.color_depth;
    let bytes_per_pixel = match depth {
        ColorDepth::Rgba => 4,
        ColorDepth::Grayscale => 2,
        ColorDepth::Indexed => 1,
        ColorDepth::Unknown(bits) => {
            return Err(IoError::Unsupported(format!(
                "Aseprite files with {bits} bits per pixel cannot be read"
            )));
        }
    };
    let raw: Cow<'_, [u8]> = if image.compressed {
        let mut buffer = vec![0; count * bytes_per_pixel];
        decompress(image.data, &mut buffer)
            .map_err(|_| damaged("the pixels of a cel could not be unpacked"))?;
        Cow::Owned(buffer)
    } else {
        Cow::Borrowed(image.data)
    };
    if raw.len() < count * bytes_per_pixel {
        return Err(damaged("a cel has fewer pixels than its size says"));
    }

    let pixels = raw[..count * bytes_per_pixel]
        .chunks_exact(bytes_per_pixel)
        .map(|pixel| match depth {
            ColorDepth::Grayscale => Rgba8::new(pixel[0], pixel[0], pixel[0], pixel[1]),
            ColorDepth::Indexed => {
                // One index means "nothing here", except on the background layer.
                if pixel[0] == file.header.transparent_index && !background {
                    return Rgba8::TRANSPARENT;
                }
                match &file.palette {
                    Some(palette) => {
                        let color = palette.colors[usize::from(pixel[0])];
                        Rgba8::new(color.red, color.green, color.blue, color.alpha)
                    }
                    None => Rgba8::TRANSPARENT,
                }
            }
            _ => Rgba8::new(pixel[0], pixel[1], pixel[2], pixel[3]),
        })
        .map(Rgba8::normalized)
        .collect();
    Image::from_pixels(width, height, pixels).ok_or_else(|| damaged("a cel has no size"))
}

/// What became of each layer of the file, by its position in the file.
#[derive(Clone, Copy)]
enum Fate {
    Image { id: LayerId, background: bool },
    Group,
    LeftOut,
}

struct LayerBuilder<'a, 'f> {
    chunks: &'a [LayerChunk<'f>],
    /// Layer opacity is only stored by newer versions of Aseprite.
    opacity_stored: bool,
    next: usize,
    fates: Vec<Fate>,
    notes: Vec<String>,
}

impl LayerBuilder<'_, '_> {
    /// The layers at nesting depth `level` from the current position on: the file lists the
    /// tree bottom to top, each group followed by what is in it one level deeper.
    fn level(&mut self, level: u16) -> Vec<Layer> {
        let mut layers = Vec::new();
        while let Some(chunk) = self.chunks.get(self.next) {
            if chunk.child_level < level {
                break;
            }
            let index = self.next;
            self.next += 1;
            let id = LayerId(index as u32 + 1);
            let name = if chunk.name.is_empty() {
                format!("Layer {}", index + 1)
            } else {
                chunk.name.to_owned()
            };
            let visible = chunk.flags.contains(LayerFlags::VISIBLE);
            match chunk.layer_type {
                LayerType::Group => {
                    let children = self.level(chunk.child_level.saturating_add(1));
                    let mut group = Layer::group(id, name, children);
                    group.visible = visible;
                    self.fates[index] = Fate::Group;
                    layers.push(group);
                }
                LayerType::Normal => {
                    let blend = match chunk.blend_mode {
                        AseBlend::Normal => BlendMode::Normal,
                        AseBlend::Multiply => BlendMode::Multiply,
                        AseBlend::Screen => BlendMode::Screen,
                        AseBlend::Overlay => BlendMode::Overlay,
                        AseBlend::Addition => BlendMode::Add,
                        other => {
                            self.notes.push(format!(
                                "Layer \"{name}\" used the blend mode {other:?}, which ACMS does \
                                 not have; it is Normal now."
                            ));
                            BlendMode::Normal
                        }
                    };
                    let mut layer = Layer::image(id, name);
                    layer.visible = visible;
                    layer.kind = LayerKind::Image(ImageLayer {
                        opacity: if self.opacity_stored {
                            chunk.opacity
                        } else {
                            255
                        },
                        blend,
                        locked: !chunk.flags.contains(LayerFlags::EDITABLE),
                        alpha_lock: false,
                    });
                    self.fates[index] = Fate::Image {
                        id,
                        background: chunk.flags.contains(LayerFlags::BACKGROUND),
                    };
                    layers.push(layer);
                }
                LayerType::Tilemap | LayerType::Unknown(_) => {
                    self.notes.push(format!(
                        "Layer \"{name}\" is a tilemap layer and was left out: Aseprite's \
                         tilemaps cannot be read yet."
                    ));
                }
            }
        }
        layers
    }
}

/// The colours a sprite uses, in the order they first appear, for a file that brings no
/// palette of its own.
fn palette_of(cels: &[CelSource]) -> Palette {
    let mut seen = HashSet::new();
    let mut colors = Vec::new();
    'cels: for cel in cels {
        for pixel in cel.image.pixels().iter().filter(|pixel| pixel.a != 0) {
            let color = Rgba8::rgb(pixel.r, pixel.g, pixel.b);
            if seen.insert(color) {
                if colors.len() == MAX_PALETTE_COLORS {
                    break 'cels;
                }
                colors.push(color);
            }
        }
    }
    if colors.is_empty() {
        Palette::default_palette()
    } else {
        Palette::from_colors(colors)
    }
}

/// Reads an Aseprite file.
///
/// Image layers, groups, frames with their durations, cels (linked ones stay linked), tags
/// and slices come across. Indexed and greyscale files become RGBA. What does not come
/// across is named in [`Imported::notes`].
pub fn import_aseprite(bytes: &[u8]) -> Result<Imported, IoError> {
    // The parser is somebody else's code reading somebody else's file: a mistake in either
    // must not take the app down.
    let parsed = std::panic::catch_unwind(|| parse_file(bytes).map_err(|err| err.to_string()));
    let file = match parsed {
        Ok(Ok(file)) => file,
        Ok(Err(why)) => return Err(damaged(format!("not a readable Aseprite file ({why})"))),
        Err(_) => return Err(damaged("not a readable Aseprite file")),
    };

    let (width, height) = (u32::from(file.header.width), u32::from(file.header.height));
    let mut builder = LayerBuilder {
        chunks: &file.layers,
        opacity_stored: file.header.flags & 1 != 0,
        next: 0,
        fates: vec![Fate::LeftOut; file.layers.len()],
        notes: Vec::new(),
    };
    let mut layers = builder.level(0);
    let (fates, mut notes) = (builder.fates, builder.notes);
    if layers.is_empty() {
        layers.push(Layer::image(
            LayerId(file.layers.len() as u32 + 1),
            "Layer 1",
        ));
    }
    match file.header.color_depth {
        ColorDepth::Indexed => {
            notes.push("The file used indexed colours; the pixels are RGBA now.".to_owned());
        }
        ColorDepth::Grayscale => {
            notes.push("The file was greyscale; the pixels are RGBA now.".to_owned());
        }
        _ => {}
    }

    let frames: Vec<Frame> = file
        .frames
        .iter()
        .map(|frame| Frame {
            duration_ms: u32::from(frame.duration).clamp(1, MAX_FRAME_MS),
        })
        .collect();
    if frames.is_empty() {
        return Err(damaged("the file has no frames"));
    }

    let mut cels = Vec::new();
    // For every cel that is a link: the frame whose picture it shows.
    let mut sources: BTreeMap<(LayerId, usize), usize> = BTreeMap::new();
    for (frame_index, frame) in file.frames.iter().enumerate() {
        for (layer_index, cel) in frame.cels.iter().enumerate() {
            let (Some(cel), Some(Fate::Image { id, background })) =
                (cel, fates.get(layer_index).copied())
            else {
                continue;
            };
            match &cel.content {
                CelContent::Image(image) => {
                    let mut picture = decode(&file, image, background)?;
                    if cel.opacity != 255 {
                        let opacity = u32::from(cel.opacity);
                        let faded = picture
                            .pixels()
                            .iter()
                            .map(|pixel| {
                                let alpha = (u32::from(pixel.a) * opacity + 127) / 255;
                                Rgba8::new(pixel.r, pixel.g, pixel.b, alpha as u8).normalized()
                            })
                            .collect();
                        picture = Image::from_pixels(picture.width(), picture.height(), faded)
                            .unwrap_or(picture);
                    }
                    cels.push(CelSource {
                        layer: id,
                        frame: frame_index,
                        origin: Point::new(i32::from(cel.x), i32::from(cel.y)),
                        image: picture,
                    });
                }
                CelContent::LinkedCel { frame_position } => {
                    let linked_to = usize::from(*frame_position);
                    // A link to a link shows what that one shows.
                    let source = sources.get(&(id, linked_to)).copied().unwrap_or(linked_to);
                    if source < frame_index {
                        sources.insert((id, frame_index), source);
                    }
                }
                _ => {}
            }
        }
    }
    let mut links: BTreeMap<(LayerId, usize), Vec<usize>> = BTreeMap::new();
    for ((layer, frame), source) in sources {
        links
            .entry((layer, source))
            .or_insert_with(|| vec![source])
            .push(frame);
    }

    let palette = match &file.palette {
        Some(palette) => {
            let count = usize::from(file.header.color_count).clamp(1, MAX_PALETTE_COLORS);
            Palette::from_colors(palette.colors[..count].iter().map(|color| {
                // The palette holds colours to draw with: the "nothing here" entry is listed
                // as the colour it would be.
                Rgba8::rgb(color.red, color.green, color.blue)
            }))
        }
        None => palette_of(&cels),
    };

    let mut tags = Vec::new();
    for tag in &file.tags {
        let last = frames.len() - 1;
        let from = usize::from(*tag.frames.start()).min(last);
        let to = usize::from(*tag.frames.end()).min(last);
        if from > to {
            continue;
        }
        let name = if tag.name.is_empty() {
            format!("Tag {}", tags.len() + 1)
        } else {
            tag.name.to_owned()
        };
        let direction = match tag.animation_direction {
            AnimationDirection::Forward => Direction::Forward,
            AnimationDirection::Reverse => Direction::Reverse,
            AnimationDirection::PingPong => Direction::PingPong,
            AnimationDirection::PingPongReverse => {
                notes.push(format!(
                    "Tag \"{name}\" played ping-pong starting backwards; it starts forwards now."
                ));
                Direction::PingPong
            }
            AnimationDirection::Unknown(_) => Direction::Forward,
        };
        #[allow(deprecated)] // Newer files keep the colour elsewhere; this is the one field
        // the parser gives us.
        let [r, g, b] = tag.color;
        tags.push(Tag {
            name,
            from,
            to,
            direction,
            color: Rgba8::rgb(r, g, b),
        });
    }

    let mut slices = Vec::new();
    for slice in &file.slices {
        let Some(key) = slice.slice_keys.first() else {
            continue;
        };
        let name = if slice.name.is_empty() {
            format!("Slice {}", slices.len() + 1)
        } else {
            slice.name.to_owned()
        };
        if key.width == 0 || key.height == 0 {
            continue;
        }
        if slice.slice_keys.len() > 1 {
            notes.push(format!(
                "Slice \"{name}\" changed from frame to frame; it is as in its first frame now."
            ));
        }
        slices.push(Slice {
            name,
            rect: Rect::new(key.x, key.y, key.width, key.height),
            center: key
                .nine_patch
                .filter(|center| center.width > 0 && center.height > 0)
                .map(|center| Rect::new(center.x, center.y, center.width, center.height)),
            pivot: key.pivot.map(|pivot| Point::new(pivot.x, pivot.y)),
            color: SLICE_COLOR,
        });
    }

    let sprite = Sprite::from_parts(width, height, palette, layers, frames, cels)?
        .with_links(
            links
                .into_iter()
                .map(|((layer, _), frames)| (layer, frames))
                .collect(),
        )
        .with_tags(tags)
        .with_slices(slices);
    Ok(Imported { sprite, notes })
}

/// [`import_aseprite`] from a file.
pub fn import_aseprite_from_path(path: &Path) -> Result<Imported, IoError> {
    import_aseprite(&std::fs::read(path)?)
}

#[cfg(test)]
mod tests {
    use acms_core::compositor;
    use aseprite_loader::loader::{AsepriteFile, LayerSelection};

    use super::*;

    /// Files saved by Aseprite itself; see `tests/samples/aseprite/README.md`.
    const SAMPLES: [&str; 7] = [
        "combine",
        "default",
        "indexed",
        "issue_4_1",
        "issue_4_2",
        "layers",
        "tags",
    ];

    fn sample(name: &str) -> Vec<u8> {
        let path = format!("../../tests/samples/aseprite/{name}.aseprite");
        std::fs::read(path).unwrap()
    }

    #[test]
    fn real_files_look_the_same_as_the_parsers_own_rendering() {
        for name in SAMPLES {
            let bytes = sample(name);
            let imported = import_aseprite(&bytes).unwrap_or_else(|err| panic!("{name}: {err}"));
            let sprite = &imported.sprite;
            // A layer whose blend mode was replaced cannot look the same.
            if imported
                .notes
                .iter()
                .any(|note| note.contains("blend mode"))
            {
                continue;
            }
            let theirs = AsepriteFile::load(&bytes).unwrap();
            assert_eq!(sprite.frames().len(), theirs.frames().len(), "{name}");
            for frame in 0..sprite.frames().len() {
                let mut expected = vec![0; sprite.bounds().area() * 4];
                theirs
                    .render_frame(frame, &mut expected, &LayerSelection::Visible)
                    .unwrap();
                let expected =
                    Image::from_rgba_bytes(sprite.width(), sprite.height(), &expected).unwrap();
                let ours = compositor::compose(sprite, frame);
                for (index, (a, b)) in ours.pixels().iter().zip(expected.pixels()).enumerate() {
                    assert_eq!(*a, b.normalized(), "{name}, frame {frame}, pixel {index}");
                }
            }
        }
    }

    #[test]
    fn what_real_files_hold_comes_across() {
        let layers = import_aseprite(&sample("layers")).unwrap().sprite;
        assert_eq!((layers.width(), layers.height()), (32, 32));
        let names: Vec<&str> = layers
            .walk()
            .iter()
            .map(|e| e.layer.name.as_str())
            .collect();
        assert_eq!(names, ["Layer 1", "Layer 2", "Layer 3"]);
        assert_eq!(layers.palette().len(), 3);

        let combine = import_aseprite(&sample("combine")).unwrap();
        let fourth = combine
            .sprite
            .layer(LayerId(4))
            .unwrap()
            .as_image()
            .unwrap();
        assert_eq!(fourth.opacity, 186);
        assert_eq!(combine.notes.len(), 1);
        assert!(
            combine.notes[0].contains("Difference"),
            "{:?}",
            combine.notes
        );

        let animation = import_aseprite(&sample("issue_4_2")).unwrap();
        assert_eq!(animation.sprite.frames().len(), 6);
        assert!(
            animation
                .sprite
                .frames()
                .iter()
                .all(|f| f.duration_ms == 100)
        );
        let tag = &animation.sprite.tags()[0];
        assert_eq!((tag.name.as_str(), tag.from, tag.to), ("idle", 0, 5));
        assert!(animation.notes[0].contains("indexed"));
        // The palette of an indexed file is the file's own.
        assert_eq!(animation.sprite.palette().len(), 32);

        let tags = import_aseprite(&sample("tags")).unwrap().sprite;
        let names: Vec<&str> = tags.tags().iter().map(|tag| tag.name.as_str()).collect();
        assert_eq!(names, ["Tag 1", "Tag 2", "Tag 3"]);
    }

    #[test]
    fn a_file_that_is_not_one_is_an_error_not_a_crash() {
        assert!(import_aseprite(b"").is_err());
        assert!(import_aseprite(b"definitely not a sprite").is_err());
        assert!(import_aseprite(&[0xFF; 4096]).is_err());
        // Every real file, cut short anywhere and with bytes changed everywhere.
        for name in SAMPLES {
            let bytes = sample(name);
            for cut in (0..bytes.len()).step_by(7) {
                let _ = import_aseprite(&bytes[..cut]);
            }
            for at in (0..bytes.len()).step_by(3) {
                let mut broken = bytes.clone();
                broken[at] ^= 0xFF;
                let _ = import_aseprite(&broken);
            }
        }
        let missing = Path::new("../../tests/samples/aseprite/none.aseprite");
        assert!(matches!(
            import_aseprite_from_path(missing),
            Err(IoError::Io(_))
        ));
    }

    #[test]
    fn file_names_are_recognised() {
        assert!(is_aseprite_path(Path::new("hero.aseprite")));
        assert!(is_aseprite_path(Path::new("C:/art/HERO.ASE")));
        assert!(!is_aseprite_path(Path::new("hero.acms")));
        assert!(!is_aseprite_path(Path::new("aseprite")));
    }

    /// Writes the parts of an Aseprite file the samples do not have, after the format's
    /// specification (`docs/ase-file-specs.md` in Aseprite's repository).
    #[derive(Default)]
    struct AseWriter {
        frames: Vec<Vec<Vec<u8>>>,
    }

    fn text(out: &mut Vec<u8>, text: &str) {
        out.extend((text.len() as u16).to_le_bytes());
        out.extend(text.as_bytes());
    }

    fn chunk(kind: u16, data: Vec<u8>) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend((data.len() as u32 + 6).to_le_bytes());
        out.extend(kind.to_le_bytes());
        out.extend(data);
        out
    }

    impl AseWriter {
        fn frame(&mut self) -> &mut Self {
            self.frames.push(Vec::new());
            self
        }

        fn push(&mut self, kind: u16, data: Vec<u8>) -> &mut Self {
            self.frames.last_mut().unwrap().push(chunk(kind, data));
            self
        }

        /// `kind`: 0 image, 1 group, 2 tilemap. `flags`: 1 visible, 2 editable.
        fn layer(
            &mut self,
            name: &str,
            kind: u16,
            level: u16,
            flags: u16,
            blend: u16,
            opacity: u8,
        ) -> &mut Self {
            let mut data = Vec::new();
            for word in [flags, kind, level, 0, 0, blend] {
                data.extend(word.to_le_bytes());
            }
            data.push(opacity);
            data.extend([0; 3]);
            text(&mut data, name);
            if kind == 2 {
                data.extend(0_u32.to_le_bytes());
            }
            self.push(0x2004, data)
        }

        fn cel_header(layer: u16, x: i16, y: i16, opacity: u8, kind: u16) -> Vec<u8> {
            let mut data = Vec::new();
            data.extend(layer.to_le_bytes());
            data.extend(x.to_le_bytes());
            data.extend(y.to_le_bytes());
            data.push(opacity);
            data.extend(kind.to_le_bytes());
            data.extend(0_i16.to_le_bytes());
            data.extend([0; 5]);
            data
        }

        fn cel(
            &mut self,
            layer: u16,
            x: i16,
            y: i16,
            opacity: u8,
            w: u16,
            h: u16,
            color: Rgba8,
        ) -> &mut Self {
            let mut data = Self::cel_header(layer, x, y, opacity, 0);
            data.extend(w.to_le_bytes());
            data.extend(h.to_le_bytes());
            for _ in 0..u32::from(w) * u32::from(h) {
                data.extend(color.to_array());
            }
            self.push(0x2005, data)
        }

        fn linked_cel(&mut self, layer: u16, to_frame: u16) -> &mut Self {
            let mut data = Self::cel_header(layer, 0, 0, 255, 1);
            data.extend(to_frame.to_le_bytes());
            self.push(0x2005, data)
        }

        fn tags(&mut self, tags: &[(&str, u16, u16, u8)]) -> &mut Self {
            let mut data = Vec::new();
            data.extend((tags.len() as u16).to_le_bytes());
            data.extend([0; 8]);
            for (name, from, to, direction) in tags {
                data.extend(from.to_le_bytes());
                data.extend(to.to_le_bytes());
                data.push(*direction);
                data.extend(0_u16.to_le_bytes());
                data.extend([0; 6]);
                data.extend([10, 20, 30]);
                data.push(0);
                text(&mut data, name);
            }
            self.push(0x2018, data)
        }

        fn slice(
            &mut self,
            name: &str,
            rect: [i32; 4],
            center: Option<[i32; 4]>,
            pivot: Option<[i32; 2]>,
        ) -> &mut Self {
            let mut data = Vec::new();
            data.extend(1_u32.to_le_bytes());
            let flags = u32::from(center.is_some()) | u32::from(pivot.is_some()) << 1;
            data.extend(flags.to_le_bytes());
            data.extend(0_u32.to_le_bytes());
            text(&mut data, name);
            data.extend(0_u32.to_le_bytes());
            for value in rect
                .into_iter()
                .chain(center.into_iter().flatten())
                .chain(pivot.into_iter().flatten())
            {
                data.extend(value.to_le_bytes());
            }
            self.push(0x2022, data)
        }

        fn finish(&self, width: u16, height: u16) -> Vec<u8> {
            let mut body = Vec::new();
            for chunks in &self.frames {
                let size: usize = chunks.iter().map(Vec::len).sum();
                body.extend((size as u32 + 16).to_le_bytes());
                body.extend(0xF1FA_u16.to_le_bytes());
                body.extend((chunks.len() as u16).to_le_bytes());
                body.extend(120_u16.to_le_bytes());
                body.extend([0; 2]);
                body.extend((chunks.len() as u32).to_le_bytes());
                for chunk in chunks {
                    body.extend(chunk);
                }
            }
            let mut out = Vec::new();
            out.extend((body.len() as u32 + 128).to_le_bytes());
            out.extend(0xA5E0_u16.to_le_bytes());
            out.extend((self.frames.len() as u16).to_le_bytes());
            out.extend(width.to_le_bytes());
            out.extend(height.to_le_bytes());
            out.extend(32_u16.to_le_bytes());
            out.extend(1_u32.to_le_bytes());
            out.extend(100_u16.to_le_bytes());
            out.extend([0; 8]);
            out.push(0);
            out.extend([0; 3]);
            out.extend(0_u16.to_le_bytes());
            out.extend([1, 1]);
            out.extend([0; 4]);
            out.extend(16_u16.to_le_bytes());
            out.extend(16_u16.to_le_bytes());
            out.resize(128, 0);
            out.extend(body);
            out
        }
    }

    const RED: Rgba8 = Rgba8::rgb(255, 0, 0);
    const GREEN: Rgba8 = Rgba8::rgb(0, 255, 0);

    /// Bottom to top: Back; a hidden group "Body" holding Arm (multiply, locked), a group
    /// "Deep" holding Hand, and Leg; a tilemap layer; Top. Three frames.
    fn built() -> Vec<u8> {
        let mut writer = AseWriter::default();
        writer
            .frame()
            .layer("Back", 0, 0, 3, 0, 255)
            .layer("Body", 1, 0, 2, 0, 255)
            .layer("Arm", 0, 1, 1, 1, 128)
            .layer("Deep", 1, 1, 3, 0, 255)
            .layer("Hand", 0, 2, 3, 0, 255)
            .layer("Leg", 0, 1, 3, 16, 255)
            .layer("Tiles", 2, 0, 3, 0, 255)
            .layer("Top", 0, 0, 3, 5, 255)
            .cel(0, 1, 2, 255, 3, 2, RED)
            .cel(4, 0, 0, 128, 1, 1, GREEN)
            .cel(7, -2, -2, 255, 4, 4, GREEN)
            .tags(&[
                ("walk", 0, 1, 2),
                ("back", 1, 2, 1),
                ("odd", 2, 2, 3),
                ("bad", 2, 9, 0),
            ])
            .slice("button", [1, 1, 6, 5], Some([2, 1, 2, 3]), Some([3, 4]))
            .slice("plain", [0, 0, 2, 2], None, None);
        writer.frame().linked_cel(0, 0).cel(5, 0, 0, 255, 1, 1, RED);
        writer.frame().linked_cel(0, 1);
        writer.finish(8, 8)
    }

    #[test]
    fn groups_links_tags_and_slices_come_across() {
        let Imported { sprite, notes } = import_aseprite(&built()).unwrap();
        assert_eq!((sprite.width(), sprite.height()), (8, 8));

        // The tree, bottom to top, with how deep each layer sits.
        let tree: Vec<(&str, usize)> = sprite
            .walk()
            .iter()
            .map(|entry| (entry.layer.name.as_str(), entry.depth))
            .collect();
        assert_eq!(
            tree,
            [
                ("Back", 0),
                ("Body", 0),
                ("Arm", 1),
                ("Deep", 1),
                ("Hand", 2),
                ("Leg", 1),
                ("Top", 0)
            ]
        );
        let layer = |name: &str| {
            let entries = sprite.walk();
            entries
                .iter()
                .find(|e| e.layer.name == name)
                .unwrap()
                .layer
                .clone()
        };
        assert!(!layer("Body").visible && layer("Body").is_group());
        let arm = layer("Arm");
        let arm_image = arm.as_image().unwrap();
        assert_eq!(
            (arm_image.blend, arm_image.opacity, arm_image.locked),
            (BlendMode::Multiply, 128, true)
        );
        assert_eq!(layer("Leg").as_image().unwrap().blend, BlendMode::Add);
        assert!(!layer("Back").as_image().unwrap().locked);

        // Cels: where they sit, what is in them, and a cel's own opacity in its pixels.
        let back = layer("Back").id;
        assert_eq!(sprite.pixel(back, 0, 1, 2), RED);
        assert_eq!(sprite.pixel(back, 0, 3, 3), RED);
        assert_eq!(sprite.pixel(back, 0, 0, 0), Rgba8::TRANSPARENT);
        assert_eq!(
            sprite.pixel(layer("Hand").id, 0, 0, 0),
            Rgba8::new(0, 255, 0, 128)
        );
        // A cel that starts outside the canvas is clipped to it.
        assert_eq!(
            sprite.cel(layer("Top").id, 0).unwrap().rect(),
            Rect::new(0, 0, 2, 2)
        );
        assert_eq!(sprite.pixel(layer("Leg").id, 1, 0, 0), RED);
        // Linked cels, also a link to a link, are one picture in three frames.
        assert_eq!(sprite.link_groups(), [(back, vec![0, 1, 2])]);
        assert_eq!(sprite.pixel(back, 2, 1, 2), RED);
        assert!(sprite.frames().iter().all(|frame| frame.duration_ms == 120));

        let tags: Vec<_> = sprite
            .tags()
            .iter()
            .map(|tag| (tag.name.as_str(), tag.from, tag.to, tag.direction))
            .collect();
        assert_eq!(
            tags,
            [
                ("walk", 0, 1, Direction::PingPong),
                ("back", 1, 2, Direction::Reverse),
                ("odd", 2, 2, Direction::PingPong),
                // A tag reaching past the last frame is cut to fit.
                ("bad", 2, 2, Direction::Forward),
            ]
        );
        assert_eq!(sprite.tags()[0].color, Rgba8::rgb(10, 20, 30));

        let button = &sprite.slices()[0];
        assert_eq!(
            (button.name.as_str(), button.rect),
            ("button", Rect::new(1, 1, 6, 5))
        );
        assert_eq!(button.center, Some(Rect::new(2, 1, 2, 3)));
        assert_eq!(button.pivot, Some(Point::new(3, 4)));
        let plain = &sprite.slices()[1];
        assert_eq!((plain.center, plain.pivot), (None, None));

        // The palette is made of the colours in use.
        let colors: Vec<Rgba8> = sprite.palette().colors().collect();
        assert_eq!(colors, [RED, GREEN]);

        // Everything that did not come across as it was is said.
        assert_eq!(notes.len(), 3, "{notes:?}");
        assert!(
            notes
                .iter()
                .any(|note| note.contains("\"Tiles\"") && note.contains("tilemap"))
        );
        assert!(
            notes
                .iter()
                .any(|note| note.contains("\"Top\"") && note.contains("blend mode"))
        );
        assert!(
            notes
                .iter()
                .any(|note| note.contains("\"odd\"") && note.contains("ping-pong"))
        );
    }

    #[test]
    fn an_imported_sprite_can_be_saved_and_loaded() {
        let sprite = import_aseprite(&built()).unwrap().sprite;
        let mut bytes = std::io::Cursor::new(Vec::new());
        crate::acms::write_acms(&sprite, &mut bytes).unwrap();
        bytes.set_position(0);
        assert_eq!(crate::acms::read_acms(bytes).unwrap(), sprite);
    }
}
