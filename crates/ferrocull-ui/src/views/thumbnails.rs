use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};

use chrono::{DateTime, Local, NaiveDate, Utc};
use ferrocull_core::{
    ColorLabel,
    media::{Item, SortKey, SortOrder},
};
use iced::{
    Color, ContentFit, Element, Fill, Shrink,
    widget::{
        Space, Stack, center, column, container, grid, image, mouse_area, responsive, row,
        scrollable, text,
    },
};

use super::rating::{StarEvent, star_rating_row};
use crate::{
    media_view::{BurstStatus, TagState},
    messages::filters::SizeStep,
    styles,
    theme::{COLOR_LABELS, colors, radius, spacing},
    views::{burst, status},
};

/// What happened inside a thumbnail card (no idx, no path — parent enriches).
#[derive(Clone)]
#[expect(
    variant_size_differences,
    reason = "BurstToggle carries a DateTime<Utc>; boxing would add an alloc per UI event"
)]
enum CellEvent {
    Clicked,
    DoubleClicked,
    HoverEnter,
    HoverExit,
    Rated(i8),
    StarHover(Option<i8>),
    BurstToggle(DateTime<Utc>),
}

/// What happened in the grid (enriched with idx/path by the cell builder closure).
#[derive(Clone)]
pub(crate) enum Event {
    CellClicked(PathBuf),
    CellDoubleClicked(usize),
    CellHover(usize, bool),
    Rated(PathBuf, i8),
    StarHover(Option<i8>),
    BurstToggle(DateTime<Utc>),
    /// Wheel scrolled over the grid — the parent snaps row-by-row.
    Wheel(iced::mouse::ScrollDelta),
    /// Viewport report from the scrollable: absolute y offset plus the grid's
    /// available width and the viewport/content heights (fires on scrolls,
    /// window resizes, and item loads). The heights let the parent tell a user
    /// scroll from an offset clamp caused by a geometry change.
    Scrolled {
        offset: f32,
        grid_width: f32,
        viewport_height: f32,
        content_height: f32,
    },
}

/// Smallest thumbnail size the photographer can choose, in logical pixels.
///
/// A card draws its info bar, star row, and badges at fixed text sizes, and the
/// floor keeps all three readable on the smallest cards the slider can produce.
/// Those are narrower than the chosen size: [`grid_metrics`] fits a whole number
/// of columns, so a rendered cell runs up to one column-share below it. Nothing
/// hides at any size.
pub(crate) const THUMBNAIL_SIZE_MIN: u32 = 150;
/// Largest thumbnail size the photographer can choose, in logical pixels.
pub(crate) const THUMBNAIL_SIZE_MAX: u32 = 448;
/// Widget ID for the thumbnail scrollable — used by `snap_to` to scroll to items.
pub(crate) const GRID_SCROLLABLE_ID: &str = "thumbnail-grid";

/// Column count and cell width for a given available content width and
/// `nominal` cell width (the chosen thumbnail size).
///
/// Mirrors iced's fluid column count (`ceil`) but floors the cell width to a
/// whole *physical* pixel (logical × `scale`) so every card edge lands on the
/// device pixel grid. iced's `crisp` snapping rounds each quad independently in
/// physical pixels at draw time; with fractional cell widths the card background
/// and the centered, `Contain`-fit image round to different edges, so photos
/// appear to drift ~1px at specific window widths. Whole-physical-pixel cells
/// make the snapping a no-op. Leftover space (under one logical pixel per
/// column) becomes a trailing margin.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "available is non-negative, so the column count is a small positive integer"
)]
#[expect(
    clippy::cast_precision_loss,
    reason = "column count is far below f32's 2^23 exact-integer range"
)]
pub(crate) fn grid_metrics(available: f32, nominal: f32, scale: f32) -> (usize, f32) {
    let cols = (((available + spacing::SM) / (nominal + spacing::SM)).ceil() as usize).max(1);
    let exact = (available - spacing::SM * (cols - 1) as f32) / cols as f32;
    let cell_width = (exact * scale).floor() / scale;
    (cols, cell_width)
}

/// The thumbnail size one notch away from `current`, clamped to the range.
///
/// The step is multiplicative (about 10 percent) so a notch feels the same at
/// either end of the range. Ten percent of even the smallest size the range
/// offers is well over a pixel, so every notch inside the range changes the
/// chosen size; the grid shows the change once the column count crosses a
/// threshold.
#[expect(
    clippy::cast_precision_loss,
    reason = "thumbnail sizes are three-digit integers, exact in f32"
)]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the scaled size is positive and stays within the u32 range"
)]
pub(crate) fn step_thumbnail_size(current: u32, direction: SizeStep) -> u32 {
    const FACTOR: f32 = 1.1;
    let stepped = match direction {
        SizeStep::Larger => (current as f32 * FACTOR).round() as u32,
        SizeStep::Smaller => (current as f32 / FACTOR).round() as u32,
    };
    clamp_thumbnail_size(stepped)
}

/// Bring a thumbnail size into the range the slider offers. Applied where a
/// persisted preference enters the app, so a hand-edited value cannot produce a
/// grid of one enormous column or of unreadable cards.
pub(crate) fn clamp_thumbnail_size(size: u32) -> u32 {
    size.clamp(THUMBNAIL_SIZE_MIN, THUMBNAIL_SIZE_MAX)
}

/// Total pinned width of the `cols` cells produced by [`grid_metrics`],
/// including inter-cell spacing.
#[expect(
    clippy::cast_precision_loss,
    reason = "column count is far below f32's 2^23 exact-integer range"
)]
fn grid_width(cols: usize, cell_width: f32) -> f32 {
    cell_width * cols as f32 + spacing::SM * (cols - 1) as f32
}

/// Fixed height of a date section header, so the update-side row model does not
/// depend on text metrics. Applied via `.height(...)` on the header container.
pub(crate) const DATE_HEADER_HEIGHT: f32 = 26.0;

/// Vertical space a date header takes above its section grid: the header itself
/// plus the `XS` gap the section column puts under it. Zero when not grouping,
/// which makes an ungrouped view one headerless section rather than a separate
/// layout path.
pub(crate) fn header_block(grouped: bool) -> f32 {
    if grouped {
        DATE_HEADER_HEIGHT + spacing::XS
    } else {
        0.0
    }
}

/// Extra content kept rendered above and below the viewport. A fast scroll then
/// reveals already-built cells, and the update side preloads their thumbnails
/// before they enter view. Replaces the old per-cell `sensor(...).anticipate`.
pub(crate) const GRID_OVERSCAN: f32 = 1000.0;

/// Half-a-pixel-plus tolerance so an already-aligned offset counts as sitting
/// exactly on a row boundary despite `f32` round-trips.
const ROW_EPS: f32 = 1.0;

/// One grid row's scroll anchor: the offset that lands it at the viewport top,
/// and the display-order index of its first card. Plain rows keep an `SM` gap
/// above them — exactly the inter-row spacing, so the previous row ends at the
/// viewport edge without its card bottoms bleeding in. Section-first rows
/// anchor their header with an `MD` gap; the `LG` section spacing above absorbs
/// it without bleed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RowStart {
    pub offset: f32,
    pub ordinal: usize,
}

/// Scroll anchors for every grid row, in display order.
///
/// `sections` is the `(start, count)` run per section from [`sections`], and
/// `header_block` the space each section's header takes above its grid. The
/// first row of each section anchors to the section top so its header stays
/// visible; later rows anchor to the row itself. Square cells make the row pitch
/// `cell_width + spacing::SM`.
#[expect(
    clippy::cast_precision_loss,
    reason = "row and column counts are far below f32's exact-integer range"
)]
pub(crate) fn row_starts(
    sections: &[(usize, usize)],
    cols: usize,
    cell_width: f32,
    header_block: f32,
) -> Vec<RowStart> {
    let pitch = cell_width + spacing::SM;
    let mut rows = Vec::new();
    // Content-y cursor; starts at the scrollable content's MD top padding.
    let mut y = spacing::MD;
    for &(start, count) in sections {
        let num_rows = count.div_ceil(cols);
        let header_top = y;
        let grid_top = header_top + header_block;
        for r in 0..num_rows {
            let row_top = grid_top + r as f32 * pitch;
            let offset = if r == 0 {
                header_top - spacing::MD
            } else {
                row_top - spacing::SM
            };
            rows.push(RowStart {
                offset,
                ordinal: start + r * cols,
            });
        }
        let grid_height =
            num_rows as f32 * cell_width + num_rows.saturating_sub(1) as f32 * spacing::SM;
        y = grid_top + grid_height + spacing::LG;
    }
    rows
}

