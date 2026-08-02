//! Compare mode view: side-by-side or stacked image comparison.
//!
//! Photo Mechanic-style compare functionality with synchronized zoom/pan.

use ferrocull_core::media::Item;
use iced::{
    Alignment, Element, Fill,
    widget::{Space, button, center, column, container, row, text},
};
use iced_aw::Spinner;

use crate::{
    messages::{
        Message,
        compare::{self, Layout, Pane},
    },
    styles,
    theme::{colors, spacing},
    views::status,
    widgets::{self, ViewState, Viewer},
};

/// What happened in an image pane (no pane identity — parent knows which pane).
#[derive(Clone)]
pub(crate) enum PaneEvent {
    Clicked,
    ViewStateChanged(widgets::Event),
}

/// Renders the top bar with filenames, position, lock, and close button.
pub(crate) fn top_bar(
    select_item: &Item,
    candidate_item: &Item,
    select_position: Option<usize>,
    total: usize,
    active_pane: Pane,
    lock_scroll: bool,
) -> Element<'static, Message> {
    let palette = crate::theme::palette();
    let select_name = select_item
        .path
        .file_name()
        .expect("item path has no filename")
        .to_string_lossy()
        .into_owned();

    let candidate_name = candidate_item
        .path
        .file_name()
        .expect("item path has no filename")
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

    let lock_icon = if lock_scroll {
        crate::icons::scroll_locked()
    } else {
        crate::icons::scroll_unlocked()
    };
    let lock_btn = button(lock_icon.size(14))
        .padding([4, 8])
        .style(if lock_scroll {
            styles::primary_button
        } else {
            styles::ghost_button
        })
        .on_press(Message::Compare(compare::Message::ToggleLockScroll));

    let close_btn = button(
        crate::icons::close()
            .size(14)
            .color(palette.background.base.text),
    )
    .padding([6, 12])
    .style(styles::ghost_button)
    .on_press(Message::Compare(compare::Message::Exit));

    // The select may be filtered out of the view; show "–" rather than a
    // misleading position.
    let position_label = select_position.map_or_else(|| "–".to_owned(), |p| (p + 1).to_string());
    let position = text(format!("{position_label} / {total}"))
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
///
/// Each pane carries its own status marks, so both panes answer "which of
/// these two is already in my selection?" regardless of which is active.
///
/// `info` is the pane's info-strip readout when the strip is open. It sits
/// beneath the photo, inside the pane, so the mapping from value to frame holds
/// in either layout and no chrome ever covers image pixels.
pub(crate) fn image_pane(
    preview: Option<&iced::widget::image::Handle>,
    is_active: bool,
    view_state: ViewState,
    label: &'static str,
    item: &Item,
    is_tagged: bool,
    info: Option<Element<'static, PaneEvent>>,
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

    // The pane label sits in the column above the marked content, so the
    // badges never cover it.
    let content = status::marked(content, item, is_tagged, spacing::MD);

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

    let mut pane_content = column![label, content].width(Fill).height(Fill);
    if let Some(info) = info {
        pane_content = pane_content.push(info);
    }

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
    on_press: Message,
) -> iced::widget::Button<'static, Message> {
    let style = if is_active {
        styles::primary_button
    } else {
        styles::secondary_button
    };
    button(text(label).size(11))
        .padding([6, 10])
        .style(style)
        .on_press(on_press)
}

/// Renders the bottom bar with navigation, promote, layout controls, and pre-mapped item controls.
pub(crate) fn bottom_bar(
    layout: Layout,
    item_controls: Element<'static, Message>,
) -> Element<'static, Message> {
    let palette = crate::theme::palette();

    let promote_btn = button(
        text("Promote (G)")
            .size(11)
            .color(palette.background.base.text),
    )
    .padding([6, 12])
    .style(styles::secondary_button)
    .on_press(Message::Compare(compare::Message::Promote));

    let nav_prev = button(
        crate::icons::nav_previous()
            .size(14)
            .color(palette.background.base.text),
    )
    .padding([10, 20])
    .style(styles::ghost_button)
    .on_press(Message::Compare(compare::Message::CandidatePrev));

    let nav_next = button(
        crate::icons::nav_next()
            .size(14)
            .color(palette.background.base.text),
    )
    .padding([10, 20])
    .style(styles::ghost_button)
    .on_press(Message::Compare(compare::Message::CandidateNext));

    let h_btn = layout_toggle_btn(
        "H",
        layout == Layout::Horizontal,
        Message::Compare(compare::Message::EnterHorizontal),
    );
    let v_btn = layout_toggle_btn(
        "V",
        layout == Layout::Vertical,
        Message::Compare(compare::Message::EnterVertical),
    );

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
    top: Element<'static, Message>,
    select_pane: Element<'static, PaneEvent>,
    candidate_pane: Element<'static, PaneEvent>,
    bottom: Element<'static, Message>,
) -> Element<'static, Message> {
    let select = select_pane.map(|e| match e {
        PaneEvent::Clicked => Message::Compare(compare::Message::ActivePaneChanged(Pane::Select)),
        PaneEvent::ViewStateChanged(e) => {
            Message::Compare(compare::Message::ViewStateChanged(Pane::Select, e))
        }
    });
    let candidate = candidate_pane.map(|e| match e {
        PaneEvent::Clicked => {
            Message::Compare(compare::Message::ActivePaneChanged(Pane::Candidate))
        }
        PaneEvent::ViewStateChanged(e) => {
            Message::Compare(compare::Message::ViewStateChanged(Pane::Candidate, e))
        }
    });

    let image_area: Element<'static, Message> = match layout {
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
