//! Exporting animations: GIF, spritesheets with metadata, and PNG sequences.

use std::io::Write;
use std::path::{Path, PathBuf};

use acms_core::compositor;
use acms_core::geom::{Point, Rect};
use acms_core::image::Image;
use acms_core::sprite::Sprite;
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use crate::png::{MAX_EXPORT_SCALE, encode_png};
use crate::{IoError, write_atomically};

fn unsupported(message: impl Into<String>) -> IoError {
    IoError::Unsupported(message.into())
}

fn check_frames(sprite: &Sprite, frames: &[usize]) -> Result<(), IoError> {
    if frames.is_empty() {
        return Err(unsupported("there are no frames to export"));
    }
    match frames.iter().find(|frame| **frame >= sprite.frames().len()) {
        Some(frame) => Err(unsupported(format!("there is no frame {}", frame + 1))),
        None => Ok(()),
    }
}

fn check_scale(scale: u32) -> Result<(), IoError> {
    if (1..=MAX_EXPORT_SCALE).contains(&scale) {
        Ok(())
    } else {
        Err(unsupported(format!(
            "the scale must be between 1 and {MAX_EXPORT_SCALE}"
        )))
    }
}

/// Writes `frames` (in that order; a frame may appear more than once) as a looping GIF.
///
/// A GIF knows only "there" and "not there": translucent pixels come out opaque. Frame times
/// are rounded to hundredths of a second, and a frame with more than 256 colours is reduced
/// to 256.
pub fn export_gif<W: Write>(
    sprite: &Sprite,
    frames: &[usize],
    scale: u32,
    writer: W,
) -> Result<(), IoError> {
    check_frames(sprite, frames)?;
    check_scale(scale)?;
    let side = |pixels: u32| {
        u16::try_from(pixels * scale).map_err(|_| unsupported("the picture is too large for a GIF"))
    };
    let (width, height) = (side(sprite.width())?, side(sprite.height())?);

    let mut encoder = gif::Encoder::new(writer, width, height, &[])?;
    encoder.set_repeat(gif::Repeat::Infinite)?;
    for &index in frames {
        let image = compositor::compose(sprite, index).scaled_up(scale);
        let mut rgba = image.to_rgba_bytes();
        // Speed only matters when the colours have to be reduced; 10 is the crate's own
        // suggestion for a balance.
        let mut frame = gif::Frame::from_rgba_speed(width, height, &mut rgba, 10);
        let hundredths = (sprite.frames()[index].duration_ms + 5) / 10;
        frame.delay = hundredths.clamp(1, u32::from(u16::MAX)) as u16;
        // Each frame replaces the one before instead of being drawn over it.
        frame.dispose = gif::DisposalMethod::Background;
        encoder.write_frame(&frame)?;
    }
    Ok(())
}

/// [`export_gif`] into a file.
pub fn export_gif_to_path(
    sprite: &Sprite,
    frames: &[usize],
    scale: u32,
    path: &Path,
) -> Result<(), IoError> {
    write_atomically(path, |writer| export_gif(sprite, frames, scale, writer))
}

/// The files of a PNG sequence for a path such as `walk.png`: `walk_1.png`, `walk_2.png`, …
/// numbered by position in the sequence, with as many digits as the last one needs.
pub fn sequence_paths(path: &Path, count: usize) -> Vec<PathBuf> {
    let stem = path
        .file_stem()
        .map_or_else(|| "frame".into(), |stem| stem.to_string_lossy());
    let digits = count.to_string().len();
    (1..=count)
        .map(|number| path.with_file_name(format!("{stem}_{number:0digits$}.png")))
        .collect()
}

/// Writes `frames` as one PNG each, next to `path` and named after it (see
/// [`sequence_paths`]). Returns the files written.
pub fn export_png_sequence(
    sprite: &Sprite,
    frames: &[usize],
    scale: u32,
    path: &Path,
) -> Result<Vec<PathBuf>, IoError> {
    check_frames(sprite, frames)?;
    check_scale(scale)?;
    let paths = sequence_paths(path, frames.len());
    for (&frame, file) in frames.iter().zip(&paths) {
        let image = compositor::compose(sprite, frame).scaled_up(scale);
        write_atomically(file, |writer| encode_png(&image, writer))?;
    }
    Ok(paths)
}

