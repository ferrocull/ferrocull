//! Theme system with Light/Dark mode and OS preference detection.
//!
//! **Darkroom Editorial** aesthetic: warm neutrals, amber accents, refined for photography.

use std::cell::Cell;

use ferrocull_core::ThemePreference;
use iced::{
    Color, Theme,
    theme::{Palette, palette},
};

#[derive(Clone, Copy)]
struct CachedTheme {
    palette: palette::Extended,
    is_dark: bool,
}

thread_local! {
    static PREFERENCE: Cell<ThemePreference> = const { Cell::new(ThemePreference::Auto) };
    static CACHED: Cell<Option<CachedTheme>> = const { Cell::new(None) };
}

fn cached() -> CachedTheme {
    CACHED.with(Cell::get).unwrap_or_else(|| {
        let theme = dark_theme();
        CachedTheme {
            palette: *theme.extended_palette(),
            is_dark: true,
        }
    })
}

/// Build an iced `Theme`. Called once per frame by iced's runtime. Pure: reads
/// from cache populated by `set_os_is_dark` (called from the app's update arms).
#[must_use]
pub(crate) fn resolve_theme() -> Theme {
    if cached().is_dark {
        dark_theme()
    } else {
        light_theme()
    }
}

/// Extended palette. Pure: reads from the same cache as `resolve_theme`.
#[must_use]
pub(crate) fn palette() -> palette::Extended {
    cached().palette
}

/// Detect the OS dark mode preference. Performs OS I/O — call from a `Task`
/// (`spawn_blocking`) or at boot, never from the render path.
#[must_use]
pub(crate) fn detect_os_is_dark() -> bool {
    // Unspecified/unreachable portal falls back to dark, not light.
    match dark_light::detect() {
        Ok(dark_light::Mode::Light) => false,
        Ok(dark_light::Mode::Dark | dark_light::Mode::Unspecified) | Err(_) => true,
    }
}

/// Set the resolved dark-mode value. Respects user preference: when the user
/// has explicitly picked Dark or Light, OS detection is ignored.
pub(crate) fn set_os_is_dark(os_is_dark: bool) {
    let is_dark = match PREFERENCE.with(Cell::get) {
        ThemePreference::Dark => true,
        ThemePreference::Light => false,
        ThemePreference::Auto => os_is_dark,
    };
    let theme = if is_dark { dark_theme() } else { light_theme() };
    CACHED.with(|c| {
        c.set(Some(CachedTheme {
            palette: *theme.extended_palette(),
            is_dark,
        }));
    });
}

