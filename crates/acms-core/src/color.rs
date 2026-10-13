//! Colours: RGBA, 8 bits per channel, straight (non-premultiplied) alpha.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgba8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba8 {
    pub const TRANSPARENT: Self = Self::new(0, 0, 0, 0);
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    pub const WHITE: Self = Self::rgb(255, 255, 255);

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// An opaque colour.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::new(r, g, b, 255)
    }

    pub const fn from_array([r, g, b, a]: [u8; 4]) -> Self {
        Self::new(r, g, b, a)
    }

    pub const fn to_array(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// The form pixels are stored in: every fully transparent colour becomes
    /// [`Rgba8::TRANSPARENT`], so two images that look the same compare equal.
    pub const fn normalized(self) -> Self {
        if self.a == 0 { Self::TRANSPARENT } else { self }
    }

    /// Parses `RRGGBB` or `RRGGBBAA`, with or without a leading `#`.
    pub fn from_hex(text: &str) -> Option<Self> {
        let digits = text.trim();
        let digits = digits.strip_prefix('#').unwrap_or(digits);
        if !digits.is_ascii() || (digits.len() != 6 && digits.len() != 8) {
            return None;
        }
        let channel = |index: usize| u8::from_str_radix(&digits[index * 2..index * 2 + 2], 16).ok();
        Some(Self::new(
            channel(0)?,
            channel(1)?,
            channel(2)?,
            if digits.len() == 8 { channel(3)? } else { 255 },
        ))
    }

    /// `#RRGGBB` for opaque colours, `#RRGGBBAA` otherwise.
    pub fn to_hex(self) -> String {
        let Self { r, g, b, a } = self;
        if a == 255 {
            format!("#{r:02X}{g:02X}{b:02X}")
        } else {
            format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
        }
    }

    /// Normal blending: this colour drawn on top of `below`.
    #[inline]
    pub fn over(self, below: Self) -> Self {
        let src_a = u32::from(self.a);
        let dst_a = u32::from(below.a);
        // Everything below is scaled by 255 so the maths stays in integers.
        let dst_weight = dst_a * (255 - src_a);
        let out_a = src_a * 255 + dst_weight;
        if out_a == 0 {
            return Self::TRANSPARENT;
        }
        // On something opaque the result is opaque, and the general rule below comes down to
        // `(src * a + dst * (255 - a) + 127) / 255`, which needs no division at all: for
        // numbers this small, `v / 255` is `(v + 1 + (v >> 8)) >> 8`. This is the common
        // case, paint over a background, and gives exactly the same values.
        if dst_a == 255 {
            let rest = 255 - src_a;
            // Red and blue are worked out together, 16 bits apart in one number; neither
            // can spill into the other, since 255 * 255 + 127 + 1 + 254 fits in 16 bits.
            let pair = |low: u8, high: u8| u32::from(low) | u32::from(high) << 16;
            let v = pair(self.r, self.b) * src_a + pair(below.r, below.b) * rest + 0x007F_007F;
            let red_blue = (v + 0x0001_0001 + ((v >> 8) & 0x00FF_00FF)) >> 8;
            let g = u32::from(self.g) * src_a + u32::from(below.g) * rest + 127;
            let green = (g + 1 + (g >> 8)) >> 8;
            return Self::new(red_blue as u8, green as u8, (red_blue >> 16) as u8, 255);
        }
        let channel = |src: u8, dst: u8| {
            let sum = u32::from(src) * src_a * 255 + u32::from(dst) * dst_weight;
            ((sum + out_a / 2) / out_a) as u8
        };
        Self::new(
            channel(self.r, below.r),
            channel(self.g, below.g),
            channel(self.b, below.b),
            ((out_a + 127) / 255) as u8,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blending_onto_an_opaque_colour_takes_the_same_values_as_the_general_rule() {
        // The general rule, written out again without the shortcut.
        let general = |src: u8, src_a: u8, dst: u8| {
            let (src_a, dst_weight) = (u32::from(src_a), 255 * (255 - u32::from(src_a)));
            let out_a = src_a * 255 + dst_weight;
            let sum = u32::from(src) * src_a * 255 + u32::from(dst) * dst_weight;
            ((sum + out_a / 2) / out_a) as u8
        };
        for src_a in 0..=255_u8 {
            for src in 0..=255_u8 {
                for dst in (0..=255_u8).step_by(3) {
                    // Three different values in the three channels, so that one leaking
                    // into another would show.
                    let above = Rgba8::new(src, 255 - src, src ^ 0x5A, src_a);
                    let below = Rgba8::rgb(dst, dst ^ 0xA5, 255 - dst);
                    let over = above.over(below);
                    let expected = Rgba8::rgb(
                        general(above.r, src_a, below.r),
                        general(above.g, src_a, below.g),
                        general(above.b, src_a, below.b),
                    );
                    assert_eq!(over, expected, "{above:?} over {below:?}");
                }
            }
        }
    }

    #[test]
    fn hex_round_trips() {
        let opaque = Rgba8::rgb(0x12, 0xAB, 0xEF);
        assert_eq!(opaque.to_hex(), "#12ABEF");
        assert_eq!(Rgba8::from_hex("#12ABEF"), Some(opaque));
        assert_eq!(Rgba8::from_hex("12abef"), Some(opaque));

        let translucent = Rgba8::new(1, 2, 3, 0x80);
        assert_eq!(translucent.to_hex(), "#01020380");
        assert_eq!(Rgba8::from_hex(" #01020380 "), Some(translucent));
    }

    #[test]
    fn bad_hex_is_rejected() {
        for bad in ["", "#", "#12345", "#1234567", "#GGGGGG", "#12345é", "red"] {
            assert_eq!(Rgba8::from_hex(bad), None, "{bad}");
        }
    }

    #[test]
    fn normalizing_only_touches_fully_transparent_colours() {
        assert_eq!(Rgba8::new(9, 9, 9, 0).normalized(), Rgba8::TRANSPARENT);
        assert_eq!(Rgba8::new(9, 9, 9, 1).normalized(), Rgba8::new(9, 9, 9, 1));
    }

    #[test]
    fn over_with_opaque_or_transparent_source() {
        let red = Rgba8::rgb(255, 0, 0);
        let blue = Rgba8::rgb(0, 0, 255);
        assert_eq!(red.over(blue), red);
        assert_eq!(Rgba8::TRANSPARENT.over(blue), blue);
        assert_eq!(red.over(Rgba8::TRANSPARENT), red);
        assert_eq!(
            Rgba8::TRANSPARENT.over(Rgba8::TRANSPARENT),
            Rgba8::TRANSPARENT
        );
    }

    #[test]
    fn over_blends_half_transparent_source() {
        let half_white = Rgba8::new(255, 255, 255, 128);
        // On opaque black: a mid grey, still opaque.
        assert_eq!(half_white.over(Rgba8::BLACK), Rgba8::rgb(128, 128, 128));
        // On nothing: unchanged.
        assert_eq!(half_white.over(Rgba8::TRANSPARENT), half_white);
        // On itself: more opaque, same colour.
        assert_eq!(half_white.over(half_white), Rgba8::new(255, 255, 255, 192));
    }
}