/// How the frames are arranged on a spritesheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SheetLayout {
    /// Left to right, starting a new row after `per_line` frames.
    Rows,
    /// Top to bottom, starting a new column after `per_line` frames.
    Columns,
    /// Fitted together into a roughly square sheet. Worth it with `trim`.
    Packed,
}

impl SheetLayout {
    pub const ALL: [Self; 3] = [Self::Rows, Self::Columns, Self::Packed];

    /// Stable lower-case name, used in command arguments.
    pub fn id(self) -> &'static str {
        match self {
            Self::Rows => "rows",
            Self::Columns => "columns",
            Self::Packed => "packed",
        }
    }

    /// Name shown in the UI.
    pub fn name(self) -> &'static str {
        match self {
            Self::Rows => "Rows",
            Self::Columns => "Columns",
            Self::Packed => "Packed",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|layout| layout.id() == id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SheetOptions {
    pub layout: SheetLayout,
    /// Frames in a row (for [`SheetLayout::Rows`]) or in a column; 0 puts them all in one.
    pub per_line: u32,
    /// Empty pixels around every frame and along the edge of the sheet.
    pub padding: u32,
    /// Each frame is cut down to what is drawn in it.
    pub trim: bool,
}

impl Default for SheetOptions {
    fn default() -> Self {
        Self {
            layout: SheetLayout::Rows,
            per_line: 0,
            padding: 0,
            trim: false,
        }
    }
}

/// Largest side of a spritesheet.
pub const MAX_SHEET_SIZE: u32 = 16_384;

/// Where one frame ended up on a sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SheetFrame {
    /// The frame of the sprite.
    pub frame: usize,
    /// Where it is on the sheet.
    pub rect: Rect,
    /// The part of the frame that is there: all of it, or with `trim` only what is drawn.
    pub source: Rect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sheet {
    pub image: Image,
    pub frames: Vec<SheetFrame>,
}

/// Lays `frames` out on one picture.
pub fn build_sheet(
    sprite: &Sprite,
    frames: &[usize],
    options: &SheetOptions,
) -> Result<Sheet, IoError> {
    check_frames(sprite, frames)?;
    let pad = options.padding as i32;

    // The picture of each frame and the part of the canvas it shows.
    let cut: Vec<(usize, Rect, Image)> = frames
        .iter()
        .map(|&frame| {
            let flat = compositor::compose(sprite, frame);
            let source = if options.trim {
                let content = flat.content_bounds();
                // An empty frame still needs a place on the sheet: one clear pixel.
                if content.is_empty() {
                    Rect::new(0, 0, 1, 1)
                } else {
                    content
                }
            } else {
                flat.bounds()
            };
            (frame, source, flat.crop(source))
        })
        .collect();

    // Top-left corner of each picture, in the order of `cut`.
    let mut corners = vec![Point::new(0, 0); cut.len()];
    let (mut width, mut height) = (0_i32, 0_i32);
    let size = |index: usize| (cut[index].1.w as i32, cut[index].1.h as i32);
    match options.layout {
        SheetLayout::Rows | SheetLayout::Columns => {
            let across = options.layout == SheetLayout::Rows;
            let per_line = match options.per_line {
                0 => cut.len(),
                n => n as usize,
            };
            // Along a line pictures follow each other; lines are as thick as their largest.
            let (mut along, mut line_start, mut thickness) = (pad, pad, 0);
            for (index, corner) in corners.iter_mut().enumerate() {
                if index > 0 && index % per_line == 0 {
                    line_start += thickness + pad;
                    along = pad;
                    thickness = 0;
                }
                let (w, h) = size(index);
                let (length, thick) = if across { (w, h) } else { (h, w) };
                *corner = if across {
                    Point::new(along, line_start)
                } else {
                    Point::new(line_start, along)
                };
                along += length + pad;
                thickness = thickness.max(thick);
                let (far_along, far_across) = (along, line_start + thickness + pad);
                if across {
                    width = width.max(far_along);
                    height = height.max(far_across);
                } else {
                    width = width.max(far_across);
                    height = height.max(far_along);
                }
            }
        }
        SheetLayout::Packed => {
            // Shelves: tallest pictures first, each shelf as wide as a square sheet would be.
            let area: i64 = (0..cut.len())
                .map(|index| {
                    let (w, h) = size(index);
                    i64::from(w + pad) * i64::from(h + pad)
                })
                .sum();
            let widest = (0..cut.len()).map(|index| size(index).0).max().unwrap_or(1);
            let target = ((area as f64).sqrt().ceil() as i32).max(widest + 2 * pad);
            let mut order: Vec<usize> = (0..cut.len()).collect();
            order.sort_by_key(|&index| (std::cmp::Reverse(size(index).1), index));

            let (mut x, mut y, mut shelf) = (pad, pad, 0);
            for index in order {
                let (w, h) = size(index);
                if x > pad && x + w + pad > target {
                    y += shelf + pad;
                    x = pad;
                    shelf = 0;
                }
                corners[index] = Point::new(x, y);
                x += w + pad;
                shelf = shelf.max(h);
                width = width.max(x);
                height = height.max(y + shelf + pad);
            }
        }
    }

    let (width, height) = (width.max(1) as u32, height.max(1) as u32);
    if width > MAX_SHEET_SIZE || height > MAX_SHEET_SIZE {
        return Err(unsupported(format!(
            "the sheet would be {width} × {height} pixels; the limit is {MAX_SHEET_SIZE} on a \
             side. Use more rows or columns."
        )));
    }
    let mut image = Image::new(width, height);
    let mut placed = Vec::with_capacity(cut.len());
    for ((frame, source, picture), corner) in cut.iter().zip(corners) {
        image.blit(picture, corner);
        placed.push(SheetFrame {
            frame: *frame,
            rect: Rect::new(corner.x, corner.y, source.w, source.h),
            source: *source,
        });
    }
    Ok(Sheet {
        image,
        frames: placed,
    })
}

#[derive(Serialize)]
struct Area {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
}

impl From<Rect> for Area {
    fn from(rect: Rect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        }
    }
}

