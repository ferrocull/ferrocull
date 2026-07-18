use std::{
    cell::Cell,
    path::{Path, PathBuf},
};

use ferrocull_core::{
    FileCategory, MediaFile, Pattern, RenderContext,
    backup::{self, execute_backup},
    hooks::{Context, Spec, run_hooks},
    ingest::{self, FileResult, execute_ingest},
    media::Item,
};
use iced::Task;
use sipper::sipper;

use super::{Ferrocull, pick_folder};
use crate::messages::{FailureInfo, IngestResult, Message, SuccessInfo, destination};

pub(super) fn update(state: &mut Ferrocull, msg: destination::Message) -> Task<Message> {
    match msg {
        destination::Message::PhotosDestChanged(path) => {
            state.photos_dest = path;
            state.persist_settings();
        }
        destination::Message::VideosDestChanged(path) => {
            state.videos_dest = path;
            state.persist_settings();
        }
        destination::Message::PhotoPatternChanged(pattern) => {
            state.photo_pattern = pattern;
            state.persist_settings();
        }
        destination::Message::VideoPatternChanged(pattern) => {
            state.video_pattern = pattern;
            state.persist_settings();
        }
        destination::Message::PatternSaveToggled(pattern) => {
            if let Some(pos) = state.saved_patterns.iter().position(|p| *p == pattern) {
                state.saved_patterns.remove(pos);
            } else {
                state.saved_patterns.insert(0, pattern);
            }
            state.persist_settings();
        }
        destination::Message::BrowsePhotosDest => {
            return pick_folder(|opt| {
                Message::Destination(destination::Message::PhotosDestPicked(opt))
            });
        }
        destination::Message::BrowseVideosDest => {
            return pick_folder(|opt| {
                Message::Destination(destination::Message::VideosDestPicked(opt))
            });
        }
        destination::Message::PhotosDestPicked(None)
        | destination::Message::VideosDestPicked(None)
        | destination::Message::BackupDestPicked(None) => {}
        destination::Message::PhotosDestPicked(Some(path)) => {
            state.photos_dest = path.display().to_string();
            state.persist_settings();
        }
        destination::Message::VideosDestPicked(Some(path)) => {
            state.videos_dest = path.display().to_string();
            state.persist_settings();
        }
        destination::Message::JobCodeChanged(code) => {
            if code.is_empty()
                || Path::new(&code)
                    .file_name()
                    .is_some_and(|f| f == code.as_str())
            {
                state.job_code = code;
            }
        }
        destination::Message::JobCodeSelected(code) => {
            state.job_code_history.add(&code);
            state
                .metadata
                .set_job_code_history(state.job_code_history.codes());
            state.job_code = code;
        }
        destination::Message::AddBackupClicked => {
            return pick_folder(|opt| {
                Message::Destination(destination::Message::BackupDestPicked(opt))
            });
        }
        destination::Message::RemoveBackup(idx) => {
            state.backup_destinations.remove(idx);
            state.persist_settings();
        }
        destination::Message::BackupDestPicked(Some(path)) => state.handle_backup_picked(path),
        destination::Message::DeleteAfterIngestToggled => {
            state.delete_after_ingest = !state.delete_after_ingest;
            state.persist_settings();
        }
        destination::Message::StartIngest => return state.handle_start_ingest(),
        destination::Message::ToggleIngestFailures => {
            state.ingest_failures_open = !state.ingest_failures_open;
        }
        destination::Message::RetryFailedIngest => {
            state.ingest_failures_open = false;
            return state.handle_retry_failed_ingest();
        }
    }
    Task::none()
}

