//! Netpbm images: PBM (black and white), PGM (grey), PPM (colour), in their plain (text) and
//! raw (binary) forms, and PAM, which can also hold transparency.
//!
//! Reading follows the Netpbm specifications (<https://netpbm.sourceforge.net/doc/>):
//!
//! - PBM `P1` / `P4`: 1 is black, 0 is white. `P4` packs eight pixels to a byte, most
//!   significant bit first, each row starting on a new byte.
//! - PGM `P2` / `P5` and PPM `P3` / `P6`: samples from 0 to a maximum value of at most 65535;
//!   raw samples take one byte, or two (most significant first) if the maximum is over 255.
//! - PAM `P7`: a header of `WIDTH`, `HEIGHT`, `DEPTH`, `MAXVAL`, `TUPLTYPE` lines ending in
//!   `ENDHDR`, then raw samples. Depth 1 is grey (or black and white, where 1 is white), 2 grey
//!   and alpha, 3 colour, 4 colour and alpha. Alpha is not premultiplied.
//! - Comments (`#` to the end of the line) may sit between any two parts of a header.
//! - A file may hold several images one after the other; they become frames.
//!
//! Writing makes raw files with a maximum of 255. Only PAM keeps transparency: the others
//! have none, so the picture is put on white first, and PGM and PBM then lose colour.

use std::path::Path;

use acms_core::color::Rgba8;
use acms_core::compositor;
use acms_core::geom::Point;
use acms_core::image::Image;
use acms_core::palette;
use acms_core::sprite::{CelSource, Frame, Layer, LayerId, MAX_SPRITE_SIZE, Palette, Sprite};

use crate::IoError;
use crate::aseprite::Imported;

/// The four kinds of Netpbm image ACMS writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetpbmKind {
    /// Black and white.
    Pbm,
    /// Grey.
    Pgm,
    /// Colour.
    Ppm,
    /// Colour and transparency.
    Pam,
}

impl NetpbmKind {
    /// The kind a file name asks for, by its extension. `.pnm` means "any of the three", and
    /// is written as colour.
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "pbm" => Some(Self::Pbm),
            "pgm" => Some(Self::Pgm),
            "ppm" | "pnm" => Some(Self::Ppm),
            "pam" => Some(Self::Pam),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Pbm => "pbm",
            Self::Pgm => "pgm",
            Self::Ppm => "ppm",
            Self::Pam => "pam",
        }
    }
}

/// Whether a file name looks like a Netpbm image.
pub fn is_netpbm_path(path: &Path) -> bool {
    NetpbmKind::from_path(path).is_some()
}

fn damaged(why: impl std::fmt::Display) -> IoError {
    IoError::Invalid(why.to_string())
}

/// Reads through a Netpbm file.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn is_space(byte: u8) -> bool {
        matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C)
    }

    /// Skips white space and comments.
    fn skip(&mut self) {
        while let Some(&byte) = self.bytes.get(self.at) {
            if Self::is_space(byte) {
                self.at += 1;
            } else if byte == b'#' {
                while self.bytes.get(self.at).is_some_and(|byte| *byte != b'\n') {
                    self.at += 1;
                }
            } else {
                break;
            }
        }
    }

    fn at_end(&mut self) -> bool {
        self.skip();
        self.at >= self.bytes.len()
    }

    /// The next run of characters that are not white space.
    fn word(&mut self) -> &'a [u8] {
        self.skip();
        let start = self.at;
        while self
            .bytes
            .get(self.at)
            .is_some_and(|byte| !Self::is_space(*byte) && *byte != b'#')
        {
            self.at += 1;
        }
        &self.bytes[start..self.at]
    }

    /// The next whole number. `what` names it in the error.
    fn number(&mut self, what: &str) -> Result<u32, IoError> {
        let word = self.word();
        let text = std::str::from_utf8(word).unwrap_or("");
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(damaged(format!("the {what} is missing or not a number")));
        }
        text.parse()
            .map_err(|_| damaged(format!("the {what} is too large")))
    }

    /// The single white-space character between a raw header and its pixels.
    fn end_of_header(&mut self) -> Result<(), IoError> {
        match self.bytes.get(self.at) {
            Some(byte) if Self::is_space(*byte) => {
                self.at += 1;
                Ok(())
            }
            _ => Err(damaged("the header does not end where it should")),
        }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], IoError> {
        let end = self
            .at
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len());
        let Some(end) = end else {
            return Err(damaged("the file ends before the picture does"));
        };
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }
}

