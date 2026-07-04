//! Theme system with Light/Dark mode and OS preference detection.
//!
//! **Darkroom Editorial** aesthetic: warm neutrals, amber accents, refined for photography.

use std::cell::Cell;

use iced::{
    Color, Theme,
    theme::{Palette, palette},
};

/// User's theme preference. `Auto` detects from OS.
#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code, reason = "variants used once theme picker UI is added")]
pub(crate) enum Preference {
    #[default]
    Auto,
    Dark,
    Light,
}

#[derive(Clone, Copy)]
struct CachedTheme {
    palette: palette::Extended,
    is_dark: bool,
}

thread_local! {
    static PREFERENCE: Cell<Preference> = const { Cell::new(Preference::Auto) };
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
        Preference::Dark => true,
        Preference::Light => false,
        Preference::Auto => os_is_dark,
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
#[allow(dead_code, reason = "used once theme picker UI is added")]
pub(crate) fn set_preference(pref: Preference) {
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

    pub(crate) const SUCCESS: Color = rgb(0x5C, 0xB8, 0x7A); // Soft sage green
    pub(crate) const WARNING: Color = rgb(0xE5, 0xA8, 0x53); // Warm gold (close to accent family)
    pub(crate) const DANGER: Color = rgb(0xD9, 0x53, 0x53); // Warm red
    pub(crate) const DANGER_HOVER: Color = rgb(0xC4, 0x3E, 0x3E);
    pub(crate) const DANGER_PRESSED: Color = rgb(0xB9, 0x47, 0x47);

    pub(crate) const REJECTED_BG: Color = rgb(0x3D, 0x1C, 0x1C); // Dark warm red

    pub(crate) const OVERLAY_BADGE: Color = Color::from_rgba(0.12, 0.11, 0.10, 0.88);
    pub(crate) const OVERLAY_DOWNLOADED: Color = Color::from_rgba(0.08, 0.07, 0.06, 0.55); // Warm black
    pub(crate) const BADGE_REJECTED: Color = Color::from_rgba(0.75, 0.15, 0.15, 0.92);
    pub(crate) const BADGE_BURST: Color = rgb(0x8C, 0x7B, 0x6A); // Warm taupe

    // Indexed to match ColorLabel discriminants 1-7
    pub(crate) const COLOR_LABEL_1: Color = rgb(0xD9, 0x4F, 0x4F); // Warm red
    pub(crate) const COLOR_LABEL_2: Color = rgb(0xE5, 0xC0, 0x4D); // Gold
    pub(crate) const COLOR_LABEL_3: Color = rgb(0x5C, 0xB8, 0x7A); // Sage green
    pub(crate) const COLOR_LABEL_4: Color = rgb(0x5A, 0x9B, 0xD9); // Sky blue
    pub(crate) const COLOR_LABEL_5: Color = rgb(0x9B, 0x6B, 0xC9); // Lavender
    pub(crate) const COLOR_LABEL_6: Color = rgb(0x4D, 0xB8, 0xB8); // Teal
    pub(crate) const COLOR_LABEL_7: Color = rgb(0xE5, 0x85, 0x45); // Burnt orange
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
