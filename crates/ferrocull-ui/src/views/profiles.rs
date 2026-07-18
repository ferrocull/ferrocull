use ferrocull_core::NamedProfile;
use iced::{
    Element, Fill, Length,
    widget::{Space, button, column, container, row, scrollable, text, text_input},
};

use crate::{
    messages::profile::Message,
    styles,
    theme::{colors, spacing},
};

pub(crate) fn profiles_panel(
    profiles: &[NamedProfile],
    current: Option<&str>,
    save_name_input: &str,
) -> Element<'static, Message> {
    let header = text("Profiles").size(13);

    let mut content = column![header].spacing(spacing::SM);

    if profiles.is_empty() {
        let palette = crate::theme::palette();
        content = content.push(
            text("No saved profiles")
                .size(11)
                .color(palette.background.strong.text),
        );
    } else {
        let items: Vec<Element<'static, Message>> = profiles
            .iter()
            .map(|named| {
                let name = &named.name;
                let is_current = current.is_some_and(|c| c == name);

                let name_text = if is_current {
                    text(format!("{name} (active)"))
                        .size(11)
                        .color(colors::SUCCESS)
                } else {
                    text(name.clone()).size(11)
                };

                let load_btn = button(text("Load").size(10))
                    .on_press(Message::ProfileSelected(name.clone()))
                    .padding([2, 6])
                    .style(styles::primary_button);

                let delete_btn = button(text("X").size(10))
                    .on_press(Message::DeleteRequested(name.clone()))
                    .padding([2, 6])
                    .style(button::danger);

                row![name_text.width(Fill), load_btn, delete_btn,]
                    .spacing(spacing::XS)
                    .align_y(iced::Alignment::Center)
                    .into()
            })
            .collect();

        let list = column(items).spacing(spacing::XS);
        let scroll = container(scrollable(list).height(Length::Shrink))
            .max_height(120)
            .width(Fill);

        content = content.push(scroll);
    }

    content = content.push(Space::new().height(spacing::SM));

    let save_row = row![
        text_input("Profile name...", save_name_input)
            .on_input(Message::NameChanged)
            .size(11)
            .width(Fill),
        button(text("Save").size(11))
            .on_press(Message::SaveRequested)
            .padding([4, 8])
            .style(styles::primary_button),
    ]
    .spacing(spacing::XS)
    .align_y(iced::Alignment::Center);

    content = content.push(save_row);

    content.into()
}