fn check_size(width: u32, height: u32) -> Result<(), IoError> {
    if width == 0 || height == 0 {
        return Err(damaged("the picture has no pixels"));
    }
    if width > MAX_SPRITE_SIZE || height > MAX_SPRITE_SIZE {
        return Err(IoError::Unsupported(format!(
            "the picture is {width} × {height}; ACMS takes at most {MAX_SPRITE_SIZE} × {MAX_SPRITE_SIZE}"
        )));
    }
    Ok(())
}

fn check_maxval(maxval: u32) -> Result<(), IoError> {
    if (1..=65535).contains(&maxval) {
        Ok(())
    } else {
        Err(damaged(format!(
            "the largest value is {maxval}; it must be 1 to 65535"
        )))
    }
}

/// A sample on a scale of 0 to `maxval`, on a scale of 0 to 255.
fn scaled(value: u32, maxval: u32) -> u8 {
    ((value * 255 + maxval / 2) / maxval) as u8
}

/// The samples of a raw raster: one byte each, or two if the maximum is over 255.
fn raw_samples(reader: &mut Reader<'_>, count: usize, maxval: u32) -> Result<Vec<u32>, IoError> {
    let wide = maxval > 255;
    let bytes = reader.take(count * if wide { 2 } else { 1 })?;
    let samples: Vec<u32> = if wide {
        bytes
            .chunks_exact(2)
            .map(|pair| u32::from(u16::from_be_bytes([pair[0], pair[1]])))
            .collect()
    } else {
        bytes.iter().map(|byte| u32::from(*byte)).collect()
    };
    if samples.iter().any(|sample| *sample > maxval) {
        return Err(damaged("a value is larger than the largest value allowed"));
    }
    Ok(samples)
}

/// The samples of a plain raster: numbers written out.
fn plain_samples(reader: &mut Reader<'_>, count: usize, maxval: u32) -> Result<Vec<u32>, IoError> {
    let mut samples = Vec::with_capacity(count);
    for _ in 0..count {
        let sample = match reader.number("pixel value") {
            Ok(sample) => sample,
            Err(_) if reader.at >= reader.bytes.len() => {
                return Err(damaged("the file ends before the picture does"));
            }
            Err(err) => return Err(err),
        };
        if sample > maxval {
            return Err(damaged("a value is larger than the largest value allowed"));
        }
        samples.push(sample);
    }
    Ok(samples)
}

/// Pixels from samples: `channels` samples per pixel, read as grey (1), grey and alpha (2),
/// colour (3) or colour and alpha (4).
fn pixels(samples: &[u32], channels: usize, maxval: u32) -> Vec<Rgba8> {
    samples
        .chunks_exact(channels)
        .map(|pixel| {
            let s = |index: usize| scaled(pixel[index], maxval);
            let color = match channels {
                1 => Rgba8::rgb(s(0), s(0), s(0)),
                2 => Rgba8::new(s(0), s(0), s(0), s(1)),
                3 => Rgba8::rgb(s(0), s(1), s(2)),
                _ => Rgba8::new(s(0), s(1), s(2), s(3)),
            };
            color.normalized()
        })
        .collect()
}

const BLACK: Rgba8 = Rgba8::BLACK;
const WHITE: Rgba8 = Rgba8::WHITE;

