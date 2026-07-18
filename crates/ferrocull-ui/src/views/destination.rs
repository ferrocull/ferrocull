use iced::{
    Element,
    widget::{button, column, row, text, text_input},
};

use crate::{messages::destination::Message, styles, theme::spacing};

pub(crate) fn destination_panel(photos_dest: &str, videos_dest: &str) -> Element<'static, Message> {
    let photos_section = dest_section(
        "Photos Destination",
        "~/Pictures",
        photos_dest,
        Message::PhotosDestChanged,
        Message::BrowsePhotosDest,
    );
    let videos_section = dest_section(
        "Videos Destination",
        "~/Videos",
        videos_dest,
        Message::VideosDestChanged,
        Message::BrowseVideosDest,
    );

    column![photos_section, videos_section]
        .spacing(spacing::MD)
        .into()
}

fn dest_section(
    label: &'static str,
    placeholder: &'static str,
    value: &str,
    on_change: fn(String) -> Message,
    on_browse: Message,
) -> Element<'static, Message> {
    column![
        text(label).size(13),
        row![
            text_input(placeholder, value)
                .on_input(on_change)
                .size(12)
                .width(180),
            button(text("Browse").size(11))
                .on_press(on_browse)
                .padding([4, 8])
                .style(styles::primary_button),
        ]
        .spacing(spacing::XS),
    ]
    .spacing(spacing::XS)
    .into()
}
