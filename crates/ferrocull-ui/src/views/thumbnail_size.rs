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
        THUMBNAIL_SIZE_MAX, THUMBNAIL_SIZE_MIN, column_range, grid_metrics, nominal_for_columns,
    },
    widgets::WheelArea,
};

/// Width of the slider track, in logical pixels.
const TRACK_WIDTH: f32 = 120.0;

/// Thumbnail size slider, flanked by the grid densities its two ends produce.
///
/// `geometry` is the measured grid width and the window scale factor, which
/// together give the column counts the grid can show.
pub(crate) fn control(size: u32, geometry: Option<(f32, f32)>) -> Element<'static, Message> {
    let track = match geometry {
        Some((width, scale)) => column_track(size, width, scale),
        None => nominal_track(size),
    };

    let control = row![
        crate::icons::thumbnails_small().size(12),
        track,
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

/// One stop per column count the grid can show, so every tick of the handle
/// changes what is on screen. The stops run from the most columns on the left,
/// under the small-thumbnail icon, to the fewest on the right; each carries the
/// nominal size that lays the grid out in that many columns.
///
/// A grid too narrow to offer a second count collapses to a single stop. iced
/// draws that as a handle at the track start and reads any drag as the value it
/// already holds.
#[expect(
    clippy::cast_precision_loss,
    reason = "thumbnail sizes are three-digit integers, exact in f32"
)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "a column count fits a grid of screen width, far below u32::MAX"
)]
fn column_track(size: u32, width: f32, scale: f32) -> Element<'static, Message> {
    let range = column_range(width, scale);
    let most = *range.end();
    let last_position = (most - *range.start()) as u32;
    let position = (most - grid_metrics(width, size as f32, scale).0) as u32;

    slider(0..=last_position, position, move |position| {
        Message::ThumbnailSizeChanged(nominal_for_columns(width, most - position as usize))
    })
    .step(1u32)
    .on_release(Message::ThumbnailSizeReleased)
    .width(TRACK_WIDTH)
    .style(styles::thumbnail_size_slider)
    .into()
}

/// The nominal size itself, offered while no grid width has been measured, as
/// an empty grid reports none. There are no column counts to step through then, so
/// the handle runs over the size range instead and the preference can still be
/// set; the column track takes over on the first viewport report.
fn nominal_track(size: u32) -> Element<'static, Message> {
    slider(
        THUMBNAIL_SIZE_MIN..=THUMBNAIL_SIZE_MAX,
        size,
        Message::ThumbnailSizeChanged,
    )
    .on_release(Message::ThumbnailSizeReleased)
    .width(TRACK_WIDTH)
    .style(styles::thumbnail_size_slider)
    .into()
}