/// Reads one image of a PAM file, after its `P7`.
fn read_pam(reader: &mut Reader<'_>, notes: &mut Vec<String>) -> Result<Image, IoError> {
    let (mut width, mut height, mut depth, mut maxval) = (None, None, None, None);
    let mut tupltype = String::new();
    loop {
        let key = reader.word();
        match key {
            b"ENDHDR" => break,
            b"WIDTH" => width = Some(reader.number("width")?),
            b"HEIGHT" => height = Some(reader.number("height")?),
            b"DEPTH" => depth = Some(reader.number("depth")?),
            b"MAXVAL" => maxval = Some(reader.number("largest value")?),
            b"TUPLTYPE" => {
                // The rest of the line, which may hold spaces.
                while reader
                    .bytes
                    .get(reader.at)
                    .is_some_and(|b| *b == b' ' || *b == b'\t')
                {
                    reader.at += 1;
                }
                let start = reader.at;
                while reader.bytes.get(reader.at).is_some_and(|b| *b != b'\n') {
                    reader.at += 1;
                }
                let line = String::from_utf8_lossy(&reader.bytes[start..reader.at]);
                if !tupltype.is_empty() {
                    tupltype.push(' ');
                }
                tupltype.push_str(line.trim());
            }
            b"" => return Err(damaged("the PAM header has no ENDHDR")),
            other => {
                return Err(damaged(format!(
                    "the PAM header has an unknown line \"{}\"",
                    String::from_utf8_lossy(other)
                )));
            }
        }
    }
    let (Some(width), Some(height), Some(depth), Some(maxval)) = (width, height, depth, maxval)
    else {
        return Err(damaged(
            "the PAM header lacks its width, height, depth or largest value",
        ));
    };
    reader.end_of_header()?;
    check_size(width, height)?;
    check_maxval(maxval)?;
    if !(1..=4).contains(&depth) {
        return Err(IoError::Unsupported(format!(
            "PAM images with {depth} values per pixel cannot be read"
        )));
    }
    let known = [
        ("BLACKANDWHITE", 1),
        ("GRAYSCALE", 1),
        ("BLACKANDWHITE_ALPHA", 2),
        ("GRAYSCALE_ALPHA", 2),
        ("RGB", 3),
        ("RGB_ALPHA", 4),
    ];
    if !known
        .iter()
        .any(|(name, d)| *name == tupltype && *d == depth)
    {
        let note = format!(
            "The PAM image is of the kind \"{tupltype}\" with {depth} values per pixel; it was \
             read as {}.",
            [
                "grey",
                "grey and transparency",
                "colour",
                "colour and transparency"
            ][depth as usize - 1]
        );
        if !notes.contains(&note) {
            notes.push(note);
        }
    }
    let count = width as usize * height as usize;
    let samples = raw_samples(reader, count * depth as usize, maxval)?;
    let pixels = pixels(&samples, depth as usize, maxval);
    Image::from_pixels(width, height, pixels).ok_or_else(|| damaged("the picture has no pixels"))
}

/// Reads one image, starting at its magic number.
fn read_image(reader: &mut Reader<'_>, notes: &mut Vec<String>) -> Result<Image, IoError> {
    let magic = reader.take(2).map_err(|_| damaged("not a Netpbm image"))?;
    let kind = match magic {
        [b'P', kind @ b'1'..=b'7'] => *kind,
        _ => return Err(damaged("not a Netpbm image")),
    };
    if kind == b'7' {
        return read_pam(reader, notes);
    }
    let width = reader.number("width")?;
    let height = reader.number("height")?;
    let maxval = match kind {
        b'1' | b'4' => 1,
        _ => reader.number("largest value")?,
    };
    check_maxval(maxval)?;
    if matches!(kind, b'4' | b'5' | b'6') {
        reader.end_of_header()?;
    }
    check_size(width, height)?;
    let (w, h) = (width as usize, height as usize);
    let count = w * h;

    let pixels = match kind {
        b'1' => {
            // Plain PBM: one digit per pixel, with or without space between them.
            let mut pixels = Vec::with_capacity(count);
            while pixels.len() < count {
                reader.skip();
                match reader.bytes.get(reader.at) {
                    Some(b'0') => pixels.push(WHITE),
                    Some(b'1') => pixels.push(BLACK),
                    Some(_) => return Err(damaged("a plain PBM pixel is not 0 or 1")),
                    None => return Err(damaged("the file ends before the picture does")),
                }
                reader.at += 1;
            }
            pixels
        }
        b'4' => {
            let row_bytes = w.div_ceil(8);
            let raster = reader.take(row_bytes * h)?;
            let mut pixels = Vec::with_capacity(count);
            for row in raster.chunks_exact(row_bytes) {
                for x in 0..w {
                    let bit = row[x / 8] & (0x80 >> (x % 8));
                    pixels.push(if bit != 0 { BLACK } else { WHITE });
                }
            }
            pixels
        }
        b'2' | b'5' | b'3' | b'6' => {
            let channels = if matches!(kind, b'3' | b'6') { 3 } else { 1 };
            let samples = if matches!(kind, b'2' | b'3') {
                plain_samples(reader, count * channels, maxval)?
            } else {
                raw_samples(reader, count * channels, maxval)?
            };
            pixels(&samples, channels, maxval)
        }
        _ => return Err(damaged("not a Netpbm image")),
    };
    Image::from_pixels(width, height, pixels).ok_or_else(|| damaged("the picture has no pixels"))
}