/// Content-space top and bottom of row `row`. Row anchors are monotonic and sit
/// within a gap of each row's content top, so the next row's anchor doubles as
/// this row's bottom; the last row has no successor and spans one pitch
/// (`cell_width + spacing::SM`) instead.
pub(crate) fn row_bounds(rows: &[RowStart], row: usize, cell_width: f32) -> (f32, f32) {
    let top = rows[row].offset;
    let bottom = rows
        .get(row + 1)
        .map_or(top + cell_width + spacing::SM, |next| next.offset);
    (top, bottom)
}

/// Whether any part of row `row` shows in the viewport at `scroll_y`.
pub(crate) fn row_in_view(
    rows: &[RowStart],
    row: usize,
    scroll_y: f32,
    viewport_height: f32,
    cell_width: f32,
) -> bool {
    let (top, bottom) = row_bounds(rows, row, cell_width);
    top < scroll_y + viewport_height && bottom > scroll_y
}

/// `y` moved by the smallest amount that brings the row spanning
/// `row_top..row_bottom` into a viewport of `viewport_height`. A row above the
/// viewport aligns to its top, one below aligns to its bottom, and a row taller
/// than the viewport aligns to its top rather than pushing its start off screen.
pub(crate) fn keep_row_in_view(y: f32, row_top: f32, row_bottom: f32, viewport_height: f32) -> f32 {
    if row_top < y {
        row_top
    } else if row_bottom > y + viewport_height {
        (row_bottom - viewport_height).min(row_top)
    } else {
        y
    }
}

/// Whole wheel notches in `delta` plus whatever `carry` already held, leaving
/// the fraction in `carry` for the next event.
///
/// A direction reversal discards the fractional carry: hi-res wheels would
/// otherwise swallow the first notch of the new direction paying off the old
/// remainder.
#[expect(
    clippy::cast_possible_truncation,
    reason = "wheel notch accumulation stays far within i32"
)]
#[expect(
    clippy::cast_precision_loss,
    reason = "the carried remainder is a small whole notch count"
)]
pub(crate) fn take_whole_notches(carry: &mut f32, delta: f32) -> i32 {
    if *carry * delta < 0.0 {
        *carry = 0.0;
    }
    *carry += delta;
    let notches = carry.trunc() as i32;
    *carry -= notches as f32;
    notches
}

/// Display-order `(start, count)` runs: one per date section under Time sort,
/// a single section under any other sort. `start` is a position in display
/// order, not an index into `items`. Both the render and the scroll-anchor row
/// model build from this one call, so they cannot drift apart.
pub(crate) fn sections(
    items: &[Item],
    sorted_view: &BTreeMap<SortKey, usize>,
    ascending: bool,
    grouped: bool,
) -> Vec<(usize, usize)> {
    if !grouped {
        return if sorted_view.is_empty() {
            Vec::new()
        } else {
            vec![(0, sorted_view.len())]
        };
    }
    if ascending {
        date_sections(items, sorted_view.values().copied())
    } else {
        date_sections(items, sorted_view.values().rev().copied())
    }
}

/// Row index `steps` rows from `offset` (positive down, negative up). When
/// `offset` sits between rows, stepping re-aligns to the nearest boundary in the
/// direction of travel; clamps within the list, and returns `None` when there is
/// no row boundary in the direction of travel (already at the first/last).
pub(crate) fn step_row(rows: &[RowStart], offset: f32, steps: i32) -> Option<usize> {
    match steps.cmp(&0) {
        std::cmp::Ordering::Greater => {
            let first = rows.iter().position(|r| r.offset > offset + ROW_EPS)?;
            Some((first + (steps.unsigned_abs() as usize - 1)).min(rows.len() - 1))
        }
        std::cmp::Ordering::Less => {
            let last = rows.iter().rposition(|r| r.offset < offset - ROW_EPS)?;
            Some(last.saturating_sub(steps.unsigned_abs() as usize - 1))
        }
        std::cmp::Ordering::Equal => None,
    }
}

/// Row one viewport page from `row` (down or up): the farthest row whose offset
/// stays within `viewport_height` of the current row's offset, but always at
/// least one row of travel so paging makes progress on short viewports. Clamps
/// to the row list; `None` when `row` is already at the edge of travel.
pub(crate) fn page_row(
    rows: &[RowStart],
    row: usize,
    viewport_height: f32,
    down: bool,
) -> Option<usize> {
    if down {
        if row + 1 >= rows.len() {
            return None;
        }
        let limit = rows[row].offset + viewport_height;
        let candidate = rows
            .partition_point(|r| r.offset <= limit + ROW_EPS)
            .saturating_sub(1);
        Some(candidate.clamp(row + 1, rows.len() - 1))
    } else {
        if row == 0 {
            return None;
        }
        let limit = rows[row].offset - viewport_height;
        let candidate = rows.partition_point(|r| r.offset < limit - ROW_EPS);
        Some(candidate.min(row - 1))
    }
}

/// Row currently at the viewport top: the last anchor at or before `offset`.
pub(crate) fn anchor_row(rows: &[RowStart], offset: f32) -> Option<usize> {
    rows.iter().rposition(|r| r.offset <= offset + ROW_EPS)
}

/// Inclusive `[first, last]` row range whose rows intersect the viewport
/// (`scroll_y ..= scroll_y + viewport_height`) grown by `overscan` on each side.
/// Row anchors are monotonic and sit within a gap of each row's content top, so
/// they double as the intersection key. `viewport_height <= 0.0` marks the
/// viewport size as not yet reported — the whole grid is then in-window. Returns
/// `None` only for an empty row list.
pub(crate) fn visible_row_window(
    rows: &[RowStart],
    scroll_y: f32,
    viewport_height: f32,
    overscan: f32,
) -> Option<(usize, usize)> {
    if rows.is_empty() {
        return None;
    }
    if viewport_height <= 0.0 {
        return Some((0, rows.len() - 1));
    }
    let lo = scroll_y - overscan;
    let hi = scroll_y + viewport_height + overscan;
    // The row before the first anchor past `lo` straddles the top edge.
    let first = rows.partition_point(|r| r.offset <= lo).saturating_sub(1);
    // The last anchor at or before `hi` is the last visible row.
    let last = rows
        .partition_point(|r| r.offset <= hi)
        .saturating_sub(1)
        .max(first);
    Some((first, last))
}

/// Top and bottom spacer heights that sandwich the visible rows `first..=last`
/// of a `num_rows`-tall grid so the rendered sub-grid sits exactly where those
/// rows sit in the full grid. The pair plus the visible sub-grid's own height
/// sum to the full grid height, so the scrollable's content height never
/// changes.
#[expect(
    clippy::cast_precision_loss,
    reason = "row counts are far below f32's exact-integer range"
)]
pub(crate) fn row_run_spacers(
    num_rows: usize,
    cell_width: f32,
    first: usize,
    last: usize,
) -> (f32, f32) {
    let pitch = cell_width + spacing::SM;
    let grid_height =
        num_rows as f32 * cell_width + (num_rows.saturating_sub(1)) as f32 * spacing::SM;
    let top = first as f32 * pitch;
    let visible = (last - first + 1) as f32 * cell_width + (last - first) as f32 * spacing::SM;
    (top, grid_height - top - visible)
}

