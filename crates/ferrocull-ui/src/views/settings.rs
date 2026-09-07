//! Settings popup card: a category rail plus the per-category panes for app
//! preferences (appearance, storage). The scrim, centering, and click-outside
//! dismissal are composed by `app::settings_overlay`; this module owns only the
//! card's interior.

use ferrocull_core::ThemePreference;
use iced::{
    Alignment, Border, Element, Fill, Length, Theme,
    widget::{Space, button, column, container, pick_list, row, text},
};

use crate::{
    app::SettingsState,
    messages::settings::{Category, Message},
    styles,
    theme::{colors, radius, spacing},
};

/// Selectable grid thumbnail resolutions (longest edge, in pixels).
const THUMBNAIL_RESOLUTIONS: [u32; 2] = [256, 512];

/// The left category rail. Selected item carries the amber "selected frame" cue.
pub(crate) fn rail(selected: Category) -> Element<'static, Message> {
    let items = Category::ALL.into_iter().map(|category| {
        button(text(category.label()).size(13))
            .width(Fill)
            .padding([spacing::SM, spacing::MD])
            .style(styles::settings_rail_item(category == selected))
            .on_press(Message::SelectCategory(category))
            .into()
    });

    column(items)
        .spacing(spacing::XS)
        .width(Length::Fixed(150.0))
        .into()
}

/// Appearance pane: theme preference.
pub(crate) fn appearance_pane(theme: ThemePreference) -> Element<'static, Message> {
    let picker = pick_list(ThemePreference::ALL, Some(theme), Message::ThemeChanged)
        .text_size(12)
        .padding([4, 8]);

    column![
        heading("THEME"),
        control_row("Appearance", picker.into()),
        hint("Auto follows your system light or dark setting."),
    ]
    .spacing(spacing::MD)
    .into()
}

/// Storage pane: thumbnail resolution and cache location, each with an inline
/// confirmation before its destructive change commits.
pub(crate) fn storage_pane(
    state: &SettingsState,
    thumbnail_resolution: u32,
    cache_display: String,
    scan_in_flight: bool,
) -> Element<'static, Message> {
    let shown_resolution = state
        .pending_thumbnail_resolution
        .unwrap_or(thumbnail_resolution);
    let resolution_picker = pick_list(
        THUMBNAIL_RESOLUTIONS,
        Some(shown_resolution),
        Message::ThumbnailResolutionSelected,
    )
    .text_size(12)
    .padding([4, 8]);

    let mut thumb_section = column![
        heading("THUMBNAIL RESOLUTION"),
        control_row("Resolution (px)", resolution_picker.into()),
        hint(
            "Higher resolution sharpens the grid on high-DPI displays and at the largest \
             thumbnail sizes, and uses more disk.",
        ),
    ]
    .spacing(spacing::MD);

    if let Some(pending) = state.pending_thumbnail_resolution {
        thumb_section = thumb_section.push(confirm_callout(
            format!(
                "Rebuild thumbnails at {pending} px? This clears the thumbnail cache and regenerates it."
            ),
            "Apply",
            Message::ConfirmThumbnailResolution,
            Message::CancelThumbnailResolution,
            scan_in_flight,
            "Waiting for the current scan to finish\u{2026}",
        ));
    }

    let field = container(
        text(cache_display)
            .size(12)
            .color(crate::theme::palette().background.base.text),
    )
    .padding([spacing::SM, spacing::MD])
    .width(Fill)
    .style(inset_field);

    let browse = button(text("Browse\u{2026}").size(12))
        .padding([spacing::SM, spacing::MD])
        .style(styles::secondary_button)
        .on_press(Message::BrowseCacheDir);

    let mut cache_section = column![
        heading("CACHE LOCATION"),
        row![field, browse]
            .spacing(spacing::SM)
            .align_y(Alignment::Center),
        hint("Thumbnails and previews live here. Changing it moves the existing files."),
    ]
    .spacing(spacing::MD);

    if let Some(pending) = &state.pending_cache_dir {
        let busy = scan_in_flight || state.cache_move_in_flight;
        let note = if state.cache_move_in_flight {
            "Moving cache\u{2026}"
        } else {
            "Waiting for the current scan to finish\u{2026}"
        };
        cache_section = cache_section.push(confirm_callout(
            format!("Move the cache to {}?", pending.display()),
            "Move",
            Message::ConfirmCacheDir,
            Message::CancelCacheDir,
            busy,
            note,
        ));
    }

    column![thumb_section, section_divider(), cache_section]
        .spacing(spacing::LG)
        .into()
}

/// Uppercase section label.
fn heading(label: &str) -> Element<'static, Message> {
    text(label.to_owned())
        .size(13)
        .color(crate::theme::palette().background.base.text)
        .into()
}

/// Muted explanatory line under a control.
fn hint(message: &str) -> Element<'static, Message> {
    text(message.to_owned())
        .size(12)
        .color(crate::theme::palette().background.weak.text)
        .into()
}

/// A label on the left, its control pushed to the right.
fn control_row(label: &str, control: Element<'static, Message>) -> Element<'static, Message> {
    row![
        text(label.to_owned()).size(13),
        Space::new().width(Fill),
        control,
    ]
    .align_y(Alignment::Center)
    .spacing(spacing::MD)
    .into()
}

/// Amber-bordered callout confirming a destructive change. `disabled` greys the
/// apply action and shows `disabled_note` instead of committing.
fn confirm_callout(
    prompt: String,
    apply_label: &'static str,
    apply_msg: Message,
    cancel_msg: Message,
    disabled: bool,
    disabled_note: &'static str,
) -> Element<'static, Message> {
    let apply = button(text(apply_label).size(12))
        .padding([spacing::XS, spacing::MD])
        .style(styles::primary_button);
    let apply = if disabled {
        apply
    } else {
        apply.on_press(apply_msg)
    };

    let cancel = button(text("Cancel").size(12))
        .padding([spacing::XS, spacing::MD])
        .style(styles::secondary_button)
        .on_press(cancel_msg);

    let mut body = column![
        text(prompt)
            .size(12)
            .color(crate::theme::palette().background.base.text),
        row![apply, cancel].spacing(spacing::SM),
    ]
    .spacing(spacing::SM);

    if disabled {
        body = body.push(text(disabled_note).size(11).color(colors::WARNING));
    }

    container(body)
        .padding(spacing::MD)
        .width(Fill)
        .style(callout)
        .into()
}

/// Inset background for the read-only cache-path field.
fn inset_field(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.weak.color.into()),
        border: Border {
            color: palette.background.strong.color,
            width: 1.0,
            radius: radius::SM.into(),
        },
        ..Default::default()
    }
}

/// Amber-tinted surface for a destructive-change callout.
fn callout(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.weak.color.into()),
        border: Border {
            color: colors::ACCENT_MUTED,
            width: 1.0,
            radius: radius::MD.into(),
        },
        ..Default::default()
    }
}

/// Hairline horizontal rule between panes' sections.
fn section_divider() -> Element<'static, Message> {
    container(Space::new().height(1))
        .width(Fill)
        .height(1)
        .style(|theme: &Theme| container::Style {
            background: Some(theme.extended_palette().background.weaker.color.into()),
            ..Default::default()
        })
        .into()
}
