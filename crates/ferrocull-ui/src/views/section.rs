//! Collapsible section component for config panel.

use iced::{
    Element,
    widget::{Space, button, column, container, row, text},
};

use crate::{
    icons, styles,
    theme::{colors, spacing},
};

pub(crate) fn collapsible_section<'a, Message: Clone + 'a>(
    title: &'a str,
    expanded: bool,
    on_toggle: Message,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let chevron = if expanded {
        icons::chevron_expanded()
    } else {
        icons::chevron_collapsed()
    };
    let chevron_color = if expanded {
        colors::ACCENT
    } else {
        crate::theme::palette().background.strong.text
    };

    let header = button(
        row![
            chevron.size(10).color(chevron_color),
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
