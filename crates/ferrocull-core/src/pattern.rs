use std::{borrow::Cow, fmt::Write};

use chrono::{DateTime, Datelike, Timelike, Utc};

/// A parsed pattern ready for rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    segments: Vec<Segment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VariableName {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    Filename,
    ExtLower,
    ExtUpper,
    CameraMake,
    CameraModel,
    Sequence,
    Iso,
    Aperture,
    Shutter,
    FocalLength,
    JobCode,
}

impl VariableName {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "YYYY" => Some(Self::Year),
            "MM" => Some(Self::Month),
            "DD" => Some(Self::Day),
            "HH" => Some(Self::Hour),
            "MIN" => Some(Self::Minute),
            "SS" => Some(Self::Second),
            "filename" => Some(Self::Filename),
            "ext" => Some(Self::ExtLower),
            "EXT" => Some(Self::ExtUpper),
            "camera_make" => Some(Self::CameraMake),
            "camera_model" => Some(Self::CameraModel),
            "seq" => Some(Self::Sequence),
            "iso" => Some(Self::Iso),
            "aperture" => Some(Self::Aperture),
            "shutter" => Some(Self::Shutter),
            "focal" => Some(Self::FocalLength),
            "jobcode" => Some(Self::JobCode),
            _ => None,
        }
    }

    const fn supports_zero_padding(self) -> bool {
        matches!(
            self,
            Self::Year
                | Self::Month
                | Self::Day
                | Self::Hour
                | Self::Minute
                | Self::Second
                | Self::Sequence
                | Self::Iso
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Variable {
        name: VariableName,
        width: Option<usize>,
    },
}

/// Context holding values for pattern variables.
#[derive(Debug, Clone)]
pub struct RenderContext {
    pub datetime: DateTime<Utc>,
    pub filename: String,
    pub extension: String,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub sequence: u32,
    pub iso: Option<u32>,
    /// Aperture as an f-number.
    pub aperture: Option<f64>,
    /// Shutter speed in seconds.
    pub shutter: Option<f64>,
    /// Focal length in millimetres.
    pub focal_length: Option<f64>,
    pub job_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("unclosed '{{' at position {position}")]
    UnclosedBrace { position: usize },
    #[error("empty variable name at position {position}")]
    EmptyVariable { position: usize },
    #[error("invalid width '{width}' at position {position}")]
    InvalidWidth { position: usize, width: String },
    #[error("width specifier is not supported for variable '{name}' at position {position}")]
    WidthNotSupported { position: usize, name: String },
    #[error("unknown variable '{name}' at position {position}")]
    UnknownVariable { position: usize, name: String },
}

impl Pattern {
    /// Parse a pattern string into segments.
    ///
    /// # Errors
    /// Returns `Error::UnclosedBrace` if a `{` is not closed.
    /// Returns `Error::EmptyVariable` if `{}` is found.
    /// Returns `Error::InvalidWidth` if width specifier is not a valid number.
    /// Returns `Error::WidthNotSupported` when width is used on non-padded variables.
    ///
    /// # Syntax
    /// - Literal text: anything not in `{}`
    /// - Variables: `{NAME}` or `{NAME:WIDTH}` for zero-padding
    pub fn parse(s: &str) -> Result<Self, Error> {
        let mut segments = Vec::new();
        let mut chars = s.char_indices();
        let mut literal_start: Option<usize> = None;

        while let Some((i, c)) = chars.next() {
            if c == '{' {
                if let Some(start) = literal_start.take() {
                    segments.push(Segment::Literal(s[start..i].to_string()));
                }

                let var_start = i + 1;
                let (var_end, _) = chars
                    .by_ref()
                    .find(|&(_, ch)| ch == '}')
                    .ok_or(Error::UnclosedBrace { position: i })?;
                let var_content = &s[var_start..var_end];

                if var_content.is_empty() {
                    return Err(Error::EmptyVariable { position: i });
                }

                let (name_str, width) = match var_content.find(':') {
                    Some(colon_pos) => {
                        let name_str = &var_content[..colon_pos];
                        let width_str = &var_content[colon_pos + 1..];
                        let width: usize =
                            width_str.parse().ok().filter(|&w| w > 0).ok_or_else(|| {
                                Error::InvalidWidth {
                                    position: var_start + colon_pos + 1,
                                    width: width_str.to_owned(),
                                }
                            })?;
                        (name_str, Some(width))
                    }
                    None => (var_content, None),
                };

                let name = VariableName::parse(name_str).ok_or_else(|| Error::UnknownVariable {
                    position: var_start,
                    name: name_str.to_owned(),
                })?;

                if width.is_some() && !name.supports_zero_padding() {
                    return Err(Error::WidthNotSupported {
                        position: var_start,
                        name: name_str.to_owned(),
                    });
                }

                segments.push(Segment::Variable { name, width });
            } else if literal_start.is_none() {
                literal_start = Some(i);
            }
        }

        if let Some(start) = literal_start {
            segments.push(Segment::Literal(s[start..].to_string()));
        }

        Ok(Self { segments })
    }

