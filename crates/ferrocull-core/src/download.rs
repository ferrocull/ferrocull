use std::{
    collections::HashSet,
    fs, io,
    path::{Component, Path, PathBuf},
};

use crate::{
    FileCategory, MediaFile,
    copy::{self, copy_with_checksum, hash_file},
    metadata_store,
    xmp::write_sidecar,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid rendered destination path '{rendered}' for {}", path.display())]
    InvalidDestinationPath { path: PathBuf, rendered: String },
    #[error(transparent)]
    Copy(#[from] copy::Error),
}

/// A download job containing source files and destination directory.
#[derive(Debug, Clone)]
pub struct Job {
    pub files: Vec<MediaFile>,
    pub dest_base: PathBuf,
    pub videos_dest: PathBuf,
    pub delete_after_download: bool,
}

/// Progress update for the download operation.
#[derive(Debug, Clone, Copy)]
pub struct Progress<'a> {
    pub current_file_index: usize,
    pub total_files: usize,
    pub file_bytes_copied: u64,
    pub file_total_bytes: u64,
    pub current_file: &'a Path,
}

/// Result of copying a single file.
pub type FileResult = Result<Success, Failure>;

#[derive(Debug)]
pub struct Success {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub checksum: String,
    pub media_type: FileCategory,
    pub source_deleted: bool,
}

#[derive(Debug)]
pub struct Failure {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub error: Error,
}

/// Write an XMP sidecar next to the destination if the file has metadata worth preserving.
///
/// Returns `Ok(())` when there's nothing to write or the write succeeded; `Err` carries
/// the underlying I/O failure so the caller can decide whether to fail the download or
/// just surface a warning. XMP carries user-authored data (ratings, labels), so silent
/// loss is worth flagging up-stack.
fn write_xmp_sidecar(media_file: &MediaFile, dest: &Path) -> io::Result<()> {
    let Some(payload) = metadata_store::ingest_payload(media_file) else {
        return Ok(());
    };

    write_sidecar(dest, &payload)
}

/// Delete the source file and its paired/sidecar files.
/// Returns true if the primary source file was deleted.
fn delete_source_files(
    media_file: &MediaFile,
    primary_sources: &HashSet<PathBuf>,
    copied_extras: &HashSet<PathBuf>,
) -> bool {
    if let Err(e) = fs::remove_file(&media_file.path) {
        tracing::warn!(
            path = %media_file.path.display(),
            error = %e,
            "failed to delete source after download"
        );
        return false;
    }

    for extra in media_file.paired_files.iter().chain(&media_file.sidecars) {
        if primary_sources.contains(extra) {
            continue;
        }
        if !copied_extras.contains(extra) {
            tracing::warn!(
                path = %extra.display(),
                "skipping extra source delete because destination copy was not verified"
            );
            continue;
        }
        if let Err(e) = fs::remove_file(extra) {
            tracing::warn!(
                path = %extra.display(),
                error = %e,
                "failed to delete paired/sidecar source file"
            );
        }
    }

    // Source XMP was read during scan; a new one is generated at the destination.
    if let Some(ref xmp) = media_file.xmp_sidecar
        && let Err(e) = fs::remove_file(xmp)
    {
        tracing::warn!(
            path = %xmp.display(),
            error = %e,
            "failed to delete source XMP sidecar"
        );
    }

    true
}

fn same_contents(left: &Path, right: &Path) -> Result<bool, io::Error> {
    Ok(hash_file(left)? == hash_file(right)?)
}

