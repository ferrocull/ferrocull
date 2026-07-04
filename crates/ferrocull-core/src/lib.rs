pub mod backup;
pub mod cache;
pub(crate) mod copy;
pub mod download;
pub(crate) mod history;
pub mod hooks;
pub mod media;
pub mod metadata_store;
pub mod pattern;
pub mod persistence;
pub mod profiles;
pub mod scan;
pub mod thumbnail;
pub mod xmp;

use std::path::PathBuf;

use chrono::{DateTime, Utc};
pub use ferrocull_media::{FileCategory, categorize_extension, is_media_file};
pub use history::JobCodeHistory;
pub use hooks::Hook;
pub use media::ColorLabel;
pub use pattern::{Pattern, RenderContext};
pub use persistence::AppSettings;
pub use profiles::{IngestConfig, NamedProfile, Profile};

/// A media file with optional related files (RAW+JPEG pairs, sidecars).
#[derive(Debug, Clone)]
pub struct MediaFile {
    pub path: PathBuf,
    pub datetime: DateTime<Utc>,
    pub media_type: FileCategory,
    pub paired_files: Vec<PathBuf>,
    pub sidecars: Vec<PathBuf>,
    pub xmp_sidecar: Option<PathBuf>,
    /// XMP rating in `[-1, 5]`: `-1` rejected, `0` unrated, `1..=5` star rating.
    pub rating: i8,
    pub color_label: Option<ColorLabel>,
    /// Rendered destination path relative to the base directory (e.g. `"2024/03/15/IMG_1234.cr2"`).
    /// If `Some`, overrides the raw source filename. May contain path separators for subdirectories.
    pub rendered_dest: Option<String>,
}
