use chrono::{NaiveDate, NaiveTime};
use ferrocull_core::{Pattern, RenderContext};
use iced::{
    Element, Fill,
    alignment::Horizontal,
    widget::{button, column, container, pick_list, row, stack, text, text_input},
};

use crate::{
    messages::destination::Message,
    styles,
    theme::{colors, spacing},
};

/// Built-in starter patterns offered in every field's preset picker.
const BUILTIN_PRESETS: &[&str] = &[
    "{YYYY}/{MM}/{DD}/{filename}.{ext}",
    "{YYYY}/{YYYY}-{MM}-{DD}/{filename}.{ext}",
    "{YYYY}-{MM}-{DD}/{filename}.{ext}",
    "{jobcode}/{YYYY}{MM}{DD}/{filename}.{ext}",
    "{YYYY}/{MM}/{filename}.{ext}",
    "{camera_model}/{YYYY}-{MM}-{DD}/{filename}.{ext}",
    "{YYYY}{MM}{DD}_{seq}.{ext}",
];

/// Width of the chevron strip exposed on the left of the merged pattern control.
const PICKER_STRIP_WIDTH: f32 = 28.0;

/// One-line reference of every token the pattern engine understands.
const TOKEN_REFERENCE: &str = "{YYYY} {MM} {DD} {HH} {MIN} {SS} {filename} {ext} {EXT} {seq} {camera_make} {camera_model} {jobcode} {iso} {aperture} {shutter} {focal}";

fn sample_context(today: NaiveDate) -> RenderContext {
    RenderContext {
        datetime: today.and_time(NaiveTime::MIN).and_utc(),
        filename: String::from("IMG_1234"),
        extension: String::from("jpg"),
        camera_make: Some(String::from("Canon")),
        camera_model: Some(String::from("EOS R5")),
        sequence: 1,
        iso: Some(400),
        aperture: Some(2.8),
        shutter: Some(1.0 / 500.0),
        focal_length: Some(50.0),
        job_code: None,
    }
}

fn eyebrow(label: &'static str) -> Element<'static, Message> {
    text(label)
        .size(10)
        .color(crate::theme::text_muted())
        .font(crate::fonts::SANS_SEMIBOLD)
        .into()
}

/// Preset options for a field: saved patterns first, then built-ins not already
/// saved (deduped).
fn preset_options(saved_patterns: &[String]) -> Vec<String> {
    let mut options: Vec<String> = saved_patterns.to_vec();
    options.extend(
        BUILTIN_PRESETS
            .iter()
            .filter(|&&preset| !saved_patterns.iter().any(|p| p == preset))
            .map(|&preset| preset.to_owned()),
    );
    options
}

fn pattern_field(
    label: &'static str,
    value: &str,
    parsed: &Result<Pattern, impl std::fmt::Display>,
    saved_patterns: &[String],
    on_input: fn(String) -> Message,
) -> Element<'static, Message> {
    // An empty or broken pattern isn't worth persisting: no save toggle, but
    // hold the slot so the header height doesn't jump when it appears.
    let save_toggle: Element<'static, Message> = if value.is_empty() || parsed.is_err() {
        iced::widget::Space::new().into()
    } else {
        let saved = saved_patterns.iter().any(|p| p == value);
        let save_label = if saved { "Forget" } else { "Save" };
        button(text(save_label).size(11))
            .padding([2, 8])
            .style(styles::ghost_button)
            .on_press(Message::PatternSaveToggled(value.to_owned()))
            .into()
    };

    let header = row![
        text(label).size(13).width(Fill),
        container(save_toggle).align_x(Horizontal::Right),
    ]
    .align_y(iced::Alignment::Center);

    // Merged control: the picker spans the full row so its menu opens at full
    // width (iced locks the menu to the pick_list's own bounds), but only a
    // chevron strip on the left is exposed — the input covers the rest.
    let picker = pick_list(preset_options(saved_patterns), None::<String>, on_input)
        .placeholder("")
        .handle(pick_list::Handle::None)
        .font(crate::fonts::MONO)
        .text_size(12)
        .padding(6)
        .style(styles::pattern_picker)
        .width(Fill);

    let chevron = container(crate::icons::dropdown_handle().size(10))
        .width(PICKER_STRIP_WIDTH)
        .center_x(PICKER_STRIP_WIDTH)
        .padding([6, 0]);

    let input = text_input(BUILTIN_PRESETS[0], value)
        .on_input(on_input)
        .font(crate::fonts::MONO)
        .style(styles::pattern_input)
        .padding(6)
        .size(12);

    let merged = stack![
        picker,
        chevron,
        row![iced::widget::Space::new().width(PICKER_STRIP_WIDTH), input],
    ];

    let mut section = column![header, merged].spacing(spacing::XS);

    if let Err(e) = parsed {
        section = section.push(text(e.to_string()).size(10).color(colors::DANGER));
    }
    section.into()
}

fn preview_row(
    label: &'static str,
    source: &'static str,
    parsed: Result<Pattern, impl std::fmt::Display>,
    ctx: &RenderContext,
) -> Element<'static, Message> {
    let rendered: Element<'static, Message> = match parsed {
        Ok(pattern) => text(pattern.render(ctx))
            .size(11)
            .font(crate::fonts::MONO)
            .color(colors::ACCENT)
            .into(),
        Err(e) => text(e.to_string()).size(11).color(colors::DANGER).into(),
    };
    column![
        row![
            eyebrow(label),
            text(source)
                .size(10)
                .font(crate::fonts::MONO)
                .color(crate::theme::text_muted()),
        ]
        .spacing(spacing::XS),
        rendered,
    ]
    .spacing(2)
    .into()
}

pub(crate) fn rename_panel(
    photo_pattern: &str,
    video_pattern: &str,
    saved_patterns: &[String],
    today: NaiveDate,
) -> Element<'static, Message> {
    let photo_parsed = Pattern::parse(photo_pattern);
    let video_parsed = Pattern::parse(video_pattern);

    let photo_section = pattern_field(
        "Photo Pattern",
        photo_pattern,
        &photo_parsed,
        saved_patterns,
        Message::PhotoPatternChanged,
    );
    let video_section = pattern_field(
        "Video Pattern",
        video_pattern,
        &video_parsed,
        saved_patterns,
        Message::VideoPatternChanged,
    );

    // Read token by token while composing a pattern, not glanced at like a
    // label, so it carries the body size rather than the label size.
    let token_reference = text(TOKEN_REFERENCE)
        .size(12)
        .font(crate::fonts::MONO)
        .color(crate::theme::text_muted());

    let photo_ctx = sample_context(today);
    let mut video_ctx = sample_context(today);
    "MVI_0001".clone_into(&mut video_ctx.filename);
    "mov".clone_into(&mut video_ctx.extension);

    let preview_section = column![
        text("Preview").size(13),
        container(
            column![
                preview_row("PHOTO", "IMG_1234.jpg", photo_parsed, &photo_ctx),
                preview_row("VIDEO", "MVI_0001.mov", video_parsed, &video_ctx),
            ]
            .spacing(spacing::SM)
        )
        .padding(spacing::SM)
        .width(Fill)
        .style(container::bordered_box),
    ]
    .spacing(spacing::XS);

    column![
        photo_section,
        video_section,
        token_reference,
        preview_section
    ]
    .spacing(spacing::MD)
    .into()
}