/// Reads a Netpbm file. Several images in one file become frames.
pub fn import_netpbm(bytes: &[u8]) -> Result<Imported, IoError> {
    let mut reader = Reader { bytes, at: 0 };
    let mut notes = Vec::new();
    let mut images: Vec<Image> = Vec::new();
    let mut skipped = 0;
    loop {
        let image = match read_image(&mut reader, &mut notes) {
            Ok(image) => image,
            // What follows a good image may be anything; the images before it stand.
            Err(err) if images.is_empty() => return Err(err),
            Err(_) => {
                notes.push(format!(
                    "After {} pictures the file holds something that is not one; the rest was \
                     left out.",
                    images.len()
                ));
                break;
            }
        };
        match images.first() {
            Some(first) if first.bounds() != image.bounds() => skipped += 1,
            _ => images.push(image),
        }
        if reader.at_end() {
            break;
        }
    }
    if skipped > 0 {
        notes.push(format!(
            "{skipped} pictures in the file are not the size of the first, and were left out."
        ));
    }

    let first = &images[0];
    let (width, height) = (first.width(), first.height());
    let (palette, _) = palette::from_image(first);
    let palette = if palette.is_empty() {
        Palette::default_palette()
    } else {
        palette
    };
    let layer = LayerId(1);
    let frames = vec![Frame { duration_ms: 100 }; images.len()];
    let cels = images
        .into_iter()
        .enumerate()
        .map(|(frame, image)| CelSource {
            layer,
            frame,
            origin: Point::new(0, 0),
            image,
        })
        .collect();
    let sprite = Sprite::from_parts(
        width,
        height,
        palette,
        vec![Layer::image(layer, "Layer 1")],
        frames,
        cels,
    )?;
    Ok(Imported { sprite, notes })
}

/// [`import_netpbm`] from a file.
pub fn import_netpbm_from_path(path: &Path) -> Result<Imported, IoError> {
    import_netpbm(&std::fs::read(path)?)
}

/// What became of the picture on its way into a Netpbm file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetpbmExport {
    /// Pixels that were partly or wholly transparent, and were put on white.
    pub on_white: usize,
    /// Pixels whose colour was lost: made grey (PGM) or black or white (PBM).
    pub changed: usize,
}

/// A colour put on white: what it looks like where nothing can be transparent.
fn on_white(color: Rgba8) -> Rgba8 {
    color.over(WHITE)
}

/// How light a colour looks, 0 to 255, by the weights PGM tools use (ITU-R BT.601).
fn luminance(color: Rgba8) -> u8 {
    let sum = 299 * u32::from(color.r) + 587 * u32::from(color.g) + 114 * u32::from(color.b);
    ((sum + 500) / 1000) as u8
}

