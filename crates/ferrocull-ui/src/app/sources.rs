use std::path::{Path, PathBuf};

use ferrocull_core::{
    media::{CaptureTime, Item, SortKey},
    xmp::Metadata,
};
use ferrocull_devices::{
    MountOptions, ScanError, ScannedFile, Source, StorageDevice, UnmountOptions, mount,
    scan_directory, scan_storage, unmount,
};
use iced::Task;

use super::{Ferrocull, ThumbnailProgress, pick_folder, spawn_thumbnail_sipper, toggle_set};
use crate::messages::{Message, sources};

pub(super) fn update(state: &mut Ferrocull, msg: sources::Message) -> Task<Message> {
    match msg {
        sources::Message::SourceToggled(path) => {
            let newly_selected = !state.selected_sources.contains(&path);
            let scan_path = path.clone();
            toggle_set(&mut state.selected_sources, path);
            let outcome = state.rebuild_sorted_view();
            state.report_focus_loss(outcome);
            if newly_selected {
                return state.scan_source_directory(scan_path);
            }
        }
        sources::Message::MountStorage(device_path) => {
            let Some(device) = find_storage_device(&state.sources, &device_path) else {
                return Task::none();
            };

            #[cfg(target_os = "linux")]
            return Task::perform(
                async move {
                    mount(&device, &MountOptions::default())
                        .await
                        .map_err(|e| e.to_string())
                },
                move |r| Message::MountResult(device_path, r),
            );

            #[cfg(not(target_os = "linux"))]
            return Task::perform(
                tokio::task::spawn_blocking(move || {
                    mount(&device, &MountOptions::default()).map_err(|e| e.to_string())
                }),
                move |r| {
                    Message::MountResult(
                        device_path,
                        r.unwrap_or_else(|e| {
                            tracing::error!("mount task panicked: {e}");
                            Err(format!("task panicked: {e}"))
                        }),
                    )
                },
            );
        }
        sources::Message::UnmountStorage(device_path) => {
            let Some(device) = find_storage_device(&state.sources, &device_path) else {
                return Task::none();
            };

            #[cfg(target_os = "linux")]
            return Task::perform(
                async move {
                    unmount(&device, &UnmountOptions::default())
                        .await
                        .map_err(|e| e.to_string())
                },
                move |r| Message::UnmountResult(device_path, r),
            );

            #[cfg(not(target_os = "linux"))]
            return Task::perform(
                tokio::task::spawn_blocking(move || {
                    unmount(&device, &UnmountOptions::default()).map_err(|e| e.to_string())
                }),
                move |r| {
                    Message::UnmountResult(
                        device_path,
                        r.unwrap_or_else(|e| {
                            tracing::error!("unmount task panicked: {e}");
                            Err(format!("task panicked: {e}"))
                        }),
                    )
                },
            );
        }
        sources::Message::AddDirectoryClicked => {
            return pick_folder(|opt| {
                Message::Sources(sources::Message::SourceDirectoryPicked(opt))
            });
        }
        sources::Message::RefreshSources => {
            return scan_storage_task();
        }
        sources::Message::SourceDirectoryPicked(None) => {}
        sources::Message::SourceDirectoryPicked(Some(p)) => {
            return state.handle_source_directory_picked(p);
        }
    }
    Task::none()
}

fn find_storage_device(sources: &[Source], device_path: &Path) -> Option<StorageDevice> {
    sources.iter().find_map(|s| match s {
        Source::Storage(d) if d.device_path == device_path => Some(d.clone()),
        _ => None,
    })
}

