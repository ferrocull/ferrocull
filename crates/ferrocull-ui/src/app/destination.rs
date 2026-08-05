use std::{
    cell::Cell,
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use ferrocull_core::{
    FileCategory, MediaFile, Pattern, RenderContext,
    hooks::{Context, Spec, run_hooks},
    ingest::{self, FileResult, execute_ingest},
    media::Item,
};
use iced::Task;
use sipper::sipper;

use super::{Ferrocull, Modal, pick_folder};
use crate::messages::{
    FailureInfo, IngestResult, IngestSnapshot, Message, SuccessInfo, destination,
};

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
            state.modal = if matches!(state.modal, Some(Modal::IngestFailures)) {
                None
            } else {
                Some(Modal::IngestFailures)
            };
        }
        destination::Message::RetryFailedIngest => {
            state.modal = None;
            return state.handle_retry_failed_ingest();
        }
    }
    Task::none()
}

fn results_to_ingest_result(results: Vec<FileResult>) -> IngestResult {
    let mut successes = Vec::new();
    let mut failures = Vec::new();
    for result in results {
        match result {
            // A file whose backup copies failed is not done: keeping it in
            // the failure list keeps it tagged, in the working set, and
            // reachable by "Retry failed", which repairs the missing backups
            // by matching the intact primary copy.
            Ok(s) if !s.backup_failures.is_empty() => failures.push(FailureInfo {
                source: s.source,
                error: s
                    .backup_failures
                    .iter()
                    .map(|(_, e)| format!("backup failed: {e}"))
                    .collect::<Vec<_>>()
                    .join("; "),
            }),
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
        camera_make: item.capture_settings.make.clone(),
        camera_model: item.capture_settings.model.clone(),
        sequence: {
            sequence.update(|s| s + 1);
            sequence.get()
        },
        iso: item.capture_settings.iso,
        aperture: item.capture_settings.aperture,
        shutter: item.capture_settings.exposure_time,
        focal_length: item.capture_settings.focal_length,
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
        let total_bytes: u64 = indices.iter().map(|&idx| self.media.item(idx).size).sum();
        let job = ingest::Job {
            files: selected,
            dest_base: PathBuf::from(&self.photos_dest),
            videos_dest: PathBuf::from(&self.videos_dest),
            backup_destinations: self.backup_destinations.clone(),
            delete_after_ingest: self.delete_after_ingest,
        };

        self.last_ingest_failures.clear();
        self.modal = None;
        self.ingest_progress = Some(super::IngestProgress {
            total_files,
            files_completed: 0,
            total_bytes,
            bytes_copied: 0,
        });

        let ingest_sipper = sipper(move |mut sender| async move {
            let tracker = Arc::new(ingest::Tracker::default());
            let worker_tracker = Arc::clone(&tracker);
            let mut handle =
                tokio::task::spawn_blocking(move || execute_ingest(&job, &worker_tracker));

            let snapshot = |counters: &ingest::Tracker| IngestSnapshot {
                files_completed: counters.files_completed.load(Ordering::Relaxed),
                bytes_copied: counters.bytes_copied.load(Ordering::Relaxed),
            };
            let mut ticks = tokio::time::interval(Duration::from_millis(100));
            let results = loop {
                tokio::select! {
                    joined = &mut handle => break joined.expect("ingest task panicked"),
                    _ = ticks.tick() => sender.send(snapshot(&tracker)).await,
                }
            };
            sender.send(snapshot(&tracker)).await;

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
        let ingested: Vec<usize> = result
            .successes
            .iter()
            .map(|success| {
                let idx = self
                    .media
                    .index_of(&success.source)
                    .expect("an ingested file's path must resolve to a media item");
                self.media
                    .mutate_item(idx, &self.config.params(), |item| item.is_ingested = true);
                self.metadata.record_ingest(
                    &self.media.item(idx).fingerprint(),
                    &success.checksum,
                    &success.destination,
                );
                idx
            })
            .collect();
        if !ingested.is_empty() {
            // A tag lives until its frame is ingested, so clearing it is durable
            // and keyed the same way the ingest itself was recorded. Frames
            // whose ingest failed keep their tag and stay in the working set.
            self.apply_tags(&ingested, false);
            self.status_message = None;
            // Recorded undo/redo entries reference pre-ingest tag state that no
            // longer holds — a stale undo would re-tag already-ingested files.
            self.undo_stack = crate::undo::Stack::default();
            // Reconcile focus: a now-ingested file may leave a "new only"
            // filter.
            self.reconcile_focus();
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

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use ferrocull_core::media::{CaptureSettings, CaptureTime};

    use super::{Cell, FileCategory, Item, PathBuf, Pattern, item_to_media_file};

    fn item(capture_settings: CaptureSettings) -> Item {
        let second = Utc
            .with_ymd_and_hms(2024, 5, 1, 10, 14, 22)
            .single()
            .expect("unambiguous test timestamp");
        Item {
            path: PathBuf::from("/cards/A/IMG_1234.CR3"),
            size: 0,
            media_type: FileCategory::Raw,
            capture_time: CaptureTime::new(second, 0),
            capture_settings,
            is_ingested: false,
            jpeg_pair: None,
            paired: Vec::new(),
            sidecars: Vec::new(),
            xmp_sidecar: None,
            rating: 0,
            color_label: None,
        }
    }

    fn rendered_dest(capture_settings: CaptureSettings) -> String {
        let pattern = Pattern::parse("{camera_make}/{camera_model}/{filename}.{ext}")
            .expect("test pattern parses");
        item_to_media_file(
            &item(capture_settings),
            &pattern,
            &pattern,
            &Cell::new(0),
            "",
        )
        .rendered_dest
        .expect("every rendered file carries a destination")
    }

    #[test]
    fn camera_tokens_resolve_from_the_scanned_item() {
        let dest = rendered_dest(CaptureSettings {
            make: Some(String::from("Canon")),
            model: Some(String::from("Canon EOS R5")),
            ..CaptureSettings::default()
        });
        assert_eq!(dest, "Canon/Canon EOS R5/IMG_1234.cr3");
    }

    #[test]
    fn a_file_without_camera_tags_still_renders_a_destination() {
        let dest = rendered_dest(CaptureSettings::default());
        assert_eq!(dest, "//IMG_1234.cr3", "both tokens render empty");
    }
}
