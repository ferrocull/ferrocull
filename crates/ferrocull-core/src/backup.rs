use std::path::{Path, PathBuf};

use ferrocull_media::FileCategory;

/// A backup destination with optional subdirectories for photos/videos.
#[derive(Debug, Clone)]
pub struct Destination {
    pub path: PathBuf,
    pub photo_subpath: Option<PathBuf>,
    pub video_subpath: Option<PathBuf>,
}

impl Destination {
    /// Resolve the full destination path for a file.
    pub(crate) fn resolve_path(
        &self,
        relative_path: &Path,
        category: Option<FileCategory>,
    ) -> PathBuf {
        let subpath = match category {
            Some(FileCategory::Photo | FileCategory::Raw | FileCategory::Sidecar) => {
                self.photo_subpath.as_deref()
            }
            Some(FileCategory::Video) => self.video_subpath.as_deref(),
            None => None,
        };

        subpath.map_or_else(
            || self.path.join(relative_path),
            |sub| self.path.join(sub).join(relative_path),
        )
    }
}
