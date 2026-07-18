use std::path::PathBuf;

use iced::{
    Element, Fill, Length,
    widget::{Space, button, column, container, row, scrollable, text},
};

use crate::{messages::destination::Message, styles, theme::spacing};

pub(crate) fn backup_panel(destinations: &[PathBuf]) -> Element<'static, Message> {
    let header = text("Backup Destinations").size(13);

    let mut content = column![header].spacing(spacing::SM);

    if destinations.is_empty() {
        let palette = crate::theme::palette();
        content = content.push(
            text("No backup destinations configured")
                .size(11)
                .color(palette.background.strong.text),
        );
    } else {
        let items: Vec<Element<'static, Message>> = destinations
            .iter()
            .enumerate()
            .map(|(idx, path)| {
                row![
                    text(path.display().to_string()).size(11).width(Fill),
                    button(text("X").size(10))
                        .on_press(Message::RemoveBackup(idx))
                        .padding([2, 6])
                        .style(button::danger),
                ]
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

    content
        .push(Space::new().height(spacing::XS))
        .push(
            button(text("Add Backup...").size(11))
                .on_press(Message::AddBackupClicked)
                .padding([4, 8])
                .style(styles::primary_button),
        )
        .into()
}
