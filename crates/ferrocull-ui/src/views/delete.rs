use iced::{
    Element,
    widget::{checkbox, column, text},
};

use crate::theme::{colors, spacing};

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Toggled,
}

/// Delete after download toggle with warning.
pub(crate) fn delete_panel(enabled: bool) -> Element<'static, Event> {
    let toggle = checkbox(enabled)
        .label("Delete source files after download")
        .on_toggle(|_| Event::Toggled)
        .size(14)
        .text_size(12);

    let mut content = column![toggle].spacing(spacing::XS);

    if enabled {
        content = content.push(
            text("Warning: Files will be permanently deleted from source")
                .size(11)
                .color(colors::WARNING),
        );
    }

    content.into()
}
