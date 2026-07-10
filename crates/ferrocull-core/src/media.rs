//! Domain types for media items, filtering, and sorting.
//!
//! These are the core data types shared between the core engine and the UI.

use std::{
    cmp::{Ordering, Reverse},
    path::PathBuf,
};

use chrono::{DateTime, Datelike, Local, Utc};
use ferrocull_media::FileCategory;

/// Standard XMP color label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum ColorLabel {
    Red = 1,
    Yellow = 2,
    Green = 3,
    Blue = 4,
    Purple = 5,
    Orange = 6,
    Gray = 7,
}

impl ColorLabel {
    pub const ALL: [Self; 7] = [
        Self::Red,
        Self::Yellow,
        Self::Green,
        Self::Blue,
        Self::Purple,
        Self::Orange,
        Self::Gray,
    ];

    /// XMP `xmp:Label` string for this color.
    #[must_use]
    pub const fn xmp_str(self) -> &'static str {
        match self {
            Self::Red => "Red",
            Self::Yellow => "Yellow",
            Self::Green => "Green",
            Self::Blue => "Blue",
            Self::Purple => "Purple",
            Self::Orange => "Orange",
            Self::Gray => "Gray",
        }
    }

    /// Parse an XMP `xmp:Label` string.
    #[must_use]
    pub fn from_xmp_str(s: &str) -> Option<Self> {
        match s {
            "Red" => Some(Self::Red),
            "Yellow" => Some(Self::Yellow),
            "Green" => Some(Self::Green),
            "Blue" => Some(Self::Blue),
            "Purple" => Some(Self::Purple),
            "Orange" => Some(Self::Orange),
            "Gray" => Some(Self::Gray),
            _ => None,
        }
    }
}

impl From<ColorLabel> for u8 {
    fn from(label: ColorLabel) -> Self {
        label as Self
    }
}

impl TryFrom<u8> for ColorLabel {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Red),
            2 => Ok(Self::Yellow),
            3 => Ok(Self::Green),
            4 => Ok(Self::Blue),
            5 => Ok(Self::Purple),
            6 => Ok(Self::Orange),
            7 => Ok(Self::Gray),
            other => Err(other),
        }
    }
}

/// Capture timestamp mirroring EXIF structure (`DateTimeOriginal` + `SubSecTimeOriginal`).
/// Keeping components separate enables trivial burst detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CaptureTime {
    /// Second-precision timestamp (from `DateTimeOriginal`).
    pub second: DateTime<Utc>,
    /// Subsecond in nanoseconds (from `SubSecTimeOriginal`, 0 if unavailable).
    pub subsec_nanos: u32,
}

/// Maximum gap in milliseconds between consecutive shots to be considered a burst.
pub const BURST_THRESHOLD_MS: i64 = 1000;

impl CaptureTime {
    #[must_use]
    pub const fn new(second: DateTime<Utc>, subsec_nanos: u32) -> Self {
        Self {
            second,
            subsec_nanos,
        }
    }

    /// Calculate milliseconds from self to other (positive if other is later).
    #[must_use]
    #[expect(
        clippy::integer_division,
        reason = "nanos to millis conversion, truncation is correct"
    )]
    pub fn millis_to(&self, other: &Self) -> i64 {
        let sec_diff = other
            .second
            .signed_duration_since(self.second)
            .num_milliseconds();
        let subsec_diff = i64::from(other.subsec_nanos) - i64::from(self.subsec_nanos);
        sec_diff + subsec_diff / 1_000_000
    }

    /// Check if two captures are within burst threshold (<=1000ms apart).
    #[must_use]
    pub fn within_burst_threshold(&self, other: &Self) -> bool {
        self.millis_to(other).abs() <= BURST_THRESHOLD_MS
    }
}

impl Ord for CaptureTime {
    fn cmp(&self, other: &Self) -> Ordering {
        self.second
            .cmp(&other.second)
            .then(self.subsec_nanos.cmp(&other.subsec_nanos))
    }
}

