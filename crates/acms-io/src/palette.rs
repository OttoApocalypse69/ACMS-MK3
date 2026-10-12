//! Palette files: GIMP `.gpl`, plain `.hex` lists, JASC `.pal`, and the colours of a PNG.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use acms_core::color::Rgba8;
use acms_core::palette::{self, MAX_PALETTE_COLORS};
use acms_core::sprite::{Palette, PaletteEntry};

use crate::png::decode_png;
use crate::{IoError, write_atomically};

/// The text formats a palette can be read from and written to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteFormat {
    /// GIMP palette: a header, then `R G B name` lines.
    Gpl,
    /// One `RRGGBB` per line, as Lospec exports them.
    Hex,
    /// JASC / Paint Shop Pro: a header with the count, then `R G B` lines.
    Pal,
}

impl PaletteFormat {
    pub const ALL: [Self; 3] = [Self::Gpl, Self::Hex, Self::Pal];

    pub fn extension(self) -> &'static str {
        match self {
            Self::Gpl => "gpl",
            Self::Hex => "hex",
            Self::Pal => "pal",
        }
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|format| format.extension() == extension)
    }
}

fn invalid(line: usize, what: &str) -> IoError {
    IoError::Invalid(format!("line {line}: {what}"))
}

/// Three whole numbers from 0 to 255 at the start of a line, and whatever follows them.
fn rgb_line(line: &str) -> Option<(Rgba8, &str)> {
    let mut rest = line.trim_start();
    let mut channels = [0_u8; 3];
    for channel in &mut channels {
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        *channel = rest[..end].parse().ok()?;
        rest = rest[end..].trim_start();
    }
    let [r, g, b] = channels;
    Some((Rgba8::rgb(r, g, b), rest))
}

/// Reads a palette from the text of a palette file. More than [`MAX_PALETTE_COLORS`] colours
/// is an error rather than a silent cut.
pub fn parse_palette(text: &str, format: PaletteFormat) -> Result<Palette, IoError> {
    let text = text.trim_start_matches('\u{feff}');
    let lines = text.lines().map(str::trim).enumerate();
    let mut entries: Vec<PaletteEntry> = Vec::new();

    match format {
        PaletteFormat::Gpl => {
            for (index, line) in lines {
                if index == 0 {
                    if line != "GIMP Palette" {
                        return Err(invalid(1, "a GIMP palette starts with `GIMP Palette`"));
                    }
                    continue;
                }
                let header = ["Name:", "Columns:", "Channels:"];
                if line.is_empty()
                    || line.starts_with('#')
                    || header.iter().any(|key| line.starts_with(key))
                {
                    continue;
                }
                let (color, name) =
                    rgb_line(line).ok_or_else(|| invalid(index + 1, "expected `R G B name`"))?;
                let name = (!name.is_empty() && name != "Untitled").then(|| name.to_owned());
                entries.push(PaletteEntry { color, name });
            }
        }
        PaletteFormat::Hex => {
            for (index, line) in lines {
                if line.is_empty() || line.starts_with(';') || line.starts_with("//") {
                    continue;
                }
                let color = Rgba8::from_hex(line)
                    .ok_or_else(|| invalid(index + 1, "expected a colour like `FF8800`"))?;
                entries.push(PaletteEntry { color, name: None });
            }
        }
        PaletteFormat::Pal => {
            let mut expected = None;
            for (index, line) in lines {
                match index {
                    0 if line != "JASC-PAL" => {
                        return Err(invalid(1, "a JASC palette starts with `JASC-PAL`"));
                    }
                    0 | 1 => continue,
                    2 => {
                        let count: usize = line
                            .parse()
                            .map_err(|_| invalid(3, "expected the number of colours"))?;
                        expected = Some(count);
                        continue;
                    }
                    _ if line.is_empty() => continue,
                    _ => {}
                }
                let (color, _) =
                    rgb_line(line).ok_or_else(|| invalid(index + 1, "expected `R G B`"))?;
                entries.push(PaletteEntry { color, name: None });
            }
            match expected {
                Some(count) if count == entries.len() => {}
                Some(count) => {
                    return Err(IoError::Invalid(format!(
                        "the header promises {count} colours but there are {}",
                        entries.len()
                    )));
                }
                None => return Err(invalid(3, "the number of colours is missing")),
            }
        }
    }

    if entries.is_empty() {
        return Err(IoError::Invalid("there are no colours in it".to_owned()));
    }
    if entries.len() > MAX_PALETTE_COLORS {
        return Err(IoError::Unsupported(format!(
            "the palette has {} colours; at most {MAX_PALETTE_COLORS} can be imported",
            entries.len()
        )));
    }
    Ok(Palette::new(entries))
}

