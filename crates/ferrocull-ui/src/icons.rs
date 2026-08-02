//! Icon vocabulary: Bootstrap Icons glyphs behind intent-named constructors.
//!
//! Each function returns a text widget already carrying the icon font; size
//! and color stay with the caller, so per-state tinting works exactly like
//! any other text. Names describe the role in this UI, not the glyph, so a
//! set swap touches only this file.

use iced::widget::Text;
use iced_fonts::bootstrap;

/// Disclosure indicator on an expanded node.
pub(crate) fn chevron_expanded<'a>() -> Text<'a> {
    bootstrap::chevron_down()
}

/// The strip that marks a control as opening a menu. It stands for the menu
/// itself, not for whether the menu is open, so it never changes with state.
pub(crate) fn dropdown_handle<'a>() -> Text<'a> {
    bootstrap::chevron_down()
}

/// Disclosure indicator on a collapsed node.
pub(crate) fn chevron_collapsed<'a>() -> Text<'a> {
    bootstrap::chevron_right()
}

/// Step to the previous frame in preview and compare.
pub(crate) fn nav_previous<'a>() -> Text<'a> {
    bootstrap::chevron_left()
}

/// Step to the next frame in preview and compare.
pub(crate) fn nav_next<'a>() -> Text<'a> {
    bootstrap::chevron_right()
}

pub(crate) fn sort_ascending<'a>() -> Text<'a> {
    bootstrap::arrow_up()
}

pub(crate) fn sort_descending<'a>() -> Text<'a> {
    bootstrap::arrow_down()
}

pub(crate) fn star_filled<'a>() -> Text<'a> {
    bootstrap::star_fill()
}

pub(crate) fn star_outline<'a>() -> Text<'a> {
    bootstrap::star()
}

/// The zero-rating mark on the rating filter pill.
pub(crate) fn unrated<'a>() -> Text<'a> {
    bootstrap::slash_circle()
}

/// Compare's scroll-lock toggle, panning both panes together.
pub(crate) fn scroll_locked<'a>() -> Text<'a> {
    bootstrap::lock()
}

/// Compare's scroll-lock toggle, panning each pane on its own.
pub(crate) fn scroll_unlocked<'a>() -> Text<'a> {
    bootstrap::unlock()
}

/// Close affordance on full-screen views.
pub(crate) fn close<'a>() -> Text<'a> {
    bootstrap::x_lg()
}

/// The reject mark: badges, filter toggle, session tally.
pub(crate) fn reject<'a>() -> Text<'a> {
    bootstrap::x_lg()
}

/// The tag check: badges and the session tally.
pub(crate) fn tag_check<'a>() -> Text<'a> {
    bootstrap::check_lg()
}

/// The already-ingested mark: badges and the status bar.
pub(crate) fn ingested<'a>() -> Text<'a> {
    bootstrap::download()
}

/// Zoom-to-preview affordance on a hovered thumbnail.
pub(crate) fn zoom<'a>() -> Text<'a> {
    bootstrap::zoom_in()
}

pub(crate) fn settings<'a>() -> Text<'a> {
    bootstrap::gear()
}

pub(crate) fn undo<'a>() -> Text<'a> {
    bootstrap::arrow_counterclockwise()
}

/// The burst pill's stacked-frames cue.
pub(crate) fn burst<'a>() -> Text<'a> {
    bootstrap::stack()
}