/// Grid scroll geometry from one viewport report, retained to interpret the
/// next one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GridGeometry {
    pub width: f32,
    pub viewport_height: f32,
    pub content_height: f32,
    pub scroll_y: f32,
}

/// What a viewport report means for the pinned anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScrollReaction {
    /// The geometry reflowed or iced clamped the offset — re-pin the stored
    /// anchor card to the viewport top under the new geometry.
    Reanchor,
    /// A genuine user scroll at unchanged geometry — adopt the reported offset
    /// as the new anchor.
    AdoptOffset,
    /// Nothing that moves the anchor happened.
    Idle,
}

/// Below this, an offset or height delta is float noise, not a real change.
const GEOM_EPS: f32 = 0.5;

/// Classify a viewport report against the previous one.
///
/// iced reports scrolls, window resizes, and content growth through the same
/// channel, and on any reflow it clamps the absolute offset against the new
/// content *before* reporting — so a bare offset move is ambiguous. The
/// geometry deltas disambiguate:
///
/// - **Width changed** — the column count reflowed, so every row sits at a new
///   offset. Re-pin the anchor regardless of whether the offset moved.
/// - **Offset moved while a height changed** — a clamp from a vertical resize
///   or content reflow, never a user scroll (scrolling does not resize the
///   viewport or the content). Re-pin; do not read the clamped offset as
///   intent.
/// - **Offset moved with all geometry unchanged** — the only true user scroll
///   (drag, touchpad, keyboard). Adopt it.
///
/// The first report (`prev` is `None`) only seeds the geometry.
pub(crate) fn scroll_reaction(
    prev: Option<GridGeometry>,
    offset: f32,
    width: f32,
    viewport_height: f32,
    content_height: f32,
) -> ScrollReaction {
    let Some(prev) = prev else {
        return ScrollReaction::Idle;
    };
    let moved = (offset - prev.scroll_y).abs() > GEOM_EPS;
    let width_changed = (width - prev.width).abs() > GEOM_EPS;
    let heights_changed = (viewport_height - prev.viewport_height).abs() > GEOM_EPS
        || (content_height - prev.content_height).abs() > GEOM_EPS;
    if width_changed || (moved && heights_changed) {
        ScrollReaction::Reanchor
    } else if moved {
        ScrollReaction::AdoptOffset
    } else {
        ScrollReaction::Idle
    }
}

/// Row containing card `ordinal`: the last row whose first card is at or before
/// it. Used to re-anchor the same card after a column-count reflow.
pub(crate) fn row_for_ordinal(rows: &[RowStart], ordinal: usize) -> Option<usize> {
    rows.iter().rposition(|r| r.ordinal <= ordinal)
}

/// The Local capture date a card is grouped under.
pub(crate) fn capture_date(item: &Item) -> NaiveDate {
    item.capture_time.second.with_timezone(&Local).date_naive()
}

/// Formats a date for display as a section header.
/// "Today", "Yesterday", weekday name for this week, or "Jan 5" for older.
fn format_date_header(date: NaiveDate, today: NaiveDate) -> Cow<'static, str> {
    if date == today {
        "Today".into()
    } else if date == today.pred_opt().expect("today is NaiveDate::MIN") {
        "Yesterday".into()
    } else {
        let days_ago = (today - date).num_days();
        if days_ago > 0 && days_ago < 7 {
            date.format("%A").to_string().into()
        } else {
            date.format("%b %-d").to_string().into()
        }
    }
}

/// Visual interaction state for a thumbnail cell.
#[derive(Clone, Copy)]
struct CellState {
    tag: TagState,
    is_hovered: bool,
    is_focused: bool,
}

/// Renders a virtualized thumbnail grid: only the rows intersecting the viewport
/// (plus [`GRID_OVERSCAN`]) are built as widgets; the rest is collapsed into
/// fixed-height spacers so the scrollable's content height and every row offset
/// stay exactly what the un-virtualized grid produced (see [`row_starts`]).
///
/// `sorted_view` holds pre-filtered, pre-sorted indices; cells borrow straight
/// from `items` so no per-render clone of the item store is needed. `tag_state`
/// and `burst_status` answer what a cell's tag mark and burst badge should say:
/// the caller owns burst grouping and pair hiding, so this view never re-derives
/// them, and both queries run for visible cells only. Click always emits
/// `Event::CellClicked(path)`; the caller decides focus vs. selection based on
/// modifier state. `thumbnail_size` is the nominal cell width the columns are
/// laid out against, and `window_scale` pins cell widths to whole physical
/// pixels (see [`grid_metrics`]). `scroll_y`/`viewport_height` are the tracked scroll
/// window; `viewport_height` is `0.0` until the first scroll report, which
/// means "unknown" — the grid then renders every row.
#[expect(
    clippy::too_many_arguments,
    reason = "grid render state has no domain grouping; a param bag would just relocate it"
)]
pub(crate) fn thumbnail_grid<'a>(
    items: &'a [Item],
    sorted_view: &'a BTreeMap<SortKey, usize>,
    tag_state: impl Fn(usize) -> TagState + 'a,
    burst_status: impl Fn(usize) -> Option<BurstStatus> + 'a,
    loaded_thumbs: &'a HashMap<PathBuf, image::Handle>,
    today: NaiveDate,
    thumbnail_size: u32,
    window_scale: f32,
    sort_order: SortOrder,
    ascending: bool,
    group_raw_jpeg: bool,
    hovered_thumbnail: Option<usize>,
    hovered_star: Option<i8>,
    focused_index: Option<usize>,
    scroll_y: f32,
    viewport_height: f32,
) -> Element<'a, Event> {
    // Empty state needs none of the grid/scrollable machinery.
    if sorted_view.is_empty() {
        return center(
            text(
                "Insert a memory card — sources appear automatically.\n\
                 Or add a folder with \u{201c}Add Directory\u{2026}\u{201d} in the Sources panel.",
            )
            .size(14)
            .align_x(iced::Alignment::Center),
        )
        .width(Fill)
        .height(Fill)
        .into();
    }

    let group_by_date = sort_order == SortOrder::Time;

    // `responsive` runs at layout time, so `size.width` is the true grid area:
    // iced has already subtracted the container's `MD` padding and, when the
    // content overflows, the embedded scrollbar's gutter.
    #[expect(
        clippy::cast_precision_loss,
        reason = "thumbnail sizes are three-digit integers, exact in f32"
    )]
    let nominal = thumbnail_size as f32;

    let grid = responsive(move |size| {
        let (cols, cell_width) = grid_metrics(size.width, nominal, window_scale);

        // Center the grid by splitting the leftover into a side margin floored
        // to the physical-pixel grid — a fractional offset would shift every
        // card off the pixel grid and reintroduce sub-pixel drift.
        let side_margin = (((size.width - grid_width(cols, cell_width)) / 2.0) * window_scale)
            .floor()
            .max(0.0)
            / window_scale;

        // Cells borrow from `items`; the map closure owns a cloned path so the
        // element is `'static` (no borrow escapes the cell). Only visible cells
        // are ever built, so this clone is O(visible), not O(items).
        let build_cell = |idx: usize| -> Element<'a, Event> {
            let item = &items[idx];
            let is_hovered = hovered_thumbnail == Some(idx);
            let cell_hovered_star = if is_hovered { hovered_star } else { None };
            let state = CellState {
                tag: tag_state(idx),
                is_hovered,
                is_focused: focused_index == Some(idx),
            };
            let show_pair = group_raw_jpeg && item.jpeg_pair.is_some();
            let burst = burst_status(idx);

            let path = item.path.clone();
            let cell = thumbnail_card(
                loaded_thumbs.get(&item.path),
                item,
                state,
                show_pair,
                burst,
                cell_hovered_star,
            );

            cell.map(move |e| match e {
                CellEvent::Clicked => Event::CellClicked(path.clone()),
                CellEvent::DoubleClicked => Event::CellDoubleClicked(idx),
                CellEvent::HoverEnter => Event::CellHover(idx, true),
                CellEvent::HoverExit => Event::CellHover(idx, false),
                CellEvent::Rated(r) => Event::Rated(path.clone(), r),
                CellEvent::StarHover(s) => Event::StarHover(s),
                CellEvent::BurstToggle(key) => Event::BurstToggle(key),
            })
        };

        // Display-order item indices; borrowed, no item clone.
        let order: Vec<usize> = if ascending {
            sorted_view.values().copied().collect()
        } else {
            sorted_view.values().rev().copied().collect()
        };

        let sections = sections(items, sorted_view, ascending, group_by_date);

        // The same row model and window function the update side uses to decide
        // which thumbnails to load (`window_item_indices`), so the rendered
        // rows and the loaded thumbnails cannot drift apart.
        let rows = row_starts(&sections, cols, cell_width, header_block(group_by_date));
        let row_window = visible_row_window(&rows, scroll_y, viewport_height, GRID_OVERSCAN)
            .expect("a non-empty sorted view yields rows");

        let cells = build_sections(
            items,
            &order,
            &sections,
            cols,
            cell_width,
            group_by_date,
            today,
            row_window,
            &build_cell,
        );

        container(cells)
            .padding(iced::padding::horizontal(side_margin))
            .width(Fill)
            .into()
    })
    .height(Shrink);

    // The embedded scrollbar (`spacing`) insets the content, so the
    // `responsive` above measures the true grid width. Zero spacing keeps the
    // gutter to the rail itself, giving cards the same `MD` gap to the rail as
    // to the left edge. The gutter exists only while the content overflows, so
    // at an exact-fit height the measured width can flip between passes.
    //
    // The `mouse_area` inside the scrollable steals the wheel from the
    // scrollable's own handler (children see wheel events first), so the parent
    // can snap row-by-row; scrollbar drag and keyboard scrolling still reach the
    // scrollable and report back through `on_scroll`.
    //
    // `on_scroll` doubles as the grid-width channel: it re-fires on any redraw
    // where the viewport or content bounds changed (scrolls, window resizes,
    // item loads), and `content_bounds` minus this container's `MD` padding is
    // exactly the `available` the `responsive` closure receives. A content-
    // wrapping `sensor` cannot do this job: iced gates `on_resize` on the
    // distance from the viewport to the sensor's *corners*, so a sensor the
    // size of the grid goes silent once the user scrolls off the top.
    scrollable(mouse_area(container(grid).padding(spacing::MD).width(Fill)).on_scroll(Event::Wheel))
        .id(GRID_SCROLLABLE_ID)
        .on_scroll(|vp| Event::Scrolled {
            offset: vp.absolute_offset().y,
            grid_width: vp.content_bounds().width - 2.0 * spacing::MD,
            viewport_height: vp.bounds().height,
            content_height: vp.content_bounds().height,
        })
        .spacing(0)
        .width(Fill)
        .height(Fill)
        .into()
}

