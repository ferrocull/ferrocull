use iced::{
    Element, Fill, Length,
    widget::{button, column, container, scrollable, text, text_input},
};

use crate::theme::spacing;

#[derive(Debug, Clone)]
pub(crate) enum Event {
    Changed(String),
    Selected(String),
}

pub(crate) fn jobcode_panel<'a>(current_code: &str, history: &'a [String]) -> Element<'a, Event> {
    let input_section = column![
        text("Job Code").size(13),
        text_input("e.g. CLIENT-001", current_code)
            .on_input(Event::Changed)
            .size(12),
    ]
    .spacing(spacing::XS);

    let mut content = column![input_section].spacing(spacing::MD);

    if !history.is_empty() {
        let history_items: Vec<Element<'a, Event>> = history
            .iter()
            .map(|code| {
                button(text(code).size(11))
                    .on_press(Event::Selected(code.clone()))
                    .padding([4, 8])
                    .width(Fill)
                    .style(button::text)
                    .into()
            })
            .collect();

        let history_list = column(history_items).spacing(2);

        let history_section = column![
            text("Recent").size(12),
            container(scrollable(history_list).height(Length::Shrink))
                .max_height(150)
                .width(Fill),
        ]
        .spacing(spacing::XS);

        content = content.push(history_section);
    }

    content.into()
}
