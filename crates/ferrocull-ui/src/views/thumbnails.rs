use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap},
    hash::{DefaultHasher, Hash, Hasher},
    path::PathBuf,
};

use chrono::{DateTime, Local, NaiveDate, Utc};
use ferrocull_core::{
    ColorLabel,
    media::{DateSelection, FilterMode, Item, SortKey, SortOrder},
};
use iced::{
    Color, ContentFit, Element, Fill, Shrink,
    widget::{
        Stack, center, column, container, grid, image, lazy, mouse_area, opaque, responsive,
        scrollable, sensor, text,
    },
};

use super::rating::{StarEvent, star_rating_row};
use crate::{
    styles,
    theme::{COLOR_LABELS, colors, radius, spacing},
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
    ThumbnailVisible(usize),
    ThumbnailHidden(usize),
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

/// Cache key derived from grid-affecting state. Uses monotonic `item_version`
/// counter (O(1)) instead of per-render content hashing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors UI state for cache invalidation"
)]
pub(crate) struct GridCacheKey {
    pub item_count: usize,
    pub item_content_version: u64,
    pub selected: BTreeSet<usize>,
    pub filter_mode: FilterMode,
    pub group_raw_jpeg: bool,
    pub hide_rejected: bool,
    pub sort_order: SortOrder,
    pub ascending: bool,
    pub selected_dates: Option<DateSelection>,
    pub selected_sources: BTreeSet<PathBuf>,
    pub group_bursts: bool,
    pub expanded_bursts: BTreeSet<DateTime<Utc>>,
    pub selected_ratings: BTreeSet<i8>,
    pub selected_color_labels: BTreeSet<Option<ColorLabel>>,
    pub hovered_thumbnail: Option<usize>,
    pub hovered_star: Option<i8>,
    pub focused_index: Option<usize>,
}

/// Total cell width including padding for controls.
pub(crate) const CELL_WIDTH: f32 = 224.0;
/// Widget ID for the thumbnail scrollable — used by `snap_to` to scroll to items.
pub(crate) const GRID_SCROLLABLE_ID: &str = "thumbnail-grid";

/// Column count and cell width for a given available content width.
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
pub(crate) fn grid_metrics(available: f32, scale: f32) -> (usize, f32) {
    let cols = (((available + spacing::SM) / (CELL_WIDTH + spacing::SM)).ceil() as usize).max(1);
    let exact = (available - spacing::SM * (cols - 1) as f32) / cols as f32;
    let cell_width = (exact * scale).floor() / scale;
    (cols, cell_width)
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
/// `section_counts` is the card count per date section (a single entry in
/// ungrouped mode). The first row of each section anchors to the section top so
/// its header stays visible; later rows anchor to the row itself. Square cells
/// make the row pitch `cell_width + spacing::SM`.
#[expect(
    clippy::cast_precision_loss,
    reason = "row and column counts are far below f32's exact-integer range"
)]
pub(crate) fn row_starts(
    section_counts: &[usize],
    cols: usize,
    cell_width: f32,
    grouped: bool,
) -> Vec<RowStart> {
    let pitch = cell_width + spacing::SM;
    let mut rows = Vec::new();
    // Content-y cursor; starts at the scrollable content's MD top padding.
    let mut y = spacing::MD;
    let mut ordinal = 0;
    for &count in section_counts {
        let num_rows = count.div_ceil(cols);
        let header_top = y;
        let grid_top = if grouped {
            header_top + DATE_HEADER_HEIGHT + spacing::XS
        } else {
            header_top
        };
        for r in 0..num_rows {
            let row_top = grid_top + r as f32 * pitch;
            let offset = if r == 0 {
                header_top - spacing::MD
            } else {
                row_top - spacing::SM
            };
            rows.push(RowStart {
                offset,
                ordinal: ordinal + r * cols,
            });
        }
        ordinal += count;
        let grid_height =
            num_rows as f32 * cell_width + num_rows.saturating_sub(1) as f32 * spacing::SM;
        y = grid_top + grid_height + spacing::LG;
    }
    rows
}