/// Writes one frame of a sprite, flattened, as a Netpbm image of the given kind.
pub fn export_netpbm(
    sprite: &Sprite,
    frame: usize,
    kind: NetpbmKind,
) -> Result<(Vec<u8>, NetpbmExport), IoError> {
    if frame >= sprite.frames().len() {
        return Err(IoError::Unsupported(format!(
            "there is no frame {}",
            frame + 1
        )));
    }
    let picture = compositor::compose(sprite, frame);
    let (width, height) = (picture.width(), picture.height());
    let mut report = NetpbmExport::default();
    let mut out = match kind {
        NetpbmKind::Pbm => format!("P4\n{width} {height}\n"),
        NetpbmKind::Pgm => format!("P5\n{width} {height}\n255\n"),
        NetpbmKind::Ppm => format!("P6\n{width} {height}\n255\n"),
        NetpbmKind::Pam => format!(
            "P7\nWIDTH {width}\nHEIGHT {height}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n"
        ),
    }
    .into_bytes();

    if kind == NetpbmKind::Pam {
        out.extend(picture.to_rgba_bytes());
        return Ok((out, report));
    }
    let flat: Vec<Rgba8> = picture
        .pixels()
        .iter()
        .map(|pixel| {
            if pixel.a != 255 {
                report.on_white += 1;
            }
            on_white(*pixel)
        })
        .collect();
    match kind {
        NetpbmKind::Ppm => {
            out.extend(flat.iter().flat_map(|c| [c.r, c.g, c.b]));
        }
        NetpbmKind::Pgm => {
            for color in &flat {
                if !(color.r == color.g && color.g == color.b) {
                    report.changed += 1;
                }
                out.push(luminance(*color));
            }
        }
        NetpbmKind::Pbm => {
            for row in flat.chunks_exact(width as usize) {
                let mut packed = vec![0_u8; row.len().div_ceil(8)];
                for (x, color) in row.iter().enumerate() {
                    if *color != BLACK && *color != WHITE {
                        report.changed += 1;
                    }
                    if luminance(*color) < 128 {
                        packed[x / 8] |= 0x80 >> (x % 8);
                    }
                }
                out.extend(packed);
            }
        }
        // Written whole above.
        NetpbmKind::Pam => {}
    }
    Ok((out, report))
}