/// Split display order into contiguous `(start, count)` runs sharing a capture
/// date. `order` yields item indices in display order; `start` is a position
/// within that sequence, not an index into `items`.
fn date_sections(items: &[Item], order: impl IntoIterator<Item = usize>) -> Vec<(usize, usize)> {
    let mut sections: Vec<(usize, usize)> = Vec::new();
    let mut current = None;
    for (position, index) in order.into_iter().enumerate() {
        let date = capture_date(&items[index]);
        if current == Some(date) {
            sections
                .last_mut()
                .expect("no open section for the current date")
                .1 += 1;
        } else {
            sections.push((position, 1));
            current = Some(date);
        }
    }
    sections
}

/// Build the section column, rendering only rows within `row_window` (global
/// row indices from [`visible_row_window`], in [`row_starts`] numbering) and
/// collapsing everything else into spacers so the total height is unchanged.
#[expect(
    clippy::too_many_arguments,
    reason = "layout inputs that a param bag would only relocate"
)]
#[expect(
    clippy::cast_precision_loss,
    reason = "row counts are far below f32's exact-integer range"
)]
fn build_sections<'a>(
    items: &'a [Item],
    order: &[usize],
    sections: &[(usize, usize)],
    cols: usize,
    cell_width: f32,
    grouped: bool,
    today: NaiveDate,
    row_window: (usize, usize),
    build_cell: &dyn Fn(usize) -> Element<'a, Event>,
) -> Element<'a, Event> {
    let header_block = header_block(grouped);

    let mut section_els: Vec<Element<'a, Event>> = Vec::with_capacity(sections.len());
    // Global index of the current section's first row, in `row_starts` numbering.
    let mut row_base = 0usize;

    for &(start, count) in sections {
        let num_rows = count.div_ceil(cols);
        let grid_height =
            num_rows as f32 * cell_width + (num_rows.saturating_sub(1)) as f32 * spacing::SM;
        let section_height = header_block + grid_height;
        let base = row_base;
        row_base += num_rows;

        // This section's rows intersected with the window, in section-local
        // row indices. An empty intersection collapses the whole section.
        let sec_first = row_window.0.max(base);
        let sec_last = row_window.1.min(base + num_rows - 1);
        if sec_first > sec_last {
            section_els.push(Space::new().height(section_height).into());
            continue;
        }
        let (first_row, last_row) = (sec_first - base, sec_last - base);
        let (top_sp, bottom_sp) = row_run_spacers(num_rows, cell_width, first_row, last_row);
        let slice_start = start + first_row * cols;
        let slice_end = (start + (last_row + 1) * cols).min(start + count);
        let cells = grid(
            order[slice_start..slice_end]
                .iter()
                .map(|&idx| build_cell(idx)),
        )
        .spacing(spacing::SM)
        .columns(cols)
        .width(grid_width(cols, cell_width));
        let body = column![
            Space::new().height(top_sp),
            cells,
            Space::new().height(bottom_sp),
        ];

        if grouped {
            let palette = crate::theme::palette();
            let date = capture_date(&items[order[start]]);
            let header = container(
                text(format_date_header(date, today))
                    .size(13)
                    .color(palette.background.base.text),
            )
            .padding([spacing::XS, spacing::SM])
            .height(DATE_HEADER_HEIGHT)
            .style(styles::date_header);
            section_els.push(column![header, body].spacing(spacing::XS).into());
        } else {
            section_els.push(body.into());
        }
    }

    column(section_els).spacing(spacing::LG).into()
}

/// Renders a thumbnail card with image, overlays, badges, and interaction handlers.
fn thumbnail_card(
    thumb: Option<&image::Handle>,
    item: &Item,
    state: CellState,
    show_pair: bool,
    burst: Option<BurstStatus>,
    hovered_star: Option<i8>,
) -> Element<'static, CellEvent> {
    let palette = crate::theme::palette();
    // The wash claims the whole card is tagged, so a partially tagged group
    // does not get one — its outline badge carries the state alone.
    let card_bg = if item.rating == -1 {
        colors::REJECTED_BG
    } else if state.tag == TagState::Tagged {
        crate::theme::tagged_wash()
    } else {
        palette.background.weak.color
    };

    let image_content: Element<'static, CellEvent> = thumb.map_or_else(placeholder, |handle| {
        image(handle.clone())
            .content_fit(ContentFit::Contain)
            .width(Fill)
            .height(Fill)
            .into()
    });

    let padded_image: Element<'static, CellEvent> =
        container(image_content).width(Fill).height(Fill).into();

    let stack = cell_overlays(padded_image, item, state, show_pair, burst, hovered_star);

    let border = iced::Border {
        radius: radius::MD.into(),
        width: 2.0,
        color: if state.is_focused {
            crate::theme::focus_color_for(item.rating == -1)
        } else {
            Color::TRANSPARENT
        },
    };
    let card = container(stack)
        .padding(border.width)
        .width(Fill)
        .height(Fill)
        .style(styles::thumbnail_card(card_bg, border));

    mouse_area(card)
        .on_press(CellEvent::Clicked)
        .on_double_click(CellEvent::DoubleClicked)
        .on_enter(CellEvent::HoverEnter)
        .on_exit(CellEvent::HoverExit)
        .into()
}

