//! Thumbnail size slider, shown in the status bar.

use iced::{
    Element,
    widget::{row, slider, text, tooltip},
};

use crate::{
    messages::filters::Message,
    styles,
    theme::spacing,
    views::thumbnails::{
        THUMBNAIL_SIZE_MAX, THUMBNAIL_SIZE_MIN, column_range, columns_for, nominal_for_columns,
    },
    widgets::{keyboard_shield, wheel_area},
};

/// Width of the slider track, in logical pixels.
const TRACK_WIDTH: f32 = 120.0;

/// Thumbnail size slider, flanked by the grid densities its two ends produce.
///
/// `grid_width` is the measured grid width, which with `scale` gives the column
/// counts the grid can show; an empty grid has none measured yet.
pub(crate) fn control(size: u32, grid_width: Option<f32>, scale: f32) -> Element<'static, Message> {
    let track = grid_width.map_or_else(
        || nominal_track(size),
        |width| column_track(size, width, scale),
    );

    // The slider claims the arrow keys whenever the pointer rests on its track;
    // the shield keeps them for the application's own focus movement.
    let track = keyboard_shield(track);

    let control = row![
        crate::icons::thumbnails_small().size(12),
        track,
        crate::icons::thumbnails_large().size(12),
    ]
    .spacing(spacing::XS)
    .align_y(iced::Alignment::Center);

    // The slider itself answers a wheel only with Ctrl held; the wrapper takes
    // every notch first, so a bare wheel over the control steps the size.
    let control = wheel_area(control, Message::ThumbnailSizeWheel);

    tooltip(
        control,
        text("Thumbnail size").size(11),
        tooltip::Position::Top,
    )
    .gap(4)
    .snap_within_viewport(true)
    .into()
}

/// One stop per column count the grid can show, so every tick of the handle
/// changes what is on screen. The stops run from the most columns on the left,
/// under the small-thumbnail icon, to the fewest on the right; each carries the
/// nominal size that lays the grid out in that many columns.
///
/// A grid too narrow to offer a second count collapses to a single stop. iced
/// draws that as a handle at the track start and reads any drag as the value it
/// already holds.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a column count fits a grid of screen width, far below u32::MAX"
)]
fn column_track(size: u32, width: f32, scale: f32) -> Element<'static, Message> {
    let range = column_range(width, scale);
    let most = *range.end();
    let last_position = (most - *range.start()) as u32;
    let position = (most - columns_for(width, size, scale)) as u32;

    track(
        slider(0..=last_position, position, move |position| {
            Message::ThumbnailSizeChanged(nominal_for_columns(width, most - position as usize))
        })
        .step(1u32),
    )
}

/// The nominal size itself, offered while no grid width has been measured, as
/// an empty grid reports none. There are no column counts to step through then,
/// so the handle runs over the size range instead and the preference can still
/// be set; the column track takes over on the first layout.
fn nominal_track(size: u32) -> Element<'static, Message> {
    track(slider(
        THUMBNAIL_SIZE_MIN..=THUMBNAIL_SIZE_MAX,
        size,
        Message::ThumbnailSizeChanged,
    ))
}

/// The presentation both tracks share: the release that settles the size, the
/// track width, and the slider styling.
fn track(slider: slider::Slider<'static, u32, Message>) -> Element<'static, Message> {
    slider
        .on_release(Message::ThumbnailSizeReleased)
        .width(TRACK_WIDTH)
        .style(styles::thumbnail_size_slider)
        .into()
}
