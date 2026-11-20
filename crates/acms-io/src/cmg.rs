//! CMG "character imagery" files: a picture written as text, one character per pixel.
//!
//! The format comes from CMG_raylib (<https://www.cem.pp.ua/Akumarin/CMG_raylib>, MIT). It has
//! no specification; what is here follows that program's source:
//!
//! - The file is read byte by byte. A newline ends a row; every other byte is one pixel.
//! - Each of the 94 printable ASCII characters other than space has a fixed colour, given in
//!   [`PALETTE`]. Any other byte (a space, a tab, anything outside ASCII) draws nothing.
//! - The picture is as wide as the longest row and has one row per line. Rows may differ in
//!   length.
//!
//! Where ACMS reads a file differently from CMG_raylib, it is on purpose:
//!
//! - A pixel that draws nothing is transparent here; CMG_raylib paints its light grey
//!   background (245, 245, 245) there.
//! - A last line without a newline after it is a row here. CMG_raylib counts only newlines
//!   for the height, so it cuts that row off.
//! - Windows line endings (`\r\n`) end a row here. CMG_raylib draws the `\r` as one more empty
//!   pixel at the end of every row.
//! - CMG_raylib reads only the first [`TOOL_READ_LIMIT`] bytes of a file. ACMS reads all of it,
//!   and says so when the file is longer.
//!
//! Writing follows the same rules backwards, so that CMG_raylib shows exactly the picture:
//! every row ends with a newline, transparent pixels are spaces, and each other pixel is the
//! character whose colour is nearest.

use std::path::Path;

use acms_core::color::Rgba8;
use acms_core::compositor;
use acms_core::geom::Point;
use acms_core::image::Image;
use acms_core::palette;
use acms_core::sprite::{CelSource, Frame, Layer, LayerId, MAX_SPRITE_SIZE, Palette, Sprite};

use crate::IoError;
use crate::aseprite::Imported;

/// How many bytes of a file CMG_raylib reads; the rest it ignores.
pub const TOOL_READ_LIMIT: usize = 10_239;

