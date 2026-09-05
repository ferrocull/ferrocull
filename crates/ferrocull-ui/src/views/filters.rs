//! Filter bar with sort, type, grouping, rating, color label, and thumbnail
//! size controls.
//!

use std::collections::BTreeSet;

use ferrocull_core::{
    ColorLabel,
    media::{FilterMode, SortOrder},
};
use iced::{
    Border, Color, Element,
    widget::{Space, button, checkbox, container, pick_list, row, slider, text, tooltip},
};

use crate::{
    messages::filters::Message,
    styles,
    theme::{COLOR_LABELS, colors, radius, spacing},
    views::thumbnails::{THUMBNAIL_SIZE_MAX, THUMBNAIL_SIZE_MIN},
};

/// Visual divider between filter groups.
fn divider() -> Element<'static, Message> {
    container(Space::new().width(1))
        .height(20)
        .width(1)
        .style(|theme: &iced::Theme| container::Style {
            background: Some(theme.extended_palette().background.strong.color.into()),
            ..Default::default()
        })
        .into()
}

pub(crate) fn sort_controls(order: SortOrder, ascending: bool) -> Element<'static, Message> {
    let sort_picker = pick_list(SortOrder::ALL, Some(order), Message::SortChanged)
        .text_size(12)
        .padding([4, 8]);

    let arrow = if ascending {
        crate::icons::sort_ascending()
    } else {
        crate::icons::sort_descending()
    };
    let asc_btn = button(arrow.size(12))
        .padding([4, 8])
        .style(styles::secondary_button)
        .on_press(Message::AscendingToggled);

    row![sort_picker, asc_btn].spacing(2).into()
}

pub(crate) fn filter_mode_controls(mode: FilterMode) -> Element<'static, Message> {
    row(FilterMode::ALL.iter().map(|&m| {
        let is_selected = m == mode;
        let btn = button(text(m.to_string()).size(11))
            .padding([4, 10])
            .style(styles::filter_pill(is_selected));
        if is_selected {
            btn.into()
        } else {
            btn.on_press(Message::FilterChanged(m)).into()
        }
    }))
    .spacing(2)
    .into()
}

/// Labeled checkbox shared by every boolean axis in the bar. `on_toggle` of
/// `None` renders it disabled, for an axis that qualifies another one and can
/// mean nothing while that one is off.
fn bool_toggle(
    value: bool,
    label: &'static str,
    on_toggle: Option<Message>,
) -> Element<'static, Message> {
    let mut toggle = checkbox(value).label(label).text_size(11);
    if let Some(msg) = on_toggle {
        toggle = toggle.on_toggle(move |_| msg);
    }
    toggle.into()
}

/// Independent, stackable "not-yet-ingested only" toggle. Rendered as a boolean
/// toggle (like the grouping toggles), distinct from the single-select type
/// pills, since it composes with the type axis rather than replacing it.
pub(crate) fn new_toggle(new_only: bool) -> Element<'static, Message> {
    bool_toggle(new_only, "New", Some(Message::NewOnlyToggled))
}

#[expect(
    clippy::fn_params_excessive_bools,
    reason = "one flag per checkbox in the row, each an independent view axis"
)]
pub(crate) fn grouping_controls(
    group_raw_jpeg: bool,
    group_bursts: bool,
    expand_bursts: bool,
    hide_rejected: bool,
) -> Element<'static, Message> {
    // The reject mark can't ride inside the checkbox's label string (it is an
    // icon-font glyph), so the icon sits as a sibling of a "Hide" label.
    let hide_rejected_toggle = row![
        bool_toggle(hide_rejected, "Hide", Some(Message::HideRejectedToggled)),
        crate::icons::reject().size(10).color(colors::DANGER),
    ]
    .spacing(3)
    .align_y(iced::Alignment::Center);

    row![
        bool_toggle(group_raw_jpeg, "R+J", Some(Message::GroupRawJpegToggled)),
        bool_toggle(group_bursts, "Bursts", Some(Message::GroupBurstsToggled)),
        bool_toggle(
            expand_bursts,
            "Expand",
            group_bursts.then_some(Message::ExpandBurstsToggled)
        ),
        hide_rejected_toggle,
    ]
    .spacing(spacing::MD)
    .into()
}

