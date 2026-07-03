use std::path::{Component, Path, PathBuf};

use ferrocull_media::FileCategory;

use crate::copy::{self, copy_with_checksum};

/// A backup destination with optional subdirectories for photos/videos.
#[derive(Debug, Clone)]
pub struct Destination {
    pub path: PathBuf,
    pub photo_subpath: Option<PathBuf>,
    pub video_subpath: Option<PathBuf>,
}

impl Destination {
    /// Resolve the full destination path for a file.
    fn resolve_path(&self, relative_path: &Path, category: Option<FileCategory>) -> PathBuf {
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

/// A backup job: copy a downloaded file to multiple destinations.
#[derive(Debug)]
pub struct Job<'a> {
    pub source_file: PathBuf,
    pub relative_path: PathBuf,
    pub media_type: Option<FileCategory>,
    pub destinations: &'a [Destination],
}

/// Progress report for backup operations.
#[derive(Debug, Clone)]
pub struct Progress<'a> {
    pub destination_index: usize,
    pub destination_path: &'a Path,
    pub copy_progress: copy::Progress,
}

/// Error from backing up to a single destination.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid backup relative path: {}", path.display())]
    InvalidRelativePath { path: PathBuf },
    #[error("backup to {} failed: {error}", destination.display())]
    Copy {
        destination: PathBuf,
        error: copy::Error,
    },
}

fn sanitize_relative_path(relative_path: &Path) -> Option<PathBuf> {
    let mut sanitized = PathBuf::new();
    for component in relative_path.components() {
        match component {
            Component::Normal(part) => sanitized.push(part),
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => return None,
        }
    }
    (!sanitized.as_os_str().is_empty()).then_some(sanitized)
}

/// Execute a backup job, copying to all destinations.
///
/// Continues to subsequent destinations even if one fails.
/// Returns `Ok((destination_path, checksum))` or `Err(Error)` for each destination.
pub fn execute_backup(
    job: &Job<'_>,
    mut progress_fn: impl FnMut(Progress),
) -> Vec<Result<(PathBuf, String), Error>> {
    let Some(relative_path) = sanitize_relative_path(&job.relative_path) else {
        return (0..job.destinations.len())
            .map(|_| {
                Err(Error::InvalidRelativePath {
                    path: job.relative_path.clone(),
                })
            })
            .collect();
    };

    job.destinations
        .iter()
        .enumerate()
        .map(|(idx, dest)| {
            let dest_path = dest.resolve_path(&relative_path, job.media_type);

            let result = copy_with_checksum(&job.source_file, &dest_path, |copy_progress| {
                progress_fn(Progress {
                    destination_index: idx,
                    destination_path: &dest_path,
                    copy_progress,
                });
            });

            match result {
                Ok(checksum) => Ok((dest_path, checksum)),
                Err(error) => Err(Error::Copy {
                    destination: dest_path,
                    error,
                }),
            }
        })
        .collect()
}