    /// Render the pattern with the given context.
    #[must_use]
    pub fn render(&self, ctx: &RenderContext) -> String {
        let mut result = String::with_capacity(64);
        for seg in &self.segments {
            match seg {
                Segment::Literal(s) => result.push_str(s),
                Segment::Variable { name, width } => {
                    let value = resolve_variable(*name, ctx);
                    match *width {
                        Some(w) if !value.is_empty() => {
                            let _ = write!(result, "{value:0>w$}");
                        }
                        Some(_) | None => result.push_str(&value),
                    }
                }
            }
        }
        result
    }
}

fn resolve_variable(name: VariableName, ctx: &RenderContext) -> Cow<'_, str> {
    match name {
        VariableName::Year => Cow::Owned(format!("{:04}", ctx.datetime.year())),
        VariableName::Month => Cow::Owned(format!("{:02}", ctx.datetime.month())),
        VariableName::Day => Cow::Owned(format!("{:02}", ctx.datetime.day())),
        VariableName::Hour => Cow::Owned(format!("{:02}", ctx.datetime.hour())),
        VariableName::Minute => Cow::Owned(format!("{:02}", ctx.datetime.minute())),
        VariableName::Second => Cow::Owned(format!("{:02}", ctx.datetime.second())),
        VariableName::Filename => Cow::Borrowed(&ctx.filename),
        VariableName::ExtLower => Cow::Owned(ctx.extension.to_lowercase()),
        VariableName::ExtUpper => Cow::Owned(ctx.extension.to_uppercase()),
        VariableName::CameraMake => Cow::Borrowed(ctx.camera_make.as_deref().unwrap_or("")),
        VariableName::CameraModel => Cow::Borrowed(ctx.camera_model.as_deref().unwrap_or("")),
        VariableName::Sequence => Cow::Owned(ctx.sequence.to_string()),
        VariableName::Iso => ctx
            .iso
            .map_or(Cow::Borrowed(""), |v| Cow::Owned(v.to_string())),
        VariableName::Aperture => ctx
            .aperture
            .map_or(Cow::Borrowed(""), |v| Cow::Owned(format!("{v:.1}"))),
        VariableName::Shutter => ctx
            .shutter
            .map_or(Cow::Borrowed(""), |v| Cow::Owned(format_shutter(v))),
        VariableName::FocalLength => ctx.focal_length.map_or(Cow::Borrowed(""), |v| {
            Cow::Owned(format!("{}mm", decimal(v)))
        }),
        VariableName::JobCode => Cow::Borrowed(ctx.job_code.as_deref().unwrap_or("")),
    }
}

