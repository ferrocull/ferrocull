use std::{
    collections::HashSet,
    fs, io,
    path::{Component, Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
};

use crate::{
    FileCategory, MediaFile, READ_CONCURRENCY,
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
    #[error("failed to write XMP sidecar at '{}'", dest.display())]
    Xmp {
        dest: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// An ingest job containing source files, destination directories, and backup
/// destinations.
#[derive(Debug, Clone)]
pub struct Job {
    pub files: Vec<MediaFile>,
    pub dest_base: PathBuf,
    pub videos_dest: PathBuf,
    pub backup_destinations: Vec<PathBuf>,
    pub delete_after_ingest: bool,
}

/// Aggregate progress counters bumped by the ingest workers. The caller polls
/// them while [`execute_ingest`] runs on another thread.
#[derive(Debug, Default)]
pub struct Tracker {
    pub files_completed: AtomicUsize,
    /// Bytes of primary media copied. Paired files, sidecars, and XMP writes
    /// advance only `files_completed`.
    pub bytes_copied: AtomicU64,
}

/// Result of copying a single file.
pub type FileResult = Result<Success, Failure>;

#[derive(Debug)]
pub struct Success {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub checksum: String,
    pub source_deleted: bool,
    /// Backup destinations (including their extras and XMP) that failed. The
    /// primary copy is intact; the source was kept if deletion was requested.
    pub backup_failures: Vec<(PathBuf, Error)>,
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
/// the underlying I/O failure so the caller can fail the file's ingest — a failed write
/// must block source deletion.
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
            "failed to delete source after ingest"
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

/// Split backup failures into real failures and destinations that already
/// hold the source's contents: an existing, matching backup is a completed
/// copy from an earlier run, not a failure. A compare error keeps the
/// original failure.
fn split_matched_backups(
    src: &Path,
    failures: Vec<(PathBuf, copy::Error)>,
) -> (Vec<(PathBuf, copy::Error)>, Vec<PathBuf>) {
    let mut real = Vec::new();
    let mut matched = Vec::new();
    for (path, error) in failures {
        if matches!(error, copy::Error::DestinationExists { .. })
            && same_contents(src, &path).is_ok_and(|same| same)
        {
            matched.push(path);
        } else {
            real.push((path, error));
        }
    }
    (real, matched)
}

/// Accept an existing destination as the primary copy when its contents
/// match the source, returning the shared checksum. This lets a re-run
/// repair a partially failed ingest; a mismatch is a genuine collision.
fn match_existing_primary(src: &Path, dest: &Path) -> Result<String, copy::Error> {
    let map_io = |source: io::Error| copy::Error::Io {
        src: src.to_path_buf(),
        dest: dest.to_path_buf(),
        source,
    };
    let source_hash = hash_file(src).map_err(map_io)?;
    if source_hash == hash_file(dest).map_err(map_io)? {
        Ok(hex::encode(source_hash))
    } else {
        Err(copy::Error::DestinationExists {
            path: dest.to_path_buf(),
        })
    }
}

/// How [`copy_or_match`] satisfied the destination.
enum CopyOutcome {
    Copied,
    /// The destination already existed with contents matching the source.
    Matched,
}

/// Copy `src` to `dest`, accepting an existing destination whose contents
/// already match the source.
fn copy_or_match(src: &Path, dest: &Path) -> Result<CopyOutcome, copy::Error> {
    match copy_with_checksum(src, dest, &[], |_| {}) {
        Ok(_) => Ok(CopyOutcome::Copied),
        Err(copy::Error::DestinationExists { path }) => match same_contents(src, dest) {
            Ok(true) => Ok(CopyOutcome::Matched),
            Ok(false) => Err(copy::Error::DestinationExists { path }),
            Err(e) => Err(copy::Error::Io {
                src: src.to_path_buf(),
                dest: dest.to_path_buf(),
                source: e,
            }),
        },
        Err(e) => Err(e),
    }
}

/// Outcome of copying a file's paired and sidecar extras.
struct Extras {
    /// Source paths safe to delete: copied successfully, or existed at the
    /// primary destination with identical contents.
    copied_or_matched: HashSet<PathBuf>,
    /// Every extra reached the primary destination (copied or verified).
    all_verified: bool,
    backup_failures: Vec<(PathBuf, Error)>,
}

/// Copy paired and sidecar files alongside the primary destination, teeing
/// each to the backup destinations.
fn copy_extras(
    media_file: &MediaFile,
    relative: &Path,
    extras_dir: &Path,
    backups: &[PathBuf],
    primary_sources: &HashSet<PathBuf>,
) -> Extras {
    let mut extras = Extras {
        copied_or_matched: HashSet::new(),
        all_verified: true,
        backup_failures: Vec::new(),
    };

    for extra in media_file.paired_files.iter().chain(&media_file.sidecars) {
        if primary_sources.contains(extra) {
            continue;
        }

        let name = extra.file_name().expect("path has no filename");
        let extra_dest = extras_dir.join(name);
        let backup_dests: Vec<PathBuf> = backups
            .iter()
            .map(|backup| backup.join(relative.with_file_name(name)))
            .collect();

        match copy_with_checksum(extra, &extra_dest, &backup_dests, |_| {}) {
            Ok(tee) => {
                extras.copied_or_matched.insert(extra.clone());
                // Extras are never rolled back, so the matched destinations
                // need no tracking here.
                let (failures, _) = split_matched_backups(extra, tee.backup_failures);
                extras
                    .backup_failures
                    .extend(failures.into_iter().map(|(p, e)| (p, e.into())));
                continue;
            }
            Err(copy::Error::DestinationExists { .. }) => match same_contents(extra, &extra_dest) {
                Ok(true) => {
                    extras.copied_or_matched.insert(extra.clone());
                }
                Ok(false) => {
                    extras.all_verified = false;
                    tracing::warn!(
                        src = %extra.display(),
                        dest = %extra_dest.display(),
                        "destination paired/sidecar exists with different contents"
                    );
                }
                Err(e) => {
                    extras.all_verified = false;
                    tracing::warn!(
                        src = %extra.display(),
                        dest = %extra_dest.display(),
                        error = %e,
                        "failed to compare paired/sidecar file with existing destination"
                    );
                }
            },
            Err(e) => {
                extras.all_verified = false;
                tracing::warn!(
                    src = %extra.display(),
                    dest = %extra_dest.display(),
                    error = %e,
                    "failed to copy paired/sidecar file"
                );
            }
        }

        // The tee wrote no backups, so bring each destination up to date on
        // its own: a backup the extra never reached must be recorded as a
        // failure, not silently skipped.
        for backup_dest in backup_dests {
            if let Err(e) = copy_or_match(extra, &backup_dest) {
                extras.backup_failures.push((backup_dest, e.into()));
            }
        }
    }

    extras
}

/// Resolve the destination path relative to the base directory: the rendered
/// pattern when present (rejecting absolute or parent-escaping components),
/// otherwise the source filename.
fn resolve_relative_path(rendered: Option<&str>, source: &Path) -> Result<PathBuf, Error> {
    let filename = source.file_name().expect("path has no filename");
    let Some(rendered) = rendered else {
        return Ok(PathBuf::from(filename));
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

    Ok(relative)
}

/// Ingest one file: tee the primary copy to all backup destinations, write
/// XMP sidecars, copy extras, and (when requested and fully verified,
/// backups included) delete the source.
fn ingest_file(
    media_file: &MediaFile,
    job: &Job,
    primary_sources: &HashSet<PathBuf>,
    tracker: &Tracker,
) -> FileResult {
    let source = media_file.path.clone();

    let base = if media_file.media_type == FileCategory::Video {
        &job.videos_dest
    } else {
        &job.dest_base
    };
    let relative = match resolve_relative_path(media_file.rendered_dest.as_deref(), &source) {
        Ok(relative) => relative,
        Err(error) => {
            return Err(Failure {
                source,
                destination: PathBuf::new(),
                error,
            });
        }
    };
    let dest = base.join(&relative);
    let backup_dests: Vec<PathBuf> = job
        .backup_destinations
        .iter()
        .map(|backup| backup.join(&relative))
        .collect();

    // Destinations that already held the source's contents before this run.
    // A rollback must leave them in place: they are verified copies from an
    // earlier run, not work to undo.
    let mut matched: HashSet<PathBuf> = HashSet::new();
    let (checksum, tee_failures) =
        match copy_with_checksum(&media_file.path, &dest, &backup_dests, |bytes| {
            tracker.bytes_copied.fetch_add(bytes, Ordering::Relaxed);
        }) {
            Ok(tee) => {
                let (failures, matched_backups) =
                    split_matched_backups(&media_file.path, tee.backup_failures);
                matched.extend(matched_backups);
                (tee.checksum, failures)
            }
            // A matching primary is a completed copy from an earlier, partially
            // failed run; each backup is then brought up to date on its own.
            Err(copy::Error::DestinationExists { .. }) => {
                match match_existing_primary(&media_file.path, &dest) {
                    Ok(checksum) => {
                        matched.insert(dest.clone());
                        let mut failures = Vec::new();
                        for backup_dest in &backup_dests {
                            match copy_or_match(&media_file.path, backup_dest) {
                                Ok(CopyOutcome::Copied) => {}
                                Ok(CopyOutcome::Matched) => {
                                    matched.insert(backup_dest.clone());
                                }
                                Err(e) => failures.push((backup_dest.clone(), e)),
                            }
                        }
                        (checksum, failures)
                    }
                    Err(error) => {
                        return Err(Failure {
                            source,
                            destination: dest,
                            error: error.into(),
                        });
                    }
                }
            }
            Err(error) => {
                return Err(Failure {
                    source,
                    destination: dest,
                    error: error.into(),
                });
            }
        };
    let mut backup_failures: Vec<(PathBuf, Error)> = tee_failures
        .into_iter()
        .map(|(path, e)| (path, e.into()))
        .collect();

    if let Err(xmp_error) = write_xmp_sidecar(media_file, &dest) {
        // Roll back the copies written during this run: a leftover fresh
        // copy would block a retry with `DestinationExists`, while a matched
        // copy predates this run and stays valid for the next retry.
        if !matched.contains(&dest) {
            copy::remove_partial(&dest);
        }
        for backup_dest in &backup_dests {
            if !matched.contains(backup_dest)
                && !backup_failures.iter().any(|(path, _)| path == backup_dest)
            {
                copy::remove_partial(backup_dest);
            }
        }
        return Err(Failure {
            source,
            destination: dest.clone(),
            error: Error::Xmp {
                dest,
                source: xmp_error,
            },
        });
    }
    for backup_dest in &backup_dests {
        if backup_failures.iter().any(|(path, _)| path == backup_dest) {
            continue;
        }
        if let Err(e) = write_xmp_sidecar(media_file, backup_dest) {
            backup_failures.push((
                backup_dest.clone(),
                Error::Xmp {
                    dest: backup_dest.clone(),
                    source: e,
                },
            ));
        }
    }

    let extras = copy_extras(
        media_file,
        &relative,
        dest.parent().expect("dest is a file within a directory"),
        &job.backup_destinations,
        primary_sources,
    );
    backup_failures.extend(extras.backup_failures);

    for (path, error) in &backup_failures {
        tracing::warn!(
            src = %source.display(),
            backup = %path.display(),
            error = %error,
            "backup copy failed"
        );
    }

    if job.delete_after_ingest && !extras.all_verified {
        tracing::warn!(
            path = %source.display(),
            "keeping source files because some paired/sidecar copies were not verified"
        );
    }
    if job.delete_after_ingest && !backup_failures.is_empty() {
        tracing::warn!(
            path = %source.display(),
            "keeping source files because some backup copies failed"
        );
    }

    let source_deleted = job.delete_after_ingest
        && extras.all_verified
        && backup_failures.is_empty()
        && delete_source_files(media_file, primary_sources, &extras.copied_or_matched);

    Ok(Success {
        source,
        destination: dest,
        checksum,
        source_deleted,
        backup_failures,
    })
}

/// Executes an ingest job, copying all files to the destination.
///
/// Videos are routed to `videos_dest`, everything else to `dest_base`. Each
/// file is streamed once, teeing the copy to every backup destination.
/// A small worker pool processes files concurrently, bumping `tracker` as
/// bytes and files complete.
///
/// If `delete_after_ingest` is true, deletes source files after the primary
/// and every backup copy verified. Deletion failures are logged but do not
/// affect the overall success status.
///
/// Returns a result for each file, in job order.
pub fn execute_ingest(job: &Job, tracker: &Tracker) -> Vec<FileResult> {
    let primary_sources: HashSet<PathBuf> = job.files.iter().map(|f| f.path.clone()).collect();
    let queue = Mutex::new(job.files.iter().enumerate());
    let (tx, rx) = mpsc::channel::<(usize, FileResult)>();

    std::thread::scope(|scope| {
        for _ in 0..READ_CONCURRENCY.min(job.files.len()) {
            let tx = tx.clone();
            let queue = &queue;
            let primary_sources = &primary_sources;
            scope.spawn(move || {
                // Keep the lock inside the closure: a `queue.lock()...next()`
                // scrutinee holds the guard across the whole loop body, which
                // serializes the workers.
                let next_file = || queue.lock().expect("ingest queue lock poisoned").next();
                while let Some((index, media_file)) = next_file() {
                    let result = ingest_file(media_file, job, primary_sources, tracker);
                    tracker.files_completed.fetch_add(1, Ordering::Relaxed);
                    tx.send((index, result)).expect("result receiver dropped");
                }
            });
        }
    });
    drop(tx);

    let mut results: Vec<Option<FileResult>> = job.files.iter().map(|_| None).collect();
    for (index, result) in rx {
        results[index] = Some(result);
    }
    results
        .into_iter()
        .map(|slot| slot.expect("worker sent no result for file index"))
        .collect()
}
