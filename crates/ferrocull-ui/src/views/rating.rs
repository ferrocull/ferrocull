use ferrocull_core::ColorLabel;
use iced::{
    Alignment, Color, Element, Fill,
    widget::{Space, button, container, mouse_area, row, text},
};

use crate::{
    styles,
    theme::{COLOR_LABELS, colors, spacing},
};

/// Events emitted by star rating interactions.
#[derive(Debug, Clone)]
pub(crate) enum StarEvent {
    Rated(i8),
    Hover(Option<i8>),
}

pub(crate) fn color_label_row(
    current_label: Option<ColorLabel>,
    swatch_size: f32,
) -> Element<'static, Option<ColorLabel>> {
    let swatch_spacing = if swatch_size > 14.0 { 4.0 } else { 2.0 };

    let swatches = row(ColorLabel::ALL.iter().map(|&label| {
        let color = COLOR_LABELS[u8::from(label) as usize];
        let is_selected = current_label == Some(label);
        let new_label = if is_selected { None } else { Some(label) };

        let radius = (swatch_size / 4.0).into();
        let border = if is_selected {
            iced::Border {
                radius,
                width: 2.0,
                color: Color::WHITE,
            }
        } else {
            iced::Border {
                radius,
                ..Default::default()
            }
        };

        let swatch = container("")
            .width(swatch_size)
            .height(swatch_size)
            .style(move |_theme| container::Style {
                background: Some(color.into()),
                border,
                ..Default::default()
            });

        mouse_area(swatch).on_press(new_label).into()
    }))
    .spacing(swatch_spacing);

    container(swatches).center_x(Fill).into()
}

/// `empty_color` is the hollow-star ink: callers on theme surfaces pass a
/// palette text color, callers on fixed dark badges pass the badge ink.
pub(crate) fn star_rating_row(
    current_rating: i8,
    hovered_star: Option<i8>,
    font_size: f32,
    empty_color: Color,
) -> Element<'static, StarEvent> {
    let display_rating = hovered_star.unwrap_or(current_rating);
    let filled_color = if hovered_star.is_some() {
        colors::RATING_STAR.scale_alpha(0.6)
    } else {
        colors::RATING_STAR
    };

    let star_spacing = if font_size > 14.0 { 4.0 } else { 2.0 };

    let stars = row((1..=5i8).map(|i| {
        let (symbol, color) = if i <= display_rating {
            (crate::icons::star_filled(), filled_color)
        } else {
            (crate::icons::star_outline(), empty_color)
        };

        let new_rating = if i == current_rating { 0 } else { i };
        let star_text = symbol.size(font_size).color(color);
        mouse_area(star_text)
            .on_press(StarEvent::Rated(new_rating))
            .on_enter(StarEvent::Hover(Some(i)))
            .into()
    }))
    .spacing(star_spacing);

    let row_with_exit = mouse_area(stars).on_exit(StarEvent::Hover(None));
    container(row_with_exit).center_x(Fill).into()
}

/// Events emitted by item editing controls (rating, color label, reject).
#[derive(Debug, Clone, Copy)]
pub(crate) enum ItemEvent {
    Rated(i8),
    ColorLabelSet(Option<ColorLabel>),
    Rejected,
    StarHover(Option<i8>),
}

/// Reusable item editing controls: star rating, color label swatches, reject button.
/// Returns a row element. The parent adds the item's path via `.map()`.
pub(crate) fn item_controls(
    rating: i8,
    color_label: Option<ColorLabel>,
    hovered_star: Option<i8>,
) -> Element<'static, ItemEvent> {
    let palette = crate::theme::palette();
    let is_rejected = rating == -1;

    let rating_widget = star_rating_row(rating, hovered_star, 18.0, palette.background.weak.text)
        .map(|e| match e {
            StarEvent::Rated(r) => ItemEvent::Rated(r),
            StarEvent::Hover(s) => ItemEvent::StarHover(s),
        });

    let color_widget = color_label_row(color_label, 14.0).map(ItemEvent::ColorLabelSet);

    let reject_style: fn(&iced::Theme, button::Status) -> button::Style = if is_rejected {
        styles::danger_button
    } else {
        styles::secondary_button
    };
    // No explicit ink: each style supplies its own readable text_color (white on
    // the danger fill when rejected, base text on the secondary fill otherwise).
    let reject_btn = button(text(if is_rejected { "Unmark" } else { "Reject (X)" }).size(11))
        .padding([6, 12])
        .style(reject_style)
        .on_press(ItemEvent::Rejected);

    row![
        rating_widget,
        Space::new().width(spacing::XS),
        color_widget,
        Space::new().width(spacing::MD),
        reject_btn,
    ]
    .align_y(Alignment::Center)
    .into()
}