/// Set theme preference and re-resolve against current OS value.
pub(crate) fn set_preference(pref: ThemePreference) {
    PREFERENCE.with(|p| p.set(pref));
    set_os_is_dark(detect_os_is_dark());
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

pub(crate) mod colors {
    use iced::Color;

    use super::rgb;

    pub(crate) const ACCENT: Color = rgb(0xD9, 0x8E, 0x48); // Warm amber
    pub(crate) const ACCENT_HOVER: Color = rgb(0xE5, 0x9E, 0x58);
    pub(crate) const ACCENT_MUTED: Color = rgb(0xA0, 0x6A, 0x35); // Subdued for backgrounds
    pub(crate) const ACCENT_PRESSED: Color = rgb(0xC6, 0x7F, 0x3D); // Pressed primary-button fill; keeps 4.5:1 with ON_ACCENT
    pub(crate) const ACCENT_DEEP: Color = rgb(0x6E, 0x44, 0x19); // Dark amber ink for selected text on amber tints in the light theme

    pub(crate) const ON_ACCENT: Color = rgb(0x2A, 0x26, 0x22); // Warm-black ink on amber button fills (both themes)

    // Focus-cursor blue — the one sanctioned cool hue. Marks the keyboard
    // cursor (focused thumbnail border); amber stays selection/state.
    pub(crate) const FOCUS_DARK: Color = rgb(0x5A, 0x9B, 0xD9);
    pub(crate) const FOCUS_LIGHT: Color = rgb(0x3D, 0x6F, 0xA6); // Darkened for >=3:1 on warm-white surfaces

    pub(crate) const SUCCESS: Color = rgb(0x5C, 0xB8, 0x7A); // Soft sage green
    pub(crate) const WARNING: Color = rgb(0xE5, 0xA8, 0x53); // Warm gold (close to accent family)
    pub(crate) const DANGER: Color = rgb(0xD9, 0x53, 0x53); // Warm red (semantic text ink)
    // Resting fill for danger *buttons*: deepened from DANGER so white ink clears
    // 4.5:1 (DANGER itself is 3.95:1 with white). Kept separate from DANGER so the
    // danger text ink stays bright enough (4.53:1) on the dark theme.
    pub(crate) const DANGER_REST: Color = rgb(0xCB, 0x47, 0x47);
    pub(crate) const DANGER_HOVER: Color = rgb(0xC4, 0x3E, 0x3E);
    pub(crate) const DANGER_PRESSED: Color = rgb(0xB9, 0x47, 0x47);

    pub(crate) const REJECTED_BG: Color = rgb(0x3D, 0x1C, 0x1C); // Dark warm red

    pub(crate) const OVERLAY_BADGE: Color = Color::from_rgba(0.12, 0.11, 0.10, 0.88);
    pub(crate) const OVERLAY_INGESTED: Color = Color::from_rgba(0.08, 0.07, 0.06, 0.55); // Warm black
    pub(crate) const BADGE_REJECTED: Color = Color::from_rgba(0.75, 0.15, 0.15, 0.92);
    pub(crate) const BADGE_BURST: Color = rgb(0x6B, 0x5D, 0x4E); // Deep warm taupe; holds 4.5:1 with BADGE_TEXT
    pub(crate) const BADGE_BURST_HOVER: Color = rgb(0x74, 0x63, 0x53); // Lightened on hover, still 4.5:1

    // Explicit warm-white text on the fixed dark badge fills above — badges
    // keep their own ink in both themes instead of inheriting theme text.
    pub(crate) const BADGE_TEXT: Color = rgb(0xF0, 0xEB, 0xE5);

    // "Already ingested" ink on OVERLAY_BADGE. Deliberately not amber: the
    // Safelight Rule reserves amber for active state, and a copied frame is
    // completed history. The muted-taupe text colour reads only 4.13:1 here;
    // this lighter taupe clears the floor at 5.94:1.
    pub(crate) const BADGE_INGESTED: Color = rgb(0xA8, 0x96, 0x82);

    pub(crate) const RATING_STAR: Color = rgb(0xE5, 0xA8, 0x53); // Rating stars/badge — WARNING's hue, its own role

    pub(crate) const TEXT_MUTED: Color = rgb(0x8C, 0x7B, 0x6A); // Warm taupe, for secondary text

    // Indexed to match ColorLabel discriminants 1-7
    pub(crate) const COLOR_LABEL_1: Color = rgb(0xD9, 0x4F, 0x4F); // Warm red
    pub(crate) const COLOR_LABEL_2: Color = rgb(0xE5, 0xC0, 0x4D); // Gold
    pub(crate) const COLOR_LABEL_3: Color = rgb(0x5C, 0xB8, 0x7A); // Sage green
    pub(crate) const COLOR_LABEL_4: Color = rgb(0x5A, 0x9B, 0xD9); // Sky blue
    pub(crate) const COLOR_LABEL_5: Color = rgb(0x9B, 0x6B, 0xC9); // Lavender
    pub(crate) const COLOR_LABEL_6: Color = rgb(0xE5, 0x6A, 0x2E); // Orange (hue ~20°, redder than gold/amber)
    pub(crate) const COLOR_LABEL_7: Color = rgb(0xAD, 0xA6, 0x9C); // Warm gray (low-saturation neutral)
}

/// Color label colors indexed by `ColorLabel` discriminant (1-7). Index 0 unused (transparent).
pub(crate) const COLOR_LABELS: [Color; 8] = [
    Color::TRANSPARENT, // 0: unused
    colors::COLOR_LABEL_1,
    colors::COLOR_LABEL_2,
    colors::COLOR_LABEL_3,
    colors::COLOR_LABEL_4,
    colors::COLOR_LABEL_5,
    colors::COLOR_LABEL_6,
    colors::COLOR_LABEL_7,
];

/// Light-theme label-bar variants: same seven hues, darkened where the
/// original fails 3:1 as a mark on warm-white (gold, green, blue, orange,
/// gray). Red and purple already pass and stay identical.
pub(crate) const COLOR_LABELS_LIGHT: [Color; 8] = [
    Color::TRANSPARENT,    // 0: unused
    colors::COLOR_LABEL_1, // Red passes as-is (3.82)
    rgb(0xA8, 0x86, 0x2B), // Gold, darkened (3.24)
    rgb(0x3D, 0x8E, 0x58), // Green, darkened (3.80)
    rgb(0x3A, 0x78, 0xB5), // Blue, darkened (4.37)
    colors::COLOR_LABEL_5, // Purple passes as-is (3.71)
    rgb(0xC0, 0x5A, 0x1E), // Orange, darkened (4.19)
    rgb(0x7D, 0x74, 0x68), // Gray, darkened (4.34)
];

/// Focus-cursor color for a card, accounting for a rejected background. In the
/// light theme the darkened focus blue (`FOCUS_LIGHT`) reads only 2.91:1 on the
/// dark-red `REJECTED_BG`; the dark-theme blue reads 5.16:1 there while staying
/// the same focus hue, so a rejected card in the light theme borrows it.
#[must_use]
pub(crate) fn focus_color_for(is_rejected: bool) -> Color {
    if cached().is_dark || is_rejected {
        colors::FOCUS_DARK
    } else {
        colors::FOCUS_LIGHT
    }
}

/// Amber wash behind tagged (working-set) thumbnail cards. A hue cue, not the
/// accessible mark — the tag check badge carries that; alpha is tuned per
/// theme so the tint reads over each base background.
#[must_use]
pub(crate) fn tagged_wash() -> Color {
    if cached().is_dark {
        colors::ACCENT_MUTED.scale_alpha(0.35)
    } else {
        colors::ACCENT_MUTED.scale_alpha(0.28)
    }
}

pub(crate) mod spacing {
    pub(crate) const XS: f32 = 4.0;
    pub(crate) const SM: f32 = 8.0;
    pub(crate) const MD: f32 = 12.0;
    pub(crate) const LG: f32 = 16.0;
}

pub(crate) mod radius {
    pub(crate) const XS: f32 = 2.0;
    pub(crate) const SM: f32 = 4.0;
    pub(crate) const MD: f32 = 6.0;
    pub(crate) const LG: f32 = 10.0;
}

fn dark_theme() -> Theme {
    let pal = Palette {
        background: rgb(0x18, 0x17, 0x16), // Warm near-black
        text: rgb(0xF0, 0xEB, 0xE5),       // Warm white
        primary: colors::ACCENT,
        success: colors::SUCCESS,
        warning: colors::WARNING,
        danger: colors::DANGER,
    };
    Theme::custom_with_fn("Ferrocull Dark", pal, palette::Extended::generate)
}

fn light_theme() -> Theme {
    let pal = Palette {
        background: rgb(0xFA, 0xF8, 0xF5), // Warm white
        text: rgb(0x2A, 0x26, 0x22),       // Warm black
        primary: colors::ACCENT,
        success: colors::SUCCESS,
        warning: colors::WARNING,
        danger: colors::DANGER,
    };
    Theme::custom_with_fn("Ferrocull Light", pal, palette::Extended::generate)
}

#[cfg(test)]
mod tests {
    use ferrocull_core::ColorLabel;
    use iced::Color;

    use super::{COLOR_LABELS, colors};

    /// WCAG relative luminance.
    fn luminance(c: Color) -> f32 {
        fn channel(v: f32) -> f32 {
            if v <= 0.039_28 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
    }

    /// WCAG contrast ratio between two colors.
    fn contrast(a: Color, b: Color) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// HSV hue (degrees) and saturation of a color, for asserting a label's hue
    /// still matches its XMP name.
    #[expect(
        clippy::float_cmp,
        reason = "channel/max equality picks the dominant hue channel; values are exact copies, not computed"
    )]
    fn hue_sat(c: Color) -> (f32, f32) {
        let (r, g, b) = (c.r, c.g, c.b);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let sat = if max == 0.0 { 0.0 } else { delta / max };
        if delta == 0.0 {
            return (0.0, sat);
        }
        let mut hue = 60.0
            * if max == r {
                ((g - b) / delta).rem_euclid(6.0)
            } else if max == g {
                (b - r) / delta + 2.0
            } else {
                (r - g) / delta + 4.0
            };
        if hue < 0.0 {
            hue += 360.0;
        }
        (hue, sat)
    }

    /// Guards against the color-label palette drifting out of correspondence
    /// with the XMP names. Each chromatic label must land in a hue window; the
    /// achromatic Gray label is asserted by low saturation instead.
    #[test]
    fn dark_labels_match_their_xmp_names() {
        // Inclusive hue windows per XMP name; wide enough for aesthetic tuning,
        // narrow enough that a swapped hue (e.g. teal ~180°) fails.
        let windows: &[(ColorLabel, f32, f32)] = &[
            (ColorLabel::Red, 350.0, 15.0),   // wraps 0°
            (ColorLabel::Orange, 12.0, 34.0), // between red and gold
            (ColorLabel::Yellow, 35.0, 60.0), // "Gold" display hue
            (ColorLabel::Green, 100.0, 165.0),
            (ColorLabel::Blue, 190.0, 235.0),
            (ColorLabel::Purple, 255.0, 300.0),
        ];
        for &(label, lo, hi) in windows {
            let (hue, _) = hue_sat(COLOR_LABELS[u8::from(label) as usize]);
            let ok = if lo > hi {
                hue >= lo || hue <= hi
            } else {
                hue >= lo && hue <= hi
            };
            assert!(
                ok,
                "{}: hue {hue:.1}° outside [{lo}, {hi}]",
                label.xmp_str()
            );
        }

        // Gray is achromatic — it must read as a neutral, not a chromatic label.
        let (_, gray_sat) = hue_sat(COLOR_LABELS[u8::from(ColorLabel::Gray) as usize]);
        assert!(
            gray_sat < 0.2,
            "Gray saturation {gray_sat:.3} too high to read as a neutral"
        );
    }

    /// The ingested mark is non-bold ink on the badge fill, below the
    /// large-text threshold at every size it draws, so it owes 4.5:1.
    /// Computed from the constants so a palette edit re-runs the check
    /// rather than invalidating a hard-coded figure.
    #[test]
    fn ingested_badge_ink_clears_wcag_aa_on_the_badge_fill() {
        let ratio = contrast(colors::BADGE_INGESTED, colors::OVERLAY_BADGE);
        assert!(
            ratio >= 4.5,
            "BADGE_INGESTED on OVERLAY_BADGE is {ratio:.2}:1, under the 4.5:1 floor"
        );
    }
}
