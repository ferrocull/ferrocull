//! The burst badge: the pill that marks a frame as one of a run, worded for
//! the view showing it but built the same everywhere, so a burst reads as a
//! burst at a glance in the grid and in the preview alike.

use iced::{
    Element,
    widget::{button, text},
};

use crate::{media_view::BurstStatus, styles};

/// A stacked-frames cue with text presentation, never emoji-colored.
const GLYPH: &str = "\u{25A3}";

/// What the badge says on a grid cell: the run's total, carried by every
/// member so a burst reads the same folded or open.
pub(crate) fn count_label(count: usize) -> String {
    format!("{GLYPH} {count}")
}

/// What the badge says in a single-frame view, which shows one member at a
/// time: how many frames a folded burst holds, or which of them is on screen.
pub(crate) fn status_label(status: BurstStatus) -> String {
    match status {
        BurstStatus::Collapsed { count, .. } => count_label(count),
        BurstStatus::Expanded {
            position, count, ..
        } => format!("{GLYPH} {position} of {count}"),
    }
}

/// The pill itself, which folds and unfolds the burst when pressed.
pub(crate) fn badge<Message: Clone + 'static>(
    label: String,
    on_press: Message,
) -> Element<'static, Message> {
    button(text(label).size(10))
        .padding([2, 6])
        .style(styles::burst_badge)
        .on_press(on_press)
        .into()
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};

    use super::{BurstStatus, count_label, status_label};

    fn key() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 5, 1, 10, 14, 22)
            .single()
            .expect("unambiguous test timestamp")
    }

    #[test]
    fn a_folded_burst_badge_counts_the_frames_behind_it() {
        assert_eq!(
            status_label(BurstStatus::Collapsed {
                key: key(),
                count: 5,
            }),
            count_label(5),
            "a folded burst reads the same in either view"
        );
        assert_eq!(count_label(5), "\u{25A3} 5");
    }

    #[test]
    fn an_open_burst_badge_says_which_frame_is_on_screen() {
        assert_eq!(
            status_label(BurstStatus::Expanded {
                key: key(),
                position: 2,
                count: 5,
            }),
            "\u{25A3} 2 of 5"
        );
    }
}