/// Every character that has a colour, with its colour, in the order CMG_raylib tests them
/// (which follows a US keyboard). Taken from CMG_raylib's `main.c` by a script, not by hand.
pub const PALETTE: [(u8, Rgba8); 94] = [
    (b'`', Rgba8::rgb(177, 10, 235)),
    (b'1', Rgba8::rgb(164, 248, 225)),
    (b'2', Rgba8::rgb(187, 75, 75)),
    (b'3', Rgba8::rgb(11, 123, 90)),
    (b'4', Rgba8::rgb(202, 119, 54)),
    (b'5', Rgba8::rgb(89, 35, 87)),
    (b'6', Rgba8::rgb(163, 101, 75)),
    (b'7', Rgba8::rgb(56, 172, 190)),
    (b'8', Rgba8::rgb(68, 145, 148)),
    (b'9', Rgba8::rgb(135, 253, 185)),
    (b'0', Rgba8::rgb(88, 33, 16)),
    (b'-', Rgba8::rgb(69, 100, 246)),
    (b'=', Rgba8::rgb(108, 133, 133)),
    (b'q', Rgba8::rgb(1, 43, 53)),
    (b'w', Rgba8::rgb(12, 223, 105)),
    (b'e', Rgba8::rgb(177, 203, 157)),
    (b'r', Rgba8::rgb(49, 57, 244)),
    (b't', Rgba8::rgb(200, 253, 223)),
    (b'y', Rgba8::rgb(88, 113, 153)),
    (b'u', Rgba8::rgb(190, 94, 238)),
    (b'i', Rgba8::rgb(126, 42, 110)),
    (b'o', Rgba8::rgb(128, 42, 236)),
    (b'p', Rgba8::rgb(87, 184, 127)),
    (b'[', Rgba8::rgb(253, 229, 208)),
    (b']', Rgba8::rgb(29, 166, 147)),
    (b'\\', Rgba8::rgb(224, 138, 173)),
    (b'a', Rgba8::rgb(96, 11, 242)),
    (b's', Rgba8::rgb(109, 94, 17)),
    (b'd', Rgba8::rgb(71, 3, 153)),
    (b'f', Rgba8::rgb(5, 231, 214)),
    (b'g', Rgba8::rgb(7, 27, 126)),
    (b'h', Rgba8::rgb(157, 138, 125)),
    (b'j', Rgba8::rgb(73, 250, 185)),
    (b'k', Rgba8::rgb(248, 95, 102)),
    (b'l', Rgba8::rgb(169, 157, 32)),
    (b';', Rgba8::rgb(69, 116, 70)),
    (b'\'', Rgba8::rgb(148, 12, 16)),
    (b'z', Rgba8::rgb(235, 109, 56)),
    (b'x', Rgba8::rgb(238, 227, 22)),
    (b'c', Rgba8::rgb(204, 128, 174)),
    (b'v', Rgba8::rgb(239, 50, 107)),
    (b'b', Rgba8::rgb(40, 62, 156)),
    (b'n', Rgba8::rgb(226, 93, 134)),
    (b'm', Rgba8::rgb(146, 27, 58)),
    (b',', Rgba8::rgb(164, 132, 217)),
    (b'.', Rgba8::rgb(185, 42, 176)),
    (b'/', Rgba8::rgb(184, 30, 52)),
    (b'~', Rgba8::rgb(111, 247, 68)),
    (b'!', Rgba8::rgb(82, 180, 193)),
    (b'@', Rgba8::rgb(120, 38, 193)),
    (b'#', Rgba8::rgb(52, 195, 37)),
    (b'$', Rgba8::rgb(150, 149, 183)),
    (b'%', Rgba8::rgb(176, 172, 171)),
    (b'^', Rgba8::rgb(112, 231, 92)),
    (b'&', Rgba8::rgb(222, 193, 233)),
    (b'*', Rgba8::rgb(250, 123, 85)),
    (b'(', Rgba8::rgb(182, 193, 208)),
    (b')', Rgba8::rgb(193, 129, 224)),
    (b'_', Rgba8::rgb(48, 124, 236)),
    (b'+', Rgba8::rgb(88, 74, 216)),
    (b'Q', Rgba8::rgb(111, 196, 141)),
    (b'W', Rgba8::rgb(58, 69, 191)),
    (b'E', Rgba8::rgb(158, 165, 140)),
    (b'R', Rgba8::rgb(3, 120, 35)),
    (b'T', Rgba8::rgb(22, 163, 242)),
    (b'Y', Rgba8::rgb(175, 13, 225)),
    (b'U', Rgba8::rgb(17, 121, 241)),
    (b'I', Rgba8::rgb(96, 125, 32)),
    (b'O', Rgba8::rgb(73, 248, 183)),
    (b'P', Rgba8::rgb(199, 225, 212)),
    (b'{', Rgba8::rgb(177, 34, 244)),
    (b'}', Rgba8::rgb(59, 93, 92)),
    (b'|', Rgba8::rgb(215, 172, 99)),
    (b'A', Rgba8::rgb(169, 14, 141)),
    (b'S', Rgba8::rgb(5, 17, 54)),
    (b'D', Rgba8::rgb(174, 64, 143)),
    (b'F', Rgba8::rgb(184, 180, 254)),
    (b'G', Rgba8::rgb(66, 221, 217)),
    (b'H', Rgba8::rgb(227, 97, 195)),
    (b'J', Rgba8::rgb(161, 224, 104)),
    (b'K', Rgba8::rgb(171, 143, 174)),
    (b'L', Rgba8::rgb(183, 81, 61)),
    (b':', Rgba8::rgb(10, 99, 125)),
    (b'"', Rgba8::rgb(111, 106, 229)),
    (b'Z', Rgba8::rgb(215, 50, 2)),
    (b'X', Rgba8::rgb(91, 189, 172)),
    (b'C', Rgba8::rgb(148, 179, 31)),
    (b'V', Rgba8::rgb(150, 235, 57)),
    (b'B', Rgba8::rgb(91, 248, 249)),
    (b'N', Rgba8::rgb(170, 243, 86)),
    (b'M', Rgba8::rgb(109, 151, 233)),
    (b'<', Rgba8::rgb(181, 198, 24)),
    (b'>', Rgba8::rgb(54, 8, 73)),
    (b'?', Rgba8::rgb(101, 43, 185)),
];

