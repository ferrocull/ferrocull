use iced::{
    Color, Element,
    widget::{button, column, row, text, text_input},
};

use crate::{styles, theme::spacing};

#[derive(Debug, Clone)]
pub(crate) enum Event {
    PhotosDestChanged(String),
    VideosDestChanged(String),
    BrowsePhotos,
    BrowseVideos,
}

pub(crate) fn destination_panel(photos_dest: &str, videos_dest: &str) -> Element<'static, Event> {
    let photos_section = dest_section(
        "Photos Destination",
        "~/Pictures",
        photos_dest,
        Event::PhotosDestChanged,
        Event::BrowsePhotos,
    );
    let videos_section = dest_section(
        "Videos Destination",
        "~/Videos",
        videos_dest,
        Event::VideosDestChanged,
        Event::BrowseVideos,
    );

    column![photos_section, videos_section]
        .spacing(spacing::MD)
        .into()
}

fn dest_section(
    label: &'static str,
    placeholder: &'static str,
    value: &str,
    on_change: fn(String) -> Event,
    on_browse: Event,
) -> Element<'static, Event> {
    column![
        text(label).size(13),
        row![
            text_input(placeholder, value)
                .on_input(on_change)
                .size(12)
                .width(180),
            button(text("Browse").size(11).color(Color::WHITE))
                .on_press(on_browse)
                .padding([4, 8])
                .style(styles::primary_button),
        ]
        .spacing(spacing::XS),
    ]
    .spacing(spacing::XS)
    .into()
}
