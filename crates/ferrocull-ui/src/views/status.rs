//! Shared status-mark vocabulary: which marks apply to a frame, and how they
//! draw. The grid, preview, and compare views all render from here so the same
//! glyph, colour, and corner mean the same thing everywhere.

use ferrocull_core::media::Item;
use iced::{
    Element, Fill,
    widget::{Stack, container, text},
};

use crate::{
    styles,
    theme::{colors, spacing},
};

/// A status mark applicable to a frame, in the order marks are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    Rejected,
    Tagged,
    Ingested,
}

/// The marks that apply to a frame, ordered rejected, tagged, ingested.
///
/// Tag state is passed separately because tagging lives in UI state, not on
/// the item. The three states are independent: ingest normally untags, but
/// this must not assume it.
#[must_use]
pub(crate) fn marks(item: &Item, is_tagged: bool) -> Vec<Mark> {
    let mut marks = Vec::with_capacity(3);
    if item.rating == -1 {
        marks.push(Mark::Rejected);
    }
    if is_tagged {
        marks.push(Mark::Tagged);
    }
    if item.is_ingested {
        marks.push(Mark::Ingested);
    }
    marks
}

/// Badge row for a frame's status marks, anchored top-left of whatever it is
/// stacked over. `None` when no mark applies, so a clean frame draws no chrome
/// at all.
///
/// `size` is the glyph size; pill padding scales with it. The grid draws at
/// 10, the full-screen views at 12 — a mark sized for a 200px tile is lost on
/// a screen. `inset` is the gap to the corner, which each view sets to line
/// the row up with its own chrome.
#[must_use]
pub(crate) fn badge_row<Message: 'static>(
    item: &Item,
    is_tagged: bool,
    size: f32,
    inset: impl Into<iced::Padding>,
) -> Option<Element<'static, Message>> {
    let marks = marks(item, is_tagged);
    if marks.is_empty() {
        return None;
    }

    let row = marks.into_iter().fold(
        iced::widget::Row::new().spacing(spacing::XS),
        |row, mark| row.push(pill(mark, size)),
    );

    Some(
        container(row)
            .width(Fill)
            .height(Fill)
            .align_x(iced::alignment::Horizontal::Left)
            .align_y(iced::alignment::Vertical::Top)
            .padding(inset)
            .into(),
    )
}

/// Glyph size for the full-screen views. A mark sized for a 200px grid tile
/// is lost on a whole screen — this reads at a glance from a normal viewing
/// distance without competing with the photograph.
const FULLSCREEN_SIZE: f32 = 18.0;

/// `content` with the frame's status marks stacked over it, for the
/// full-screen views. Overlaying here rather than inside the zoom/pan viewer
/// anchors the marks to the viewport, so they stay put under zoom and pan;
/// stacking over whatever `content` is means they draw over the loading
/// spinner too, and state is legible before the pixels arrive.
#[must_use]
pub(crate) fn marked<Message: 'static>(
    content: Element<'static, Message>,
    item: &Item,
    is_tagged: bool,
    inset: impl Into<iced::Padding>,
) -> Element<'static, Message> {
    match badge_row(item, is_tagged, FULLSCREEN_SIZE, inset) {
        Some(badges) => Stack::new()
            .width(Fill)
            .height(Fill)
            .push(content)
            .push(badges)
            .into(),
        None => content,
    }
}

/// One mark as a pill. Rejected is a filled red badge; tagged and ingested are
/// ink on the shared warm-black badge fill, differing only in hue.
fn pill<Message: 'static>(mark: Mark, size: f32) -> Element<'static, Message> {
    let pad_y = size * 0.2;
    match mark {
        // U+2717 ballot X — the same reject mark the "Hide ✗" filter uses.
        Mark::Rejected => container(text("\u{2717}").size(size))
            .padding([pad_y, size * 0.6])
            .style(styles::rounded_badge(colors::BADGE_REJECTED))
            .into(),
        Mark::Tagged => container(text("\u{2713}").size(size).color(colors::ACCENT))
            .padding([pad_y, size * 0.5])
            .style(styles::overlay_badge)
            .into(),
        Mark::Ingested => container(text("\u{2913}").size(size).color(colors::BADGE_INGESTED))
            .padding([pad_y, size * 0.5])
            .style(styles::overlay_badge)
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::{TimeZone, Utc};
    use ferrocull_core::{
        FileCategory,
        media::{CaptureTime, Item},
    };

    use super::{Mark, marks};

    fn item(rating: i8, is_ingested: bool) -> Item {
        let second = Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0).unwrap();
        Item {
            path: PathBuf::from("/cards/A/DSC_0001.NEF"),
            source_id: "DSC_0001.NEF".to_owned(),
            size: 0,
            media_type: FileCategory::Raw,
            capture_time: CaptureTime::new(second, 0),
            is_ingested,
            jpeg_pair: None,
            paired: Vec::new(),
            sidecars: Vec::new(),
            xmp_sidecar: None,
            rating,
            color_label: None,
        }
    }

    #[test]
    fn clean_frame_has_no_marks() {
        assert!(marks(&item(0, false), false).is_empty());
    }

    #[test]
    fn tagged_frame_has_the_tagged_mark_alone() {
        assert_eq!(marks(&item(0, false), true), vec![Mark::Tagged]);
    }

    #[test]
    fn rejected_frame_has_the_rejected_mark_alone() {
        assert_eq!(marks(&item(-1, false), false), vec![Mark::Rejected]);
    }

    #[test]
    fn ingested_frame_has_the_ingested_mark_alone() {
        assert_eq!(marks(&item(0, true), false), vec![Mark::Ingested]);
    }

    #[test]
    fn tagged_and_rejected_shows_both_rejected_first() {
        assert_eq!(
            marks(&item(-1, false), true),
            vec![Mark::Rejected, Mark::Tagged]
        );
    }

    /// Ingest untags in practice, but the states are independent and the
    /// function must report both rather than assume the pairing is impossible.
    #[test]
    fn tagged_and_ingested_shows_both() {
        assert_eq!(
            marks(&item(0, true), true),
            vec![Mark::Tagged, Mark::Ingested]
        );
    }

    #[test]
    fn all_three_states_show_in_documented_order() {
        assert_eq!(
            marks(&item(-1, true), true),
            vec![Mark::Rejected, Mark::Tagged, Mark::Ingested]
        );
    }
}
