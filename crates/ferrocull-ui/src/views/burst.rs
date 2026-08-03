//! The burst badge: the pill that marks a frame as one of a run, worded for
//! the view showing it but built the same everywhere, so a burst reads as a
//! burst at a glance in the grid and in the preview alike.

use iced::{
    Element,
    widget::{button, row, text},
};

use crate::{media_view::BurstStatus, styles, theme::spacing};

/// What the badge says on a grid cell: the total when folded, position over
/// total when open.
///
/// The grid spells a position `2/5` where the info strip spells it `2 of 5`.
/// The strip renders capture settings in photographic notation and the badge
/// sits immediately left of a shutter speed, so `2/5   1/250` would put two
/// unrelated fractions one gap apart; a grid cell has no such neighbour and is
/// space-starved at a thumbnail's corner. It is a close call rather than an
/// obvious split: the pill's fill, glyph, and text size already separate the
/// layers, so the strip's ambiguity would be a glance-level one.
pub(crate) fn cell_label(status: BurstStatus) -> String {
    match status {
        BurstStatus::Collapsed { count, .. } => count.to_string(),
        BurstStatus::Expanded {
            position, count, ..
        } => format!("{position}/{count}"),
    }
}

/// What the badge says in a single-frame view, which shows one member at a
/// time: how many frames a folded burst holds, or which of them is on screen.
pub(crate) fn strip_label(status: BurstStatus) -> String {
    match status {
        BurstStatus::Collapsed { count, .. } => count.to_string(),
        BurstStatus::Expanded {
            position, count, ..
        } => format!("{position} of {count}"),
    }
}

/// How prominent the pill is, set by where it sits. On a grid cell it is one
/// mark among many and stays at the label scale; leading a full-screen view's
/// info strip it is the frame's headline and takes the title scale.
#[derive(Clone, Copy)]
pub(crate) enum Size {
    Cell,
    Strip,
}

impl Size {
    /// Text size and symmetric `[vertical, horizontal]` padding for this scale.
    const fn metrics(self) -> (f32, [u16; 2]) {
        match self {
            Self::Cell => (10.0, [2, 6]),
            Self::Strip => (13.0, [3, 10]),
        }
    }
}

/// The pill itself, which folds and unfolds the burst when pressed.
pub(crate) fn badge<Message: Clone + 'static>(
    label: String,
    size: Size,
    on_press: Message,
) -> Element<'static, Message> {
    let (text_size, padding) = size.metrics();
    button(
        row![
            crate::icons::burst().size(text_size),
            text(label).size(text_size),
        ]
        .spacing(spacing::XS)
        .align_y(iced::Alignment::Center),
    )
    .padding(padding)
    .style(styles::burst_badge)
    .on_press(on_press)
    .into()
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};

    use super::{BurstStatus, cell_label, strip_label};

    fn key() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 5, 1, 10, 14, 22)
            .single()
            .expect("unambiguous test timestamp")
    }

    fn collapsed() -> BurstStatus {
        BurstStatus::Collapsed {
            key: key(),
            count: 5,
        }
    }

    fn expanded() -> BurstStatus {
        BurstStatus::Expanded {
            key: key(),
            position: 2,
            count: 5,
        }
    }

    #[test]
    fn a_folded_burst_badge_counts_the_frames_behind_it() {
        assert_eq!(
            strip_label(collapsed()),
            cell_label(collapsed()),
            "a folded burst reads the same in either view"
        );
        assert_eq!(cell_label(collapsed()), "5");
    }

    #[test]
    fn an_open_burst_badge_says_which_frame_is_on_screen() {
        assert_eq!(strip_label(expanded()), "2 of 5");
    }

    #[test]
    fn a_grid_cell_spells_the_position_as_a_fraction() {
        assert_eq!(cell_label(expanded()), "2/5");
    }
}