/// The colour of a character, if it has one.
pub fn color_of(byte: u8) -> Option<Rgba8> {
    PALETTE
        .iter()
        .find(|(character, _)| *character == byte)
        .map(|(_, color)| *color)
}

/// The CMG colours as a palette, in [`PALETTE`]'s order.
pub fn palette() -> Palette {
    Palette::from_colors(PALETTE.iter().map(|(_, color)| *color))
}

/// Whether a file name looks like a CMG file. The format has no extension of its own; ACMS
/// uses `.cmg`.
pub fn is_cmg_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("cmg"))
}

/// The rows of a CMG file, as bytes, without their line endings. `None` if there are more
/// than a sprite can be tall: counted as they are found, so a file of nothing but line
/// endings makes no list of millions of rows.
fn rows(bytes: &[u8]) -> Option<Vec<&[u8]>> {
    let limit = MAX_SPRITE_SIZE as usize;
    let mut rows: Vec<&[u8]> = Vec::new();
    for row in bytes.split(|byte| *byte == b'\n') {
        // One more is allowed for the empty piece after a last newline.
        if rows.len() > limit {
            return None;
        }
        rows.push(row);
    }
    // The newline after the last row ends it; it does not start another.
    if rows.last().is_some_and(|row| row.is_empty()) {
        rows.pop();
    }
    if rows.len() > limit {
        return None;
    }
    for row in &mut rows {
        if let Some(without) = row.strip_suffix(b"\r") {
            *row = without;
        }
    }
    Some(rows)
}

/// Reads a CMG file into a sprite with one layer and one frame, and the CMG colours as its
/// palette.
pub fn import_cmg(bytes: &[u8]) -> Result<Imported, IoError> {
    let limit = MAX_SPRITE_SIZE as usize;
    let rows = rows(bytes).ok_or_else(|| {
        IoError::Unsupported(format!(
            "the picture has more than {limit} rows; ACMS takes at most {limit}"
        ))
    })?;
    let width = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    let height = rows.len();
    if width == 0 {
        return Err(IoError::Invalid("the file has no pixels in it".to_owned()));
    }
    let limit = MAX_SPRITE_SIZE as usize;
    if width > limit || height > limit {
        return Err(IoError::Unsupported(format!(
            "the picture is {width} × {height} characters; ACMS takes at most {limit} × {limit}"
        )));
    }

    let (w, h) = (width as u32, height as u32);
    let mut image = Image::new(w, h);
    let mut colorless = 0;
    for (y, row) in rows.iter().enumerate() {
        for (x, byte) in row.iter().enumerate() {
            match color_of(*byte) {
                Some(color) => image.set(x as i32, y as i32, color),
                None if *byte == b' ' => {}
                None => colorless += 1,
            }
        }
    }

    let mut notes = Vec::new();
    if colorless > 0 {
        notes.push(format!(
            "{colorless} characters that have no colour in CMG (tabs, letters outside ASCII \
             and the like) are transparent."
        ));
    }
    if bytes.len() > TOOL_READ_LIMIT {
        notes.push(format!(
            "The file is {} bytes long. CMG_raylib reads only the first {TOOL_READ_LIMIT}, so it \
             shows less of the picture than ACMS does.",
            bytes.len()
        ));
    }

    let layer = LayerId(1);
    let sprite = Sprite::from_parts(
        w,
        h,
        palette(),
        vec![Layer::image(layer, "Layer 1")],
        vec![Frame { duration_ms: 100 }],
        vec![CelSource {
            layer,
            frame: 0,
            origin: Point::new(0, 0),
            image,
        }],
    )?;
    Ok(Imported { sprite, notes })
}

/// [`import_cmg`] from a file.
pub fn import_cmg_from_path(path: &Path) -> Result<Imported, IoError> {
    import_cmg(&crate::read_file(path)?)
}

