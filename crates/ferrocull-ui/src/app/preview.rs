use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use ferrocull_core::{
    cache::{PreviewCache, cache_key_from_disk},
    thumbnail::extract_largest_preview,
};
use iced::Task;

use super::{Ferrocull, ViewMode};
use crate::messages::{Message, preview};

pub(super) fn update(state: &mut Ferrocull, msg: preview::Message) -> Task<Message> {
    match msg {
        preview::Message::Close => {
            if let ViewMode::Preview(p) = &state.view_mode {
                let index = p.index;
                let navigated_away = p.index != p.opened_at;
                state.preview_generation = state.preview_generation.wrapping_add(1);
                state.view_mode = ViewMode::Grid;
                state.preview_cache.clear();
                state.preview_loading.clear();
                state.hovered_star = None;
                // Rejecting or refiltering the previewed card can hide it; focus
                // must stay on a visible item.
                if state.media.is_visible(index) {
                    state.focused_index = Some(index);
                    if navigated_away {
                        return state.scroll_focus_into_view(index);
                    }
                }
            }
        }
        preview::Message::Prev | preview::Message::Next => {
            let forward = matches!(msg, preview::Message::Next);
            let current = match state.view_mode {
                ViewMode::Preview(ref p) => p.index,
                _ => return Task::none(),
            };
            if let Some(new_idx) = state.adjacent_index(current, forward)
                && let ViewMode::Preview(ref mut p) = state.view_mode
            {
                p.index = new_idx;
                return state.load_preview_for_index(new_idx);
            }
        }
        preview::Message::NavigateTo(idx) => {
            if let ViewMode::Preview(ref mut p) = state.view_mode {
                p.index = idx;
                return state.load_preview_for_index(idx);
            }
        }
        preview::Message::ToggleBurst => {
            let ViewMode::Preview(ref p) = state.view_mode else {
                return Task::none();
            };
            let index = p.index;
            // A frame outside any burst has nothing to fold: `B` and the badge
            // are both no-ops there.
            if let Some(status) = state.burst_status(index) {
                return state.toggle_burst(status.key(), index);
            }
        }
        preview::Message::ViewStateChanged(event) => {
            if let ViewMode::Preview(ref mut p) = state.view_mode {
                use crate::widgets::Event;
                match event {
                    Event::Zoomed { scale, offset } => {
                        p.view_state.scale = scale;
                        p.view_state.offset = offset;
                    }
                    Event::Panned { offset } => {
                        p.view_state.offset = offset;
                    }
                }
            }
        }
        preview::Message::ResetZoom => {
            if let ViewMode::Preview(ref mut p) = state.view_mode {
                p.view_state.toggle_zoom();
            }
        }
    }
    Task::none()
}

impl Ferrocull {
    /// Load previews for current index and adjacent images (for instant navigation).
    /// Returns Task that will emit `PreviewLoaded` for each image loaded.
    pub(super) fn load_preview_for_index(&mut self, idx: usize) -> Task<Message> {
        let indices = [
            Some(idx),
            self.adjacent_index(idx, false),
            self.adjacent_index(idx, true),
        ];

        let paths: HashSet<PathBuf> = indices
            .into_iter()
            .flatten()
            .map(|i| self.media.item(i).path.clone())
            .collect();

        self.load_previews_for_paths(paths)
    }

    /// Evict stale cache entries, skip already-loaded/loading paths, mark new ones
    /// as loading, and spawn batch preview extraction tasks.
    pub(super) fn load_previews_for_paths(
        &mut self,
        paths_to_keep: HashSet<PathBuf>,
    ) -> Task<Message> {
        let request_generation = self.preview_generation;
        self.preview_cache.retain(|p, _| paths_to_keep.contains(p));

        let paths_to_load: Vec<PathBuf> = paths_to_keep
            .into_iter()
            .filter(|p| {
                !self.preview_cache.contains_key(p)
                    && self.preview_loading.get(p).copied() != Some(request_generation)
            })
            .collect();

        for path in &paths_to_load {
            self.preview_loading
                .insert(path.clone(), request_generation);
        }

        let disk_cache = Arc::clone(&self.preview_disk_cache);
        Task::batch(paths_to_load.into_iter().map(move |path| {
            let cache = Arc::clone(&disk_cache);
            Task::perform(
                tokio::task::spawn_blocking(move || {
                    let result = load_or_extract_preview(&cache, &path);
                    (path, result)
                }),
                move |r| match r {
                    Ok((loaded_path, result)) => {
                        Message::PreviewLoaded(request_generation, loaded_path, result)
                    }
                    Err(e) => {
                        tracing::error!("preview load task panicked: {e}");
                        Message::Noop
                    }
                },
            )
        }))
    }

    /// Load a single thumbnail from disk cache if not already loaded. Decodes
    /// the cached JPEG to RGBA off the render thread so a fast scroll doesn't
    /// spike a frame decoding dozens of newly-revealed thumbnails at draw time.
    pub(super) fn load_thumbnail(&self, item_idx: usize) -> Task<Message> {
        let item = self.media.item(item_idx);
        if self.loaded_thumbs.contains_key(&item.path) {
            return Task::none();
        }

        let path = item.path.clone();
        let cache = Arc::clone(&self.thumbnail_cache);
        Task::perform(
            tokio::task::spawn_blocking(move || {
                let key = cache_key_from_disk(&path).ok()?;
                let entry = cache.load(&key).ok()??;
                let handle = decode_thumbnail(&entry.jpeg, &path)?;
                Some((path, handle))
            }),
            |r| {
                r.unwrap_or_else(|e| {
                    tracing::error!("thumbnail load task panicked: {e}");
                    None
                })
            },
        )
        .and_then(|(path, handle)| Task::done(Message::ThumbnailLoaded(path, handle)))
    }
}

/// Decode a cached thumbnail JPEG into an RGBA image handle. A corrupt cached
/// JPEG is external data, so a decode failure is logged and treated as a cache
/// miss (`None`), never a crash.
fn decode_thumbnail(jpeg: &[u8], path: &Path) -> Option<iced::widget::image::Handle> {
    match image::load_from_memory(jpeg) {
        Ok(img) => {
            let rgba = img.into_rgba8();
            let (width, height) = (rgba.width(), rgba.height());
            Some(iced::widget::image::Handle::from_rgba(
                width,
                height,
                rgba.into_raw(),
            ))
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), "corrupt cached thumbnail, ignoring: {e}");
            None
        }
    }
}

/// Load an extracted preview JPEG from the disk cache, extracting and caching it
/// on a miss. The disk cache is a performance optimization, not a source of
/// truth, so read/write failures degrade to a fresh extraction (logged).
fn load_or_extract_preview(cache: &PreviewCache, path: &Path) -> Result<Vec<u8>, String> {
    let key = cache_key_from_disk(path).map_err(|e| e.to_string())?;
    match cache.load(&key) {
        Ok(Some(jpeg)) => return Ok(jpeg),
        Ok(None) => {}
        Err(e) => tracing::warn!(path = %path.display(), "preview cache read failed: {e}"),
    }

    let jpeg = extract_largest_preview(path).map_err(|e| e.to_string())?;
    if let Err(e) = cache.put(&key, &jpeg) {
        tracing::warn!(path = %path.display(), "preview cache write failed: {e}");
    }
    Ok(jpeg)
}
