//! View state types.

/// Color theme applied to rendered pages: the original page, or a Nord dark mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    /// Original page colors.
    #[default]
    Off,
    /// Nord-tinted dark mode: background and text map onto a dark Nord background
    /// and a light Nord foreground (by inverted luminance), while colored objects
    /// keep their hue with inverted lightness — a soft dark reading mode.
    Nord,
}

impl Theme {
    pub fn label(self) -> &'static str {
        match self {
            Theme::Off => "off",
            Theme::Nord => "nord",
        }
    }

    /// Stable small integer for cache keys.
    pub fn discriminant(self) -> u8 {
        match self {
            Theme::Off => 0,
            Theme::Nord => 1,
        }
    }

    /// Toggle between the original page and Nord dark mode.
    pub fn next(self) -> Self {
        match self {
            Theme::Off => Theme::Nord,
            Theme::Nord => Theme::Off,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "none" => Some(Theme::Off),
            "nord" | "dark" => Some(Theme::Nord),
            _ => None,
        }
    }

    /// Apply the theme to RGBA8 pixel data in place (alpha untouched).
    pub fn apply(self, data: &mut [u8]) {
        match self {
            Theme::Off => {}
            Theme::Nord => {
                // Greyscale pixels (background + text) map onto the Nord duotone;
                // coloured objects keep their hue/saturation with inverted
                // lightness (dark-mode adapted) — blended by saturation so the two
                // meet smoothly without colouring text antialiasing.
                const BG: [f32; 3] = [0x2E as f32, 0x34 as f32, 0x40 as f32]; // nord0
                const FG: [f32; 3] = [0xD8 as f32, 0xDE as f32, 0xE9 as f32]; // nord4
                for px in data.chunks_exact_mut(4) {
                    let (h, s, l) = rgb_to_hsl(px[0], px[1], px[2]);

                    // Greyscale path: Nord background ↔ foreground by inverted luma.
                    let lum = 0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32;
                    let t = 1.0 - lum / 255.0;
                    let duo = [
                        BG[0] + (FG[0] - BG[0]) * t,
                        BG[1] + (FG[1] - BG[1]) * t,
                        BG[2] + (FG[2] - BG[2]) * t,
                    ];

                    // Colour path: same hue & saturation, inverted lightness.
                    let (ir, ig, ib) = hsl_to_rgb(h, s, 1.0 - l);

                    // Blend: saturated -> keep colour, greyscale -> Nord duotone.
                    let w = smoothstep(0.10, 0.45, s);
                    px[0] = (duo[0] + (ir as f32 - duo[0]) * w) as u8;
                    px[1] = (duo[1] + (ig as f32 - duo[1]) * w) as u8;
                    px[2] = (duo[2] + (ib as f32 - duo[2]) * w) as u8;
                }
            }
        }
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// RGB (0–255) → HSL with hue in degrees [0,360), saturation & lightness [0,1].
fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let r = r as f32 / 255.0;
    let g = g as f32 / 255.0;
    let b = b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d == 0.0 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * ((g - b) / d)
    } else if max == g {
        60.0 * ((b - r) / d) + 120.0
    } else {
        60.0 * ((r - g) / d) + 240.0
    };
    ((h + 360.0) % 360.0, s, l)
}

/// HSL → RGB (0–255).
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h2 = h / 60.0;
    let x = c * (1.0 - (h2 % 2.0 - 1.0).abs());
    let (r, g, b) = if h2 < 1.0 {
        (c, x, 0.0)
    } else if h2 < 2.0 {
        (x, c, 0.0)
    } else if h2 < 3.0 {
        (0.0, c, x)
    } else if h2 < 4.0 {
        (0.0, x, c)
    } else if h2 < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    let m = l - c / 2.0;
    let to_u8 = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (to_u8(r), to_u8(g), to_u8(b))
}

/// Display sharpness: the cap on encoded-image pixels. Higher tiers render more
/// pixels (crisper) at the cost of slower encode/transmit; `Full` renders at the
/// terminal's native resolution (no cap). Only affects kitty (sixel always
/// renders native).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sharpness {
    Low,
    #[default]
    Medium,
    High,
    Full,
}

impl Sharpness {
    /// Cap on encoded visible pixels, or `None` for native resolution.
    pub fn max_display_pixels(self) -> Option<u64> {
        match self {
            Sharpness::Low => Some(1_000_000),
            Sharpness::Medium => Some(2_500_000),
            Sharpness::High => Some(5_000_000),
            Sharpness::Full => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Sharpness::Low => "low",
            Sharpness::Medium => "medium",
            Sharpness::High => "high",
            Sharpness::Full => "full",
        }
    }

    /// Next sharper tier (saturates at `Full`).
    pub fn sharper(self) -> Self {
        match self {
            Sharpness::Low => Sharpness::Medium,
            Sharpness::Medium => Sharpness::High,
            Sharpness::High | Sharpness::Full => Sharpness::Full,
        }
    }

    /// Next softer tier (saturates at `Low`).
    pub fn softer(self) -> Self {
        match self {
            Sharpness::Full => Sharpness::High,
            Sharpness::High => Sharpness::Medium,
            Sharpness::Medium | Sharpness::Low => Sharpness::Low,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Sharpness::Low),
            "medium" | "med" => Some(Sharpness::Medium),
            "high" => Some(Sharpness::High),
            "full" | "native" => Some(Sharpness::Full),
            _ => None,
        }
    }
}

/// How a page is scaled to the viewport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fit {
    /// Whole page visible, centered (the startup default).
    Page,
    /// Page width fills the viewport; scroll vertically.
    Width,
    /// Explicit zoom factor, relative to fit-page width (1.0 == fit-page).
    Zoom(f32),
}