/// What became of the picture on its way into a CMG file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CmgExport {
    /// Pixels whose colour is not one of the 94 and were given the nearest one.
    pub approximated: usize,
    /// Pixels that were partly transparent: they are a colour if mostly opaque, a space if
    /// mostly not.
    pub translucent: usize,
    /// The file is longer than CMG_raylib reads.
    pub too_long: bool,
}

/// Writes one frame of a sprite, flattened, as a CMG file.
pub fn export_cmg(sprite: &Sprite, frame: usize) -> Result<(Vec<u8>, CmgExport), IoError> {
    if frame >= sprite.frames().len() {
        return Err(IoError::Unsupported(format!(
            "there is no frame {}",
            frame + 1
        )));
    }
    let picture = compositor::compose(sprite, frame);
    let palette = palette();
    let mut report = CmgExport::default();
    let width = picture.width() as usize;
    let mut text = Vec::with_capacity((width + 1) * picture.height() as usize);

    for y in 0..picture.height() as i32 {
        for x in 0..picture.width() as i32 {
            let pixel = picture.get(x, y);
            if pixel.a != 0 && pixel.a != 255 {
                report.translucent += 1;
            }
            if pixel.a < 128 {
                text.push(b' ');
                continue;
            }
            let opaque = Rgba8::rgb(pixel.r, pixel.g, pixel.b);
            let exact = PALETTE.iter().find(|(_, color)| *color == opaque);
            let character = match exact {
                Some((character, _)) => *character,
                None => {
                    report.approximated += 1;
                    let nearest = palette::nearest(&palette, opaque);
                    PALETTE
                        .iter()
                        .find(|(_, color)| Some(*color) == nearest)
                        .map_or(b' ', |(character, _)| *character)
                }
            };
            text.push(character);
        }
        text.push(b'\n');
    }
    report.too_long = text.len() > TOOL_READ_LIMIT;
    Ok((text, report))
}