/// Build status overlays, badges, and hover info for a thumbnail cell.
fn cell_overlays(
    base: Element<'static, CellEvent>,
    item: &Item,
    state: CellState,
    show_pair: bool,
    burst: Option<BurstStatus>,
    hovered_star: Option<i8>,
) -> Stack<'static, CellEvent> {
    let mut stack = Stack::new().width(Fill).height(Fill).push(base);

    if item.rating != -1 && item.is_ingested {
        stack = stack.push(color_overlay(colors::OVERLAY_INGESTED));
    }

    // Top-left status badges: rejected, tagged, and ingested share one row so
    // every applicable state stays visible. The ingested dim above is the
    // fast-scan cue; this pill is the guaranteed mark over any photo.
    if let Some(badges) = status::badge_row(item, state.tag, 10.0, spacing::XS) {
        stack = stack.push(badges);
    }

    if show_pair {
        stack = stack.push(pair_badge());
    }

    if let Some(status) = burst {
        stack = stack.push(burst_badge(status));
    }

    if let Some(label) = item.color_label {
        stack = stack.push(color_label_bar(label));
    }

    if !state.is_hovered && item.rating > 0 {
        stack = stack.push(rated_badge(item.rating));
    }

    if state.is_hovered {
        let filename = item
            .path
            .file_name()
            .expect("item path has no filename")
            .to_string_lossy()
            .into_owned();

        stack = stack.push(bottom_info_overlay(item.rating, hovered_star, filename));

        stack = stack.push(preview_icon());
    }

    stack
}

/// Bottom overlay with stars and filename on semi-transparent background.
fn bottom_info_overlay(
    rating: i8,
    hovered_star: Option<i8>,
    filename: String,
) -> Element<'static, CellEvent> {
    // On the fixed dark info bar every glyph uses the explicit badge ink —
    // theme text would go dark-on-dark in the light theme.
    let stars = star_rating_row(rating, hovered_star, 12.0, colors::BADGE_TEXT).map(|e| match e {
        StarEvent::Rated(r) => CellEvent::Rated(r),
        StarEvent::Hover(s) => CellEvent::StarHover(s),
    });
    let name = text(filename).size(11).color(colors::BADGE_TEXT);

    let info_column = column![stars, name]
        .spacing(1)
        .align_x(iced::Alignment::Center);

    let info_bar = container(info_column)
        .width(Fill)
        .padding(4)
        .style(styles::solid_fill(colors::OVERLAY_BADGE));

    container(info_bar)
        .width(Fill)
        .height(Fill)
        .align_y(iced::alignment::Vertical::Bottom)
        .into()
}

/// Quiet skeleton for a thumbnail still decoding — a flat tonal tile, no
/// glyph. Undecodable files never reach a distinct state here: extraction
/// failures degrade to a cache miss upstream, so "pending" is the only case.
fn placeholder<Message: 'static>() -> Element<'static, Message> {
    container(Space::new())
        .width(Fill)
        .height(Fill)
        .style(styles::skeleton_tile)
        .into()
}

/// "R+J" badge positioned in bottom-right corner for RAW+JPEG pairs.
fn pair_badge<Message: 'static>() -> Element<'static, Message> {
    let badge = container(text("R+J").size(11))
        .padding([2, 4])
        .style(styles::overlay_badge);

    container(badge)
        .width(Fill)
        .height(Fill)
        .align_x(iced::alignment::Horizontal::Right)
        .align_y(iced::alignment::Vertical::Bottom)
        .padding(spacing::XS)
        .into()
}

/// A full-cell wash. Transparent to input, so presses reach the card's
/// `mouse_area` and a washed frame still focuses and opens in preview.
fn color_overlay<Message: 'static>(color: Color) -> Element<'static, Message> {
    container("")
        .width(Fill)
        .height(Fill)
        .style(styles::solid_fill(color))
        .into()
}

/// Rating indicator badge positioned in bottom-left corner (shown when not hovered).
fn rated_badge<Message: 'static>(rating: i8) -> Element<'static, Message> {
    let badge = container(
        row![
            crate::icons::star_filled()
                .size(9)
                .color(colors::RATING_STAR),
            text(rating.to_string()).size(11).color(colors::RATING_STAR),
        ]
        .spacing(2)
        .align_y(iced::Alignment::Center),
    )
    .padding([2, 4])
    .style(styles::overlay_badge);

    container(badge)
        .width(Fill)
        .height(Fill)
        .align_x(iced::alignment::Horizontal::Left)
        .align_y(iced::alignment::Vertical::Bottom)
        .padding(spacing::XS)
        .into()
}

/// Burst badge positioned in top-right corner.
fn burst_badge(status: BurstStatus) -> Element<'static, CellEvent> {
    let clickable_badge = burst::badge(
        burst::cell_label(status),
        burst::Size::Cell,
        CellEvent::BurstToggle(status.key()),
    );

    container(clickable_badge)
        .width(Fill)
        .height(Fill)
        .align_x(iced::alignment::Horizontal::Right)
        .align_y(iced::alignment::Vertical::Top)
        .padding(spacing::XS)
        .into()
}

/// Color label bar at the bottom of the thumbnail card.
fn color_label_bar<Message: 'static>(label: ColorLabel) -> Element<'static, Message> {
    // Light theme uses darkened variants so the bar reads as a mark on
    // warm-white; hue identity (user XMP data) is preserved.
    let labels = if crate::theme::palette().is_dark {
        &COLOR_LABELS
    } else {
        &crate::theme::COLOR_LABELS_LIGHT
    };
    let color = labels[u8::from(label) as usize];

    let bar = container("")
        .width(Fill)
        .height(4.0)
        .style(styles::solid_fill(color));

    container(bar)
        .width(Fill)
        .height(Fill)
        .align_y(iced::alignment::Vertical::Bottom)
        .into()
}

/// Preview affordance (zoom-in glyph) positioned in bottom-right corner on
/// hover.
fn preview_icon() -> Element<'static, CellEvent> {
    let icon = container(crate::icons::zoom().size(14))
        .padding([4, 6])
        .style(styles::rounded_badge(
            colors::OVERLAY_BADGE.scale_alpha(0.7),
        ));

    container(mouse_area(icon).on_press(CellEvent::DoubleClicked))
        .width(Fill)
        .height(Fill)
        .align_x(iced::alignment::Horizontal::Right)
        .align_y(iced::alignment::Vertical::Bottom)
        .padding(spacing::XS)
        .into()
}

#[cfg(test)]
mod tests {
    use super::{
        DATE_HEADER_HEIGHT, RowStart, THUMBNAIL_SIZE_MAX, THUMBNAIL_SIZE_MIN, anchor_row,
        clamp_thumbnail_size, grid_metrics, grid_width, header_block, keep_row_in_view, row_bounds,
        row_for_ordinal, row_in_view, row_run_spacers, row_starts, step_row, step_thumbnail_size,
        take_whole_notches, visible_row_window,
    };
    use crate::{messages::filters::SizeStep, theme::spacing};

    // A round cell width keeps the expected offsets easy to read.
    const CW: f32 = 100.0;
    const PITCH: f32 = CW + spacing::SM; // 108

    fn offsets(rows: &[RowStart]) -> Vec<f32> {
        rows.iter().map(|r| r.offset).collect()
    }

    fn ordinals(rows: &[RowStart]) -> Vec<usize> {
        rows.iter().map(|r| r.ordinal).collect()
    }

    /// Gap kept above a snapped plain row: the row sits `SM` below the viewport
    /// top, and the anchor math cancels all but `MD - SM` of the top padding.
    const GAP: f32 = spacing::MD - spacing::SM; // 4