/// Display-order card count per date section (Time sort) or a single section
/// (any other sort). Time sort keeps each date contiguous, so counting runs
/// over `sorted_view` reproduces exactly what the grouped view renders.
pub(crate) fn section_counts(
    items: &[Item],
    sorted_view: &BTreeMap<SortKey, usize>,
    ascending: bool,
    grouped: bool,
) -> Vec<usize> {
    if !grouped {
        return if sorted_view.is_empty() {
            Vec::new()
        } else {
            vec![sorted_view.len()]
        };
    }
    let ordered: Box<dyn Iterator<Item = usize>> = if ascending {
        Box::new(sorted_view.values().copied())
    } else {
        Box::new(sorted_view.values().rev().copied())
    };
    let mut counts: Vec<usize> = Vec::new();
    let mut current: Option<NaiveDate> = None;
    for idx in ordered {
        let date = capture_date(&items[idx]);
        if current == Some(date) {
            *counts
                .last_mut()
                .expect("current date implies a section exists") += 1;
        } else {
            counts.push(1);
            current = Some(date);
        }
    }
    counts
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

/// Row currently at the viewport top: the last anchor at or before `offset`.
pub(crate) fn anchor_row(rows: &[RowStart], offset: f32) -> Option<usize> {
    rows.iter().rposition(|r| r.offset <= offset + ROW_EPS)
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

/// Background color for selected cards - warm-tinted to match darkroom palette.
const SELECTED_BG: Color = Color::from_rgb(0.28, 0.26, 0.24);

/// Formats a date for display as a section header.
/// "Today", "Yesterday", weekday name for this week, or "Jan 5" for older.
fn format_date_header(date: NaiveDate, today: NaiveDate) -> Cow<'static, str> {
    if date == today {
        "Today".into()
    } else if date == today.pred_opt().expect("today is never NaiveDate::MIN") {
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

/// An item with its grid index and resolved thumbnail handle.
struct IndexedItem {
    idx: usize,
    item: Item,
    thumb: Option<image::Handle>,
}

/// Groups indexed items by date, preserving the item index for selection checks.
/// Returns groups in ascending (oldest first) or descending (newest first) date order.
fn group_by_date_indexed(
    items: Vec<IndexedItem>,
    ascending: bool,
) -> Vec<(NaiveDate, Vec<IndexedItem>)> {
    let mut groups: BTreeMap<NaiveDate, Vec<IndexedItem>> = BTreeMap::new();

    for indexed in items {
        let date = capture_date(&indexed.item);
        groups.entry(date).or_default().push(indexed);
    }

    // BTreeMap iterates in ascending order
    if ascending {
        groups.into_iter().collect()
    } else {
        groups.into_iter().rev().collect()
    }
}

/// Burst badge data resolved per rendered cell (count via `burst_map` length).
#[derive(Clone, Copy)]
struct BurstBadgeInfo {
    count: usize,
    burst_key: DateTime<Utc>,
}

/// Visual interaction state for a thumbnail cell.
#[derive(Clone, Copy)]
struct CellState {
    is_selected: bool,
    is_hovered: bool,
    is_focused: bool,
}

/// Renders a thumbnail grid, rebuilding only when `cache_key` or the derived
/// column geometry changes.
///
/// `sorted_view` holds pre-filtered, pre-sorted indices. The badge count is
/// resolved from `burst_map`'s member-list length (no per-member count stored).
/// Click always emits `Event::CellClicked(path)`; the caller decides whether
/// that means focus or selection toggle based on modifier state. `window_scale`
/// pins cell widths to whole physical pixels (see [`grid_metrics`]).
#[expect(
    clippy::too_many_arguments,
    reason = "grid needs items + view + burst indices + date; a param bag would just relocate them"
)]
pub(crate) fn thumbnail_grid<'a>(
    items: &'a [Item],
    sorted_view: &'a BTreeMap<SortKey, usize>,
    selected: &'a BTreeSet<usize>,
    loaded_thumbs: &'a HashMap<PathBuf, image::Handle>,
    burst_of: &'a HashMap<usize, DateTime<Utc>>,
    burst_map: &'a HashMap<DateTime<Utc>, Vec<usize>>,
    today: NaiveDate,
    window_scale: f32,
    cache_key: &GridCacheKey,
) -> Element<'a, Event> {
    // Empty state needs none of the grid/scrollable machinery.
    if sorted_view.is_empty() {
        return center(text("Select a source to scan for photos").size(14))
            .width(Fill)
            .height(Fill)
            .into();
    }

    // Extract Copy values from cache_key before it's captured by the closures.
    let item_version = cache_key.item_content_version;
    let hovered_thumbnail = cache_key.hovered_thumbnail;
    let hovered_star = cache_key.hovered_star;
    let focused_index = cache_key.focused_index;
    let ascending = cache_key.ascending;
    let group_raw_jpeg = cache_key.group_raw_jpeg;
    let group_by_date = cache_key.sort_order == SortOrder::Time;

    // Folded to a hash: the responsive closure below runs on every layout
    // pass, so it must capture only cheap Copy values.
    let mut hasher = DefaultHasher::new();
    cache_key.hash(&mut hasher);
    let key_hash = hasher.finish();

    // `responsive` runs at layout time, so `size.width` is the true grid area:
    // iced has already subtracted the container's `MD` padding and, when the
    // content overflows, the embedded scrollbar's gutter.
    let grid = responsive(move |size| {
        let (cols, cell_width) = grid_metrics(size.width, window_scale);

        // Center the grid by splitting the leftover into a side margin floored
        // to the physical-pixel grid — a fractional offset would shift every
        // card off the pixel grid and reintroduce sub-pixel drift.
        let side_margin = (((size.width - grid_width(cols, cell_width)) / 2.0) * window_scale)
            .floor()
            .max(0.0)
            / window_scale;

        // Keyed on the derived column geometry, not the raw width: cell_width
        // moves in whole-pixel steps, so a resize drag doesn't rebuild the
        // cells on every pixel.
        let cells = lazy((key_hash, cols, cell_width.to_bits()), move |_| {
            // Resolve handle lookups here so the full loaded_thumbs HashMap
            // doesn't need to be cloned into the build_cell closure.
            let resolve = |i: usize| {
                let item = items[i].clone();
                let thumb = loaded_thumbs.get(&item.path).cloned();
                IndexedItem {
                    idx: i,
                    item,
                    thumb,
                }
            };
            let indexed_items: Vec<IndexedItem> = if ascending {
                sorted_view.values().copied().map(resolve).collect()
            } else {
                sorted_view.values().copied().rev().map(resolve).collect()
            };

            let selected = selected.clone();
            // Resolve each burst member's badge count here, so the hot
            // incremental-insert path never stores per-member counts.
            let burst_info: HashMap<usize, BurstBadgeInfo> = burst_of
                .iter()
                .map(|(&idx, &burst_key)| {
                    let count = burst_map[&burst_key].len();
                    (idx, BurstBadgeInfo { count, burst_key })
                })
                .collect();

            let build_cell = move |indexed: IndexedItem| -> Element<'static, Event> {
                let idx = indexed.idx;
                let is_hovered = hovered_thumbnail == Some(idx);
                let cell_hovered_star = if is_hovered { hovered_star } else { None };

                let state = CellState {
                    is_selected: selected.contains(&idx),
                    is_hovered,
                    is_focused: focused_index == Some(idx),
                };
                let show_pair = group_raw_jpeg && indexed.item.jpeg_pair.is_some();
                let burst = burst_info.get(&idx).copied();

                let path = indexed.item.path.clone();
                let cell = thumbnail_card(
                    indexed.thumb.as_ref(),
                    &indexed.item,
                    state,
                    show_pair,
                    burst,
                    cell_hovered_star,
                );

                let mapped = cell.map(move |e| match e {
                    CellEvent::Clicked => Event::CellClicked(path.clone()),
                    CellEvent::DoubleClicked => Event::CellDoubleClicked(idx),
                    CellEvent::HoverEnter => Event::CellHover(idx, true),
                    CellEvent::HoverExit => Event::CellHover(idx, false),
                    CellEvent::Rated(r) => Event::Rated(path.clone(), r),
                    CellEvent::StarHover(s) => Event::StarHover(s),
                    CellEvent::BurstToggle(key) => Event::BurstToggle(key),
                });

                sensor(mapped)
                    .on_show(move |_| Event::ThumbnailVisible(idx))
                    .on_hide(Event::ThumbnailHidden(idx))
                    .anticipate(1000.0)
                    .key(item_version)
                    .into()
            };

            if group_by_date {
                render_grouped_by_date(
                    indexed_items,
                    ascending,
                    today,
                    cols,
                    cell_width,
                    &build_cell,
                )
            } else {
                item_grid(indexed_items, cols, cell_width, &build_cell).into()
            }
        });

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

/// Render items grouped by date with section headers.
fn render_grouped_by_date(
    indexed_items: Vec<IndexedItem>,
    ascending: bool,
    today: NaiveDate,
    cols: usize,
    cell_width: f32,
    build_cell: &impl Fn(IndexedItem) -> Element<'static, Event>,
) -> Element<'static, Event> {
    let date_groups = group_by_date_indexed(indexed_items, ascending);

    let palette = crate::theme::palette();
    let sections: Vec<Element<'static, Event>> = date_groups
        .into_iter()
        .map(|(date, group_items)| {
            let header = container(
                text(format_date_header(date, today))
                    .size(13)
                    .color(palette.background.base.text),
            )
            .padding([spacing::XS, spacing::SM])
            .height(DATE_HEADER_HEIGHT)
            .style(styles::date_header);

            let grid_content = item_grid(group_items, cols, cell_width, build_cell);

            column![header, grid_content].spacing(spacing::XS).into()
        })
        .collect();

    let content = column(sections).spacing(spacing::LG);

    content.into()
}