fn run_backups(
    results: &[FileResult],
    backup_dests: &[PathBuf],
    photos_dest: &Path,
    videos_dest: &Path,
) {
    let destinations: Vec<backup::Destination> = backup_dests
        .iter()
        .map(|p| backup::Destination {
            path: p.clone(),
            photo_subpath: None,
            video_subpath: None,
        })
        .collect();

    for result in results {
        let Ok(success) = result else { continue };

        let relative_path = success
            .destination
            .strip_prefix(photos_dest)
            .or_else(|_| success.destination.strip_prefix(videos_dest))
            .expect("destination starts with photos or videos base")
            .to_path_buf();

        let job = backup::Job {
            source_file: success.destination.clone(),
            relative_path,
            media_type: Some(success.media_type),
            destinations: &destinations,
        };

        let backup_results = execute_backup(&job, |_| {});

        for backup_result in backup_results {
            match backup_result {
                Ok((dest_path, checksum)) => {
                    if checksum == success.checksum {
                        tracing::info!(backup = %dest_path.display(), "backup complete");
                    } else {
                        tracing::warn!(
                            source = %success.destination.display(),
                            backup = %dest_path.display(),
                            expected = %success.checksum,
                            actual = %checksum,
                            "backup checksum mismatch"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(%e, "backup failed");
                }
            }
        }
    }
}

fn results_to_ingest_result(results: Vec<FileResult>) -> IngestResult {
    let mut successes = Vec::new();
    let mut failures = Vec::new();
    for result in results {
        match result {
            Ok(s) => successes.push(SuccessInfo {
                source: s.source,
                destination: s.destination,
                checksum: s.checksum,
            }),
            Err(f) => failures.push(FailureInfo {
                source: f.source,
                error: f.error.to_string(),
            }),
        }
    }
    IngestResult {
        successes,
        failures,
    }
}

fn item_to_media_file(
    item: &Item,
    photo_pattern: &Pattern,
    video_pattern: &Pattern,
    sequence: &Cell<u32>,
    job_code: &str,
) -> MediaFile {
    let media_type = item.media_type;

    let pattern = if media_type == FileCategory::Video {
        video_pattern
    } else {
        photo_pattern
    };

    let stem = item
        .path
        .file_stem()
        .expect("scanned file has stem")
        .to_string_lossy()
        .into_owned();
    let ext = item
        .path
        .extension()
        .expect("scanned file has extension")
        .to_string_lossy()
        .into_owned();
    let ctx = RenderContext {
        datetime: item.capture_time.second,
        filename: stem,
        extension: ext,
        camera_make: None,
        camera_model: None,
        sequence: {
            sequence.update(|s| s + 1);
            sequence.get()
        },
        iso: None,
        aperture: None,
        shutter: None,
        job_code: (!job_code.is_empty()).then(|| job_code.to_owned()),
    };
    let rendered_dest = Some(pattern.render(&ctx));

    MediaFile {
        path: item.path.clone(),
        datetime: item.capture_time.second,
        media_type,
        paired_files: item.paired.clone(),
        sidecars: item.sidecars.clone(),
        xmp_sidecar: item.xmp_sidecar.clone(),
        rating: item.rating,
        color_label: item.color_label,
        rendered_dest,
    }
}

impl Ferrocull {
    /// Handle ingest start: ingest the current selection.
    fn handle_start_ingest(&mut self) -> Task<Message> {
        let indices: Vec<usize> = self.selected.iter().copied().collect();
        self.start_ingest_for(&indices)
    }

    /// Re-run ingest for exactly the files that failed last time.
    fn handle_retry_failed_ingest(&mut self) -> Task<Message> {
        let total = self.last_ingest_failures.len();
        // filter_map, not map+expect: a rescan between failure and retry can
        // legitimately drop failed sources from the loaded set.
        let indices: Vec<usize> = self
            .last_ingest_failures
            .iter()
            .filter_map(|failure| self.media.index_of(&failure.source))
            .collect();
        if indices.is_empty() {
            self.error(format!(
                "{total} failed file(s) are no longer loaded — rescan the source and ingest again"
            ));
            return Task::none();
        }
        if indices.len() < total {
            self.echo(format!(
                "Retrying {} of {total} failed files — the rest are no longer loaded",
                indices.len()
            ));
        }
        self.start_ingest_for(&indices)
    }

    /// Ingest the given items: create sipper for progress tracking.
    fn start_ingest_for(&mut self, indices: &[usize]) -> Task<Message> {
        let photo_pattern = match Pattern::parse(&self.photo_pattern) {
            Ok(pattern) => pattern,
            Err(e) => {
                self.error(format!("Invalid photo pattern: {e}"));
                return Task::none();
            }
        };
        let video_pattern = match Pattern::parse(&self.video_pattern) {
            Ok(pattern) => pattern,
            Err(e) => {
                self.error(format!("Invalid video pattern: {e}"));
                return Task::none();
            }
        };
        let sequence = Cell::new(0u32);

        let selected: Vec<MediaFile> = indices
            .iter()
            .map(|&idx| self.media.item(idx))
            .map(|item| {
                item_to_media_file(
                    item,
                    &photo_pattern,
                    &video_pattern,
                    &sequence,
                    &self.job_code,
                )
            })
            .collect();

        if selected.is_empty() {
            return Task::none();
        }

        self.status_message = None;
        let total_files = selected.len();
        let photos_dest = PathBuf::from(&self.photos_dest);
        let videos_dest = PathBuf::from(&self.videos_dest);
        let job = ingest::Job {
            files: selected,
            dest_base: photos_dest.clone(),
            videos_dest: videos_dest.clone(),
            delete_after_ingest: self.delete_after_ingest,
        };

        self.last_ingest_failures.clear();
        self.ingest_failures_open = false;
        self.ingest_progress = Some(super::IngestProgress {
            total_files,
            files_completed: 0,
        });

        let backup_dests = self.backup_destinations.clone();
        let ingest_sipper = sipper(move |mut sender| async move {
            let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();

            let handle = tokio::task::spawn_blocking(move || {
                let results = execute_ingest(&job, |progress| {
                    if progress.file_bytes_copied == progress.file_total_bytes {
                        let _ = progress_tx.send(progress.current_file_index + 1);
                    }
                });

                if !backup_dests.is_empty() {
                    run_backups(&results, &backup_dests, &photos_dest, &videos_dest);
                }

                results
            });

            while let Some(progress) = progress_rx.recv().await {
                sender.send(progress).await;
            }

            let results = handle.await.expect("ingest task panicked");
            results_to_ingest_result(results)
        });

        Task::sip(
            ingest_sipper,
            Message::IngestProgressUpdate,
            Message::IngestComplete,
        )
    }

    pub(super) fn handle_ingest_complete(&mut self, result: &IngestResult) -> Task<Message> {
        self.ingest_progress = None;
        self.last_ingest_failures.clone_from(&result.failures);
        for success in &result.successes {
            let idx = self
                .media
                .index_of(&success.source)
                .expect("an ingested file's path must resolve to a media item");
            self.selected.remove(&idx);
            self.media
                .mutate_item(idx, &self.config.params(), |item| item.is_ingested = true);
            let source_id = self.media.item(idx).source_id.clone();

            self.metadata
                .record_ingest(&source_id, &success.checksum, &success.destination);
        }
        if !result.successes.is_empty() {
            self.status_message = None;
            // Recorded undo/redo entries reference pre-ingest tag state that no
            // longer holds — a stale undo would re-tag already-ingested files.
            self.undo_stack = crate::undo::Stack::default();
            // Reconcile selection/focus: a now-ingested file may leave a
            // "new only" filter.
            self.reconcile_selection();
        }

        if result.successes.is_empty() || self.hooks.is_empty() {
            return Task::none();
        }

        let ctx = Context {
            dest_dir: PathBuf::from(&self.photos_dest),
            files_ingested: result
                .successes
                .iter()
                .map(|s| s.destination.clone())
                .collect(),
        };
        let enabled_hooks: Vec<_> = self.hooks.iter().filter(|h| h.enabled).cloned().collect();

        Task::perform(
            tokio::task::spawn_blocking(move || {
                let hook_specs: Vec<_> = enabled_hooks
                    .iter()
                    .map(|h| Spec {
                        name: &h.name,
                        command: &h.command,
                    })
                    .collect();
                for (i, hook_result) in run_hooks(&hook_specs, &ctx).into_iter().enumerate() {
                    if let Err(e) = hook_result {
                        tracing::warn!(hook = i, error = %e, "post-ingest hook failed");
                    }
                }
            }),
            |r| {
                if let Err(e) = r {
                    tracing::error!("hooks task panicked: {e}");
                }
                Message::HooksComplete
            },
        )
    }

    fn handle_backup_picked(&mut self, path: PathBuf) {
        if !self.backup_destinations.contains(&path) {
            self.backup_destinations.push(path);
            self.persist_settings();
        }
    }
}