/// Writes a palette as the text of a palette file. None of the formats can hold
/// transparency or ramps: colours are written opaque, and ramps are left out.
pub fn format_palette(palette: &Palette, format: PaletteFormat, name: &str) -> String {
    let mut out = String::new();
    // Writing to a `String` cannot fail.
    match format {
        PaletteFormat::Gpl => {
            let columns = palette.len().clamp(1, 16);
            let _ = writeln!(out, "GIMP Palette\nName: {name}\nColumns: {columns}\n#");
            for entry in palette.entries() {
                let Rgba8 { r, g, b, .. } = entry.color;
                let label = entry.name.as_deref().unwrap_or("Untitled");
                let _ = writeln!(out, "{r:>3} {g:>3} {b:>3}\t{label}");
            }
        }
        PaletteFormat::Hex => {
            for Rgba8 { r, g, b, .. } in palette.colors() {
                let _ = writeln!(out, "{r:02x}{g:02x}{b:02x}");
            }
        }
        PaletteFormat::Pal => {
            let _ = writeln!(out, "JASC-PAL\n0100\n{}", palette.len());
            for Rgba8 { r, g, b, .. } in palette.colors() {
                let _ = writeln!(out, "{r} {g} {b}");
            }
        }
    }
    out
}

/// What [`load_palette`] found.
#[derive(Debug, PartialEq, Eq)]
pub struct LoadedPalette {
    pub palette: Palette,
    /// The file was a picture with more colours than a palette holds; only the first
    /// [`MAX_PALETTE_COLORS`] were taken.
    pub truncated: bool,
}

/// Reads a palette from a `.gpl`, `.hex` or `.pal` file, or takes the colours of a `.png`.
pub fn load_palette(path: &Path) -> Result<LoadedPalette, IoError> {
    let is_png = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
    if is_png {
        let image = decode_png(&std::fs::read(path)?)?;
        let (palette, truncated) = palette::from_image(&image);
        if palette.is_empty() {
            return Err(IoError::Invalid("the picture is empty".to_owned()));
        }
        return Ok(LoadedPalette { palette, truncated });
    }

    let format = PaletteFormat::from_path(path).ok_or_else(|| {
        IoError::Unsupported("palettes are read from .gpl, .hex, .pal and .png files".to_owned())
    })?;
    let bytes = std::fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(LoadedPalette {
        palette: parse_palette(&text, format)?,
        truncated: false,
    })
}

