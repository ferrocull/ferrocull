//! Compare mode view: side-by-side or stacked image comparison.
//!
//! Photo Mechanic-style compare functionality with synchronized zoom/pan.

use std::path::PathBuf;

use ferrocull_core::media::Item;
use iced::{
    Alignment, Element, Fill,
    widget::{Space, button, center, column, container, row, text},
};
use iced_aw::Spinner;

use super::rating;
use crate::{
    messages::compare::{Layout, Pane},
    styles,
    theme::{colors, spacing},
    widgets::{self, ViewState, Viewer},
};

/// What happened in an image pane (no pane identity — parent knows which pane).
#[derive(Clone)]
pub(crate) enum PaneEvent {
    Clicked,
    ViewStateChanged(widgets::Event),
}

/// Module-level event composed from sub-function events.
#[derive(Clone)]
pub(crate) enum Event {
    Close,
    ToggleLock,
    Prev,
    Next,
    Promote,
    SwitchHorizontal,
    SwitchVertical,
    SetActivePane(Pane),
    ViewStateChanged(Pane, widgets::Event),
    Item(PathBuf, rating::ItemEvent),
}

/// Renders the top bar with filenames, position, lock, and close button.
pub(crate) fn top_bar(
    select_item: &Item,
    candidate_item: &Item,
    select_index: usize,
    total: usize,
    active_pane: Pane,
    lock_scroll: bool,
) -> Element<'static, Event> {
    let palette = crate::theme::palette();
    let select_name = select_item
        .path
        .file_name()
        .expect("scanned file has filename")
        .to_string_lossy()
        .into_owned();

    let candidate_name = candidate_item
        .path
        .file_name()
        .expect("scanned file has filename")
        .to_string_lossy()
        .into_owned();

    let select_text = text(select_name)
        .size(12)
        .color(if active_pane == Pane::Select {
            colors::ACCENT
        } else {
            palette.background.base.text
        });

    let candidate_text = text(candidate_name)
        .size(12)
        .color(if active_pane == Pane::Candidate {
            colors::ACCENT
        } else {
            palette.background.base.text
        });

    let lock_icon = if lock_scroll { "🔒" } else { "🔓" };
    let lock_btn = button(text(lock_icon).size(14))
        .padding([4, 8])
        .style(if lock_scroll {
            styles::primary_button
        } else {
            styles::ghost_button
        })
        .on_press(Event::ToggleLock);

    let close_btn = button(text("✕").size(14).color(palette.background.base.text))
        .padding([6, 12])
        .style(styles::ghost_button)
        .on_press(Event::Close);

    let position = text(format!("{} / {}", select_index + 1, total))
        .size(11)
        .color(palette.background.weak.text);

    container(
        row![
            position,
            Space::new().width(spacing::LG),
            select_text,
            Space::new().width(Fill),
            lock_btn,
            Space::new().width(Fill),
            candidate_text,
            Space::new().width(spacing::LG),
            close_btn,
        ]
        .align_y(Alignment::Center)
        .padding([spacing::SM, spacing::LG]),
    )
    .style(styles::preview_bar)
    .width(Fill)
    .into()
}