#[derive(Serialize)]
struct Size {
    w: u32,
    h: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FrameMeta {
    frame: Area,
    rotated: bool,
    trimmed: bool,
    sprite_source_size: Area,
    source_size: Size,
    /// Milliseconds.
    duration: u32,
}

/// The frames by name, written as a JSON object in sheet order.
struct NamedFrames(Vec<(String, FrameMeta)>);

impl Serialize for NamedFrames {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, frame) in &self.0 {
            map.serialize_entry(name, frame)?;
        }
        map.end()
    }
}

#[derive(Serialize)]
struct TagMeta {
    name: String,
    from: usize,
    to: usize,
    direction: &'static str,
    color: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LayerMeta {
    name: String,
    opacity: u8,
    blend_mode: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    app: &'static str,
    version: &'static str,
    image: String,
    format: &'static str,
    size: Size,
    scale: &'static str,
    frame_tags: Vec<TagMeta>,
    layers: Vec<LayerMeta>,
    slices: Vec<()>,
}

#[derive(Serialize)]
struct SheetMeta {
    frames: NamedFrames,
    meta: Meta,
}

/// The metadata of a sheet, in the layout Aseprite writes (its "hash" form), so importers
/// made for Aseprite sheets read it: one entry per frame, named `<name> <number>.acms`, and
/// the tags and layers under `meta`.
///
/// `image_name` is the file name of the sheet's picture. Tags are only listed when the sheet
/// holds every frame of the sprite once, in order: otherwise their frame numbers would not
/// mean the entries of this file.
pub fn sheet_json(
    sprite: &Sprite,
    sheet: &Sheet,
    name: &str,
    image_name: &str,
) -> Result<String, IoError> {
    let canvas = sprite.bounds();
    let frames = sheet
        .frames
        .iter()
        .enumerate()
        .map(|(index, placed)| {
            let meta = FrameMeta {
                frame: placed.rect.into(),
                rotated: false,
                trimmed: placed.source != canvas,
                sprite_source_size: placed.source.into(),
                source_size: Size {
                    w: canvas.w,
                    h: canvas.h,
                },
                duration: sprite.frames()[placed.frame].duration_ms,
            };
            (format!("{name} {index}.acms"), meta)
        })
        .collect();

    let whole = sheet.frames.len() == sprite.frames().len()
        && sheet.frames.iter().enumerate().all(|(i, f)| f.frame == i);
    let frame_tags = sprite
        .tags()
        .iter()
        .filter(|_| whole)
        .map(|tag| TagMeta {
            name: tag.name.clone(),
            from: tag.from,
            to: tag.to,
            direction: tag.direction.id(),
            color: tag.color.to_hex().to_lowercase() + if tag.color.a == 255 { "ff" } else { "" },
        })
        .collect();
    let layers = sprite
        .walk()
        .iter()
        .filter_map(|entry| {
            let image = entry.layer.as_image()?;
            Some(LayerMeta {
                name: entry.layer.name.clone(),
                opacity: image.opacity,
                blend_mode: image.blend.id(),
            })
        })
        .collect();

    let meta = SheetMeta {
        frames: NamedFrames(frames),
        meta: Meta {
            app: "ACMS mk3",
            version: env!("CARGO_PKG_VERSION"),
            image: image_name.to_owned(),
            format: "RGBA8888",
            size: Size {
                w: sheet.image.width(),
                h: sheet.image.height(),
            },
            scale: "1",
            frame_tags,
            layers,
            slices: Vec::new(),
        },
    };
    Ok(serde_json::to_string_pretty(&meta)?)
}

/// Writes a spritesheet to `path` and, with `with_json`, its metadata next to it under the
/// same name with the extension `.json`. Returns the sheet that was written.
pub fn export_sheet(
    sprite: &Sprite,
    frames: &[usize],
    options: &SheetOptions,
    path: &Path,
    with_json: bool,
) -> Result<Sheet, IoError> {
    let sheet = build_sheet(sprite, frames, options)?;
    write_atomically(path, |writer| encode_png(&sheet.image, writer))?;
    if with_json {
        let name = path
            .file_stem()
            .map_or_else(|| "sprite".into(), |stem| stem.to_string_lossy());
        let image_name = path
            .file_name()
            .map_or_else(|| "sheet.png".into(), |file| file.to_string_lossy());
        let json = sheet_json(sprite, &sheet, &name, &image_name)?;
        write_atomically(&path.with_extension("json"), |writer| {
            Ok(writer.write_all(json.as_bytes())?)
        })?;
    }
    Ok(sheet)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::png::decode_png;
    use crate::test_util::{TempPath, animated_sprite, drawn_sprite};
    use acms_core::color::Rgba8;
    use acms_core::playback;

    /// The frames of a GIF as full pictures, with their delays in hundredths of a second.
    fn decode_gif(bytes: &[u8]) -> Vec<(Image, u16)> {
        let mut options = gif::DecodeOptions::new();
        options.set_color_output(gif::ColorOutput::RGBA);
        let mut decoder = options.read_info(bytes).unwrap();
        let (width, height) = (u32::from(decoder.width()), u32::from(decoder.height()));
        let mut frames = Vec::new();
        while let Some(frame) = decoder.read_next_frame().unwrap() {
            // Every frame this exporter writes covers the whole picture.
            assert_eq!((frame.left, frame.top), (0, 0));
            assert_eq!(
                (u32::from(frame.width), u32::from(frame.height)),
                (width, height)
            );
            let image = Image::from_rgba_bytes(width, height, &frame.buffer).unwrap();
            let pixels = image.pixels().iter().map(|px| px.normalized()).collect();
            let image = Image::from_pixels(width, height, pixels).unwrap();
            frames.push((image, frame.delay));
        }
        frames
    }

    fn gif_of(sprite: &Sprite, frames: &[usize], scale: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        export_gif(sprite, frames, scale, &mut bytes).unwrap();
        bytes
    }

    #[test]
    fn a_gif_holds_every_frame_with_its_duration() {
        let sprite = animated_sprite();
        let all: Vec<usize> = (0..4).collect();
        let bytes = gif_of(&sprite, &all, 1);
        assert_eq!(&bytes[..6], b"GIF89a");
        // It loops for ever.
        assert!(bytes.windows(11).any(|window| window == b"NETSCAPE2.0"));

        let frames = decode_gif(&bytes);
        assert_eq!(frames.len(), 4);
        for (index, (image, delay)) in frames.iter().enumerate() {
            assert_eq!(*image, compositor::compose(&sprite, index), "frame {index}");
            assert_eq!(u32::from(*delay) * 10, sprite.frames()[index].duration_ms);
        }
        // Frame 2 has no ground: what frame 1 showed there is gone, not left behind.
        assert_eq!(frames[2].0.get(0, 9), Rgba8::TRANSPARENT);
    }

    #[test]
    fn a_gif_of_a_tag_plays_it_in_its_direction_and_can_be_enlarged() {
        let sprite = animated_sprite();
        let walk = playback::sequence(&sprite, Some(1));
        assert_eq!(walk, [1, 2, 3, 2]);
        let frames = decode_gif(&gif_of(&sprite, &walk, 3));
        assert_eq!(frames.len(), 4);
        for ((image, _), &frame) in frames.iter().zip(&walk) {
            assert_eq!(*image, compositor::compose(&sprite, frame).scaled_up(3));
        }
        let delays: Vec<u16> = frames.iter().map(|(_, delay)| *delay).collect();
        assert_eq!(delays, [8, 12, 6, 12]);
    }

    #[test]
    fn a_gif_makes_translucent_pixels_opaque_and_rounds_short_frames_up() {
        use acms_core::document::Document;

        // The drawn sprite has a half-transparent blue line.
        let mut document = Document::new(drawn_sprite());
        document
            .transact("Duration", |tx| tx.set_frame_duration(0, 3))
            .unwrap();
        let frames = decode_gif(&gif_of(document.sprite(), &[0], 1));
        assert_eq!(frames[0].0.get(12, 8), Rgba8::rgb(40, 90, 220));
        assert_eq!(frames[0].0.get(0, 0), Rgba8::TRANSPARENT);
        assert_eq!(frames[0].1, 1);
    }

    #[test]
    fn exports_refuse_what_cannot_be_done() {
        let sprite = animated_sprite();
        let mut out = Vec::new();
        for (frames, scale) in [
            (&[][..], 1),
            (&[0, 9][..], 1),
            (&[0][..], 0),
            (&[0][..], 500),
        ] {
            assert!(matches!(
                export_gif(&sprite, frames, scale, &mut out),
                Err(IoError::Unsupported(_))
            ));
        }
        let wide = Sprite::new(4096, 1).unwrap();
        assert!(matches!(
            export_gif(&wide, &[0], 32, &mut out),
            Err(IoError::Unsupported(_))
        ));
        assert!(matches!(
            build_sheet(&sprite, &[], &SheetOptions::default()),
            Err(IoError::Unsupported(_))
        ));
        let strip = SheetOptions::default();
        let many = vec![0; 8];
        assert!(matches!(
            build_sheet(&wide, &many, &strip),
            Err(IoError::Unsupported(_))
        ));
    }

    /// Checks that every frame's place on the sheet shows the right part of the right frame,
    /// that no two overlap, and that nothing else is on the sheet.
    fn check_sheet(sprite: &Sprite, sheet: &Sheet, padding: u32) {
        let mut covered = Image::new(sheet.image.width(), sheet.image.height());
        for placed in &sheet.frames {
            let expected = compositor::compose(sprite, placed.frame).crop(placed.source);
            assert_eq!(sheet.image.crop(placed.rect), expected, "{placed:?}");
            assert_eq!(
                (placed.rect.w, placed.rect.h),
                (placed.source.w, placed.source.h)
            );
            assert_eq!(placed.rect.intersect(sheet.image.bounds()), placed.rect);
            // With padding, nothing may come closer than that to it.
            let pad = padding as i32;
            let claimed = Rect::from_edges(
                placed.rect.x - pad,
                placed.rect.y - pad,
                placed.rect.right() + pad,
                placed.rect.bottom() + pad,
            );
            assert_eq!(
                claimed.intersect(sheet.image.bounds()),
                claimed,
                "{placed:?}"
            );
            for y in placed.rect.y..placed.rect.bottom() {
                for x in placed.rect.x..placed.rect.right() {
                    assert_eq!(covered.get(x, y), Rgba8::TRANSPARENT, "overlap at {x}, {y}");
                    covered.set(x, y, Rgba8::WHITE);
                }
            }
        }
        for other in &sheet.frames {
            for placed in &sheet.frames {
                if other.rect != placed.rect {
                    let pad = padding as i32;
                    let grown = Rect::from_edges(
                        placed.rect.x - pad,
                        placed.rect.y - pad,
                        placed.rect.right() + pad,
                        placed.rect.bottom() + pad,
                    );
                    assert!(grown.intersect(other.rect).is_empty());
                }
            }
        }
        for y in 0..sheet.image.height() as i32 {
            for x in 0..sheet.image.width() as i32 {
                if covered.get(x, y).a == 0 {
                    assert_eq!(
                        sheet.image.get(x, y),
                        Rgba8::TRANSPARENT,
                        "stray at {x}, {y}"
                    );
                }
            }
        }
    }

    fn sheet(options: SheetOptions) -> Sheet {
        let sprite = animated_sprite();
        let all: Vec<usize> = (0..4).collect();
        let sheet = build_sheet(&sprite, &all, &options).unwrap();
        check_sheet(&sprite, &sheet, options.padding);
        let frames: Vec<usize> = sheet.frames.iter().map(|placed| placed.frame).collect();
        assert_eq!(frames, all);
        sheet
    }

    fn size(sheet: &Sheet) -> (u32, u32) {
        (sheet.image.width(), sheet.image.height())
    }

    #[test]
    fn sheets_in_rows_and_columns() {
        let options = SheetOptions::default();
        let strip = sheet(options);
        assert_eq!(size(&strip), (64, 12));
        assert_eq!(strip.frames[2].rect, Rect::new(32, 0, 16, 12));

        let grid = sheet(SheetOptions {
            per_line: 2,
            ..options
        });
        assert_eq!(size(&grid), (32, 24));
        assert_eq!(grid.frames[3].rect, Rect::new(16, 12, 16, 12));

        let column = sheet(SheetOptions {
            layout: SheetLayout::Columns,
            ..options
        });
        assert_eq!(size(&column), (16, 48));
        let columns = sheet(SheetOptions {
            layout: SheetLayout::Columns,
            per_line: 3,
            ..options
        });
        assert_eq!(size(&columns), (32, 36));
        assert_eq!(columns.frames[3].rect, Rect::new(16, 0, 16, 12));

        let padded = sheet(SheetOptions {
            per_line: 2,
            padding: 3,
            ..options
        });
        assert_eq!(size(&padded), (3 + 16 + 3 + 16 + 3, 3 + 12 + 3 + 12 + 3));
        assert_eq!(padded.frames[3].rect, Rect::new(22, 18, 16, 12));
    }

    #[test]
    fn trimmed_and_packed_sheets_keep_only_what_is_drawn() {
        let trimmed = sheet(SheetOptions {
            trim: true,
            ..SheetOptions::default()
        });
        // Frames 0, 1 and 3 are the block and the ground; frame 2 is the block alone.
        assert_eq!(trimmed.frames[0].source, Rect::new(0, 3, 16, 8));
        assert_eq!(trimmed.frames[2].source, Rect::new(7, 3, 4, 5));
        assert_eq!(size(&trimmed), (16 + 16 + 4 + 16, 8));

        for padding in [0, 2] {
            let packed = sheet(SheetOptions {
                layout: SheetLayout::Packed,
                trim: true,
                padding,
                ..SheetOptions::default()
            });
            let (w, h) = size(&packed);
            let plain = size(&sheet(SheetOptions {
                padding,
                ..SheetOptions::default()
            }));
            assert!(
                w * h < plain.0 * plain.1,
                "{w} × {h} is no smaller than a strip"
            );
        }

        // A frame with nothing in it still gets a place.
        let empty = Sprite::new(8, 8).unwrap();
        let options = SheetOptions {
            trim: true,
            ..SheetOptions::default()
        };
        let sheet = build_sheet(&empty, &[0], &options).unwrap();
        assert_eq!(sheet.frames[0].rect, Rect::new(0, 0, 1, 1));
        assert_eq!(size(&sheet), (1, 1));
    }

    #[test]
    fn the_metadata_is_laid_out_the_way_aseprite_writes_it() {
        let sprite = animated_sprite();
        let all: Vec<usize> = (0..4).collect();
        let options = SheetOptions {
            trim: true,
            padding: 1,
            ..SheetOptions::default()
        };
        let sheet = build_sheet(&sprite, &all, &options).unwrap();
        let text = sheet_json(&sprite, &sheet, "hero", "hero.png").unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();

        // The frames are listed in order in the file itself, not only once parsed.
        let positions: Vec<usize> = (0..4)
            .map(|index| text.find(&format!("\"hero {index}.acms\"")).unwrap())
            .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));

