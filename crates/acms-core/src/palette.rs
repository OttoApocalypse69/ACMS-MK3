//! Palette tools: sorting, ramps, finding the nearest colour, building a palette from a picture.
//!
//! These all return a new [`Palette`]; putting it on a sprite is a transaction
//! ([`Transaction::set_palette`](crate::document::Transaction::set_palette)).

use crate::color::Rgba8;
use crate::image::Image;
use crate::sprite::{Palette, PaletteEntry, Ramp};

/// Most colours a palette is given when it is made from a picture or a file.
pub const MAX_PALETTE_COLORS: usize = 256;

/// Most colours a generated ramp can have.
pub const MAX_RAMP_STEPS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    /// Greys first, then around the colour wheel; within a hue from dark to light.
    Hue,
    /// From dark to light.
    Brightness,
}

/// How bright a colour looks, from 0 to 255 000.
pub fn brightness(color: Rgba8) -> u32 {
    299 * u32::from(color.r) + 587 * u32::from(color.g) + 114 * u32::from(color.b)
}

/// The hue of a colour in degrees (0 to 359), or `None` for a grey.
pub fn hue(color: Rgba8) -> Option<u32> {
    let (r, g, b) = (i32::from(color.r), i32::from(color.g), i32::from(color.b));
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let spread = max - min;
    if spread == 0 {
        return None;
    }
    // Sixty degrees per sector of the colour wheel, worked out in whole numbers.
    let degrees = if max == r {
        60 * (g - b) / spread
    } else if max == g {
        120 + 60 * (b - r) / spread
    } else {
        240 + 60 * (r - g) / spread
    };
    Some(degrees.rem_euclid(360) as u32)
}

/// The palette with its colours in another order. Ramps keep their own order.
pub fn sorted(palette: &Palette, key: SortKey) -> Palette {
    let mut entries = palette.entries().to_vec();
    // A stable sort, so colours that compare equal stay in the order the user had them.
    match key {
        SortKey::Brightness => entries.sort_by_key(|entry| brightness(entry.color)),
        SortKey::Hue => entries.sort_by_key(|entry| {
            let hue = hue(entry.color);
            (hue.is_some(), hue, brightness(entry.color))
        }),
    }
    Palette::new(entries).with_ramps(palette.ramps().to_vec())
}

/// `steps` colours evenly spaced from `from` to `to`, both included. `steps` is clamped to
/// `2..=MAX_RAMP_STEPS`.
pub fn ramp_between(from: Rgba8, to: Rgba8, steps: usize) -> Ramp {
    let steps = steps.clamp(2, MAX_RAMP_STEPS);
    let last = (steps - 1) as i32;
    let mix = |a: u8, b: u8, step: i32| {
        let (a, b) = (i32::from(a), i32::from(b));
        // Rounded to the nearest whole value.
        (a + ((b - a) * step * 2 + last * (b - a).signum()) / (2 * last)) as u8
    };
    (0..=last)
        .map(|step| {
            Rgba8::new(
                mix(from.r, to.r, step),
                mix(from.g, to.g, step),
                mix(from.b, to.b, step),
                mix(from.a, to.a, step),
            )
        })
        .collect()
}

/// The palette with `color` added at the end, unless it is already in it.
pub fn with_color(palette: &Palette, color: Rgba8) -> Palette {
    let mut entries = palette.entries().to_vec();
    if !palette.contains(color) {
        entries.push(PaletteEntry { color, name: None });
    }
    Palette::new(entries).with_ramps(palette.ramps().to_vec())
}

/// The palette with the entry at `from` moved so that it ends up at position `to`. Positions
/// that do not exist leave the palette as it is.
pub fn with_moved(palette: &Palette, from: usize, to: usize) -> Palette {
    let mut entries = palette.entries().to_vec();
    if from < entries.len() && to < entries.len() {
        let entry = entries.remove(from);
        entries.insert(to, entry);
    }
    Palette::new(entries).with_ramps(palette.ramps().to_vec())
}

