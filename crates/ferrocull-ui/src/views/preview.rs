//! Full-screen preview mode for culling workflow.

use std::path::PathBuf;

use ferrocull_core::media::Item;
use iced::{
    Alignment, Element, Fill,
    widget::{Space, button, center, column, container, row, text},
};
use iced_aw::Spinner;

use super::rating;
use crate::{
    styles,
    theme::spacing,
    widgets::{self, ViewState, Viewer},
};

/// Module-level events emitted by the preview overlay.
#[derive(Clone)]
pub(crate) enum Event {
    Close,
    Prev,
    Next,
    ViewStateChanged(widgets::Event),
    Item(PathBuf, rating::ItemEvent),
}

/// Renders the top bar with filename, position, and close button.
pub(crate) fn top_bar(item: &Item, index: usize, total: usize) -> Element<'static, Event> {
    let palette = crate::theme::palette();
    let filename = item
        .path
        .file_name()
        .expect("scanned file has filename")
        .to_string_lossy()
        .into_owned();

    let position_text = text(format!("{} / {}", index + 1, total))
        .size(13)
        .color(palette.background.weak.text);

    let filename_text = text(filename).size(13).color(palette.background.base.text);

    let close_btn = button(text("✕").size(14).color(palette.background.base.text))
        .padding([6, 12])
        .style(styles::ghost_button)
        .on_press(Event::Close);

    container(
        row![
            position_text,
            Space::new().width(Fill),
            filename_text,
            Space::new().width(Fill),
            close_btn,
        ]
        .align_y(Alignment::Center)
        .padding([spacing::SM, spacing::LG]),
    )
    .style(styles::preview_bar)
    .width(Fill)
    .into()
}

/// Renders the image viewer area.
pub(crate) fn image_area(
    preview_image: Option<&iced::widget::image::Handle>,
    view_state: ViewState,
) -> Element<'static, Event> {
    let Some(handle) = preview_image else {
        return center(Spinner::new().width(40.0).height(40.0).circle_radius(3.0))
            .width(Fill)
            .height(Fill)
            .into();
    };

    Viewer::new(handle.clone(), view_state, Event::ViewStateChanged)
        .min_scale(0.25)
        .max_scale(8.0)
        .scale_step(0.25)
        .width(Fill)
        .height(Fill)
        .into()
}

/// Renders the bottom bar with navigation and pre-mapped item controls.
pub(crate) fn bottom_bar(item_controls: Element<'static, Event>) -> Element<'static, Event> {
    let palette = crate::theme::palette();

    let nav_prev = button(text("‹").size(24).color(palette.background.base.text))
        .padding([10, 20])
        .style(styles::ghost_button)
        .on_press(Event::Prev);

    let nav_next = button(text("›").size(24).color(palette.background.base.text))
        .padding([10, 20])
        .style(styles::ghost_button)
        .on_press(Event::Next);

    container(
        row![
            nav_prev,
            Space::new().width(spacing::LG),
            item_controls,
            Space::new().width(Fill),
            nav_next,
        ]
        .align_y(Alignment::Center)
        .padding([spacing::SM, spacing::LG]),
    )
    .style(styles::preview_bar)
    .width(Fill)
    .into()
}

/// Assembles the full preview overlay from pre-built sub-elements.
pub(crate) fn compose(
    top: Element<'static, Event>,
    image: Element<'static, Event>,
    bottom: Element<'static, Event>,
) -> Element<'static, Event> {
    let content = column![
        top,
        container(image)
            .width(Fill)
            .height(Fill)
            .center_x(Fill)
            .center_y(Fill),
        bottom,
    ];

    container(content)
        .width(Fill)
        .height(Fill)
        .style(styles::preview_background)
        .into()
}
