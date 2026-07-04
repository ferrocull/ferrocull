use ferrocull_core::Hook;
use iced::{
    Color, Element, Fill, Length,
    widget::{Space, button, checkbox, column, container, row, scrollable, text, text_input},
};

use crate::{messages::profile::Message, styles, theme::spacing};

pub(crate) fn hooks_panel(hooks: &[Hook]) -> Element<'static, Message> {
    let header = text("Post-Download Hooks").size(13);

    let mut content = column![header].spacing(spacing::SM);

    if hooks.is_empty() {
        let palette = crate::theme::palette();
        content = content.push(
            text("No hooks configured")
                .size(11)
                .color(palette.background.strong.text),
        );
    } else {
        let items: Vec<Element<'static, Message>> = hooks
            .iter()
            .enumerate()
            .map(|(idx, hook)| {
                let enabled_checkbox =
                    checkbox(hook.enabled).on_toggle(move |_| Message::HookToggled(idx));

                let name_label = text(hook.name.clone()).size(11).width(Length::Fixed(80.0));

                let command_input = text_input("command", &hook.command)
                    .size(10)
                    .width(Fill)
                    .on_input(move |cmd| Message::HookCommandEdited(idx, cmd));

                let remove_btn = button(text("X").size(10))
                    .on_press(Message::HookRemoved(idx))
                    .padding([2, 6])
                    .style(button::danger);

                row![enabled_checkbox, name_label, command_input, remove_btn]
                    .spacing(spacing::XS)
                    .align_y(iced::Alignment::Center)
                    .into()
            })
            .collect();

        let list = column(items).spacing(spacing::XS);
        let scroll = container(scrollable(list).height(Length::Shrink))
            .max_height(150)
            .width(Fill);

        content = content.push(scroll);
    }

    content
        .push(Space::new().height(spacing::XS))
        .push(
            button(text("Add Hook...").size(11).color(Color::WHITE))
                .on_press(Message::HookAddRequested)
                .padding([4, 8])
                .style(styles::primary_button),
        )
        .into()
}