/// Shutter speed as a filename-safe fraction: `1/500` would open a directory
/// level, so the divider is a hyphen. A second or longer reads as a decimal.
fn format_shutter(seconds: f64) -> String {
    if seconds >= 1.0 {
        return format!("{}s", decimal(seconds));
    }
    let denominator = 1.0 / seconds;
    // The third-stop marks just under a second (1/1.6, 1/1.3) round to a whole
    // denominator that names a different exposure entirely.
    if denominator < 10.0 {
        return format!("1-{}", decimal(denominator));
    }
    format!("1-{denominator:.0}")
}

/// One decimal place with a trailing `.0` dropped: `2.5`, `30`, `50`.
fn decimal(value: f64) -> String {
    let text = format!("{value:.1}");
    text.strip_suffix(".0").unwrap_or(&text).to_owned()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{Pattern, RenderContext};

    fn context() -> RenderContext {
        RenderContext {
            datetime: Utc
                .with_ymd_and_hms(2024, 5, 1, 10, 14, 22)
                .single()
                .expect("unambiguous test timestamp"),
            filename: String::from("IMG_1234"),
            extension: String::from("cr3"),
            camera_make: None,
            camera_model: None,
            sequence: 1,
            iso: Some(400),
            aperture: Some(2.8),
            shutter: Some(0.002),
            focal_length: Some(50.0),
            job_code: None,
        }
    }

    fn render(pattern: &str, ctx: &RenderContext) -> String {
        Pattern::parse(pattern)
            .expect("test pattern parses")
            .render(ctx)
    }

    #[test]
    fn camera_tokens_render_as_the_camera_reports_them() {
        let mut ctx = context();
        ctx.camera_make = Some(String::from("Canon"));
        ctx.camera_model = Some(String::from("Canon EOS R5"));
        assert_eq!(
            render("{camera_make}/{camera_model}/{filename}.{ext}", &ctx),
            "Canon/Canon EOS R5/IMG_1234.cr3",
            "spaces and case survive: they are legal in path components"
        );
    }

    #[test]
    fn camera_tokens_render_empty_without_exif() {
        assert_eq!(
            render("{camera_make}{camera_model}{filename}", &context()),
            "IMG_1234",
            "an absent camera identity contributes nothing"
        );
    }

    #[test]
    fn exposure_tokens_render_capture_settings() {
        assert_eq!(
            render("{iso}_{aperture}_{shutter}_{focal}", &context()),
            "400_2.8_1-500_50mm"
        );
    }

    #[test]
    fn shutter_never_renders_a_path_separator() {
        let mut ctx = context();
        ctx.shutter = Some(1.0 / 8000.0);
        assert!(!render("{shutter}", &ctx).contains('/'));
    }

    #[test]
    fn long_exposures_render_as_seconds() {
        let mut ctx = context();
        ctx.shutter = Some(2.5);
        assert_eq!(render("{shutter}", &ctx), "2.5s");

        ctx.shutter = Some(30.0);
        assert_eq!(render("{shutter}", &ctx), "30s");
    }

    #[test]
    fn third_stop_marks_below_a_second_keep_their_denominator() {
        let mut ctx = context();
        ctx.shutter = Some(0.625);
        assert_eq!(render("{shutter}", &ctx), "1-1.6");

        ctx.shutter = Some(1.0 / 1.3);
        assert_eq!(render("{shutter}", &ctx), "1-1.3");
    }

    #[test]
    fn fractional_focal_lengths_keep_their_decimal() {
        let mut ctx = context();
        ctx.focal_length = Some(10.5);
        assert_eq!(render("{focal}", &ctx), "10.5mm");
    }

    #[test]
    fn absent_capture_settings_render_empty() {
        let mut ctx = context();
        ctx.iso = None;
        ctx.aperture = None;
        ctx.shutter = None;
        ctx.focal_length = None;
        assert_eq!(render("a{iso}{aperture}{shutter}{focal}b", &ctx), "ab");
    }

    #[test]
    fn iso_accepts_zero_padding() {
        assert_eq!(render("{iso:5}", &context()), "00400");
    }
}
