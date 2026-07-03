//! Centralized media file type definitions.
//!
//! Single source of truth for file extension categorization across the codebase.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileCategory {
    Photo,
    Raw,
    Video,
    Sidecar,
}

pub const PHOTO_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tiff", "tif", "heic", "heif"];
pub const RAW_EXTENSIONS: &[&str] = &[
    "cr2", "cr3", "nef", "arw", "orf", "rw2", "dng", "raf", "pef", "srw", "iiq", "3fr", "rwl",
    "x3f", "fff", "gpr",
];
pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mov", "avi", "mkv", "mts", "m2ts", "mxf"];
pub const SIDECAR_EXTENSIONS: &[&str] = &["xmp", "thm", "wav", "mp3"];

/// Check if a path is a recognized media file by extension.
#[must_use]
pub fn is_media_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .and_then(categorize_extension)
        .is_some()
}

/// Categorize a file extension into Photo/Raw/Video/Sidecar.
#[must_use]
pub fn categorize_extension(ext: &str) -> Option<FileCategory> {
    [
        (PHOTO_EXTENSIONS, FileCategory::Photo),
        (RAW_EXTENSIONS, FileCategory::Raw),
        (VIDEO_EXTENSIONS, FileCategory::Video),
        (SIDECAR_EXTENSIONS, FileCategory::Sidecar),
    ]
    .into_iter()
    .find_map(|(exts, cat)| {
        exts.iter()
            .any(|e| e.eq_ignore_ascii_case(ext))
            .then_some(cat)
    })
}