impl Fit {
    pub fn label(self) -> String {
        match self {
            Fit::Page => "fit-page".to_string(),
            Fit::Width => "fit-width".to_string(),
            Fit::Zoom(z) => format!("{}%", (z * 100.0).round() as i32),
        }
    }

    /// Serialize for persistence (`page` / `width` / `zoom <factor>`).
    pub fn serialize(self) -> String {
        match self {
            Fit::Page => "page".to_string(),
            Fit::Width => "width".to_string(),
            Fit::Zoom(z) => format!("zoom {z}"),
        }
    }

    /// Parse the persisted form; `None` if unrecognized.
    pub fn parse(s: &str) -> Option<Fit> {
        let s = s.trim();
        match s {
            "page" => Some(Fit::Page),
            "width" => Some(Fit::Width),
            _ => s
                .strip_prefix("zoom ")
                .and_then(|z| z.trim().parse::<f32>().ok())
                .map(Fit::Zoom),
        }
    }
}

/// View rotation, in 90° clockwise steps, applied to every page. A transient
/// view state (like zoom/pan), not persisted across sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rotation {
    /// Upright (0°), the default.
    #[default]
    None,
    Cw90,
    Cw180,
    Cw270,
}

impl Rotation {
    /// Number of 90° clockwise turns (0..=3) — also the stable cache discriminant.
    pub fn quarter_turns(self) -> u8 {
        match self {
            Rotation::None => 0,
            Rotation::Cw90 => 1,
            Rotation::Cw180 => 2,
            Rotation::Cw270 => 3,
        }
    }

    /// Whether this rotation swaps the width and height axes (90° / 270°), so the
    /// displayed aspect ratio is the page's inverted.
    pub fn swaps_axes(self) -> bool {
        matches!(self, Rotation::Cw90 | Rotation::Cw270)
    }

    /// One step clockwise (wraps 270° → 0°).
    pub fn cw(self) -> Self {
        match self {
            Rotation::None => Rotation::Cw90,
            Rotation::Cw90 => Rotation::Cw180,
            Rotation::Cw180 => Rotation::Cw270,
            Rotation::Cw270 => Rotation::None,
        }
    }

    /// One step counter-clockwise (wraps 0° → 270°).
    pub fn ccw(self) -> Self {
        match self {
            Rotation::None => Rotation::Cw270,
            Rotation::Cw270 => Rotation::Cw180,
            Rotation::Cw180 => Rotation::Cw90,
            Rotation::Cw90 => Rotation::None,
        }
    }

    /// Status-line label; empty when upright so it shows only once rotated.
    pub fn label(self) -> &'static str {
        match self {
            Rotation::None => "",
            Rotation::Cw90 => "90°",
            Rotation::Cw180 => "180°",
            Rotation::Cw270 => "270°",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nord_maps_white_to_dark_and_black_to_light() {
        let mut white = [255u8, 255, 255, 255];
        Theme::Nord.apply(&mut white);
        assert_eq!(&white[0..3], &[0x2E, 0x34, 0x40]); // nord0 background
        assert_eq!(white[3], 255);

        let mut black = [0u8, 0, 0, 255];
        Theme::Nord.apply(&mut black);
        assert_eq!(&black[0..3], &[0xD8, 0xDE, 0xE9]); // nord4 foreground
    }

    #[test]
    fn nord_preserves_hue_of_colored_objects() {
        // Pure red keeps its hue (not flattened to the duotone gradient).
        let mut red = [255u8, 0, 0, 255];
        Theme::Nord.apply(&mut red);
        assert!(red[0] > red[1] && red[0] > red[2], "red stays reddish: {red:?}");

        // Dark blue keeps its hue but is lightened (lightness inverted for dark).
        let mut blue = [0u8, 0, 128, 255];
        Theme::Nord.apply(&mut blue);
        assert!(blue[2] > blue[0] && blue[2] > blue[1], "blue stays bluish: {blue:?}");
        assert!(blue[2] > 160, "dark blue should lighten: {blue:?}");
    }

    #[test]
    fn theme_toggles_and_parses() {
        assert_eq!(Theme::Off.next(), Theme::Nord);
        assert_eq!(Theme::Nord.next(), Theme::Off);
        assert_eq!(Theme::parse("NORD"), Some(Theme::Nord));
        assert_eq!(Theme::parse("dark"), Some(Theme::Nord));
        assert_eq!(Theme::parse("off"), Some(Theme::Off));
        assert_eq!(Theme::parse("invert"), None);
        assert_eq!(Theme::parse("bogus"), None);
    }

    #[test]
    fn rotation_cycles_both_ways_and_wraps() {
        // Four clockwise steps return to upright.
        let mut r = Rotation::None;
        for _ in 0..4 {
            r = r.cw();
        }
        assert_eq!(r, Rotation::None);
        // cw then ccw is identity; ccw from upright wraps to 270°.
        assert_eq!(Rotation::None.cw().ccw(), Rotation::None);
        assert_eq!(Rotation::None.ccw(), Rotation::Cw270);
        assert_eq!(Rotation::Cw270.cw(), Rotation::None);
    }

    #[test]
    fn rotation_axis_swap_and_turns() {
        assert_eq!(Rotation::None.quarter_turns(), 0);
        assert_eq!(Rotation::Cw270.quarter_turns(), 3);
        assert!(!Rotation::None.swaps_axes());
        assert!(!Rotation::Cw180.swaps_axes());
        assert!(Rotation::Cw90.swaps_axes());
        assert!(Rotation::Cw270.swaps_axes());
    }
}
