//! Source panel with storage indicators.

use std::{collections::BTreeSet, path::PathBuf};

use ferrocull_devices::Source;
use iced::{
    Center, Element, Fill, Length,
    widget::{Space, button, checkbox, column, container, progress_bar, row, text},
};

use crate::{icons, messages::sources::Message, styles, theme::spacing};

pub(crate) fn sources_panel(
    sources: &[Source],
    selected: &BTreeSet<PathBuf>,
) -> Element<'static, Message> {
    let palette = crate::theme::palette();
    let source_list: Element<'static, Message> = if sources.is_empty() {
        container(
            text("No sources detected")
                .size(12)
                .color(palette.background.strong.text),
        )
        .padding([spacing::LG, 0.0])
        .into()
    } else {
        column(sources.iter().map(|source| {
            let path = source.path();
            let (toggle_msg, action) = match source {
                Source::Storage(s) if s.mount_point.is_none() => (
                    None,
                    Some(Action {
                        label: "Mount",
                        message: Message::MountStorage(s.device_path.clone()),
                        style: styles::primary_button,
                    }),
                ),
                Source::Storage(s) => (
                    Some(Message::SourceToggled(path.to_path_buf())),
                    Some(Action {
                        label: "Unmount",
                        message: Message::UnmountStorage(s.device_path.clone()),
                        style: styles::secondary_button,
                    }),
                ),
                _ => (Some(Message::SourceToggled(path.to_path_buf())), None),
            };
            let is_selected = toggle_msg.is_some() && selected.contains(path);
            source_row(source, is_selected, toggle_msg, action, &palette)
        }))
        .spacing(spacing::SM)
        .into()
    };

    let actions = row![
        button(
            text("Add Directory...")
                .size(10)
                .color(palette.background.base.text)
        )
        .on_press(Message::AddDirectoryClicked)
        .padding([spacing::XS, spacing::SM])
        .style(styles::secondary_button),
        Space::new().width(Fill),
        button(text("Refresh").size(10).color(palette.background.weak.text))
            .on_press(Message::RefreshSources)
            .padding([spacing::XS, spacing::SM])
            .style(styles::ghost_button),
    ]
    .spacing(spacing::SM);

    column![source_list, Space::new().height(spacing::SM), actions,]
        .spacing(spacing::XS)
        .into()
}

struct Action {
    label: &'static str,
    message: Message,
    style: fn(&iced::Theme, button::Status) -> button::Style,
}

fn source_row(
    source: &Source,
    is_selected: bool,
    on_toggle: Option<Message>,
    action: Option<Action>,
    palette: &iced::theme::palette::Extended,
) -> Element<'static, Message> {
    let (icon, name, subtitle, storage) = match source {
        Source::Storage(s) => (
            icons::storage_source(),
            s.name.clone(),
            s.mount_point.as_ref().map_or_else(
                || s.device_path.display().to_string(),
                |mp| mp.display().to_string(),
            ),
            s.total_bytes.zip(s.used_bytes),
        ),
        Source::Camera(c) => (icons::camera_source(), c.name.clone(), c.port.clone(), None),
        Source::Directory(p) => (
            icons::directory_source(),
            p.file_name().map_or_else(
                || "Directory".to_owned(),
                |n| n.to_string_lossy().into_owned(),
            ),
            p.display().to_string(),
            None,
        ),
    };

    let mut header = row![].spacing(spacing::SM).align_y(Center);

    if let Some(toggle_msg) = on_toggle {
        header = header.push(checkbox(is_selected).on_toggle(move |_| toggle_msg.clone()));
    }

    header = header
        .push(icon.size(14))
        .push(text(name).size(12).color(palette.background.base.text))
        .push(Space::new().width(Fill));

    if let Some(act) = action {
        header = header.push(
            button(text(act.label).size(10).color(palette.background.base.text))
                .on_press(act.message)
                .padding([spacing::XS, spacing::SM])
                .style(act.style),
        );
    }

    // Details sit outside the header row so they span the card's full width
    // instead of being boxed into the column right of the checkbox and icon.
    let mut path_row = row![
        text(subtitle)
            .size(10)
            .color(palette.background.strong.text),
    ]
    .spacing(spacing::SM)
    .align_y(Center);

    if let Some((total, used)) = storage {
        path_row = path_row.push(Space::new().width(Fill)).push(
            text(format_storage(used, total))
                .size(10)
                .color(palette.background.strong.text),
        );
    }

    let mut details = column![path_row].spacing(spacing::XS);

    if let Some((total, used)) = storage {
        details = details.push(
            container(
                progress_bar(0.0..=1.0, storage_ratio(used, total)).style(styles::storage_progress),
            )
            .width(Fill)
            .height(Length::Fixed(3.0)),
        );
    }

    column![header, details].spacing(spacing::SM).into()
}

#[expect(
    clippy::cast_precision_loss,
    reason = "display-only ratio, precision irrelevant"
)]
fn storage_ratio(used: u64, total: u64) -> f32 {
    if total > 0 {
        (used as f32) / (total as f32)
    } else {
        0.0
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "display-only formatting, precision irrelevant"
)]
fn format_storage(used: u64, total: u64) -> String {
    fn fmt_bytes(bytes: u64) -> String {
        const GB: u64 = 1024 * 1024 * 1024;
        const MB: u64 = 1024 * 1024;
        if bytes >= GB {
            format!("{:.1} GB", bytes as f64 / GB as f64)
        } else {
            format!("{:.0} MB", bytes as f64 / MB as f64)
        }
    }
    format!("{} / {}", fmt_bytes(used), fmt_bytes(total))
}