/// Writes a palette to a `.gpl`, `.hex` or `.pal` file, chosen by the file's extension.
pub fn save_palette(palette: &Palette, path: &Path) -> Result<(), IoError> {
    let format = PaletteFormat::from_path(path).ok_or_else(|| {
        IoError::Unsupported("palettes are written as .gpl, .hex or .pal files".to_owned())
    })?;
    let name = path
        .file_stem()
        .map_or_else(|| "Palette".into(), |stem| stem.to_string_lossy());
    let text = format_palette(palette, format, &name);
    write_atomically(path, |writer| Ok(writer.write_all(text.as_bytes())?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::png::encode_png;
    use crate::test_util::TempPath;
    use acms_core::image::Image;

    const RED: Rgba8 = Rgba8::rgb(255, 0, 0);
    const TEAL: Rgba8 = Rgba8::rgb(0x2A, 0x9D, 0x9C);

    fn colors(palette: &Palette) -> Vec<Rgba8> {
        palette.colors().collect()
    }

    fn sample() -> Palette {
        Palette::new(vec![
            PaletteEntry {
                color: RED,
                name: Some("Signal red".to_owned()),
            },
            PaletteEntry {
                color: TEAL,
                name: None,
            },
            PaletteEntry {
                color: Rgba8::BLACK,
                name: None,
            },
        ])
    }

    #[test]
    fn every_format_round_trips_its_colours() {
        for format in PaletteFormat::ALL {
            let text = format_palette(&sample(), format, "Sample");
            let parsed = parse_palette(&text, format).unwrap();
            assert_eq!(colors(&parsed), colors(&sample()), "{format:?}");
        }
        // Only GIMP palettes have names.
        let text = format_palette(&sample(), PaletteFormat::Gpl, "Sample");
        assert_eq!(parse_palette(&text, PaletteFormat::Gpl).unwrap(), sample());
    }

    #[test]
    fn the_files_look_like_other_programs_write_them() {
        assert_eq!(
            format_palette(&sample(), PaletteFormat::Gpl, "Sample"),
            "GIMP Palette\nName: Sample\nColumns: 3\n#\n255   0   0\tSignal red\n \
             42 157 156\tUntitled\n  0   0   0\tUntitled\n"
        );
        assert_eq!(
            format_palette(&sample(), PaletteFormat::Hex, "Sample"),
            "ff0000\n2a9d9c\n000000\n"
        );
        assert_eq!(
            format_palette(&sample(), PaletteFormat::Pal, "Sample"),
            "JASC-PAL\n0100\n3\n255 0 0\n42 157 156\n0 0 0\n"
        );
    }

    #[test]
    fn files_written_by_other_programs_are_read() {
        let gimp = "GIMP Palette\r\nName: Sunset\r\nColumns: 4\r\n# a comment\r\n\r\n255 0 0\tred\r\n  \
                    42\t157\t156\r\n0 0 0 Index 2\r\n";
        let palette = parse_palette(gimp, PaletteFormat::Gpl).unwrap();
        assert_eq!(colors(&palette), [RED, TEAL, Rgba8::BLACK]);
        assert_eq!(palette.entries()[0].name.as_deref(), Some("red"));
        assert_eq!(palette.entries()[1].name, None);
        assert_eq!(palette.entries()[2].name.as_deref(), Some("Index 2"));

        let hex = "\u{feff}; from lospec\n#FF0000\n2a9d9c\n\n000000\n";
        let palette = parse_palette(hex, PaletteFormat::Hex).unwrap();
        assert_eq!(colors(&palette), [RED, TEAL, Rgba8::BLACK]);

        let jasc = "JASC-PAL\n0100\n2\n255 0 0\n42 157 156\n";
        let palette = parse_palette(jasc, PaletteFormat::Pal).unwrap();
        assert_eq!(colors(&palette), [RED, TEAL]);
    }

    #[test]
    fn broken_files_are_errors_that_say_where() {
        let line = |result: Result<Palette, IoError>| result.unwrap_err().to_string();
        assert!(line(parse_palette("255 0 0", PaletteFormat::Gpl)).contains("line 1"));
        assert!(
            line(parse_palette(
                "GIMP Palette\n255 0 0\n300 0 0\n",
                PaletteFormat::Gpl
            ))
            .contains("line 3")
        );
        assert!(line(parse_palette("ff0000\nnope\n", PaletteFormat::Hex)).contains("line 2"));
        assert!(
            line(parse_palette(
                "JASC-PAL\n0100\n3\n1 2 3\n",
                PaletteFormat::Pal
            ))
            .contains("promises 3")
        );
        assert!(line(parse_palette("JASC-PAL\n0100\n", PaletteFormat::Pal)).contains("missing"));
        assert!(line(parse_palette("", PaletteFormat::Hex)).contains("no colours"));

        let many: String = (0..300).map(|n| format!("{n:06x}\n")).collect();
        assert!(matches!(
            parse_palette(&many, PaletteFormat::Hex),
            Err(IoError::Unsupported(_))
        ));
    }

    #[test]
    fn palettes_are_saved_and_loaded_by_extension() {
        for extension in ["gpl", "hex", "PAL"] {
            let target = TempPath::new(&format!("palette.{extension}"));
            save_palette(&sample(), target.path()).unwrap();
            let loaded = load_palette(target.path()).unwrap();
            assert_eq!(colors(&loaded.palette), colors(&sample()));
            assert!(!loaded.truncated);
        }

        let target = TempPath::new("palette.txt");
        assert!(matches!(
            save_palette(&sample(), target.path()),
            Err(IoError::Unsupported(_))
        ));
        std::fs::write(target.path(), "ff0000").unwrap();
        assert!(matches!(
            load_palette(target.path()),
            Err(IoError::Unsupported(_))
        ));
        let missing = TempPath::new("missing.gpl");
        assert!(matches!(load_palette(missing.path()), Err(IoError::Io(_))));
    }

    #[test]
    fn a_png_gives_its_colours() {
        let mut image = Image::new(2, 2);
        image.set(0, 0, TEAL);
        image.set(1, 0, RED);
        image.set(1, 1, TEAL);
        let mut bytes = Vec::new();
        encode_png(&image, &mut bytes).unwrap();
        let target = TempPath::new("palette.png");
        std::fs::write(target.path(), bytes).unwrap();

        let loaded = load_palette(target.path()).unwrap();
        assert_eq!(colors(&loaded.palette), [TEAL, RED]);

        let mut bytes = Vec::new();
        encode_png(&Image::new(2, 2), &mut bytes).unwrap();
        std::fs::write(target.path(), bytes).unwrap();
        assert!(matches!(
            load_palette(target.path()),
            Err(IoError::Invalid(_))
        ));
    }
}
