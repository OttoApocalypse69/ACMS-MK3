//! PNG reading, writing and export.

use std::io::{Cursor, Write};
use std::path::Path;

use acms_core::compositor;
use acms_core::image::Image;
use acms_core::sprite::Sprite;
use png::{BitDepth, ColorType, Transformations};

use crate::{IoError, write_atomically};

/// Largest factor [`export_png`] accepts.
pub const MAX_EXPORT_SCALE: u32 = 64;

/// Writes an image as an 8-bit RGBA PNG.
pub fn encode_png(image: &Image, writer: impl Write) -> Result<(), IoError> {
    let mut encoder = png::Encoder::new(writer, image.width(), image.height());
    encoder.set_color(ColorType::Rgba);
    encoder.set_depth(BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&image.to_rgba_bytes())?;
    writer.finish()?;
    Ok(())
}

/// Reads any PNG into an RGBA image. Palettes, greyscale and 16-bit samples are converted.
pub fn decode_png(bytes: &[u8]) -> Result<Image, IoError> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;

    let size = reader
        .output_buffer_size()
        .ok_or_else(|| IoError::Invalid("a PNG image is too large to load".to_owned()))?;
    let mut buffer = vec![0; size];
    let info = reader.next_frame(&mut buffer)?;
    let data = &buffer[..info.line_size * info.height as usize];

    let rgba: Vec<u8> = match info.color_type {
        ColorType::Rgba => data.to_vec(),
        ColorType::Rgb => data
            .chunks_exact(3)
            .flat_map(|px| [px[0], px[1], px[2], 255])
            .collect(),
        ColorType::GrayscaleAlpha => data
            .chunks_exact(2)
            .flat_map(|px| [px[0], px[0], px[0], px[1]])
            .collect(),
        ColorType::Grayscale => data.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        // `normalize_to_color8` expands palettes, so this only happens for a broken decoder.
        ColorType::Indexed => {
            return Err(IoError::Invalid(
                "a PNG image has a palette that could not be expanded".to_owned(),
            ));
        }
    };
    Image::from_rgba_bytes(info.width, info.height, &rgba)
        .ok_or_else(|| IoError::Invalid("a PNG image has the wrong amount of data".to_owned()))
}

/// Writes one flattened frame as a PNG, each sprite pixel becoming `scale × scale` pixels.
pub fn export_png(
    sprite: &Sprite,
    frame: usize,
    scale: u32,
    writer: impl Write,
) -> Result<(), IoError> {
    if !(1..=MAX_EXPORT_SCALE).contains(&scale) {
        return Err(IoError::Invalid(format!(
            "the export scale must be between 1 and {MAX_EXPORT_SCALE}, not {scale}"
        )));
    }
    sprite.check_cel(sprite.layers()[0].id, frame)?;
    let flat = compositor::compose(sprite, frame);
    encode_png(&flat.scaled_up(scale), writer)
}

/// [`export_png`] into a file.
pub fn export_png_to_path(
    sprite: &Sprite,
    frame: usize,
    scale: u32,
    path: &Path,
) -> Result<(), IoError> {
    write_atomically(path, |writer| export_png(sprite, frame, scale, writer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{TempPath, drawn_sprite};
    use acms_core::color::Rgba8;

    fn encoded(image: &Image) -> Vec<u8> {
        let mut bytes = Vec::new();
        encode_png(image, &mut bytes).unwrap();
        bytes
    }

    #[test]
    fn encode_then_decode_returns_the_same_image() {
        let image = compositor::compose(&drawn_sprite(), 0);
        assert_eq!(decode_png(&encoded(&image)).unwrap(), image);
    }

    #[test]
    fn translucent_and_transparent_pixels_survive() {
        let mut image = Image::new(3, 1);
        image.set(0, 0, Rgba8::new(10, 20, 30, 1));
        image.set(1, 0, Rgba8::new(255, 0, 255, 128));
        assert_eq!(decode_png(&encoded(&image)).unwrap(), image);
    }

    #[test]
    fn other_png_colour_types_are_converted() {
        let encode = |color: ColorType, data: &[u8]| {
            let mut bytes = Vec::new();
            let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
            encoder.set_color(color);
            encoder.set_depth(BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(data).unwrap();
            writer.finish().unwrap();
            bytes
        };
        let pixels = |bytes: Vec<u8>| decode_png(&bytes).unwrap().pixels().to_vec();

        assert_eq!(
            pixels(encode(ColorType::Rgb, &[1, 2, 3, 4, 5, 6])),
            [Rgba8::rgb(1, 2, 3), Rgba8::rgb(4, 5, 6)]
        );
        assert_eq!(
            pixels(encode(ColorType::Grayscale, &[7, 200])),
            [Rgba8::rgb(7, 7, 7), Rgba8::rgb(200, 200, 200)]
        );
        assert_eq!(
            pixels(encode(ColorType::GrayscaleAlpha, &[7, 9, 200, 0])),
            [Rgba8::new(7, 7, 7, 9), Rgba8::new(200, 200, 200, 0)]
        );
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode_png(b"definitely not a png").is_err());
        assert!(decode_png(&[]).is_err());
        let mut truncated = encoded(&Image::new(8, 8));
        truncated.truncate(truncated.len() / 2);
        assert!(decode_png(&truncated).is_err());
    }

    #[test]
    fn export_scales_by_whole_factors() {
        let sprite = drawn_sprite();
        let flat = compositor::compose(&sprite, 0);

        let mut bytes = Vec::new();
        export_png(&sprite, 0, 1, &mut bytes).unwrap();
        assert_eq!(decode_png(&bytes).unwrap(), flat);

        let mut bytes = Vec::new();
        export_png(&sprite, 0, 4, &mut bytes).unwrap();
        let big = decode_png(&bytes).unwrap();
        assert_eq!((big.width(), big.height()), (24 * 4, 16 * 4));
        for (x, y) in [(2, 2), (12, 7), (22, 14), (0, 0)] {
            for (dx, dy) in [(0, 0), (3, 0), (0, 3), (3, 3)] {
                assert_eq!(big.get(x * 4 + dx, y * 4 + dy), flat.get(x, y));
            }
        }
    }

    #[test]
    fn export_rejects_bad_scales_and_frames() {
        let sprite = drawn_sprite();
        assert!(export_png(&sprite, 0, 0, Vec::new()).is_err());
        assert!(export_png(&sprite, 0, MAX_EXPORT_SCALE + 1, Vec::new()).is_err());
        assert!(export_png(&sprite, 5, 1, Vec::new()).is_err());
    }

    #[test]
    fn export_to_a_file_leaves_no_temporary_file_behind() {
        let target = TempPath::new("export.png");
        let sprite = drawn_sprite();
        export_png_to_path(&sprite, 0, 2, target.path()).unwrap();

        let bytes = std::fs::read(target.path()).unwrap();
        assert_eq!(decode_png(&bytes).unwrap().width(), 48);
        let mut temp = target.path().as_os_str().to_owned();
        temp.push(".tmp");
        assert!(!Path::new(&temp).exists());
    }
}
