//! Thumbnail size slider, shown in the status bar.

use iced::{
    Element,
    widget::{row, slider, text, tooltip},
};

use crate::{
    messages::filters::Message,
    styles,
    theme::spacing,
    views::thumbnails::{THUMBNAIL_SIZE_MAX, THUMBNAIL_SIZE_MIN},
    widgets::WheelArea,
};

/// Thumbnail size slider, flanked by the grid densities its two ends produce.
pub(crate) fn control(size: u32) -> Element<'static, Message> {
    let control = row![
        crate::icons::thumbnails_small().size(12),
        slider(
            THUMBNAIL_SIZE_MIN..=THUMBNAIL_SIZE_MAX,
            size,
            Message::ThumbnailSizeChanged
        )
        .on_release(Message::ThumbnailSizeReleased)
        .width(120)
        .style(styles::thumbnail_size_slider),
        crate::icons::thumbnails_large().size(12),
    ]
    .spacing(spacing::XS)
    .align_y(iced::Alignment::Center);

    // The slider itself answers a wheel only with Ctrl held; the wrapper takes
    // every notch first, so a bare wheel over the control steps the size.
    let control = WheelArea::new(control).on_scroll(Message::ThumbnailSizeWheel);

    tooltip(
        control,
        text("Thumbnail size").size(11),
        tooltip::Position::Top,
    )
    .gap(4)
    .snap_within_viewport(true)
    .into()
}
