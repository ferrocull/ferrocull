//! Full-screen preview mode for culling workflow.

use ferrocull_core::media::Item;
use iced::{
    Alignment, Element, Fill,
    widget::{Space, button, center, column, container, row, text},
};
use iced_aw::Spinner;

use crate::{
    media_view::BurstStatus,
    messages::{Message, preview},
    styles,
    theme::spacing,
    views::{self, status},
    widgets::{ViewState, Viewer},
};

/// Renders the top bar with filename, position, and close button.
pub(crate) fn top_bar(
    item: &Item,
    position: Option<usize>,
    total: usize,
) -> Element<'static, Message> {
    let palette = crate::theme::palette();
    let filename = item
        .path
        .file_name()
        .expect("item path has no filename")
        .to_string_lossy()
        .into_owned();

    // The item may not be in the filtered view (e.g. it was just rejected with
    // "hide rejected" on); show "–" rather than a misleading position.
    let position_label = position.map_or_else(|| "–".to_owned(), |p| (p + 1).to_string());
    let position_text = text(format!("{position_label} / {total}"))
        .size(13)
        .color(palette.background.weak.text);

    let filename_text = text(filename).size(13).color(palette.background.base.text);

    let close_btn = button(
        crate::icons::close()
            .size(14)
            .color(palette.background.base.text),
    )
        .padding([6, 12])
        .style(styles::ghost_button)
        .on_press(Message::Preview(preview::Message::Close));

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

/// Renders the image viewer area with the frame's status marks overlaid.
pub(crate) fn image_area(
    preview_image: Option<&iced::widget::image::Handle>,
    view_state: ViewState,
    item: &Item,
    is_tagged: bool,
) -> Element<'static, Message> {
    let content: Element<'static, Message> = preview_image.map_or_else(
        || {
            center(Spinner::new().width(40.0).height(40.0).circle_radius(3.0))
                .width(Fill)
                .height(Fill)
                .into()
        },
        |handle| {
            Viewer::new(handle.clone(), view_state, |e| {
                Message::Preview(preview::Message::ViewStateChanged(e))
            })
            .min_scale(0.25)
            .max_scale(8.0)
            .scale_step(0.25)
            .width(Fill)
            .height(Fill)
            .into()
        },
    );

    // The horizontal inset matches the top bar's padding so the row's left
    // edge lines up with the position text directly above it; the vertical one
    // is larger so the row clears the bar rather than hugging it.
    status::marked(content, item, is_tagged, [spacing::LG, spacing::LG])
}

/// The info strip's burst badge: the grid's pill, worded for the one frame on
/// screen, folding the same burst when pressed.
pub(crate) fn burst_badge(status: BurstStatus) -> Element<'static, Message> {
    views::burst::badge(
        views::burst::status_label(status),
        views::burst::Size::Strip,
        Message::Preview(preview::Message::ToggleBurst),
    )
}

/// Renders the bottom bar with navigation and pre-mapped item controls.
pub(crate) fn bottom_bar(item_controls: Element<'static, Message>) -> Element<'static, Message> {
    let palette = crate::theme::palette();

    let nav_prev = button(text("‹").size(24).color(palette.background.base.text))
        .padding([10, 20])
        .style(styles::ghost_button)
        .on_press(Message::Preview(preview::Message::Prev));

    let nav_next = button(text("›").size(24).color(palette.background.base.text))
        .padding([10, 20])
        .style(styles::ghost_button)
        .on_press(Message::Preview(preview::Message::Next));

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
///
/// `info` is the info-strip readout when the strip is open. It sits between the
/// image and the bottom bar, so it never covers image pixels and the bottom bar
/// keeps its single job of carrying the item controls.
pub(crate) fn compose(
    top: Element<'static, Message>,
    image: Element<'static, Message>,
    info: Option<Element<'static, Message>>,
    bottom: Element<'static, Message>,
) -> Element<'static, Message> {
    let content = column![
        top,
        container(image)
            .width(Fill)
            .height(Fill)
            .center_x(Fill)
            .center_y(Fill),
    ]
    .extend(info)
    .push(bottom);

    container(content)
        .width(Fill)
        .height(Fill)
        .style(styles::preview_background)
        .into()
}