/// [`export_cmg`] into a file.
pub fn export_cmg_to_path(
    sprite: &Sprite,
    frame: usize,
    path: &Path,
) -> Result<CmgExport, IoError> {
    let (text, report) = export_cmg(sprite, frame)?;
    crate::write_atomically(path, |file| {
        use std::io::Write;
        file.write_all(&text)?;
        Ok(())
    })?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use acms_core::document::Document;
    use acms_core::geom::Rect;

    use super::*;

    const RAYWHITE: Rgba8 = Rgba8::rgb(245, 245, 245);

    fn sample(name: &str) -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/samples/cmg")
            .join(name);
        std::fs::read(path).unwrap()
    }

    fn pixel(sprite: &Sprite, x: i32, y: i32) -> Rgba8 {
        sprite.pixel(LayerId(1), 0, x, y)
    }

    #[test]
    fn the_table_has_every_printable_character_but_space_once() {
        let mut characters: Vec<u8> = PALETTE.iter().map(|(c, _)| *c).collect();
        characters.sort_unstable();
        let printable: Vec<u8> = (b'!'..=b'~').collect();
        assert_eq!(characters, printable);
        // No two characters share a colour, so writing a colour back gives its character.
        let mut colors: Vec<[u8; 4]> = PALETTE.iter().map(|(_, c)| c.to_array()).collect();
        colors.sort_unstable();
        colors.dedup();
        assert_eq!(colors.len(), 94);
        assert!(PALETTE.iter().all(|(_, color)| color.a == 255));
        // Spot checks against CMG_raylib's main.c.
        assert_eq!(color_of(b'`'), Some(Rgba8::rgb(177, 10, 235)));
        assert_eq!(color_of(b'\\'), Some(Rgba8::rgb(224, 138, 173)));
        assert_eq!(color_of(b'\''), Some(Rgba8::rgb(148, 12, 16)));
        assert_eq!(color_of(b'?'), Some(Rgba8::rgb(101, 43, 185)));
        assert_eq!(color_of(b' '), None);
        assert_eq!(palette().len(), 94);
    }

    #[test]
    fn rows_columns_and_colours_are_read() {
        let imported = import_cmg(b"ab\n`  ?\nz\n").unwrap();
        let sprite = &imported.sprite;
        // As wide as the longest row, one row per line.
        assert_eq!((sprite.width(), sprite.height()), (4, 3));
        assert_eq!(pixel(sprite, 0, 0), color_of(b'a').unwrap());
        assert_eq!(pixel(sprite, 1, 0), color_of(b'b').unwrap());
        // A short row leaves the rest of its line empty, and so does a space.
        assert_eq!(pixel(sprite, 2, 0), Rgba8::TRANSPARENT);
        assert_eq!(pixel(sprite, 1, 1), Rgba8::TRANSPARENT);
        assert_eq!(pixel(sprite, 3, 1), color_of(b'?').unwrap());
        assert_eq!(pixel(sprite, 0, 2), color_of(b'z').unwrap());
        assert!(imported.notes.is_empty());
        // The palette is CMG's, to draw with colours the format can hold.
        assert_eq!(sprite.palette().len(), 94);
        assert_eq!(sprite.layer_count(), 1);
        assert_eq!(sprite.frames().len(), 1);
    }

    #[test]
    fn line_endings_and_a_last_line_without_one() {
        let unix = import_cmg(b"ab\ncd\n").unwrap().sprite;
        // Windows line endings give the same picture.
        assert_eq!(import_cmg(b"ab\r\ncd\r\n").unwrap().sprite, unix);
        // So does leaving off the last newline: the last line is still a row.
        assert_eq!(import_cmg(b"ab\ncd").unwrap().sprite, unix);
        // Empty lines are empty rows.
        let gaps = import_cmg(b"a\n\n\nb\n").unwrap().sprite;
        assert_eq!((gaps.width(), gaps.height()), (1, 4));
        assert_eq!(pixel(&gaps, 0, 3), color_of(b'b').unwrap());
    }

    #[test]
    fn characters_without_a_colour_are_transparent_and_said() {
        let imported = import_cmg("a\tb é".as_bytes()).unwrap();
        // Each byte is a pixel, as CMG_raylib reads it: é is two.
        assert_eq!(imported.sprite.width(), 6);
        assert_eq!(pixel(&imported.sprite, 1, 0), Rgba8::TRANSPARENT);
        assert_eq!(pixel(&imported.sprite, 2, 0), color_of(b'b').unwrap());
        assert_eq!(imported.notes.len(), 1);
        assert!(
            imported.notes[0].starts_with("3 characters"),
            "{:?}",
            imported.notes
        );
    }

    #[test]
    fn what_cannot_be_a_picture_is_refused() {
        assert!(import_cmg(b"").is_err());
        assert!(import_cmg(b"\n\n\n").is_err());
        assert!(import_cmg(b"   \n  ").is_ok());
        let wide = vec![b'a'; MAX_SPRITE_SIZE as usize + 1];
        assert!(matches!(import_cmg(&wide), Err(IoError::Unsupported(_))));
        let missing = Path::new("../../tests/samples/cmg/none.cmg");
        assert!(matches!(import_cmg_from_path(missing), Err(IoError::Io(_))));
    }

    #[test]
    fn a_long_file_is_read_whole_and_said_to_be_long() {
        let mut text = Vec::new();
        for _ in 0..200 {
            text.extend_from_slice(
                b"abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ!?\n",
            );
        }
        assert!(text.len() > TOOL_READ_LIMIT);
        let imported = import_cmg(&text).unwrap();
        assert_eq!(imported.sprite.height(), 200);
        assert_eq!(imported.notes.len(), 1);
        assert!(imported.notes[0].contains("10239"), "{:?}", imported.notes);
    }

    #[test]
    fn written_files_read_back_to_the_same_picture() {
        let source = b"`1234567890-=\nqwertyuiop[]\\\nasdfghjkl;' zx\n  ~!@#$%^&*()_+\n";
        let sprite = import_cmg(source).unwrap().sprite;
        let (text, report) = export_cmg(&sprite, 0).unwrap();
        assert_eq!(report, CmgExport::default());
        // Every row ends with a newline and has the full width, so the size survives.
        assert!(text.ends_with(b"\n"));
        let width = sprite.width() as usize;
        assert_eq!(width, 15);
        let rows: Vec<&[u8]> = text
            .split(|b| *b == b'\n')
            .filter(|row| !row.is_empty())
            .collect();
        assert!(rows.iter().all(|row| row.len() == width));
        let again = import_cmg(&text).unwrap().sprite;
        assert_eq!(again, sprite);
        // And the text is the original, padded with spaces to the full width.
        assert_eq!(rows[0], b"`1234567890-=  ");
        assert_eq!(rows[2], b"asdfghjkl;' zx ");
    }

    #[test]
    fn other_colours_become_the_nearest_and_are_counted() {
        let mut document = Document::new(Sprite::new(4, 1).unwrap());
        let pixels = [
            color_of(b'Z').unwrap(),      // exactly a CMG colour
            Rgba8::rgb(214, 51, 3),       // next to Z's colour
            Rgba8::new(10, 99, 125, 200), // ':' but partly transparent: still ':'
            Rgba8::new(10, 99, 125, 60),  // mostly transparent: a space
        ];
        let image = Image::from_pixels(4, 1, pixels.to_vec()).unwrap();
        document
            .transact("Paint", |tx| {
                tx.set_pixels(LayerId(1), 0, Rect::new(0, 0, 4, 1), &image)
            })
            .unwrap();
        let (text, report) = export_cmg(document.sprite(), 0).unwrap();
        assert_eq!(text, b"ZZ: \n");
        assert_eq!(report.approximated, 1);
        assert_eq!(report.translucent, 2);
        assert!(!report.too_long);
        assert!(export_cmg(document.sprite(), 3).is_err());
    }

    #[test]
    fn a_picture_too_long_for_the_tool_is_said_to_be() {
        let sprite = Sprite::new(100, 120).unwrap();
        let (text, report) = export_cmg(&sprite, 0).unwrap();
        assert_eq!(text.len(), 101 * 120);
        assert!(report.too_long);
    }

    /// CMG_raylib's own rendering of the first 10 239 bytes of its main.c at 5 pixels a
    /// character, from its repository, compared with what ACMS reads from the same bytes.
    #[test]
    fn the_picture_is_the_one_cmg_raylib_draws() {
        let source = sample("main.c");
        let read = &source[..TOOL_READ_LIMIT];
        let sprite = import_cmg(read).unwrap().sprite;

        let reference = crate::png::decode_png(&sample("main.c.png")).unwrap();
        let scale = 5;
        // CMG_raylib leaves out a last line that has no newline after it; ACMS keeps it.
        let newlines = read.iter().filter(|b| **b == b'\n').count() as u32;
        assert_eq!(reference.height(), newlines * scale);
        assert_eq!(sprite.height(), newlines + 1);
        assert_eq!(reference.width(), sprite.width() * scale);

        let picture = compositor::compose(&sprite, 0);
        for y in 0..reference.height() as i32 {
            for x in 0..reference.width() as i32 {
                let ours = picture.get(x / scale as i32, y / scale as i32);
                // Where ACMS has nothing, CMG_raylib shows its background.
                let ours = if ours.a == 0 { RAYWHITE } else { ours };
                assert_eq!(ours, reference.get(x, y), "pixel {x}, {y}");
            }
        }
    }

    #[test]
    fn a_file_of_endless_rows_is_refused_without_listing_them() {
        let bytes = vec![b'\n'; 1_000_000];
        assert!(matches!(import_cmg(&bytes), Err(IoError::Unsupported(_))));
        // As many rows as a sprite can be tall is fine, with or without a last newline.
        let mut tall = b"R\n".repeat(MAX_SPRITE_SIZE as usize);
        assert_eq!(import_cmg(&tall).unwrap().sprite.height(), MAX_SPRITE_SIZE);
        tall.pop();
        assert_eq!(import_cmg(&tall).unwrap().sprite.height(), MAX_SPRITE_SIZE);
    }
}