/// Renders a single image pane with border indicating active state.
/// Doesn't know its pane identity for event routing — parent maps `PaneEvent`.
/// `label` is display text ("SELECT" or "CANDIDATE").
pub(crate) fn image_pane(
    preview: Option<&iced::widget::image::Handle>,
    is_active: bool,
    view_state: ViewState,
    label: &'static str,
) -> Element<'static, PaneEvent> {
    let palette = crate::theme::palette();
    let content: Element<'static, PaneEvent> = preview.map_or_else(
        || {
            center(Spinner::new().width(40.0).height(40.0).circle_radius(3.0))
                .width(Fill)
                .height(Fill)
                .into()
        },
        |handle| {
            Viewer::new(handle.clone(), view_state, PaneEvent::ViewStateChanged)
                .min_scale(0.25)
                .max_scale(8.0)
                .scale_step(0.25)
                .width(Fill)
                .height(Fill)
                .into()
        },
    );

    let border_color = if is_active {
        colors::ACCENT
    } else {
        palette.background.strong.color
    };

    let label = button(text(label).size(10).color(if is_active {
        colors::ACCENT
    } else {
        palette.background.strong.text
    }))
    .padding([2, 6])
    .style(styles::ghost_button)
    .on_press(PaneEvent::Clicked);

    let border_width = if is_active { 2.0 } else { 1.0 };

    let pane_content = column![label, content].width(Fill).height(Fill);

    container(pane_content)
        .width(Fill)
        .height(Fill)
        .padding(2)
        .style(move |_theme| container::Style {
            border: iced::Border {
                color: border_color,
                width: border_width,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// Layout toggle button (H/V) with active state styling.
fn layout_toggle_btn(
    label: &'static str,
    is_active: bool,
    on_press: Event,
) -> iced::widget::Button<'static, Event> {
    let palette = crate::theme::palette();
    let color = if is_active {
        colors::ACCENT
    } else {
        palette.background.base.text
    };
    let style = if is_active {
        styles::primary_button
    } else {
        styles::secondary_button
    };
    button(text(label).size(11).color(color))
        .padding([6, 10])
        .style(style)
        .on_press(on_press)
}

/// Renders the bottom bar with navigation, promote, layout controls, and pre-mapped item controls.
pub(crate) fn bottom_bar(
    layout: Layout,
    item_controls: Element<'static, Event>,
) -> Element<'static, Event> {
    let palette = crate::theme::palette();

    let promote_btn = button(
        text("Promote (G)")
            .size(11)
            .color(palette.background.base.text),
    )
    .padding([6, 12])
    .style(styles::secondary_button)
    .on_press(Event::Promote);

    let nav_prev = button(text("‹").size(24).color(palette.background.base.text))
        .padding([10, 20])
        .style(styles::ghost_button)
        .on_press(Event::Prev);

    let nav_next = button(text("›").size(24).color(palette.background.base.text))
        .padding([10, 20])
        .style(styles::ghost_button)
        .on_press(Event::Next);

    let h_btn = layout_toggle_btn("H", layout == Layout::Horizontal, Event::SwitchHorizontal);
    let v_btn = layout_toggle_btn("V", layout == Layout::Vertical, Event::SwitchVertical);

    container(
        row![
            nav_prev,
            Space::new().width(spacing::MD),
            item_controls,
            Space::new().width(spacing::MD),
            promote_btn,
            Space::new().width(Fill),
            h_btn,
            Space::new().width(spacing::XS),
            v_btn,
            Space::new().width(spacing::MD),
            nav_next,
        ]
        .align_y(Alignment::Center)
        .padding([spacing::SM, spacing::LG]),
    )
    .style(styles::preview_bar)
    .width(Fill)
    .into()
}

/// Assembles the full compare overlay from pre-built sub-elements.
/// Maps pane events to module events with pane identity.
pub(crate) fn compose(
    layout: Layout,
    top: Element<'static, Event>,
    select_pane: Element<'static, PaneEvent>,
    candidate_pane: Element<'static, PaneEvent>,
    bottom: Element<'static, Event>,
) -> Element<'static, Event> {
    let select = select_pane.map(|e| match e {
        PaneEvent::Clicked => Event::SetActivePane(Pane::Select),
        PaneEvent::ViewStateChanged(e) => Event::ViewStateChanged(Pane::Select, e),
    });
    let candidate = candidate_pane.map(|e| match e {
        PaneEvent::Clicked => Event::SetActivePane(Pane::Candidate),
        PaneEvent::ViewStateChanged(e) => Event::ViewStateChanged(Pane::Candidate, e),
    });

    let image_area: Element<'static, Event> = match layout {
        Layout::Horizontal => row![select, candidate]
            .spacing(2)
            .width(Fill)
            .height(Fill)
            .into(),
        Layout::Vertical => column![select, candidate]
            .spacing(2)
            .width(Fill)
            .height(Fill)
            .into(),
    };

    let content = column![
        top,
        container(image_area)
            .width(Fill)
            .height(Fill)
            .padding(spacing::XS),
        bottom,
    ];

    container(content)
        .width(Fill)
        .height(Fill)
        .style(styles::preview_background)
        .into()
}
