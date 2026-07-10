//! Reusable, theme-aware widget styles.
//!
//! **Darkroom Editorial** aesthetic: refined shadows, warm tones, subtle depth.

use iced::{
    Border, Color, Shadow, Theme, Vector,
    border::Radius,
    widget::{button, container, pick_list, progress_bar, text_input},
};

use crate::theme::{colors, radius};

/// Sidebar panel with subtle border.
#[must_use]
pub(crate) fn panel(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.weakest.color.into()),
        border: Border {
            color: palette.background.weaker.color,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

/// Amber accent button for primary actions.
#[must_use]
pub(crate) fn primary_button(theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme.extended_palette();
    let (bg, text, shadow_alpha) = match status {
        button::Status::Active => (colors::ACCENT, Color::WHITE, 0.15),
        button::Status::Hovered => (colors::ACCENT_HOVER, Color::WHITE, 0.2),
        button::Status::Pressed => (colors::ACCENT_MUTED, Color::WHITE, 0.1),
        button::Status::Disabled => (
            palette.background.strong.text,
            palette.background.weak.text,
            0.0,
        ),
    };

    button::Style {
        background: Some(bg.into()),
        text_color: text,
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: radius::SM.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, shadow_alpha),
            offset: Vector::new(0.0, 1.0),
            blur_radius: 2.0,
        },
        ..Default::default()
    }
}

/// Secondary button with subtle background and border.
#[must_use]
pub(crate) fn secondary_button(theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme.extended_palette();
    let (bg, border_color) = match status {
        button::Status::Active => (
            palette.background.weak.color,
            palette.background.strong.color,
        ),
        button::Status::Hovered => (
            palette.background.neutral.color,
            palette.background.stronger.color,
        ),
        button::Status::Pressed => (
            palette.background.weakest.color,
            palette.background.strong.color,
        ),
        button::Status::Disabled => (
            palette.background.weakest.color,
            palette.background.weaker.color,
        ),
    };

    let text_color = match status {
        button::Status::Disabled => palette.background.strong.text,
        _ => palette.background.base.text,
    };

    button::Style {
        background: Some(bg.into()),
        text_color,
        border: Border {
            color: border_color,
            width: 1.0,
            radius: radius::SM.into(),
        },
        ..Default::default()
    }
}

/// Compact toggle for filter pills (tighter padding, smaller radius).
pub(crate) fn filter_pill(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme: &Theme, status: button::Status| {
        if selected {
            button::Style {
                background: Some(colors::ACCENT_MUTED.into()),
                text_color: Color::WHITE,
                border: Border {
                    color: colors::ACCENT,
                    width: 1.0,
                    radius: radius::LG.into(),
                },
                ..Default::default()
            }
        } else {
            let palette = theme.extended_palette();
            let (bg, border) = match status {
                button::Status::Hovered => (
                    palette.background.neutral.color,
                    palette.background.strong.color,
                ),
                _ => (Color::TRANSPARENT, palette.background.weaker.color),
            };

            button::Style {
                background: Some(bg.into()),
                text_color: palette.background.weak.text,
                border: Border {
                    color: border,
                    width: 1.0,
                    radius: radius::LG.into(),
                },
                ..Default::default()
            }
        }
    }
}

/// Right half of the merged pattern control: a text input whose left corners
/// are squared so the preset picker sits flush against it.
#[must_use]
pub(crate) fn pattern_input(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let mut style = text_input::default(theme, status);
    style.border.radius = Radius {
        top_left: 0.0,
        bottom_left: 0.0,
        ..Radius::from(radius::XS)
    };
    style
}

/// Left half of the merged pattern control: a chevron-only pick list whose
/// right corners are squared to continue the input's outline.
#[must_use]
pub(crate) fn pattern_picker(theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let palette = theme.extended_palette();
    let mut style = pick_list::default(theme, status);
    style.background = palette.background.base.color.into();
    style.border.radius = Radius {
        top_right: 0.0,
        bottom_right: 0.0,
        ..Radius::from(radius::XS)
    };
    style
}