/// The palette without the entries at `indices`. Ramps lose those colours too, and a ramp
/// left with fewer than two colours goes.
pub fn without_entries(palette: &Palette, indices: &[usize]) -> Palette {
    let entries: Vec<PaletteEntry> = palette
        .entries()
        .iter()
        .enumerate()
        .filter(|(index, _)| !indices.contains(index))
        .map(|(_, entry)| entry.clone())
        .collect();
    let kept = |color: &Rgba8| entries.iter().any(|entry| entry.color == *color);
    let ramps = palette
        .ramps()
        .iter()
        .map(|ramp| ramp.iter().copied().filter(|color| kept(color)).collect())
        .collect();
    Palette::new(entries).with_ramps(ramps)
}

/// The palette with one more ramp. Colours of the ramp that the palette does not have yet
/// are added to it; a colour is only kept once in a ramp. `None` if that leaves fewer than
/// two colours.
pub fn with_ramp(palette: &Palette, colors: &[Rgba8]) -> Option<Palette> {
    let mut ramp: Ramp = Vec::with_capacity(colors.len());
    for color in colors.iter().map(|color| color.normalized()) {
        if !ramp.contains(&color) {
            ramp.push(color);
        }
    }
    if ramp.len() < 2 {
        return None;
    }
    let mut entries = palette.entries().to_vec();
    for &color in &ramp {
        if !entries.iter().any(|entry| entry.color == color) {
            entries.push(PaletteEntry { color, name: None });
        }
    }
    let mut ramps = palette.ramps().to_vec();
    ramps.push(ramp);
    Some(Palette::new(entries).with_ramps(ramps))
}

/// The palette without the ramp at `index`. Its colours stay.
pub fn without_ramp(palette: &Palette, index: usize) -> Palette {
    let mut ramps = palette.ramps().to_vec();
    if index < ramps.len() {
        ramps.remove(index);
    }
    Palette::new(palette.entries().to_vec()).with_ramps(ramps)
}

/// The palette with every `from` turned into `to`, in the colours and in the ramps.
pub fn with_replaced(palette: &Palette, from: Rgba8, to: Rgba8) -> Palette {
    let swap = |color: Rgba8| if color == from { to } else { color };
    let mut entries: Vec<PaletteEntry> = Vec::with_capacity(palette.len());
    for entry in palette.entries() {
        let color = swap(entry.color);
        // Replacing by a colour the palette already has must not list it twice.
        if !entries.iter().any(|kept| kept.color == color) {
            entries.push(PaletteEntry {
                color,
                name: entry.name.clone(),
            });
        }
    }
    let ramps = palette
        .ramps()
        .iter()
        .map(|ramp| {
            let mut swapped: Ramp = Vec::with_capacity(ramp.len());
            for color in ramp.iter().map(|color| swap(*color)) {
                if !swapped.contains(&color) {
                    swapped.push(color);
                }
            }
            swapped
        })
        .collect();
    Palette::new(entries).with_ramps(ramps)
}

/// The palette colour closest to `color`, going by red, green and blue. `None` for an empty
/// palette.
pub fn nearest(palette: &Palette, color: Rgba8) -> Option<Rgba8> {
    let distance = |entry: Rgba8| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        // Weighted roughly by how sensitive the eye is to each channel.
        3 * d(entry.r, color.r) + 4 * d(entry.g, color.g) + 2 * d(entry.b, color.b)
    };
    palette
        .colors()
        .filter(|entry| entry.a != 0)
        .min_by_key(|entry| distance(*entry))
}

