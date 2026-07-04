use iced::{
    Element,
    widget::{checkbox, column, text},
};

use crate::{
    messages::destination::Message,
    theme::{colors, spacing},
};

/// Delete after download toggle with warning.
pub(crate) fn delete_panel(enabled: bool) -> Element<'static, Message> {
    let toggle = checkbox(enabled)
        .label("Delete source files after download")
        .on_toggle(|_| Message::DeleteAfterDownloadToggled)
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