impl PartialOrd for CaptureTime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone)]
pub struct Item {
    pub path: PathBuf,
    /// Stable identifier for persistence (canonical path string).
    pub source_id: String,
    /// Media type determined at scan time (RAW, Photo, or Video).
    pub media_type: FileCategory,
    pub capture_time: CaptureTime,
    pub is_downloaded: bool,
    /// For RAW files: path to paired JPEG (if exists). JPEGs never have this set.
    pub jpeg_pair: Option<PathBuf>,
    /// All paired files from the source card (JPEG, companion video, etc.).
    pub paired: Vec<PathBuf>,
    /// Non-XMP sidecar files from the source card (THM, WAV, MP3).
    pub sidecars: Vec<PathBuf>,
    /// XMP sidecar file from the source card, if present.
    pub xmp_sidecar: Option<PathBuf>,
    /// XMP rating in `[-1, 5]`: `-1` rejected, `0` unrated, `1..=5` star rating.
    pub rating: i8,
    /// XMP color label, or `None` if unclassified.
    pub color_label: Option<ColorLabel>,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum SortOrder {
    #[default]
    Time,
    Rating,
    Filename,
}

impl SortOrder {
    pub const ALL: [Self; 3] = [Self::Time, Self::Rating, Self::Filename];
}

impl std::fmt::Display for SortOrder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Time => write!(f, "Date"),
            Self::Rating => write!(f, "Rating"),
            Self::Filename => write!(f, "Name"),
        }
    }
}

/// Sort key for BTreeMap-based sorted view.
///
/// Enables O(log n) insertion and O(1) ascending/descending toggle.
/// Includes filename + full path as final tiebreaker to prevent collisions for identically-named
/// files from different source directories (common with multi-card ingest).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SortKey {
    Time {
        capture_time: CaptureTime,
        filename: String,
        path: PathBuf,
    },
    /// Reverse so 5-star sorts before unrated and rejected (`-1`); secondary by time, name, path.
    Rating {
        rating: Reverse<i8>,
        capture_time: CaptureTime,
        filename: String,
        path: PathBuf,
    },
    Filename {
        filename: String,
        path: PathBuf,
    },
}

impl SortKey {
    #[must_use]
    pub fn from_item(item: &Item, order: SortOrder) -> Self {
        let filename = item
            .path
            .file_name()
            .expect("scanned file has filename")
            .to_string_lossy()
            .into_owned();
        let path = item.path.clone();

        match order {
            SortOrder::Time => Self::Time {
                capture_time: item.capture_time,
                filename,
                path,
            },
            SortOrder::Rating => Self::Rating {
                rating: Reverse(item.rating),
                capture_time: item.capture_time,
                filename,
                path,
            },
            SortOrder::Filename => Self::Filename { filename, path },
        }
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum FilterMode {
    #[default]
    All,
    NewOnly,
    PhotosOnly,
    VideosOnly,
    RawOnly,
}

impl FilterMode {
    pub const ALL: [Self; 5] = [
        Self::All,
        Self::NewOnly,
        Self::PhotosOnly,
        Self::VideosOnly,
        Self::RawOnly,
    ];

    #[must_use]
    pub fn matches(self, item: &Item) -> bool {
        match self {
            Self::All => true,
            Self::NewOnly => !item.is_downloaded && item.rating != -1,
            Self::PhotosOnly => item.media_type == FileCategory::Photo,
            Self::VideosOnly => item.media_type == FileCategory::Video,
            Self::RawOnly => item.media_type == FileCategory::Raw,
        }
    }
}

impl std::fmt::Display for FilterMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::All => write!(f, "All"),
            Self::NewOnly => write!(f, "New"),
            Self::PhotosOnly => write!(f, "Photos"),
            Self::VideosOnly => write!(f, "Videos"),
            Self::RawOnly => write!(f, "RAW"),
        }
    }
}

/// Represents a selected date: year only, year+month, or year+month+day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DateSelection {
    pub year: i32,
    pub month: Option<u32>,
    pub day: Option<u32>,
}

impl DateSelection {
    #[must_use]
    pub const fn year_only(year: i32) -> Self {
        Self {
            year,
            month: None,
            day: None,
        }
    }

    #[must_use]
    pub const fn year_month(year: i32, month: u32) -> Self {
        Self {
            year,
            month: Some(month),
            day: None,
        }
    }

    #[must_use]
    pub const fn year_month_day(year: i32, month: u32, day: u32) -> Self {
        Self {
            year,
            month: Some(month),
            day: Some(day),
        }
    }
}

/// Checks if an item matches the selected date filter.
/// Returns true if no date selected (show all) or item matches the selected date.
#[must_use]
pub fn matches_date_filter(item: &Item, selected_date: Option<DateSelection>) -> bool {
    let Some(sel) = selected_date else {
        return true;
    };

    let local = item.capture_time.second.with_timezone(&Local);
    local.year() == sel.year
        && sel.month.is_none_or(|m| local.month() == m)
        && sel.day.is_none_or(|d| local.day() == d)
}
