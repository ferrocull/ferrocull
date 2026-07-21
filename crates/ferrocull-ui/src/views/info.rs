//! The info strip: a frame's capture settings, rendered as one quiet line.
//!
//! Formatting and diffing are pure and kept apart from the compare layout, so a
//! future loupe readout reads the same values the same way.

use chrono::Local;
use ferrocull_core::media::{CaptureSettings, CaptureTime};
use iced::{
    Alignment, Element,
    widget::{container, row, text},
};

use crate::theme::spacing;

/// Fields in display order: shutter, aperture, ISO, focal length, capture time.
pub(crate) const FIELD_COUNT: usize = 5;

/// Stands in for a value the file does not carry, so both panes keep the same
/// five columns and absence reads as absence rather than as a gap.
const ABSENT: &str = "\u{2014}";

/// Render one frame's settings in photographic notation.
#[expect(
    clippy::integer_division,
    reason = "nanos to hundredths of a second, truncation is correct"
)]
pub(crate) fn readout(
    settings: CaptureSettings,
    capture_time: CaptureTime,
) -> [String; FIELD_COUNT] {
    let local = capture_time.second.with_timezone(&Local);
    let hundredths = capture_time.subsec_nanos / 10_000_000;

    [
        // A zero or negative exposure time is malformed EXIF; showing it as
        // absent beats rendering the `1/inf` it would divide into.
        settings
            .exposure_time
            .filter(|seconds| *seconds > 0.0)
            .map_or_else(absent, shutter),
        settings
            .aperture
            .map_or_else(absent, |value| format!("f/{}", decimal(value))),
        settings
            .iso
            .map_or_else(absent, |value| format!("ISO {value}")),
        settings
            .focal_length
            .map_or_else(absent, |value| format!("{}mm", decimal(value))),
        format!("{}.{hundredths:02}", local.format("%H:%M:%S")),
    ]
}

/// Which fields differ between two rendered readouts — the tie-breaking
/// differences the photographer is looking for. Compares what is displayed, so
/// two apertures that round to the same notation read as equal.
pub(crate) fn differing(
    select: &[String; FIELD_COUNT],
    candidate: &[String; FIELD_COUNT],
) -> [bool; FIELD_COUNT] {
    std::array::from_fn(|field| select[field] != candidate[field])
}

/// One pane's readout as a single centered line. Differing fields take the
/// brighter body ink; equal ones stay in the muted secondary tone.
pub(crate) fn strip<Message: 'static>(
    values: &[String; FIELD_COUNT],
    differing: [bool; FIELD_COUNT],
) -> Element<'static, Message> {
    let palette = crate::theme::palette();
    let fields = values.iter().zip(differing).map(|(value, differs)| {
        text(value.clone())
            .size(10)
            .color(if differs {
                palette.background.base.text
            } else {
                palette.background.weak.text
            })
            .into()
    });

    container(
        row(fields)
            .spacing(spacing::MD)
            .align_y(Alignment::Center)
            .wrap(),
    )
    .center_x(iced::Fill)
    .padding([spacing::XS, spacing::SM])
    .into()
}

fn absent() -> String {
    ABSENT.to_owned()
}

/// Shutter speed in the notation photographers read: a fraction below a second,
/// a decimal above it.
fn shutter(seconds: f64) -> String {
    if seconds >= 1.0 {
        return format!("{}s", decimal(seconds));
    }
    format!("1/{:.0}", 1.0 / seconds)
}

/// One decimal place with a trailing `.0` dropped: `2.8`, `8`, `50`.
fn decimal(value: f64) -> String {
    let text = format!("{value:.1}");
    text.strip_suffix(".0").unwrap_or(&text).to_owned()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{CaptureSettings, CaptureTime, differing, readout};

    /// A capture time in UTC; the readout renders it in local time, so only
    /// fields other than the timestamp are asserted against it.
    fn time() -> CaptureTime {
        CaptureTime::new(
            Utc.with_ymd_and_hms(2024, 5, 1, 10, 14, 22)
                .single()
                .expect("unambiguous test timestamp"),
            450_000_000,
        )
    }

    fn settings() -> CaptureSettings {
        CaptureSettings {
            exposure_time: Some(1.0 / 500.0),
            aperture: Some(2.8),
            iso: Some(400),
            focal_length: Some(50.0),
        }
    }

    #[test]
    fn renders_settings_in_photographic_notation() {
        let values = readout(settings(), time());
        assert_eq!(values[0], "1/500", "shutter is a fraction");
        assert_eq!(values[1], "f/2.8", "aperture is an f-number");
        assert_eq!(values[2], "ISO 400");
        assert_eq!(values[3], "50mm");
    }

    #[test]
    fn renders_whole_stops_without_a_trailing_zero() {
        let values = readout(
            CaptureSettings {
                aperture: Some(8.0),
                focal_length: Some(135.0),
                ..settings()
            },
            time(),
        );
        assert_eq!(values[1], "f/8");
        assert_eq!(values[3], "135mm");
    }

    #[test]
    fn renders_long_exposures_in_seconds() {
        let values = readout(
            CaptureSettings {
                exposure_time: Some(2.5),
                ..settings()
            },
            time(),
        );
        assert_eq!(values[0], "2.5s");
    }

    #[test]
    fn renders_capture_time_with_subseconds() {
        let values = readout(settings(), time());
        assert!(
            values[4].ends_with(":22.45"),
            "seconds and hundredths are shown: {}",
            values[4]
        );
    }

    #[test]
    fn malformed_exposure_time_renders_as_absent() {
        let values = readout(
            CaptureSettings {
                exposure_time: Some(0.0),
                ..settings()
            },
            time(),
        );
        assert_eq!(values[0], "\u{2014}", "a zero shutter speed is not 1/inf");
    }

    #[test]
    fn missing_values_render_as_dashes() {
        let values = readout(CaptureSettings::default(), time());
        assert_eq!(
            &values[..4],
            ["\u{2014}", "\u{2014}", "\u{2014}", "\u{2014}"],
            "a file without EXIF keeps all four setting columns"
        );
    }

    #[test]
    fn identical_frames_have_no_differing_fields() {
        let values = readout(settings(), time());
        assert_eq!(differing(&values, &values), [false; 5]);
    }

    #[test]
    fn only_the_changed_field_differs() {
        let select = readout(settings(), time());
        let candidate = readout(
            CaptureSettings {
                iso: Some(1600),
                ..settings()
            },
            time(),
        );
        assert_eq!(
            differing(&select, &candidate),
            [false, false, true, false, false],
            "ISO alone differs"
        );
    }

    #[test]
    fn a_value_present_on_one_side_only_differs() {
        let select = readout(settings(), time());
        let candidate = readout(
            CaptureSettings {
                focal_length: None,
                ..settings()
            },
            time(),
        );
        assert_eq!(
            differing(&select, &candidate),
            [false, false, false, true, false],
            "a missing focal length is a difference, not a match"
        );
    }

    #[test]
    fn apertures_that_render_alike_are_not_differences() {
        let select = readout(settings(), time());
        let candidate = readout(
            CaptureSettings {
                aperture: Some(2.79),
                ..settings()
            },
            time(),
        );
        assert!(
            !differing(&select, &candidate)[1],
            "both render as f/2.8, so nothing is emphasized"
        );
    }
}