/// Amber progress bar for storage and import indicators.
#[must_use]
pub(crate) fn storage_progress(theme: &Theme) -> progress_bar::Style {
    let palette = theme.extended_palette();
    progress_bar::Style {
        background: palette.background.strong.color.into(),
        bar: colors::ACCENT.into(),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: radius::XS.into(),
        },
    }
}

/// Danger button for destructive actions (e.g., reject).
#[must_use]
pub(crate) fn danger_button(theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme.extended_palette();
    let (bg, text) = match status {
        button::Status::Active => (colors::DANGER, Color::WHITE),
        button::Status::Hovered => (colors::DANGER_HOVER, Color::WHITE),
        button::Status::Pressed => (colors::DANGER_PRESSED, Color::WHITE),
        button::Status::Disabled => (palette.background.strong.text, palette.background.weak.text),
    };

    button::Style {
        background: Some(bg.into()),
        text_color: text,
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: radius::SM.into(),
        },
        ..Default::default()
    }
}

/// Dark background for fullscreen preview overlay.
#[must_use]
pub(crate) fn preview_background(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.base.color.into()),
        ..Default::default()
    }
}

/// Semi-transparent bar for preview top/bottom controls.
#[must_use]
pub(crate) fn preview_bar(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    let base = palette.background.base.color;
    container::Style {
        background: Some(Color::from_rgba(base.r, base.g, base.b, 0.85).into()),
        ..Default::default()
    }
}

/// Grid content area - slightly deeper than panel for photo focus.
#[must_use]
pub(crate) fn grid_background(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.base.color.into()),
        ..Default::default()
    }
}

/// Date group header in thumbnail grid.
#[must_use]
pub(crate) fn date_header(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    let elevated = palette.background.weak.color;
    container::Style {
        background: Some(Color::from_rgba(elevated.r, elevated.g, elevated.b, 0.6).into()),
        border: Border {
            radius: radius::SM.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Status bar with elevated appearance.
#[must_use]
pub(crate) fn status_bar(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.weakest.color.into()),
        border: Border::default(),
        shadow: Shadow {
            color: if palette.is_dark {
                Color::from_rgba(0.0, 0.0, 0.0, 0.3)
            } else {
                Color::from_rgba(0.0, 0.0, 0.0, 0.1)
            },
            offset: Vector::new(0.0, -2.0),
            blur_radius: 8.0,
        },
        ..Default::default()
    }
}

/// Full-screen dimming scrim behind the Settings popup. A warm near-black at
/// partial opacity so the grid reads as pushed back, not hidden.
#[must_use]
pub(crate) fn scrim(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgba(0.06, 0.05, 0.05, 0.62).into()),
        ..Default::default()
    }
}

/// Elevated Settings card floating above the scrim.
#[must_use]
pub(crate) fn settings_card(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.weakest.color.into()),
        border: Border {
            color: palette.background.weaker.color,
            width: 1.0,
            radius: radius::LG.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.45),
            offset: Vector::new(0.0, 8.0),
            blur_radius: 32.0,
        },
        ..Default::default()
    }
}