    #[test]
    fn ungrouped_row_offsets_step_by_pitch() {
        // 7 cards, 3 columns → 3 rows. First row anchors to the top (offset 0);
        // later rows keep an SM gap so the previous row ends at the viewport
        // edge instead of bleeding its card bottoms in.
        let rows = row_starts(&[(0, 7)], 3, CW, 0.0);
        assert_eq!(offsets(&rows), vec![0.0, PITCH + GAP, 2.0 * PITCH + GAP]);
        assert_eq!(ordinals(&rows), vec![0, 3, 6]);
    }

    #[test]
    fn grouped_sections_add_header_and_section_gaps() {
        // Section 0: 4 cards / 3 cols = 2 rows. Section 1: 5 cards / 3 cols = 2 rows.
        let rows = row_starts(&[(0, 4), (4, 5)], 3, CW, header_block(true));
        let header_block = DATE_HEADER_HEIGHT + spacing::XS; // 30

        // Section 0, row 0 snaps to the header top (offset 0).
        // Section 0, row 1: header_block + pitch below the content top, minus
        // the SM gap kept above a snapped plain row.
        let s0_r1 = header_block + PITCH + GAP;
        // Section 1 header top, in content-y: MD + header_block + grid0 + LG.
        let grid0_height = 2.0 * CW + spacing::SM; // 208
        let s1_header_content_y = spacing::MD + header_block + grid0_height + spacing::LG;
        let s1_r0 = s1_header_content_y - spacing::MD; // header snap keeps the MD gap
        let s1_r1 = s1_r0 + header_block + PITCH + GAP;

        assert_eq!(offsets(&rows), vec![0.0, s0_r1, s1_r0, s1_r1]);
        // First card ordinal per row across both sections.
        assert_eq!(ordinals(&rows), vec![0, 3, 4, 7]);
    }

    #[test]
    fn step_row_moves_one_row_per_step_from_aligned_offset() {
        let rows = row_starts(&[(0, 7)], 3, CW, 0.0);
        let o = offsets(&rows);
        assert_eq!(step_row(&rows, o[0], 1), Some(1));
        assert_eq!(step_row(&rows, o[0], 2), Some(2));
        assert_eq!(step_row(&rows, o[1], -1), Some(0));
        assert_eq!(step_row(&rows, o[2], -1), Some(1));
        assert_eq!(step_row(&rows, o[0], 0), None);
    }

    #[test]
    fn step_row_realigns_from_unaligned_offset() {
        let rows = row_starts(&[(0, 7)], 3, CW, 0.0); // offsets 0, 116, 224
        // Mid-way between row 0 and row 1 after a free drag.
        assert_eq!(step_row(&rows, 50.0, 1), Some(1));
        assert_eq!(step_row(&rows, 50.0, -1), Some(0));
        // Between row 1 and row 2.
        assert_eq!(step_row(&rows, 170.0, 1), Some(2));
        assert_eq!(step_row(&rows, 170.0, -1), Some(1));
    }

    #[test]
    fn step_row_clamps_within_rows_and_noops_past_the_ends() {
        let rows = row_starts(&[(0, 7)], 3, CW, 0.0); // 3 rows
        let o = offsets(&rows);
        // A next/previous boundary exists: overshooting steps clamp to it.
        assert_eq!(step_row(&rows, o[1], 5), Some(2));
        assert_eq!(step_row(&rows, o[1], -5), Some(0));
        // No boundary in the direction of travel: stay put, never reverse.
        assert_eq!(step_row(&rows, o[2], 1), None);
        assert_eq!(step_row(&rows, o[2] + 40.0, 1), None);
        assert_eq!(step_row(&rows, o[0], -1), None);
    }

    #[test]
    fn page_row_travels_a_viewport_and_clamps_at_the_ends() {
        use super::page_row;
        let rows = rows_every(20, 100.0); // offsets 0..=1900

        // A 350-tall viewport from row 2 (200): the last row within 200+350=550
        // is row 5; going up from row 10 (1000): first row at/after 650 is 7.
        assert_eq!(page_row(&rows, 2, 350.0, true), Some(5));
        assert_eq!(page_row(&rows, 10, 350.0, false), Some(7));

        // Near the ends the page clamps to the first/last row.
        assert_eq!(page_row(&rows, 18, 350.0, true), Some(19));
        assert_eq!(page_row(&rows, 1, 350.0, false), Some(0));

        // At the edges there is nowhere to go.
        assert_eq!(page_row(&rows, 19, 350.0, true), None);
        assert_eq!(page_row(&rows, 0, 350.0, false), None);

        // A tiny (or unreported, 0.0) viewport still makes one row of progress.
        assert_eq!(page_row(&rows, 5, 0.0, true), Some(6));
        assert_eq!(page_row(&rows, 5, 0.0, false), Some(4));
    }

    #[test]
    fn anchor_row_finds_row_containing_offset() {
        let rows = row_starts(&[(0, 7)], 3, CW, 0.0);
        let o = offsets(&rows);
        assert_eq!(anchor_row(&rows, o[0]), Some(0));
        assert_eq!(anchor_row(&rows, 50.0), Some(0));
        assert_eq!(anchor_row(&rows, o[1]), Some(1));
        assert_eq!(anchor_row(&rows, 170.0), Some(1));
        assert_eq!(anchor_row(&rows, o[2]), Some(2));
    }

    #[test]
    fn row_for_ordinal_maps_card_to_its_row() {
        let rows = row_starts(&[(0, 4), (4, 5)], 3, CW, header_block(true)); // ordinals 0, 3, 4, 7
        assert_eq!(row_for_ordinal(&rows, 0), Some(0));
        assert_eq!(row_for_ordinal(&rows, 3), Some(1));
        assert_eq!(row_for_ordinal(&rows, 4), Some(2));
        assert_eq!(row_for_ordinal(&rows, 5), Some(2)); // card 5 shares row with 4
        assert_eq!(row_for_ordinal(&rows, 7), Some(3));
    }

    #[test]
    fn reanchor_is_a_noop_at_unchanged_geometry() {
        // The resize-pinning invariant: anchoring an exact row offset and
        // re-resolving it under identical geometry returns the same offset.
        let rows = row_starts(&[(0, 4), (4, 5)], 3, CW, header_block(true));
        for row in &rows {
            let anchor = anchor_row(&rows, row.offset).expect("row exists");
            let ordinal = rows[anchor].ordinal;
            let target = row_for_ordinal(&rows, ordinal).expect("ordinal maps to a row");
            assert!(
                (rows[target].offset - row.offset).abs() < 1e-3,
                "offset {} did not round-trip",
                row.offset
            );
        }
    }

    #[test]
    fn reanchor_keeps_top_card_visible_across_column_change() {
        // Grouped [4, 5]. At 3 columns the last row starts at card 7; after a
        // reflow to 2 columns that card must still sit in the row placed at top.
        let old = row_starts(&[(0, 4), (4, 5)], 3, CW, header_block(true));
        let new = row_starts(&[(0, 4), (4, 5)], 2, CW, header_block(true));

        // Anchored at the section-1 second row (offset for card 7) under 3 cols.
        let old_idx = 3;
        let anchored_ordinal = old[old_idx].ordinal; // 7
        let target = row_for_ordinal(&new, anchored_ordinal).expect("card maps to a row");

        // Card 7 must fall within [ordinal, next_ordinal) of the target row.
        let start = new[target].ordinal;
        let end = new.get(target + 1).map_or(usize::MAX, |r| r.ordinal);
        assert!(
            start <= anchored_ordinal && anchored_ordinal < end,
            "card {anchored_ordinal} not contained in target row [{start}, {end})"
        );
    }

    #[test]
    fn persisted_anchor_does_not_drift_across_chained_reflows() {
        // A resize drag reflows through many column counts. Re-anchoring must
        // use the PERSISTED card ordinal each time — re-deriving it from the
        // target row's first card would walk the anchor backwards (12 → 10 at
        // 5 cols → 9 at 3 cols → 8 at 2 cols), drifting toward the grid start.
        let anchor = 12;
        for cols in [5, 4, 3, 2, 3, 4, 5] {
            let rows = row_starts(&[(0, 72)], cols, CW, 0.0);
            let target = row_for_ordinal(&rows, anchor).expect("card maps to a row");
            let start = rows[target].ordinal;
            assert!(
                start <= anchor && anchor < start + cols,
                "{cols} cols: anchor {anchor} not in top row starting at {start}"
            );
        }
        // Returning to the original geometry restores the original top card.
        let rows = row_starts(&[(0, 72)], 2, CW, 0.0);
        let target = row_for_ordinal(&rows, anchor).expect("card maps to a row");
        assert_eq!(rows[target].ordinal, 12);
    }

