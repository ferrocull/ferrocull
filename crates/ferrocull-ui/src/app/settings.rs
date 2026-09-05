//! Settings popup: opening/closing, live theme changes, and the two
//! destructive changes (thumbnail resolution, cache location) that stage a
//! pending value and commit through a side-effecting `Task` on confirmation.

use std::sync::Arc;

use ferrocull_core::cache::{self, PreviewCache, ThumbnailCache};
use ferrocull_devices::ScannedFile;
use iced::Task;

use super::{
    Ferrocull, Modal, SettingsState, ThumbnailProgress, pick_folder, spawn_thumbnail_sipper,
};
use crate::messages::{Message, settings};

pub(super) fn update(state: &mut Ferrocull, msg: settings::Message) -> Task<Message> {
    match msg {
        settings::Message::Open => {
            state.modal = Some(Modal::Settings(SettingsState::new()));
        }
        settings::Message::Close => {
            state.modal = None;
        }
        settings::Message::SelectCategory(category) => {
            if let Some(s) = state.settings_mut() {
                s.category = category;
            }
        }
        settings::Message::ThemeChanged(preference) => {
            // Theme applies live: `set_preference` reseeds the render-time cache,
            // so the next frame recolors.
            state.theme_preference = preference;
            crate::theme::set_preference(preference);
            state.persist_settings();
        }
        settings::Message::ThumbnailResolutionSelected(resolution) => {
            let current = state.thumbnail_resolution;
            if let Some(s) = state.settings_mut() {
                // Re-selecting the committed resolution clears the staged change.
                s.pending_thumbnail_resolution = (resolution != current).then_some(resolution);
            }
        }
        settings::Message::ConfirmThumbnailResolution => {
            return confirm_thumbnail_resolution(state);
        }
        settings::Message::CancelThumbnailResolution => {
            if let Some(s) = state.settings_mut() {
                s.pending_thumbnail_resolution = None;
            }
        }
        settings::Message::BrowseCacheDir => {
            return pick_folder(|opt| Message::Settings(settings::Message::CacheDirChosen(opt)));
        }
        settings::Message::CacheDirChosen(None) => {}
        settings::Message::CacheDirChosen(Some(path)) => {
            let current = state.cache_root();
            if let Some(s) = state.settings_mut() {
                // Picking the current location is a no-op.
                s.pending_cache_dir = (Some(&path) != current.as_ref()).then_some(path);
            }
        }
        settings::Message::ConfirmCacheDir => {
            return confirm_cache_dir(state);
        }
        settings::Message::CancelCacheDir => {
            if let Some(s) = state.settings_mut() {
                s.pending_cache_dir = None;
            }
        }
        settings::Message::CacheMoved(result) => {
            handle_cache_moved(state, result);
        }
    }
    Task::none()
}

/// Commit a staged thumbnail resolution: clear the thumbnail cache (its key
/// carries no resolution, so stale entries would shadow the new resolution),
/// then regenerate over the loaded media at the new resolution.
fn confirm_thumbnail_resolution(state: &mut Ferrocull) -> Task<Message> {
    let Some(resolution) = state
        .settings()
        .and_then(|s| s.pending_thumbnail_resolution)
    else {
        return Task::none();
    };
    // A scan in flight holds cache handles; the confirm control is disabled
    // while it runs, but guard here too against a racing message.
    if state.scan_in_flight() {
        return Task::none();
    }

    if let Err(e) = state.thumbnail_cache.clear() {
        state.error(format!("could not clear thumbnail cache: {e}"));
        return Task::none();
    }

    state.thumbnail_resolution = resolution;
    state.persist_settings();
    if let Some(s) = state.settings_mut() {
        s.pending_thumbnail_resolution = None;
    }
    // Drop decoded thumbnails so the grid reloads them at the new resolution
    // once regeneration writes fresh cache entries.
    state.loaded_thumbs.clear();

    state.regenerate_thumbnails()
}

/// Commit a staged cache location: move the cache files off the update loop,
/// reporting back via [`settings::Message::CacheMoved`].
fn confirm_cache_dir(state: &mut Ferrocull) -> Task<Message> {
    let Some(new_dir) = state.settings().and_then(|s| s.pending_cache_dir.clone()) else {
        return Task::none();
    };
    if state.scan_in_flight() {
        return Task::none();
    }
    let old_root = state.cache_root().expect("cache root unresolved");

    if let Some(s) = state.settings_mut() {
        s.cache_move_in_flight = true;
    }

    Task::perform(
        tokio::task::spawn_blocking(move || {
            cache::relocate(&old_root, &new_dir)
                .map(|()| new_dir)
                .map_err(Arc::new)
        }),
        |r| {
            let result = r.expect("cache relocation task panicked");
            Message::Settings(settings::Message::CacheMoved(result))
        },
    )
}

/// Reopen the caches at the new root and swap the handles in, or surface the
/// failure. Decoded thumbnails stay valid — the images are unchanged, only
/// their on-disk location moved.
fn handle_cache_moved(
    state: &mut Ferrocull,
    result: Result<std::path::PathBuf, Arc<cache::Error>>,
) {
    if let Some(s) = state.settings_mut() {
        s.cache_move_in_flight = false;
    }

    let new_root = match result {
        Ok(root) => root,
        Err(e) => {
            state.error(format!("cache move failed: {e}"));
            return;
        }
    };

    // `relocate` just created these namespace directories, so a reopen failure
    // is a broken invariant, not a runtime error.
    let thumbnail = ThumbnailCache::open_in_root(&new_root)
        .unwrap_or_else(|e| panic!("reopen thumbnail cache at {}: {e}", new_root.display()));
    let preview = PreviewCache::open_in_root(&new_root)
        .unwrap_or_else(|e| panic!("reopen preview cache at {}: {e}", new_root.display()));

    state.thumbnail_cache = Arc::new(thumbnail);
    state.preview_disk_cache = Arc::new(preview);
    state.cache_dir = Some(new_root);
    state.persist_settings();
    if let Some(s) = state.settings_mut() {
        s.pending_cache_dir = None;
    }
}

impl Ferrocull {
    /// Re-run the thumbnail pipeline over the loaded media at the current resolution.
    /// Re-emitted `ExifLoaded` events are no-ops for items already present, so
    /// only the thumbnails regenerate.
    fn regenerate_thumbnails(&mut self) -> Task<Message> {
        let files: Vec<ScannedFile> = self
            .media
            .items()
            .iter()
            .map(|item| ScannedFile {
                path: item.path.clone(),
                size: item.size,
                media_type: item.media_type,
                paired: item.paired.clone(),
                sidecars: item.sidecars.clone(),
                xmp_sidecar: item.xmp_sidecar.clone(),
            })
            .collect();

        if files.is_empty() {
            return Task::none();
        }

        if let Some(ref mut progress) = self.thumbnail_progress {
            progress.total += files.len();
        } else {
            self.thumbnail_progress = Some(ThumbnailProgress {
                total: files.len(),
                completed: 0,
            });
        }
        self.thumbnail_jobs_in_flight += 1;

        spawn_thumbnail_sipper(
            files,
            self.thumbnail_resolution,
            Arc::clone(&self.thumbnail_cache),
        )
    }
}
