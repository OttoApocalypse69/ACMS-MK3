//! Dither patterns: which of two colours a pixel gets (see `PLAN.md`, section 4.4).

use std::sync::Arc;

use crate::selection::Mask;

/// Highest value of the density setting; it is a percentage.
pub const MAX_DENSITY: u8 = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pattern {
    /// Ordered dither with a 2×2 threshold matrix: five distinct densities.
    Bayer2,
    /// 4×4: seventeen densities.
    Bayer4,
    /// 8×8: sixty-five densities.
    Bayer8,
    /// Every other pixel, whatever the density.
    Checker,
    /// A user-made tile: the pixels in the mask get the first colour.
    Custom(Arc<Mask>),
}

/// The threshold of a pixel in the Bayer matrix of side `2^bits`, from 0 to `4^bits - 1`.
fn bayer(x: i32, y: i32, bits: u32) -> u32 {
    let (x, y) = (x as u32, y as u32);
    (0..bits).fold(0, |value, bit| {
        let (xb, yb) = ((x >> bit) & 1, (y >> bit) & 1);
        (value << 2) | ((xb ^ yb) << 1) | yb
    })
}

impl Pattern {
    /// The patterns that need nothing from the user.
    pub const BUILT_IN: [Self; 4] = [Self::Bayer2, Self::Bayer4, Self::Bayer8, Self::Checker];

    /// Stable lower-case name, used in command arguments.
    pub fn id(&self) -> &'static str {
        match self {
            Self::Bayer2 => "bayer2",
            Self::Bayer4 => "bayer4",
            Self::Bayer8 => "bayer8",
            Self::Checker => "checker",
            Self::Custom(_) => "custom",
        }
    }

    /// Name shown in the UI.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Bayer2 => "Bayer 2×2",
            Self::Bayer4 => "Bayer 4×4",
            Self::Bayer8 => "Bayer 8×8",
            Self::Checker => "Checker",
            Self::Custom(_) => "Custom",
        }
    }

    pub fn built_in(id: &str) -> Option<Self> {
        Self::BUILT_IN
            .into_iter()
            .find(|pattern| pattern.id() == id)
    }

    /// A pattern from a tile; `None` if the tile has no pixels at all.
    pub fn custom(tile: Mask) -> Option<Self> {
        (tile.width() > 0 && tile.height() > 0).then(|| Self::Custom(Arc::new(tile)))
    }

    /// Whether the density setting changes what the pattern looks like.
    pub fn uses_density(&self) -> bool {
        matches!(self, Self::Bayer2 | Self::Bayer4 | Self::Bayer8)
    }

    /// Whether the pixel at a canvas position gets the first of the two colours. `density` is
    /// the share of pixels that do, in percent.
    pub fn picks_first(&self, x: i32, y: i32, density: u8) -> bool {
        let ordered = |bits: u32| {
            let cells = 1_u32 << (2 * bits);
            let density = u32::from(density.min(MAX_DENSITY));
            // (threshold + ½) / cells < density / 100
            (2 * bayer(x, y, bits) + 1) * 100 < density * 2 * cells
        };
        match self {
            Self::Bayer2 => ordered(1),
            Self::Bayer4 => ordered(2),
            Self::Bayer8 => ordered(3),
            Self::Checker => (x + y).rem_euclid(2) == 0,
            Self::Custom(tile) => tile.contains(
                x.rem_euclid(tile.width() as i32),
                y.rem_euclid(tile.height() as i32),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rect;

    fn rows(pattern: &Pattern, size: i32, density: u8) -> Vec<String> {
        (0..size)
            .map(|y| {
                (0..size)
                    .map(|x| {
                        if pattern.picks_first(x, y, density) {
                            '#'
                        } else {
                            '.'
                        }
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn bayer_matrices_are_the_standard_ones() {
        let matrix = |bits: u32| -> Vec<Vec<u32>> {
            let side = 1 << bits;
            (0..side)
                .map(|y| (0..side).map(|x| bayer(x, y, bits)).collect())
                .collect()
        };
        assert_eq!(matrix(1), [[0, 2], [3, 1]]);
        assert_eq!(
            matrix(2),
            [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]]
        );
        // Every threshold appears exactly once.
        let mut all: Vec<u32> = matrix(3).into_iter().flatten().collect();
        all.sort_unstable();
        assert_eq!(all, (0..64).collect::<Vec<_>>());
    }

    #[test]
    fn density_sets_the_share_of_first_colour_pixels() {
        for (pattern, side) in [
            (Pattern::Bayer2, 2),
            (Pattern::Bayer4, 4),
            (Pattern::Bayer8, 8),
        ] {
            let count =
                |density| rows(&pattern, side, density).concat().matches('#').count() as i32;
            assert_eq!(count(0), 0);
            assert_eq!(count(100), side * side);
            assert_eq!(count(50), side * side / 2);
            assert_eq!(count(25), side * side / 4);
            // More density never takes a pixel away.
            for density in 0..100_u8 {
                let (low, high) = (
                    rows(&pattern, side, density),
                    rows(&pattern, side, density + 1),
                );
                for (a, b) in low.concat().chars().zip(high.concat().chars()) {
                    assert!(a != '#' || b == '#');
                }
            }
        }
        assert_eq!(
            rows(&Pattern::Bayer2, 4, 50),
            ["#.#.", ".#.#", "#.#.", ".#.#"]
        );
        assert_eq!(
            rows(&Pattern::Bayer4, 4, 25),
            ["#.#.", "....", "#.#.", "...."]
        );
    }

    #[test]
    fn checker_and_custom_ignore_density_and_repeat() {
        for density in [0, 50, 100] {
            assert_eq!(rows(&Pattern::Checker, 3, density), ["#.#", ".#.", "#.#"]);
        }
        assert!(Pattern::Checker.picks_first(-1, -1, 0));
        assert!(!Pattern::Checker.picks_first(-1, 0, 0));

        // A 3×2 tile with its left column set.
        let tile = Mask::from_rect(3, 2, Rect::new(0, 0, 1, 2));
        let custom = Pattern::custom(tile).unwrap();
        assert_eq!(rows(&custom, 6, 0), ["#..#.."; 6]);
        assert!(custom.picks_first(-3, -1, 0));
        assert!(!custom.picks_first(-1, 5, 0));
        assert!(Pattern::custom(Mask::empty(0, 0)).is_none());
    }

    #[test]
    fn ids_round_trip_for_the_built_in_patterns() {
        for pattern in Pattern::BUILT_IN {
            assert_eq!(Pattern::built_in(pattern.id()), Some(pattern.clone()));
            assert_eq!(pattern.uses_density(), pattern != Pattern::Checker);
        }
        assert_eq!(Pattern::built_in("custom"), None);
    }
}