#[cfg(target_os = "linux")]
pub(super) fn scan_storage_task() -> Task<Message> {
    Task::perform(scan_storage(), Message::SourcesRefreshed)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn scan_storage_task() -> Task<Message> {
    Task::perform(tokio::task::spawn_blocking(scan_storage), |r| {
        Message::SourcesRefreshed(r.unwrap_or_else(|e| {
            tracing::error!("source scan task panicked: {e}");
            Err(ScanError::Backend(format!("scan task panicked: {e}")))
        }))
    })
}

impl Ferrocull {
    fn scan_source_directory(&mut self, directory: PathBuf) -> Task<Message> {
        self.scan_jobs_in_flight += 1;
        self.scanning = true;
        Task::perform(
            tokio::task::spawn_blocking(move || scan_directory(&directory)),
            |r| {
                Message::ScanComplete(r.unwrap_or_else(|e| {
                    tracing::error!("scan task panicked: {e}");
                    Vec::new()
                }))
            },
        )
    }

    fn handle_source_directory_picked(&mut self, folder_path: PathBuf) -> Task<Message> {
        // Validate at the boundary — once we add to `sources`/`selected_sources`,
        // downstream code (`scan_source_directory`) trusts the path is a directory.
        if !folder_path.is_dir() {
            self.status_message = Some(format!("not a directory: {}", folder_path.display()));
            return Task::none();
        }

        let already_exists = self.sources.iter().any(|s| match s {
            Source::Storage(d) => d.mount_point.as_ref().is_some_and(|mp| *mp == folder_path),
            Source::Directory(p) => *p == folder_path,
            Source::Camera(_) => false,
        });

        if already_exists {
            if self.selected_sources.insert(folder_path.clone()) {
                let outcome = self.rebuild_sorted_view();
                self.report_focus_loss(outcome);
                return self.scan_source_directory(folder_path);
            }
            return Task::none();
        }

        self.sources.push(Source::Directory(folder_path.clone()));
        self.selected_sources.insert(folder_path.clone());
        let outcome = self.rebuild_sorted_view();
        self.report_focus_loss(outcome);
        self.scan_source_directory(folder_path)
    }

    pub(super) fn handle_sources_refreshed(
        &mut self,
        result: Result<Vec<StorageDevice>, ScanError>,
    ) {
        let storage_devices = match result {
            Ok(devices) => devices,
            Err(e) => {
                self.status_message = Some(format!("source scan failed: {e}"));
                return;
            }
        };

        let user_dirs: Vec<Source> = std::mem::take(&mut self.sources)
            .into_iter()
            .filter(|s| matches!(s, Source::Directory(_)))
            .collect();
        self.sources = storage_devices
            .into_iter()
            .map(Source::Storage)
            .chain(user_dirs)
            .collect();

        let before = self.selected_sources.len();
        self.selected_sources
            .retain(|p| self.sources.iter().any(|s| s.path() == p));
        if self.selected_sources.len() != before {
            let outcome = self.rebuild_sorted_view();
            self.report_focus_loss(outcome);
        }
    }

    pub(super) fn handle_mount_result(
        &mut self,
        device_path: &Path,
        result: Result<PathBuf, String>,
    ) {
        match result {
            Ok(mount_point) => {
                self.status_message = None;
                for source in &mut self.sources {
                    if let Source::Storage(s) = source
                        && s.device_path == device_path
                    {
                        s.mount_point = Some(mount_point);
                        break;
                    }
                }
            }
            Err(e) => self.status_message = Some(e),
        }
    }

    pub(super) fn handle_unmount_result(&mut self, device_path: &Path, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.status_message = None;
                for source in &mut self.sources {
                    if let Source::Storage(s) = source
                        && s.device_path == device_path
                    {
                        if let Some(mp) = s.mount_point.take() {
                            self.selected_sources.remove(&mp);
                        }
                        break;
                    }
                }
                let outcome = self.rebuild_sorted_view();
                self.report_focus_loss(outcome);
            }
            Err(e) => self.status_message = Some(e),
        }
    }

    /// `Item`s are created when `ExifLoaded` arrives (not here).
    pub(super) fn handle_scan_complete(
        &mut self,
        scanned_files: Vec<ScannedFile>,
    ) -> Task<Message> {
        self.scan_jobs_in_flight = self.scan_jobs_in_flight.saturating_sub(1);
        self.scanning = self.scan_jobs_in_flight > 0;

        if scanned_files.is_empty() {
            return Task::none();
        }

        if let Some(ref mut progress) = self.thumbnail_progress {
            progress.total += scanned_files.len();
        } else {
            self.thumbnail_progress = Some(ThumbnailProgress {
                total: scanned_files.len(),
                completed: 0,
            });
        }

        self.thumbnail_jobs_in_flight += 1;

        spawn_thumbnail_sipper(scanned_files)
    }

    /// Handle EXIF loaded: create `Item` with `capture_time` already set.
    /// O(log n) insertion via `BTreeMap` `sorted_view` if item passes current filters.
    pub(super) fn handle_exif_loaded(
        &mut self,
        scanned: ScannedFile,
        capture_time: CaptureTime,
        xmp_metadata: Option<&Metadata>,
    ) {
        let jpeg_pair = scanned
            .paired
            .iter()
            .find(|p| {
                let ext = p
                    .extension()
                    .expect("paired file has extension")
                    .to_str()
                    .expect("paired file extension is ASCII");
                ext.eq_ignore_ascii_case("jpg") || ext.eq_ignore_ascii_case("jpeg")
            })
            .cloned();
        let path = scanned.path;
        if self.item_index.contains_key(&path) {
            return;
        }

        let source_id = path.canonicalize().map_or_else(
            |_| path.to_string_lossy().into_owned(),
            |p| p.to_string_lossy().into_owned(),
        );

        let is_downloaded = self
            .db
            .is_downloaded(&source_id)
            .expect("is_downloaded query failed");

        let (rating, color_label) = self
            .db
            .rating_and_color(&source_id)
            .expect("rating_and_color query failed")
            .unwrap_or_else(|| xmp_metadata.map_or((0, None), |x| (x.rating, x.color_label)));

        if let Some(ref jpeg_path) = jpeg_pair {
            self.hidden_jpeg_paths.insert(jpeg_path.clone());
        }

        let item = Item {
            path: path.clone(),
            source_id,
            media_type: scanned.media_type,
            capture_time,
            is_downloaded,
            jpeg_pair,
            paired: scanned.paired,
            sidecars: scanned.sidecars,
            xmp_sidecar: scanned.xmp_sidecar,
            rating,
            color_label,
        };

        let idx = self.items.len();
        self.items.push(item);
        self.item_index.insert(path, idx);
        self.item_version += 1;

        if self.passes_filters(&self.items[idx]) {
            let key = SortKey::from_item(&self.items[idx], self.sort_order);
            self.sorted_view.insert(key.clone(), idx);
            self.sort_key_by_idx.insert(idx, key);
        }
    }
}