/// The distinct colours of a picture, in the order they first appear from the top left, up
/// to [`MAX_PALETTE_COLORS`]. Transparency is ignored: a colour counts as if it were opaque.
/// Returns the palette and whether the picture had more colours than fit.
pub fn from_image(image: &Image) -> (Palette, bool) {
    let mut seen = std::collections::HashSet::new();
    let mut colors = Vec::new();
    let mut more = false;
    for pixel in image.pixels() {
        if pixel.a == 0 {
            continue;
        }
        let color = Rgba8::rgb(pixel.r, pixel.g, pixel.b);
        if seen.insert(color) {
            if colors.len() == MAX_PALETTE_COLORS {
                more = true;
                break;
            }
            colors.push(color);
        }
    }
    (Palette::from_colors(colors), more)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgba8 = Rgba8::rgb(255, 0, 0);
    const GREEN: Rgba8 = Rgba8::rgb(0, 255, 0);
    const BLUE: Rgba8 = Rgba8::rgb(0, 0, 255);
    const GREY: Rgba8 = Rgba8::rgb(128, 128, 128);

    fn colors(palette: &Palette) -> Vec<Rgba8> {
        palette.colors().collect()
    }

    #[test]
    fn hue_and_brightness() {
        assert_eq!(hue(RED), Some(0));
        assert_eq!(hue(GREEN), Some(120));
        assert_eq!(hue(BLUE), Some(240));
        assert_eq!(hue(Rgba8::rgb(255, 255, 0)), Some(60));
        assert_eq!(hue(Rgba8::rgb(255, 0, 255)), Some(300));
        assert_eq!(hue(GREY), None);
        assert!(brightness(Rgba8::WHITE) > brightness(GREEN));
        assert!(brightness(GREEN) > brightness(RED));
        assert!(brightness(RED) > brightness(BLUE));
        assert_eq!(brightness(Rgba8::BLACK), 0);
    }

    #[test]
    fn sorting_reorders_colours_and_keeps_ramps() {
        let dark_red = Rgba8::rgb(100, 0, 0);
        let palette = Palette::from_colors([BLUE, RED, Rgba8::WHITE, GREEN, dark_red, GREY])
            .with_ramps(vec![vec![RED, dark_red]]);

        let by_hue = sorted(&palette, SortKey::Hue);
        assert_eq!(
            colors(&by_hue),
            [GREY, Rgba8::WHITE, dark_red, RED, GREEN, BLUE]
        );
        assert_eq!(by_hue.ramps(), [vec![RED, dark_red]]);

        let by_brightness = sorted(&palette, SortKey::Brightness);
        assert_eq!(
            colors(&by_brightness),
            [BLUE, dark_red, RED, GREY, GREEN, Rgba8::WHITE]
        );
    }

    #[test]
    fn a_generated_ramp_runs_evenly_between_its_ends() {
        let ramp = ramp_between(Rgba8::BLACK, Rgba8::WHITE, 5);
        let greys: Vec<u8> = ramp.iter().map(|color| color.r).collect();
        assert_eq!(greys, [0, 64, 128, 191, 255]);
        assert!(ramp.iter().all(|color| color.a == 255));

        let down = ramp_between(Rgba8::WHITE, Rgba8::BLACK, 5);
        let greys: Vec<u8> = down.iter().map(|color| color.r).collect();
        assert_eq!(greys, [255, 191, 127, 64, 0]);

        assert_eq!(ramp_between(RED, BLUE, 0), [RED, BLUE]);
        assert_eq!(ramp_between(RED, BLUE, 1000).len(), MAX_RAMP_STEPS);
        assert_eq!(ramp_between(RED, BLUE, 3)[1], Rgba8::rgb(127, 0, 128));
    }

    #[test]
    fn adding_and_removing_colours() {
        let palette = Palette::from_colors([RED, GREEN]).with_ramps(vec![vec![RED, GREEN]]);
        assert_eq!(colors(&with_color(&palette, BLUE)), [RED, GREEN, BLUE]);
        assert_eq!(with_color(&palette, RED), palette);

        // A ramp that loses a colour and has only one left goes too.
        let smaller = without_entries(&palette, &[0]);
        assert_eq!(colors(&smaller), [GREEN]);
        assert!(smaller.ramps().is_empty());
        assert_eq!(without_entries(&palette, &[9]), palette);
    }

    #[test]
    fn moving_a_colour_keeps_the_others_in_order() {
        let palette =
            Palette::from_colors([RED, GREEN, BLUE, GREY]).with_ramps(vec![vec![RED, GREY]]);
        assert_eq!(
            colors(&with_moved(&palette, 0, 2)),
            [GREEN, BLUE, RED, GREY]
        );
        assert_eq!(
            colors(&with_moved(&palette, 3, 0)),
            [GREY, RED, GREEN, BLUE]
        );
        assert_eq!(with_moved(&palette, 1, 1), palette);
        assert_eq!(with_moved(&palette, 1, 9), palette);
        assert_eq!(with_moved(&palette, 0, 3).ramps(), palette.ramps());
    }

    #[test]
    fn ramps_bring_their_colours_into_the_palette() {
        let palette = Palette::from_colors([RED]);
        let with = with_ramp(&palette, &[RED, GREEN, GREEN, BLUE]).unwrap();
        assert_eq!(colors(&with), [RED, GREEN, BLUE]);
        assert_eq!(with.ramps(), [vec![RED, GREEN, BLUE]]);
        assert_eq!(with_ramp(&palette, &[RED, RED]), None);

        let without = without_ramp(&with, 0);
        assert!(without.ramps().is_empty());
        assert_eq!(colors(&without), [RED, GREEN, BLUE]);
        assert_eq!(without_ramp(&with, 5), with);
    }

    #[test]
    fn replacing_a_colour_never_lists_one_twice() {
        let palette = Palette::from_colors([RED, GREEN, BLUE]).with_ramps(vec![vec![RED, GREEN]]);
        let replaced = with_replaced(&palette, RED, GREY);
        assert_eq!(colors(&replaced), [GREY, GREEN, BLUE]);
        assert_eq!(replaced.ramps(), [vec![GREY, GREEN]]);

        // Into a colour that is already there: one entry is left, where the first one was.
        let merged = with_replaced(&palette, RED, BLUE);
        assert_eq!(colors(&merged), [BLUE, GREEN]);
        let merged = with_replaced(&palette, BLUE, RED);
        assert_eq!(colors(&merged), [RED, GREEN]);
        // A ramp whose two colours became one is dropped.
        assert!(with_replaced(&palette, RED, GREEN).ramps().is_empty());
    }

    #[test]
    fn nearest_colour() {
        let palette = Palette::from_colors([Rgba8::BLACK, Rgba8::WHITE, RED]);
        assert_eq!(nearest(&palette, Rgba8::rgb(200, 30, 20)), Some(RED));
        assert_eq!(
            nearest(&palette, Rgba8::rgb(20, 30, 20)),
            Some(Rgba8::BLACK)
        );
        assert_eq!(
            nearest(&palette, Rgba8::rgb(230, 240, 220)),
            Some(Rgba8::WHITE)
        );
        assert_eq!(nearest(&Palette::default(), RED), None);
    }

    #[test]
    fn a_palette_from_a_picture_lists_each_colour_once() {
        let mut image = Image::new(3, 2);
        image.set(0, 0, RED);
        image.set(1, 0, BLUE);
        image.set(2, 0, RED);
        image.set(0, 1, Rgba8::new(0, 0, 255, 90));
        image.set(1, 1, GREEN);
        let (palette, more) = from_image(&image);
        assert_eq!(colors(&palette), [RED, BLUE, GREEN]);
        assert!(!more);

        // More colours than fit: the first ones are kept and the caller is told.
        let pixels = (0..300_u32)
            .map(|n| Rgba8::rgb(n as u8, (n >> 8) as u8, 7))
            .collect();
        let image = Image::from_pixels(300, 1, pixels).unwrap();
        let (palette, more) = from_image(&image);
        assert_eq!(palette.len(), MAX_PALETTE_COLORS);
        assert!(more);
    }
}