    use super::{GridGeometry, ScrollReaction, scroll_reaction};

    fn geom(width: f32, vh: f32, ch: f32, scroll_y: f32) -> GridGeometry {
        GridGeometry {
            width,
            viewport_height: vh,
            content_height: ch,
            scroll_y,
        }
    }

    #[test]
    fn first_report_only_seeds_geometry() {
        assert_eq!(
            scroll_reaction(None, 0.0, 800.0, 600.0, 3000.0),
            ScrollReaction::Idle
        );
    }

    #[test]
    fn content_growth_while_parked_is_idle() {
        // Thumbnails stream in: offset pinned at the top, content_height
        // climbing. Must not touch the anchor.
        let prev = geom(800.0, 600.0, 3000.0, 0.0);
        assert_eq!(
            scroll_reaction(Some(prev), 0.0, 800.0, 600.0, 3450.0),
            ScrollReaction::Idle
        );
    }

    #[test]
    fn pure_offset_move_is_a_user_scroll() {
        let prev = geom(800.0, 600.0, 3000.0, 500.0);
        assert_eq!(
            scroll_reaction(Some(prev), 620.0, 800.0, 600.0, 3000.0),
            ScrollReaction::AdoptOffset
        );
    }

    #[test]
    fn width_change_reanchors_even_without_an_offset_move() {
        // A horizontal resize reflows the columns: every row moves to a new
        // offset even if the raw scroll value happens to stay put.
        let prev = geom(800.0, 600.0, 3000.0, 500.0);
        assert_eq!(
            scroll_reaction(Some(prev), 500.0, 900.0, 600.0, 2700.0),
            ScrollReaction::Reanchor
        );
    }

    #[test]
    fn vertical_resize_clamp_reanchors_not_adopts() {
        // Growing the viewport height near the bottom makes iced clamp the
        // offset toward the start. With the width unchanged this looks exactly
        // like a user scroll by offset alone, but the coincident height change
        // marks it a clamp — re-pin, don't adopt.
        let prev = geom(800.0, 600.0, 3000.0, 2400.0);
        assert_eq!(
            scroll_reaction(Some(prev), 2100.0, 800.0, 900.0, 3000.0),
            ScrollReaction::Reanchor
        );
    }

    #[test]
    fn content_shrink_clamp_reanchors() {
        // A filter change shortens the content; iced clamps the offset. Height
        // (content) changed alongside the offset move → clamp, not scroll.
        let prev = geom(800.0, 600.0, 3000.0, 2400.0);
        assert_eq!(
            scroll_reaction(Some(prev), 1500.0, 800.0, 600.0, 2100.0),
            ScrollReaction::Reanchor
        );
    }

    #[test]
    fn float_noise_is_idle() {
        let prev = geom(800.0, 600.0, 3000.0, 500.0);
        assert_eq!(
            scroll_reaction(Some(prev), 500.2, 800.1, 600.1, 3000.2),
            ScrollReaction::Idle
        );
    }