/// Category rail item — amber "selected frame" cue when active, subtle hover
/// otherwise. Mirrors the date-tree selection so the rail reads as native.
pub(crate) fn settings_rail_item(
    selected: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme: &Theme, status: button::Status| {
        let palette = theme.extended_palette();

        let bg = match (selected, status) {
            (true, button::Status::Hovered) => colors::ACCENT_MUTED.scale_alpha(0.38),
            (true, _) => colors::ACCENT_MUTED.scale_alpha(0.28),
            (false, button::Status::Hovered) => palette.background.neutral.color,
            (false, _) => Color::TRANSPARENT,
        };

        let text_color = if selected {
            colors::ACCENT
        } else {
            palette.background.base.text
        };

        button::Style {
            background: Some(bg.into()),
            text_color,
            border: Border {
                radius: radius::SM.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

/// Collapsible section header with distinct background.
pub(crate) fn section_toggle(expanded: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme: &Theme, status: button::Status| {
        let palette = theme.extended_palette();

        let bg = match status {
            button::Status::Hovered => palette.background.neutral.color,
            button::Status::Pressed => palette.background.weakest.color,
            _ => palette.background.weak.color,
        };

        let text_color = if expanded {
            palette.background.base.text
        } else {
            palette.background.weak.text
        };

        button::Style {
            background: Some(bg.into()),
            text_color,
            border: Border {
                radius: radius::SM.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

/// Collapsed panel edge handle (expand button).
pub(crate) fn panel_handle_collapsed(theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme.extended_palette();

    let (bg, text_color) = match status {
        button::Status::Hovered => (
            palette.background.neutral.color,
            palette.background.weak.text,
        ),
        button::Status::Pressed => (
            palette.background.strong.color,
            palette.background.strong.text,
        ),
        button::Status::Active | button::Status::Disabled => {
            (palette.background.weak.color, palette.background.weak.text)
        }
    };

    button::Style {
        background: Some(bg.into()),
        text_color,
        border: Border::default(),
        ..Default::default()
    }
}

/// Expanded panel-edge splitter handle. A container has no hover state — the
/// resize cursor supplies the affordance.
#[must_use]
pub(crate) fn panel_handle_expanded(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.weaker.color.into()),
        text_color: Some(palette.background.strong.text),
        ..Default::default()
    }
}

/// Date tree item — transparent by default, subtle bg on hover, accent-tinted when selected.
pub(crate) fn date_tree_item(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme: &Theme, status: button::Status| {
        let palette = theme.extended_palette();

        let bg = match (selected, status) {
            (true, button::Status::Hovered) => colors::ACCENT_MUTED.scale_alpha(0.55),
            (true, _) => colors::ACCENT_MUTED.scale_alpha(0.4),
            (false, button::Status::Hovered) => palette.background.neutral.color,
            (false, _) => Color::TRANSPARENT,
        };

        let text_color = if selected {
            colors::ACCENT
        } else {
            palette.background.base.text
        };

        button::Style {
            background: Some(bg.into()),
            text_color,
            border: Border {
                radius: radius::XS.into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

/// Thumbnail card background with optional border (focused/selected state).
pub(crate) fn thumbnail_card(bg: Color, border: Border) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(bg.into()),
        border,
        ..Default::default()
    }
}

/// Semi-transparent overlay badge (info bar, pair badge, rated badge).
#[must_use]
pub(crate) fn overlay_badge(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(colors::OVERLAY_BADGE.into()),
        border: Border {
            radius: radius::SM.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Rounded badge with custom background (rejected, burst, preview icon).
pub(crate) fn rounded_badge(bg: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(bg.into()),
        border: Border {
            radius: radius::SM.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Burst count badge button: warm taupe pill that brightens on hover/press.
pub(crate) fn burst_badge(theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme.extended_palette();
    let bg = match status {
        button::Status::Hovered | button::Status::Pressed => colors::BADGE_BURST_HOVER,
        _ => colors::BADGE_BURST,
    };

    button::Style {
        background: Some(bg.into()),
        text_color: palette.background.base.text,
        border: Border {
            radius: radius::SM.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Solid fill container (color overlay, color label bar).
pub(crate) fn solid_fill(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(color.into()),
        ..Default::default()
    }
}

/// Invisible button for clickable areas (icons, badges).
#[must_use]
pub(crate) fn ghost_button(theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme.extended_palette();
    let bg = match status {
        button::Status::Hovered => palette.background.neutral.color,
        button::Status::Pressed => palette.background.weakest.color,
        _ => Color::TRANSPARENT,
    };

    button::Style {
        background: Some(bg.into()),
        text_color: palette.background.base.text,
        border: Border::default(),
        ..Default::default()
    }
}

/// Borderless icon button: no background box in any state. The glyph reads
/// muted at rest and tints amber on hover/press — the color is the affordance.
pub(crate) fn icon_button(theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme.extended_palette();
    let text_color = match status {
        button::Status::Hovered => colors::ACCENT_HOVER,
        button::Status::Pressed => colors::ACCENT,
        _ => palette.background.weak.text,
    };

    button::Style {
        background: None,
        text_color,
        border: Border::default(),
        ..Default::default()
    }
}
