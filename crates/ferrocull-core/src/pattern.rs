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
    pub aperture: Option<f64>,
    pub shutter: Option<String>,
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
        VariableName::Shutter => Cow::Borrowed(ctx.shutter.as_deref().unwrap_or("")),
        VariableName::JobCode => Cow::Borrowed(ctx.job_code.as_deref().unwrap_or("")),
    }
}