    /// Evenly-spaced anchors keep the window arithmetic easy to read; the real
    /// `row_starts` offsets are monotonic too, which is all the window needs.
    #[expect(
        clippy::cast_precision_loss,
        reason = "small loop indices are exact in f32"
    )]
    fn rows_every(n: usize, step: f32) -> Vec<RowStart> {
        (0..n)
            .map(|i| RowStart {
                offset: i as f32 * step,
                ordinal: i * 3,
            })
            .collect()
    }

    #[test]
    fn window_unknown_viewport_renders_every_row() {
        let rows = rows_every(20, 100.0);
        assert_eq!(visible_row_window(&rows, 0.0, 0.0, 1000.0), Some((0, 19)));
    }

    #[test]
    fn window_of_empty_rows_is_none() {
        assert_eq!(visible_row_window(&[], 500.0, 600.0, 1000.0), None);
    }

    #[test]
    fn window_covers_viewport_plus_overscan() {
        let rows = rows_every(20, 100.0); // offsets 0..=1900
        // Viewport [500, 800] grown by 150 → [350, 950]: row 3 (300) straddles
        // the top, row 9 (900) is the last anchor within the bottom.
        assert_eq!(visible_row_window(&rows, 500.0, 300.0, 150.0), Some((3, 9)));
    }

    #[test]
    fn window_clamps_against_the_top() {
        let rows = rows_every(20, 100.0);
        // scroll 0, viewport 300, overscan 150 → [-150, 450].
        assert_eq!(visible_row_window(&rows, 0.0, 300.0, 150.0), Some((0, 4)));
    }

    #[test]
    fn window_never_returns_last_below_first() {
        let rows = rows_every(5, 100.0); // offsets 0..=400
        // Window entirely below the content (scrolled past the end).
        let (first, last) = visible_row_window(&rows, 10_000.0, 300.0, 150.0).expect("rows exist");
        assert!(first <= last);
        assert_eq!((first, last), (4, 4));
    }

    #[test]
    fn row_run_spacers_sum_to_full_grid_height() {
        let pitch = CW + spacing::SM;
        let grid_height = 10.0 * CW + 9.0 * spacing::SM; // 10 rows
        let (top, bottom) = row_run_spacers(10, CW, 3, 6);
        let visible = 4.0 * CW + 3.0 * spacing::SM; // rows 3..=6
        assert!(
            (top - 3.0 * pitch).abs() < 1e-3,
            "top spacer offsets to row 3"
        );
        assert!(
            (top + visible + bottom - grid_height).abs() < 1e-3,
            "spacers + visible rows fill the full grid height"
        );
        assert!(bottom >= 0.0);
    }

    #[test]
    fn row_run_spacers_full_range_has_no_padding() {
        let (top, bottom) = row_run_spacers(5, CW, 0, 4);
        assert!(top.abs() < 1e-3 && bottom.abs() < 1e-3);
    }

    #[test]
    fn row_run_spacers_last_row_visible_has_zero_bottom() {
        let (_, bottom) = row_run_spacers(8, CW, 5, 7);
        assert!(
            bottom.abs() < 1e-3,
            "no bottom spacer when the last row shows"
        );
    }

    #[test]
    fn date_sections_split_into_contiguous_same_day_runs() {
        use chrono::{TimeZone, Utc};
        use ferrocull_core::{
            FileCategory,
            media::{CaptureSettings, CaptureTime, Item},
        };

        // 24h apart at the same UTC time-of-day → distinct Local dates in any
        // zone; equal instants share a date. So the runs hold timezone-agnostic.
        let item_on_day = |day: u32| Item {
            path: format!("/x/{day}").into(),
            size: 0,
            media_type: FileCategory::Raw,
            capture_time: CaptureTime::new(
                Utc.with_ymd_and_hms(2024, 1, day, 12, 0, 0).unwrap(),
                0,
            ),
            capture_settings: CaptureSettings::default(),
            is_ingested: false,
            jpeg_pair: None,
            paired: Vec::new(),
            sidecars: Vec::new(),
            xmp_sidecar: None,
            rating: 0,
            color_label: None,
        };
        let items = vec![
            item_on_day(1),
            item_on_day(1),
            item_on_day(2),
            item_on_day(3),
            item_on_day(3),
        ];
        let order: Vec<usize> = (0..items.len()).collect();
        assert_eq!(
            super::date_sections(&items, order.iter().copied()),
            vec![(0, 2), (2, 1), (3, 2)]
        );
    }

    /// Every card edge must land on a whole *physical* pixel, otherwise iced's
    /// `crisp` snapping — which rounds each quad in physical pixels after
    /// applying the window scale factor — rounds the card and its centered
    /// image to different edges and the photos drift ~1px at specific widths.
    /// Sweep realistic physical window widths at common monitor scale factors
    /// and assert the physical-pixel grid invariants hold at each.
    #[test]
    #[expect(
        clippy::cast_precision_loss,
        reason = "sweep bounds and column counts are far below f32's exact-integer range"
    )]
    fn grid_cells_are_pixel_aligned() {
        for nominal in [THUMBNAIL_SIZE_MIN as f32, 224.0, THUMBNAIL_SIZE_MAX as f32] {
            for scale in [1.0_f32, 1.25, 1.5, 1.75, 2.0] {
                for w in 200..=6000 {
                    let available = w as f32 / scale;
                    let (cols, cell_width) = grid_metrics(available, nominal, scale);

                    assert!(cols >= 1, "width {available} ×{scale}: at least one column");

                    // The cell width and the column stride (cell + spacing) must
                    // both be whole physical pixels — column edges are
                    // `k * stride`, so this puts every card edge on the pixel
                    // grid. Tolerance covers f32 round-trips through the logical
                    // representation; the GPU sees the same values.
                    let cell_phys = cell_width * scale;
                    assert!(
                        (cell_phys - cell_phys.round()).abs() < 1e-3,
                        "width {available} ×{scale}: cell {cell_width} is {cell_phys} physical px"
                    );
                    let stride_phys = (cell_width + spacing::SM) * scale;
                    assert!(
                        (stride_phys - stride_phys.round()).abs() < 1e-3,
                        "width {available} ×{scale}: stride is {stride_phys} physical px"
                    );

                    let total = grid_width(cols, cell_width);
                    assert!(
                        total <= available + 1e-3,
                        "width {available} ×{scale}: grid {total} must fit"
                    );
                    // Flooring drops less than one physical pixel per column, so
                    // the trailing margin is bounded by the column count.
                    assert!(
                        available - total < cols as f32 / scale + 1e-3,
                        "width {available} ×{scale}: leftover {} too large",
                        available - total
                    );
                    // Cells never exceed the intended maximum card width.
                    assert!(
                        cell_width <= nominal,
                        "width {available} ×{scale}: cell {cell_width} exceeds max {nominal}"
                    );
                }
            }
        }
    }

    #[test]
    fn thumbnail_size_steps_stay_in_range() {
        assert_eq!(
            step_thumbnail_size(THUMBNAIL_SIZE_MIN, SizeStep::Smaller),
            THUMBNAIL_SIZE_MIN
        );
        assert_eq!(
            step_thumbnail_size(THUMBNAIL_SIZE_MAX, SizeStep::Larger),
            THUMBNAIL_SIZE_MAX
        );
        assert!(step_thumbnail_size(THUMBNAIL_SIZE_MIN, SizeStep::Larger) > THUMBNAIL_SIZE_MIN);
        assert!(step_thumbnail_size(THUMBNAIL_SIZE_MAX, SizeStep::Smaller) < THUMBNAIL_SIZE_MAX);
    }

    #[test]
    fn thumbnail_size_steps_move_by_at_least_one_pixel() {
        for size in THUMBNAIL_SIZE_MIN..THUMBNAIL_SIZE_MAX {
            let larger = step_thumbnail_size(size, SizeStep::Larger);
            assert!(larger > size, "{size} did not grow (got {larger})");
        }
        for size in (THUMBNAIL_SIZE_MIN + 1)..=THUMBNAIL_SIZE_MAX {
            let smaller = step_thumbnail_size(size, SizeStep::Smaller);
            assert!(smaller < size, "{size} did not shrink (got {smaller})");
        }
    }

    #[test]
    fn thumbnail_size_step_round_trips_from_the_default() {
        let larger = step_thumbnail_size(224, SizeStep::Larger);
        assert_eq!(step_thumbnail_size(larger, SizeStep::Smaller), 224);
    }

    /// Three rows of the round-width grid, pitch 108 apart from a zero top.
    fn three_rows() -> Vec<RowStart> {
        vec![
            RowStart {
                offset: 0.0,
                ordinal: 0,
            },
            RowStart {
                offset: PITCH,
                ordinal: 3,
            },
            RowStart {
                offset: 2.0 * PITCH,
                ordinal: 6,
            },
        ]
    }

    #[test]
    fn row_bounds_span_to_the_next_row() {
        let rows = three_rows();
        assert_eq!(row_bounds(&rows, 0, CW), (0.0, PITCH));
        assert_eq!(row_bounds(&rows, 1, CW), (PITCH, 2.0 * PITCH));
    }

    #[test]
    fn row_bounds_of_the_last_row_span_one_pitch() {
        let rows = three_rows();
        assert_eq!(row_bounds(&rows, 2, CW), (2.0 * PITCH, 3.0 * PITCH));
    }

    #[test]
    fn row_in_view_covers_any_overlap_with_the_viewport() {
        let rows = three_rows();
        // A viewport holding row 0 whole and clipping into row 1.
        let (scroll_y, viewport) = (0.0, PITCH + 1.0);
        assert!(row_in_view(&rows, 0, scroll_y, viewport, CW));
        assert!(row_in_view(&rows, 1, scroll_y, viewport, CW));
        assert!(!row_in_view(&rows, 2, scroll_y, viewport, CW));
        // Scrolled past row 0 entirely.
        assert!(!row_in_view(&rows, 0, 2.0 * PITCH, PITCH, CW));
    }

    #[test]
    fn keep_row_in_view_pulls_the_offset_to_the_nearer_edge() {
        // Above the viewport: align the row's top.
        assert_eq!(keep_row_in_view(500.0, 100.0, 208.0, 300.0), 100.0);
        // Below: align the row's bottom, so the offset lands a viewport above it.
        assert_eq!(keep_row_in_view(0.0, 400.0, 508.0, 300.0), 208.0);
        // Already inside: the offset is left alone.
        assert_eq!(keep_row_in_view(100.0, 150.0, 258.0, 300.0), 100.0);
    }

    #[test]
    fn keep_row_in_view_never_scrolls_a_tall_row_past_its_top() {
        // A row taller than the viewport aligns to its top rather than its
        // bottom, which would push the row's start off screen.
        assert_eq!(keep_row_in_view(0.0, 400.0, 900.0, 300.0), 400.0);
    }

    #[test]
    fn whole_notches_accumulate_from_fractions() {
        let mut carry = 0.0_f32;
        assert_eq!(take_whole_notches(&mut carry, 0.4), 0);
        assert_eq!(take_whole_notches(&mut carry, 0.4), 0);
        assert_eq!(take_whole_notches(&mut carry, 0.4), 1);
        assert!((carry - 0.2_f32).abs() < 1e-5, "remainder carried: {carry}");
    }

    #[test]
    fn whole_notches_count_down_on_negative_deltas() {
        let mut carry = 0.0_f32;
        assert_eq!(take_whole_notches(&mut carry, -1.5), -1);
        assert!((carry + 0.5_f32).abs() < 1e-5, "remainder carried: {carry}");
    }

    #[test]
    fn whole_notches_discard_the_carry_on_a_reversal() {
        let mut carry = 0.0_f32;
        assert_eq!(take_whole_notches(&mut carry, 0.6), 0);
        // The first notch of the new direction must not pay off the old carry.
        assert_eq!(take_whole_notches(&mut carry, -1.0), -1);
        assert!(carry.abs() < 1e-5, "carry discarded: {carry}");
    }

    #[test]
    fn thumbnail_size_clamps_to_the_range() {
        assert_eq!(
            clamp_thumbnail_size(THUMBNAIL_SIZE_MIN - 1),
            THUMBNAIL_SIZE_MIN
        );
        assert_eq!(clamp_thumbnail_size(224), 224);
        assert_eq!(
            clamp_thumbnail_size(THUMBNAIL_SIZE_MAX + 1),
            THUMBNAIL_SIZE_MAX
        );
    }
}