/// Copy paired and sidecar files alongside the primary destination.
///
/// Returns `(copied_or_matched, all_verified)`: paths safe to delete after download
/// (copied successfully, or existed at destination with identical contents) and
/// whether every extra was either successfully copied or verified.
fn copy_extras(
    media_file: &MediaFile,
    extras_dir: &Path,
    primary_sources: &HashSet<PathBuf>,
) -> (HashSet<PathBuf>, bool) {
    let mut copied_or_matched = HashSet::new();
    let mut all_verified = true;

    for extra in media_file.paired_files.iter().chain(&media_file.sidecars) {
        if primary_sources.contains(extra) {
            continue;
        }

        let name = extra.file_name().expect("scanned file has filename");

        let extra_dest = extras_dir.join(name);
        match copy_with_checksum(extra, &extra_dest, |_| {}) {
            Ok(_) => {
                copied_or_matched.insert(extra.clone());
            }
            Err(copy::Error::DestinationExists { .. }) => match same_contents(extra, &extra_dest) {
                Ok(true) => {
                    copied_or_matched.insert(extra.clone());
                }
                Ok(false) => {
                    all_verified = false;
                    tracing::warn!(
                        src = %extra.display(),
                        dest = %extra_dest.display(),
                        "destination paired/sidecar exists with different contents"
                    );
                }
                Err(e) => {
                    all_verified = false;
                    tracing::warn!(
                        src = %extra.display(),
                        dest = %extra_dest.display(),
                        error = %e,
                        "failed to compare paired/sidecar file with existing destination"
                    );
                }
            },
            Err(e) => {
                all_verified = false;
                tracing::warn!(
                    src = %extra.display(),
                    dest = %extra_dest.display(),
                    error = %e,
                    "failed to copy paired/sidecar file"
                );
            }
        }
    }

    (copied_or_matched, all_verified)
}

fn resolve_destination_path(
    base: &Path,
    rendered: Option<&str>,
    source: &Path,
) -> Result<PathBuf, Error> {
    let filename = source.file_name().expect("scanned file has filename");
    let Some(rendered) = rendered else {
        return Ok(base.join(filename));
    };

    let mut relative = PathBuf::new();
    for component in Path::new(rendered).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                return Err(Error::InvalidDestinationPath {
                    path: source.to_path_buf(),
                    rendered: rendered.to_owned(),
                });
            }
        }
    }

    if relative.as_os_str().is_empty() {
        return Err(Error::InvalidDestinationPath {
            path: source.to_path_buf(),
            rendered: rendered.to_owned(),
        });
    }

    Ok(base.join(relative))
}

/// Executes a download job, copying all files to the destination.
///
/// Videos are routed to `videos_dest`, everything else to `dest_base`.
///
/// If `delete_after_download` is true, deletes source files after successful
/// copy and checksum verification. Deletion failures are logged but do not
/// affect the overall success status.
///
/// Returns a result for each file in the job.
pub fn execute_download(job: &Job, mut progress_fn: impl FnMut(Progress<'_>)) -> Vec<FileResult> {
    let total_files = job.files.len();
    let primary_sources: HashSet<PathBuf> = job.files.iter().map(|f| f.path.clone()).collect();

    job.files
        .iter()
        .enumerate()
        .map(|(index, media_file)| {
            let source = media_file.path.clone();

            let base = if media_file.media_type == FileCategory::Video {
                &job.videos_dest
            } else {
                &job.dest_base
            };
            let dest = match resolve_destination_path(
                base,
                media_file.rendered_dest.as_deref(),
                &source,
            ) {
                Ok(dest) => dest,
                Err(error) => {
                    return Err(Failure {
                        source,
                        destination: PathBuf::new(),
                        error,
                    });
                }
            };

            let result = copy_with_checksum(&media_file.path, &dest, |progress| {
                progress_fn(Progress {
                    current_file_index: index,
                    total_files,
                    file_bytes_copied: progress.bytes_copied,
                    file_total_bytes: progress.total_bytes,
                    current_file: &media_file.path,
                });
            });

            match result {
                Ok(checksum) => {
                    let (copied_or_matched, all_verified) = copy_extras(
                        media_file,
                        dest.parent().expect("dest is a file within a directory"),
                        &primary_sources,
                    );
                    if let Err(e) = write_xmp_sidecar(media_file, &dest) {
                        tracing::warn!(
                            path = %dest.display(),
                            error = %e,
                            "XMP sidecar write failed; rating/label may not travel with the file",
                        );
                    }

                    if job.delete_after_download && !all_verified {
                        tracing::warn!(
                            path = %source.display(),
                            "keeping source files because some paired/sidecar copies were not verified"
                        );
                    }

                    let source_deleted = job.delete_after_download
                        && all_verified
                        && delete_source_files(media_file, &primary_sources, &copied_or_matched);

                    Ok(Success {
                        source,
                        destination: dest,
                        checksum,
                        media_type: media_file.media_type,
                        source_deleted,
                    })
                }
                Err(error) => Err(Failure {
                    source,
                    destination: dest,
                    error: error.into(),
                }),
            }
        })
        .collect()
}