pub(crate) fn rating_filter(selected: &BTreeSet<i8>) -> Element<'_, Message> {
    let rating_label = crate::icons::star_filled().size(13).color(colors::ACCENT);

    let rating_buttons = row((0..=5i8).map(|rating| {
        let is_selected = selected.contains(&rating);
        let label: Element<'static, Message> = if rating == 0 {
            crate::icons::unrated().size(11).into()
        } else {
            text(rating.to_string()).size(11).into()
        };
        button(label)
            .padding([3, 7])
            .style(styles::filter_pill(is_selected))
            .on_press(Message::RatingFilterToggled(rating))
            .into()
    }))
    .spacing(2);

    row![rating_label, rating_buttons]
        .spacing(spacing::XS)
        .align_y(iced::Alignment::Center)
        .into()
}

pub(crate) fn color_label_filter(selected: &BTreeSet<Option<ColorLabel>>) -> Element<'_, Message> {
    let palette = crate::theme::palette();
    let all_options = std::iter::once(None).chain(ColorLabel::ALL.map(Some));
    row(all_options.map(|label| {
        let is_selected = selected.contains(&label);
        let (bg_color, border_color) = label.map_or(
            (
                palette.background.weak.color,
                palette.background.strong.text,
            ),
            |l| (COLOR_LABELS[u8::from(l) as usize], Color::TRANSPARENT),
        );
        button(text("").size(6))
            .padding([7, 9])
            .style(color_swatch(bg_color, border_color, is_selected))
            .on_press(Message::ColorLabelFilterToggled(label))
            .into()
    }))
    .spacing(3)
    .into()
}

/// Thumbnail size slider, flanked by the grid densities its two ends produce.
pub(crate) fn thumbnail_size_control(size: u32) -> Element<'static, Message> {
    let control = row![
        crate::icons::thumbnails_small().size(12),
        slider(
            THUMBNAIL_SIZE_MIN..=THUMBNAIL_SIZE_MAX,
            size,
            Message::ThumbnailSizeChanged
        )
        .on_release(Message::ThumbnailSizeReleased)
        .width(90)
        .style(styles::thumbnail_size_slider),
        crate::icons::thumbnails_large().size(12),
    ]
    .spacing(spacing::XS)
    .align_y(iced::Alignment::Center);

    tooltip(
        control,
        text("Thumbnail size").size(11),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .snap_within_viewport(true)
    .into()
}

pub(crate) fn filter_bar<'a>(
    sort: Element<'a, Message>,
    filter_mode: Element<'a, Message>,
    new_toggle: Element<'a, Message>,
    grouping: Element<'a, Message>,
    rating: Element<'a, Message>,
    color_label: Element<'a, Message>,
    thumbnail_size: Element<'a, Message>,
) -> Element<'a, Message> {
    row![
        sort,
        divider(),
        filter_mode,
        divider(),
        new_toggle,
        divider(),
        grouping,
        Space::new().width(iced::Fill),
        rating,
        divider(),
        color_label,
        divider(),
        thumbnail_size,
    ]
    .spacing(spacing::MD)
    .align_y(iced::Alignment::Center)
    .into()
}

/// Button style for color label swatches.
fn color_swatch(
    bg_color: Color,
    border_color: Color,
    is_selected: bool,
) -> impl Fn(&iced::Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let (ring_color, ring_width) = if is_selected {
            (colors::ACCENT, 2.0)
        } else if border_color != Color::TRANSPARENT {
            (border_color, 1.0)
        } else if status == button::Status::Hovered {
            (Color::from_rgba(1.0, 1.0, 1.0, 0.3), 1.0)
        } else {
            (Color::TRANSPARENT, 0.0)
        };

        button::Style {
            background: Some(bg_color.into()),
            border: Border {
                color: ring_color,
                width: ring_width,
                radius: radius::XS.into(),
            },
            ..Default::default()
        }
    }
}