/// Build a grid of thumbnail cells pinned to `cols` whole-physical-pixel cells
/// so card edges stay on the device pixel grid (see [`grid_metrics`]).
fn item_grid(
    items: Vec<IndexedItem>,
    cols: usize,
    cell_width: f32,
    build_cell: &impl Fn(IndexedItem) -> Element<'static, Event>,
) -> iced::widget::Grid<'static, Event, iced::Theme, iced::Renderer> {
    grid(items.into_iter().map(build_cell))
        .spacing(spacing::SM)
        .columns(cols)
        .width(grid_width(cols, cell_width))
}

/// Renders a thumbnail card with image, overlays, badges, and interaction handlers.
fn thumbnail_card(
    thumb: Option<&image::Handle>,
    item: &Item,
    state: CellState,
    show_pair: bool,
    burst: Option<BurstBadgeInfo>,
    hovered_star: Option<i8>,
) -> Element<'static, CellEvent> {
    let palette = crate::theme::palette();
    let card_bg = if item.rating == -1 {
        colors::REJECTED_BG
    } else if state.is_selected {
        SELECTED_BG
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
            colors::ACCENT
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
    burst: Option<BurstBadgeInfo>,
    hovered_star: Option<i8>,
) -> Stack<'static, CellEvent> {
    let mut stack = Stack::new().width(Fill).height(Fill).push(base);

    if item.rating == -1 {
        stack = stack.push(rejected_badge());
    } else if item.is_downloaded {
        stack = stack.push(color_overlay(colors::OVERLAY_DOWNLOADED));
    }

    if show_pair {
        stack = stack.push(pair_badge());
    }

    if let Some(burst) = burst {
        stack = stack.push(burst_badge(burst.count, burst.burst_key));
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
            .expect("scanned file has filename")
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
    let palette = crate::theme::palette();
    let stars = star_rating_row(rating, hovered_star, 12.0).map(|e| match e {
        StarEvent::Rated(r) => CellEvent::Rated(r),
        StarEvent::Hover(s) => CellEvent::StarHover(s),
    });
    let name = text(filename).size(9).color(palette.background.base.text);

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

fn placeholder<Message: 'static>() -> Element<'static, Message> {
    container(center(text("?").size(24)))
        .width(Fill)
        .height(Fill)
        .style(container::bordered_box)
        .into()
}

/// "R+J" badge positioned in bottom-right corner for RAW+JPEG pairs.
fn pair_badge<Message: 'static>() -> Element<'static, Message> {
    let badge = container(text("R+J").size(9))
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

fn color_overlay<Message: 'static>(color: Color) -> Element<'static, Message> {
    container(opaque(container("").width(Fill).height(Fill)))
        .width(Fill)
        .height(Fill)
        .style(styles::solid_fill(color))
        .into()
}

fn rejected_badge<Message: 'static>() -> Element<'static, Message> {
    let badge = container(text("X").size(10))
        .padding([2, 6])
        .style(styles::rounded_badge(colors::BADGE_REJECTED));

    container(badge)
        .width(Fill)
        .height(Fill)
        .align_x(iced::alignment::Horizontal::Left)
        .align_y(iced::alignment::Vertical::Top)
        .padding(spacing::XS)
        .into()
}

/// Rating indicator badge positioned in bottom-left corner (shown when not hovered).
fn rated_badge<Message: 'static>(rating: i8) -> Element<'static, Message> {
    let badge = container(text(format!("★{rating}")).size(10).color(colors::WARNING))
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

/// Burst count badge positioned in top-right corner.
fn burst_badge(count: usize, burst_key: DateTime<Utc>) -> Element<'static, CellEvent> {
    let badge = container(text(format!("{count}")).size(10))
        .padding([2, 6])
        .style(styles::rounded_badge(colors::BADGE_BURST));

    let clickable_badge = mouse_area(badge).on_press(CellEvent::BurstToggle(burst_key));

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
    let color = COLOR_LABELS[u8::from(label) as usize];

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

/// Preview icon (magnifying glass) positioned in bottom-right corner on hover.
fn preview_icon() -> Element<'static, CellEvent> {
    let icon = container(text("\u{1F50D}").size(14))
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
        CELL_WIDTH, DATE_HEADER_HEIGHT, anchor_row, grid_metrics, grid_width, row_for_ordinal,
        row_starts, step_row,
    };
    use crate::theme::spacing;

    // A round cell width keeps the expected offsets easy to read.
    const CW: f32 = 100.0;
    const PITCH: f32 = CW + spacing::SM; // 108

    fn offsets(rows: &[super::RowStart]) -> Vec<f32> {
        rows.iter().map(|r| r.offset).collect()
    }

    fn ordinals(rows: &[super::RowStart]) -> Vec<usize> {
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
        let rows = row_starts(&[7], 3, CW, false);
        assert_eq!(offsets(&rows), vec![0.0, PITCH + GAP, 2.0 * PITCH + GAP]);
        assert_eq!(ordinals(&rows), vec![0, 3, 6]);
    }

    #[test]
    fn grouped_sections_add_header_and_section_gaps() {
        // Section 0: 4 cards / 3 cols = 2 rows. Section 1: 5 cards / 3 cols = 2 rows.
        let rows = row_starts(&[4, 5], 3, CW, true);
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
        let rows = row_starts(&[7], 3, CW, false);
        let o = offsets(&rows);
        assert_eq!(step_row(&rows, o[0], 1), Some(1));
        assert_eq!(step_row(&rows, o[0], 2), Some(2));
        assert_eq!(step_row(&rows, o[1], -1), Some(0));
        assert_eq!(step_row(&rows, o[2], -1), Some(1));
        assert_eq!(step_row(&rows, o[0], 0), None);
    }

    #[test]
    fn step_row_realigns_from_unaligned_offset() {
        let rows = row_starts(&[7], 3, CW, false); // offsets 0, 116, 224
        // Mid-way between row 0 and row 1 after a free drag.
        assert_eq!(step_row(&rows, 50.0, 1), Some(1));
        assert_eq!(step_row(&rows, 50.0, -1), Some(0));
        // Between row 1 and row 2.
        assert_eq!(step_row(&rows, 170.0, 1), Some(2));
        assert_eq!(step_row(&rows, 170.0, -1), Some(1));
    }

    #[test]
    fn step_row_clamps_within_rows_and_noops_past_the_ends() {
        let rows = row_starts(&[7], 3, CW, false); // 3 rows
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
    fn anchor_row_finds_row_containing_offset() {
        let rows = row_starts(&[7], 3, CW, false);
        let o = offsets(&rows);
        assert_eq!(anchor_row(&rows, o[0]), Some(0));
        assert_eq!(anchor_row(&rows, 50.0), Some(0));
        assert_eq!(anchor_row(&rows, o[1]), Some(1));
        assert_eq!(anchor_row(&rows, 170.0), Some(1));
        assert_eq!(anchor_row(&rows, o[2]), Some(2));
    }

    #[test]
    fn row_for_ordinal_maps_card_to_its_row() {
        let rows = row_starts(&[4, 5], 3, CW, true); // ordinals 0, 3, 4, 7
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
        let rows = row_starts(&[4, 5], 3, CW, true);
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
        let old = row_starts(&[4, 5], 3, CW, true);
        let new = row_starts(&[4, 5], 2, CW, true);

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
            let rows = row_starts(&[72], cols, CW, false);
            let target = row_for_ordinal(&rows, anchor).expect("card maps to a row");
            let start = rows[target].ordinal;
            assert!(
                start <= anchor && anchor < start + cols,
                "{cols} cols: anchor {anchor} not in top row starting at {start}"
            );
        }
        // Returning to the original geometry restores the original top card.
        let rows = row_starts(&[72], 2, CW, false);
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
        for scale in [1.0_f32, 1.25, 1.5, 1.75, 2.0] {
            for w in 200..=6000 {
                let available = w as f32 / scale;
                let (cols, cell_width) = grid_metrics(available, scale);

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
                    cell_width <= CELL_WIDTH,
                    "width {available} ×{scale}: cell {cell_width} exceeds max {CELL_WIDTH}"
                );
            }
        }
    }
}
