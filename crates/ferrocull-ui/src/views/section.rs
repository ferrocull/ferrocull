//! Collapsible section component for config panel.

use iced::{
    Element,
    widget::{Space, button, column, container, row, text},
};

use crate::{
    styles,
    theme::{colors, spacing},
};

/// Unicode chevrons for expand/collapse indicators.
const CHEVRON_DOWN: &str = "▾";
const CHEVRON_RIGHT: &str = "▸";

pub(crate) fn collapsible_section<'a, Message: Clone + 'a>(
    title: &'a str,
    expanded: bool,
    on_toggle: Message,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let chevron = if expanded {
        CHEVRON_DOWN
    } else {
        CHEVRON_RIGHT
    };
    let chevron_color = if expanded {
        colors::ACCENT
    } else {
        crate::theme::palette().background.strong.text
    };

    let header = button(
        row![
            text(chevron).size(10).color(chevron_color),
            Space::new().width(spacing::SM),
            text(title).size(12),
        ]
        .align_y(iced::Alignment::Center),
    )
    .padding([spacing::SM, spacing::MD])
    .width(iced::Fill)
    .style(styles::section_toggle(expanded))
    .on_press(on_toggle);

    let mut col = column![header].spacing(spacing::SM);
    if expanded {
        let indented_content = container(content).padding([0.0, spacing::MD]);
        col = col.push(indented_content);
    }
    col.into()
}