/// [`export_netpbm`] into a file, of the kind its name asks for.
pub fn export_netpbm_to_path(
    sprite: &Sprite,
    frame: usize,
    path: &Path,
) -> Result<NetpbmExport, IoError> {
    let kind = NetpbmKind::from_path(path).ok_or_else(|| {
        IoError::Unsupported(
            "the file name must end in .pbm, .pgm, .ppm, .pnm or .pam to say which kind to write"
                .to_owned(),
        )
    })?;
    let (bytes, report) = export_netpbm(sprite, frame, kind)?;
    crate::write_atomically(path, |file| {
        use std::io::Write;
        file.write_all(&bytes)?;
        Ok(())
    })?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use acms_core::document::Document;
    use acms_core::geom::Rect;

    use super::*;

    fn sample(name: &str) -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/samples/netpbm")
            .join(name);
        std::fs::read(path).unwrap()
    }

    fn read(bytes: &[u8]) -> Sprite {
        import_netpbm(bytes).unwrap().sprite
    }

    fn picture(sprite: &Sprite, frame: usize) -> Image {
        compositor::compose(sprite, frame)
    }

    fn px(sprite: &Sprite, x: i32, y: i32) -> Rgba8 {
        sprite.pixel(LayerId(1), 0, x, y)
    }

    const GREY: fn(u8) -> Rgba8 = |v| Rgba8::rgb(v, v, v);

    #[test]
    fn files_written_by_another_program_read_as_it_drew_them() {
        // Pillow wrote each file and a PNG of the same picture: see make_samples.py.
        for (file, png) in [
            ("colour.ppm", "colour.png"),
            ("grey.pgm", "grey.png"),
            ("grey16.pgm", "grey.png"),
            ("bits.pbm", "bits.png"),
        ] {
            let imported = import_netpbm(&sample(file)).unwrap();
            assert!(imported.notes.is_empty(), "{file}: {:?}", imported.notes);
            let expected = crate::png::decode_png(&sample(png)).unwrap();
            assert_eq!(picture(&imported.sprite, 0), expected, "{file}");
        }
    }

    #[test]
    fn plain_files_with_comments_anywhere() {
        let ppm =
            b"P3 # colour\n# a comment line\n2 1\n# the largest value\n255\n255 0 0  0 0 255\n";
        let sprite = read(ppm);
        assert_eq!(
            (px(&sprite, 0, 0), px(&sprite, 1, 0)),
            (Rgba8::rgb(255, 0, 0), Rgba8::rgb(0, 0, 255))
        );

        let pgm = b"P2\n3 1\n4\n0 2 4\n";
        let sprite = read(pgm);
        assert_eq!(
            (px(&sprite, 0, 0), px(&sprite, 1, 0), px(&sprite, 2, 0)),
            (GREY(0), GREY(128), GREY(255))
        );

        // Plain PBM digits may be written with or without space between them.
        let pbm = b"P1\n# pixels\n5 2\n10101\n0 1 0 1 0\n";
        let sprite = read(pbm);
        let row = |y| {
            (0..5)
                .map(|x| px(&sprite, x, y) == Rgba8::BLACK)
                .collect::<Vec<_>>()
        };
        assert_eq!(row(0), [true, false, true, false, true]);
        assert_eq!(row(1), [false, true, false, true, false]);
    }

    #[test]
    fn raw_files_and_their_corners() {
        // One byte a sample, the maximum below 255: scaled to the full range.
        let mut pgm = b"P5 2 1 15\n".to_vec();
        pgm.extend([0, 15]);
        let sprite = read(&pgm);
        assert_eq!((px(&sprite, 0, 0), px(&sprite, 1, 0)), (GREY(0), GREY(255)));

        // Two bytes a sample, most significant first.
        let mut ppm = b"P6\n1 1\n65535\n".to_vec();
        ppm.extend([0xFF, 0xFF, 0x80, 0x00, 0x00, 0x00]);
        assert_eq!(px(&read(&ppm), 0, 0), Rgba8::rgb(255, 128, 0));

        // A raw PBM row of 9 pixels takes two bytes; the rest of the second is padding.
        let mut pbm = b"P4\n9 2\n".to_vec();
        pbm.extend([0b1000_0000, 0b1000_0000, 0b0000_0001, 0b0111_1111]);
        let sprite = read(&pbm);
        let row = |y| {
            (0..9)
                .map(|x| px(&sprite, x, y) == Rgba8::BLACK)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            row(0),
            [true, false, false, false, false, false, false, false, true]
        );
        assert_eq!(
            row(1),
            [false, false, false, false, false, false, false, true, false]
        );

        // A comment between the header and a raw raster is not allowed to eat into it.
        let mut tight = b"P5 1 1 255\n".to_vec();
        tight.push(b'#');
        assert_eq!(px(&read(&tight), 0, 0), GREY(b'#'));
    }

    #[test]
    fn pam_with_and_without_transparency() {
        let pam = |depth: u32, tupltype: &str, maxval: u32, raster: &[u8]| {
            let mut bytes = format!(
                "P7\n# made by hand\nWIDTH 2\nHEIGHT 1\nDEPTH {depth}\nMAXVAL {maxval}\nTUPLTYPE {tupltype}\nENDHDR\n"
            )
            .into_bytes();
            bytes.extend(raster);
            import_netpbm(&bytes).unwrap()
        };
        let rgba = pam(4, "RGB_ALPHA", 255, &[255, 0, 0, 128, 0, 0, 0, 0]);
        assert_eq!(px(&rgba.sprite, 0, 0), Rgba8::new(255, 0, 0, 128));
        assert_eq!(px(&rgba.sprite, 1, 0), Rgba8::TRANSPARENT);
        assert!(rgba.notes.is_empty());

        let rgb = pam(3, "RGB", 255, &[1, 2, 3, 4, 5, 6]);
        assert_eq!(px(&rgb.sprite, 1, 0), Rgba8::rgb(4, 5, 6));
        let grey_alpha = pam(2, "GRAYSCALE_ALPHA", 255, &[200, 255, 10, 0]);
        assert_eq!(px(&grey_alpha.sprite, 0, 0), GREY(200));
        assert_eq!(px(&grey_alpha.sprite, 1, 0), Rgba8::TRANSPARENT);
        // In PAM, unlike PBM, 1 is white.
        let bw = pam(1, "BLACKANDWHITE", 1, &[0, 1]);
        assert_eq!(
            (px(&bw.sprite, 0, 0), px(&bw.sprite, 1, 0)),
            (Rgba8::BLACK, Rgba8::WHITE)
        );
        let bwa = pam(2, "BLACKANDWHITE_ALPHA", 1, &[1, 1, 1, 0]);
        assert_eq!(
            (px(&bwa.sprite, 0, 0), px(&bwa.sprite, 1, 0)),
            (Rgba8::WHITE, Rgba8::TRANSPARENT)
        );

        // A kind ACMS does not know is read by its depth, and said.
        let odd = pam(3, "HSV", 255, &[1, 2, 3, 4, 5, 6]);
        assert_eq!(px(&odd.sprite, 0, 0), Rgba8::rgb(1, 2, 3));
        assert_eq!(odd.notes.len(), 1);
        assert!(odd.notes[0].contains("\"HSV\""), "{:?}", odd.notes);
    }

    #[test]
    fn several_pictures_in_one_file_are_frames() {
        let mut bytes = Vec::new();
        for value in [10_u8, 20, 30] {
            bytes.extend(b"P5 2 2 255\n");
            bytes.extend([value; 4]);
        }
        let sprite = read(&bytes);
        assert_eq!(sprite.frames().len(), 3);
        assert_eq!(sprite.pixel(LayerId(1), 2, 1, 1), GREY(30));

        // Kinds may be mixed; a picture of another size is left out, and said.
        bytes.extend(b"P1 2 2 1 1 0 0\n");
        bytes.extend(b"P5 3 1 255\n\x00\x00\x00");
        let imported = import_netpbm(&bytes).unwrap();
        assert_eq!(imported.sprite.frames().len(), 4);
        assert_eq!(imported.sprite.pixel(LayerId(1), 3, 0, 0), Rgba8::BLACK);
        assert_eq!(imported.notes.len(), 1);
        assert!(
            imported.notes[0].starts_with("1 pictures"),
            "{:?}",
            imported.notes
        );

        // Something else after good pictures: they stand, and the rest is said to be left out.
        let mut trailing = b"P5 1 1 255\n\x05".to_vec();
        trailing.extend(b"\nrubbish");
        let imported = import_netpbm(&trailing).unwrap();
        assert_eq!(imported.sprite.frames().len(), 1);
        assert_eq!(imported.notes.len(), 1);
    }

    #[test]
    fn what_is_not_a_picture_is_an_error_not_a_crash() {
        for bad in [
            &b""[..],
            b"P",
            b"P9 1 1 255\n\x00",
            b"GIF89a",
            b"P5 0 4 255\n",
            b"P5 1 1 0\n\x00",
            b"P5 1 1 70000\n\x00\x00",
            b"P5 2 2 255\n\x00",
            b"P2 2 1 10\n5 11\n",
            b"P2 2 1 10\n5",
            b"P3 1 1 255\n1 2 x\n",
            b"P1 2 1\n1 2\n",
            b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 3\nENDHDR\n\x00\x00\x00",
            b"P7\nWIDTH 1\nHEIGHT 1\nDEPTH 5\nMAXVAL 255\nENDHDR\n\x00\x00\x00\x00\x00",
            b"P7\nWIDTH 1\nHEIGHT 1\n",
            b"P5 99999 1 255\n",
            b"P5 1 1 255\n\x00\x00",
        ] {
            let _ = import_netpbm(bad);
        }
        assert!(import_netpbm(b"P5 2 2 255\n\x00").is_err());
        assert!(import_netpbm(b"P2 2 1 10\n5 11\n").is_err());
        assert!(matches!(
            import_netpbm(b"P5 5000 1 255\n"),
            Err(IoError::Unsupported(_))
        ));
        // Every sample cut short anywhere, and with bytes changed everywhere.
        for name in ["colour.ppm", "grey.pgm", "grey16.pgm", "bits.pbm"] {
            let bytes = sample(name);
            for cut in 0..bytes.len() {
                let _ = import_netpbm(&bytes[..cut]);
            }
            for at in 0..bytes.len() {
                let mut broken = bytes.clone();
                broken[at] ^= 0xFF;
                let _ = import_netpbm(&broken);
            }
        }
    }

    /// A sprite with the given pixels in one row.
    fn row_sprite(pixels: &[Rgba8]) -> Sprite {
        let mut document = Document::new(Sprite::new(pixels.len() as u32, 1).unwrap());
        let image = Image::from_pixels(pixels.len() as u32, 1, pixels.to_vec()).unwrap();
        let rect = Rect::new(0, 0, pixels.len() as u32, 1);
        document
            .transact("Paint", |tx| tx.set_pixels(LayerId(1), 0, rect, &image))
            .unwrap();
        document.sprite().clone()
    }

    #[test]
    fn each_kind_is_written_and_reads_back() {
        let pixels = [
            Rgba8::BLACK,
            Rgba8::WHITE,
            Rgba8::rgb(200, 10, 40),
            Rgba8::new(0, 0, 255, 128),
            Rgba8::TRANSPARENT,
        ];
        let sprite = row_sprite(&pixels);

        // PAM keeps everything.
        let (pam, report) = export_netpbm(&sprite, 0, NetpbmKind::Pam).unwrap();
        assert_eq!(report, NetpbmExport::default());
        assert!(pam.starts_with(
            b"P7\nWIDTH 5\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n"
        ));
        assert_eq!(picture(&read(&pam), 0), picture(&sprite, 0));

        // PPM: what was transparent is on white.
        let (ppm, report) = export_netpbm(&sprite, 0, NetpbmKind::Ppm).unwrap();
        assert!(ppm.starts_with(b"P6\n5 1\n255\n"));
        assert_eq!(
            report,
            NetpbmExport {
                on_white: 2,
                changed: 0
            }
        );
        let back = read(&ppm);
        assert_eq!(px(&back, 2, 0), Rgba8::rgb(200, 10, 40));
        assert_eq!(px(&back, 3, 0), Rgba8::rgb(127, 127, 255));
        assert_eq!(px(&back, 4, 0), Rgba8::WHITE);

        // PGM: grey, by how light each colour looks.
        let (pgm, report) = export_netpbm(&sprite, 0, NetpbmKind::Pgm).unwrap();
        assert!(pgm.starts_with(b"P5\n5 1\n255\n"));
        assert_eq!(
            report,
            NetpbmExport {
                on_white: 2,
                changed: 2
            }
        );
        let back = read(&pgm);
        assert_eq!(px(&back, 0, 0), Rgba8::BLACK);
        assert_eq!(px(&back, 2, 0), GREY(70));

        // PBM: black where it is dark, white where it is light.
        let (pbm, report) = export_netpbm(&sprite, 0, NetpbmKind::Pbm).unwrap();
        assert_eq!(pbm, b"P4\n5 1\n\xA0");
        assert_eq!(
            report,
            NetpbmExport {
                on_white: 2,
                changed: 2
            }
        );
    }

    #[test]
    fn pbm_rows_are_packed_and_padded() {
        let sprite = read(&sample("bits.pbm"));
        let (written, report) = export_netpbm(&sprite, 0, NetpbmKind::Pbm).unwrap();
        assert_eq!(report, NetpbmExport::default());
        // What Pillow wrote, byte for byte.
        assert_eq!(written, sample("bits.pbm"));
        let sprite = read(&sample("colour.ppm"));
        assert_eq!(
            export_netpbm(&sprite, 0, NetpbmKind::Ppm).unwrap().0,
            sample("colour.ppm")
        );
        let sprite = read(&sample("grey.pgm"));
        assert_eq!(
            export_netpbm(&sprite, 0, NetpbmKind::Pgm).unwrap().0,
            sample("grey.pgm")
        );
    }

    #[test]
    fn the_kind_comes_from_the_file_name() {
        let kind = |name: &str| NetpbmKind::from_path(Path::new(name));
        assert_eq!(kind("a.pbm"), Some(NetpbmKind::Pbm));
        assert_eq!(kind("a.PGM"), Some(NetpbmKind::Pgm));
        assert_eq!(kind("a.ppm"), Some(NetpbmKind::Ppm));
        assert_eq!(kind("a.pnm"), Some(NetpbmKind::Ppm));
        assert_eq!(kind("a.pam"), Some(NetpbmKind::Pam));
        assert_eq!(kind("a.png"), None);
        assert!(is_netpbm_path(Path::new("x/y.pam")));
        let sprite = Sprite::new(2, 2).unwrap();
        let path = std::env::temp_dir().join("acms-netpbm-no-kind.img");
        assert!(matches!(
            export_netpbm_to_path(&sprite, 0, &path),
            Err(IoError::Unsupported(_))
        ));
        assert!(export_netpbm(&sprite, 1, NetpbmKind::Ppm).is_err());
    }
}
