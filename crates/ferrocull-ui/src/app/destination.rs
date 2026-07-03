use std::{
    cell::Cell,
    collections::HashSet,
    path::{Path, PathBuf},
};

use ferrocull_core::{
    FileCategory, MediaFile, Pattern, RenderContext,
    backup::{self, execute_backup},
    download::{self, FileResult, execute_download},
    hooks::{Context, Spec, run_hooks},
    media::Item,
};
use iced::Task;
use sipper::sipper;

use super::{Ferrocull, pick_folder};
use crate::messages::{DownloadResult, Message, SuccessInfo, destination};

pub(super) fn update(state: &mut Ferrocull, msg: destination::Message) -> Task<Message> {
    match msg {
        destination::Message::PhotosDestChanged(path) => state.photos_dest = path,
        destination::Message::VideosDestChanged(path) => state.videos_dest = path,
        destination::Message::PhotoPatternChanged(pattern) => state.photo_pattern = pattern,
        destination::Message::VideoPatternChanged(pattern) => state.video_pattern = pattern,
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
        }
        destination::Message::VideosDestPicked(Some(path)) => {
            state.videos_dest = path.display().to_string();
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
            if let Err(e) = state.job_code_history.save(&state.jobcode_path) {
                tracing::warn!(error = %e, "failed to save jobcode history");
            }
            state.job_code = code;
        }
        destination::Message::AddBackupClicked => {
            return pick_folder(|opt| {
                Message::Destination(destination::Message::BackupDestPicked(opt))
            });
        }
        destination::Message::RemoveBackup(idx) => {
            state.backup_destinations.remove(idx);
        }
        destination::Message::BackupDestPicked(Some(path)) => state.handle_backup_picked(path),
        destination::Message::DeleteAfterDownloadToggled => {
            state.delete_after_download = !state.delete_after_download;
        }
        destination::Message::StartDownload => return state.handle_start_download(),
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

fn results_to_download_result(results: Vec<FileResult>) -> DownloadResult {
    let failure_count = results.iter().filter(|r| r.is_err()).count();
    let successes = results
        .into_iter()
        .filter_map(Result::ok)
        .map(|s| SuccessInfo {
            source: s.source,
            destination: s.destination,
            checksum: s.checksum,
        })
        .collect();
    DownloadResult {
        successes,
        failure_count,
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
    /// Handle download start: create sipper for progress tracking.
    fn handle_start_download(&mut self) -> Task<Message> {
        let photo_pattern = match Pattern::parse(&self.photo_pattern) {
            Ok(pattern) => pattern,
            Err(e) => {
                self.status_message = Some(format!("Invalid photo pattern: {e}"));
                return Task::none();
            }
        };
        let video_pattern = match Pattern::parse(&self.video_pattern) {
            Ok(pattern) => pattern,
            Err(e) => {
                self.status_message = Some(format!("Invalid video pattern: {e}"));
                return Task::none();
            }
        };
        let sequence = Cell::new(0u32);
        let mut seen_paths: HashSet<PathBuf> = HashSet::new();

        let selected: Vec<MediaFile> = self
            .selected
            .iter()
            .map(|&idx| &self.items[idx])
            .filter(|item| seen_paths.insert(item.path.clone()))
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
        let job = download::Job {
            files: selected,
            dest_base: photos_dest.clone(),
            videos_dest: videos_dest.clone(),
            delete_after_download: self.delete_after_download,
        };

        self.last_download_failures = 0;
        self.download_progress = Some(super::DownloadProgress {
            total_files,
            files_completed: 0,
        });

        let backup_dests = self.backup_destinations.clone();
        let download_sipper = sipper(move |mut sender| async move {
            let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();

            let handle = tokio::task::spawn_blocking(move || {
                let results = execute_download(&job, |progress| {
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

            let results = handle.await.unwrap_or_else(|e| {
                tracing::error!("download task panicked: {e}");
                Vec::new()
            });
            results_to_download_result(results)
        });

        Task::sip(
            download_sipper,
            Message::DownloadProgressUpdate,
            Message::DownloadComplete,
        )
    }

    pub(super) fn handle_download_complete(&mut self, result: &DownloadResult) -> Task<Message> {
        self.download_progress = None;
        self.last_download_failures = result.failure_count;
        for success in &result.successes {
            let idx = *self
                .item_index
                .get(&success.source)
                .expect("downloaded path is in item_index");
            self.selected.remove(&idx);
            let item = &mut self.items[idx];
            item.is_downloaded = true;
            self.item_version += 1;
            let source_id = item.source_id.clone();

            self.db
                .record_download(&source_id, &success.checksum, &success.destination)
                .expect("record_download query failed");
        }
        if !result.successes.is_empty() {
            self.status_message = None;
            let outcome = self.rebuild_sorted_view();
            self.report_focus_loss(outcome);
        }

        if result.successes.is_empty() || self.hooks.is_empty() {
            return Task::none();
        }

        let ctx = Context {
            dest_dir: PathBuf::from(&self.photos_dest),
            files_downloaded: result
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
                        tracing::warn!(hook = i, error = %e, "post-download hook failed");
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
        }
    }
}