        let frame = &json["frames"]["hero 2.acms"];
        assert_eq!(
            frame["frame"],
            serde_json::json!({ "x": 35, "y": 1, "w": 4, "h": 5 })
        );
        assert_eq!(frame["rotated"], false);
        assert_eq!(frame["trimmed"], true);
        assert_eq!(
            frame["spriteSourceSize"],
            serde_json::json!({ "x": 7, "y": 3, "w": 4, "h": 5 })
        );
        assert_eq!(frame["sourceSize"], serde_json::json!({ "w": 16, "h": 12 }));
        assert_eq!(frame["duration"], 120);

        let meta = &json["meta"];
        assert_eq!(meta["image"], "hero.png");
        assert_eq!(meta["format"], "RGBA8888");
        assert_eq!(meta["size"], serde_json::json!({ "w": 57, "h": 10 }));
        assert_eq!(meta["scale"], "1");
        assert_eq!(
            meta["frameTags"],
            serde_json::json!([
                { "name": "Idle", "from": 0, "to": 0, "direction": "forward", "color": "#5a5ac8ff" },
                { "name": "Walk", "from": 1, "to": 3, "direction": "pingpong", "color": "#c85a5a80" }
            ])
        );
        assert_eq!(
            meta["layers"],
            serde_json::json!([
                { "name": "Ground", "opacity": 255, "blendMode": "normal" },
                { "name": "Body", "opacity": 255, "blendMode": "normal" }
            ])
        );
        assert_eq!(meta["slices"], serde_json::json!([]));

