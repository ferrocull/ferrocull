use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
};

use chrono::{DateTime, Local, NaiveDate, Utc};
use ferrocull_core::{
    ColorLabel,
    media::{DateSelection, FilterMode, Item, SortKey, SortOrder},
};
use iced::{
    Color, ContentFit, Element, Fill,
    widget::{
        Stack, center, column, container, grid, image, lazy, mouse_area, opaque, scrollable,
        sensor, text,
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
    Scrolled(scrollable::Viewport),
}

/// Cache key derived from grid-affecting state. Uses monotonic `item_version`
/// counter (O(1)) instead of per-render content hashing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors UI state for cache invalidation"
)]
pub(crate) struct GridCacheKey {
    /// Viewport width as `f32::to_bits()` — forces grid rebuild on resize.
    pub viewport_width_bits: u32,
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
        let date = indexed
            .item
            .capture_time
            .second
            .with_timezone(&Local)
            .date_naive();
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

/// Renders a thumbnail grid, rebuilding only when `cache_key` changes.
///
/// `sorted_view` holds pre-filtered, pre-sorted indices. The badge count is
/// resolved from `burst_map`'s member-list length (no per-member count stored).
/// Click always emits `Event::CellClicked(path)`; the caller decides whether
/// that means focus or selection toggle based on modifier state.
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
    cache_key: GridCacheKey,
) -> Element<'a, Event> {
    // Extract Copy values from cache_key before it's consumed by lazy().
    let item_version = cache_key.item_content_version;
    let hovered_thumbnail = cache_key.hovered_thumbnail;
    let hovered_star = cache_key.hovered_star;
    let focused_index = cache_key.focused_index;
    let ascending = cache_key.ascending;
    let group_raw_jpeg = cache_key.group_raw_jpeg;
    let group_by_date = cache_key.sort_order == SortOrder::Time;

    lazy(cache_key, move |_| {
        if sorted_view.is_empty() {
            return Element::from(
                center(text("Select a source to scan for photos").size(14))
                    .width(Fill)
                    .height(Fill),
            );
        }

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
                let count = burst_map.get(&burst_key).map_or(0, Vec::len);
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
            render_grouped_by_date(indexed_items, ascending, today, &build_cell)
        } else {
            render_flat_grid(indexed_items, &build_cell)
        }
    })
    .into()
}

/// Render items grouped by date with section headers.
fn render_grouped_by_date(
    indexed_items: Vec<IndexedItem>,
    ascending: bool,
    today: NaiveDate,
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
            .style(styles::date_header);

            let grid_content = item_grid(group_items, build_cell);

            column![header, grid_content].spacing(spacing::XS).into()
        })
        .collect();

    let content = column(sections).spacing(spacing::LG);

    Element::from(
        scrollable(container(content).padding(spacing::MD))
            .id(GRID_SCROLLABLE_ID)
            .width(Fill)
            .height(Fill)
            .on_scroll(Event::Scrolled),
    )
}

/// Render items as a flat grid without date grouping.
fn render_flat_grid(
    indexed_items: Vec<IndexedItem>,
    build_cell: &impl Fn(IndexedItem) -> Element<'static, Event>,
) -> Element<'static, Event> {
    let grid_content = item_grid(indexed_items, build_cell);

    Element::from(
        scrollable(container(grid_content).padding(spacing::MD))
            .id(GRID_SCROLLABLE_ID)
            .width(Fill)
            .height(Fill)
            .on_scroll(Event::Scrolled),
    )
}

/// Build a grid of thumbnail cells from indexed items.
fn item_grid(
    items: Vec<IndexedItem>,
    build_cell: &impl Fn(IndexedItem) -> Element<'static, Event>,
) -> iced::widget::Grid<'static, Event, iced::Theme, iced::Renderer> {
    grid(items.into_iter().map(build_cell))
        .fluid(CELL_WIDTH)
        .spacing(spacing::SM)
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

    let padded_image: Element<'static, CellEvent> = container(image_content)
        .width(Fill)
        .height(Fill)
        .into();

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
