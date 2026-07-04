use iced::{
    Element, Fill, Length,
    widget::{button, column, container, scrollable, text, text_input},
};

use crate::{messages::destination::Message, theme::spacing};

pub(crate) fn jobcode_panel<'a>(current_code: &str, history: &'a [String]) -> Element<'a, Message> {
    let input_section = column![
        text("Job Code").size(13),
        text_input("e.g. CLIENT-001", current_code)
            .on_input(Message::JobCodeChanged)
            .size(12),
    ]
    .spacing(spacing::XS);

    let mut content = column![input_section].spacing(spacing::MD);

    if !history.is_empty() {
        let history_items: Vec<Element<'a, Message>> = history
            .iter()
            .map(|code| {
                button(text(code).size(11))
                    .on_press(Message::JobCodeSelected(code.clone()))
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