        // Untrimmed frames say so; a sheet of some frames only does not list the tags.
        let plain = build_sheet(&sprite, &[1, 2], &SheetOptions::default()).unwrap();
        let text = sheet_json(&sprite, &plain, "hero", "hero.png").unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["frames"]["hero 0.acms"]["trimmed"], false);
        assert_eq!(json["frames"]["hero 1.acms"]["duration"], 120);
        assert_eq!(json["meta"]["frameTags"], serde_json::json!([]));
    }

    #[test]
    fn a_sheet_and_its_metadata_are_written_side_by_side() {
        let sprite = animated_sprite();
        let all: Vec<usize> = (0..4).collect();
        let png = TempPath::new("sheet.png");
        let json = TempPath::new("sheet.json");
        let options = SheetOptions {
            per_line: 2,
            ..SheetOptions::default()
        };

        let sheet = export_sheet(&sprite, &all, &options, png.path(), true).unwrap();
        let written = decode_png(&std::fs::read(png.path()).unwrap()).unwrap();
        assert_eq!(written, sheet.image);
        let text = std::fs::read_to_string(json.path()).unwrap();
        let meta: serde_json::Value = serde_json::from_str(&text).unwrap();
        let file_name = png
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(meta["meta"]["image"], file_name.as_str());
        // Cutting the sheet up by the metadata gives the frames back.
        for (index, entry) in meta["frames"].as_object().unwrap().values().enumerate() {
            let rect = Rect::new(
                entry["frame"]["x"].as_i64().unwrap() as i32,
                entry["frame"]["y"].as_i64().unwrap() as i32,
                entry["frame"]["w"].as_u64().unwrap() as u32,
                entry["frame"]["h"].as_u64().unwrap() as u32,
            );
            // serde_json keeps object keys sorted, and these sort in frame order.
            assert_eq!(written.crop(rect), compositor::compose(&sprite, index));
        }

        std::fs::remove_file(json.path()).unwrap();
        export_sheet(&sprite, &all, &options, png.path(), false).unwrap();
        assert!(!json.path().exists());
    }

    #[test]
    fn a_png_sequence_is_numbered_from_one() {
        let names = |count| -> Vec<String> {
            sequence_paths(Path::new("out/walk.png"), count)
                .iter()
                .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(names(3), ["walk_1.png", "walk_2.png", "walk_3.png"]);
        assert_eq!(names(12)[0], "walk_01.png");
        assert_eq!(names(12)[11], "walk_12.png");

        let sprite = animated_sprite();
        let target = TempPath::new("sequence.png");
        let files = export_png_sequence(&sprite, &[1, 2, 3, 2], 2, target.path()).unwrap();
        assert_eq!(files.len(), 4);
        for (file, frame) in files.iter().zip([1, 2, 3, 2]) {
            let image = decode_png(&std::fs::read(file).unwrap()).unwrap();
            assert_eq!(image, compositor::compose(&sprite, frame).scaled_up(2));
            std::fs::remove_file(file).unwrap();
        }
        assert!(!target.path().exists());
    }
}
