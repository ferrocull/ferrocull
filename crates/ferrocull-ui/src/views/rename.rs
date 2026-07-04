use chrono::{NaiveDate, NaiveTime};
use ferrocull_core::{Pattern, RenderContext};
use iced::{
    Element, Fill,
    widget::{column, container, text, text_input},
};

use crate::{
    messages::destination::Message,
    theme::{colors, spacing},
};

fn sample_context(today: NaiveDate) -> RenderContext {
    RenderContext {
        datetime: today.and_time(NaiveTime::MIN).and_utc(),
        filename: String::from("IMG_1234"),
        extension: String::from("jpg"),
        camera_make: Some(String::from("Canon")),
        camera_model: Some(String::from("EOS R5")),
        sequence: 1,
        iso: None,
        aperture: None,
        shutter: None,
        job_code: None,
    }
}

pub(crate) fn rename_panel(
    photo_pattern: &str,
    video_pattern: &str,
    today: NaiveDate,
) -> Element<'static, Message> {
    let help_text = text(
        "{YYYY} {MM} {DD} {HH} {MIN} {SS} {filename} {ext} {EXT} {seq} {camera_make} {camera_model} {jobcode}",
    )
    .size(10);

    let photo_parsed = Pattern::parse(photo_pattern);
    let video_parsed = Pattern::parse(video_pattern);

    let mut photo_section = column![
        text("Photo Pattern").size(13),
        text_input("{YYYY}/{MM}/{DD}/{filename}.{ext}", photo_pattern)
            .on_input(Message::PhotoPatternChanged)
            .size(12),
    ]
    .spacing(spacing::XS);

    if let Err(ref e) = photo_parsed {
        photo_section = photo_section.push(text(e.to_string()).size(10).color(colors::DANGER));
    }

    let mut video_section = column![
        text("Video Pattern").size(13),
        text_input("{YYYY}/{MM}/{DD}/{filename}.{ext}", video_pattern)
            .on_input(Message::VideoPatternChanged)
            .size(12),
    ]
    .spacing(spacing::XS);

    if let Err(ref e) = video_parsed {
        video_section = video_section.push(text(e.to_string()).size(10).color(colors::DANGER));
    }

    let photo_ctx = sample_context(today);
    let mut video_ctx = sample_context(today);
    "MVI_0001".clone_into(&mut video_ctx.filename);
    "mov".clone_into(&mut video_ctx.extension);

    let photo_preview: Element<'static, Message> = match photo_parsed {
        Ok(pattern) => text(pattern.render(&photo_ctx)).size(11).into(),
        Err(e) => text(e.to_string()).size(11).color(colors::DANGER).into(),
    };
    let video_preview: Element<'static, Message> = match video_parsed {
        Ok(pattern) => text(pattern.render(&video_ctx)).size(11).into(),
        Err(e) => text(e.to_string()).size(11).color(colors::DANGER).into(),
    };

    let preview_section = column![
        text("Preview").size(13),
        container(column![photo_preview, video_preview].spacing(2))
            .padding(spacing::SM)
            .width(Fill)
            .style(container::bordered_box),
    ]
    .spacing(spacing::XS);

    column![help_text, photo_section, video_section, preview_section]
        .spacing(spacing::MD)
        .into()
}
