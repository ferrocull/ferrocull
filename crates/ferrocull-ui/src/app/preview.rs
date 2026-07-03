use std::{collections::HashSet, path::PathBuf};

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
                state.focused_index = Some(index);
                if navigated_away {
                    return state.scroll_grid_to_item(index);
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
                p.view_state = crate::widgets::ViewState::new();
                return state.load_preview_for_index(new_idx);
            }
        }
        preview::Message::NavigateTo(idx) => {
            if let ViewMode::Preview(ref mut p) = state.view_mode {
                p.index = idx;
                p.view_state = crate::widgets::ViewState::new();
                return state.load_preview_for_index(idx);
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
            .map(|i| self.items[i].path.clone())
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

        Task::batch(paths_to_load.into_iter().map(move |path| {
            Task::perform(
                tokio::task::spawn_blocking(move || {
                    let result = ferrocull_core::thumbnail::extract_largest_preview(&path)
                        .map_err(|e| e.to_string());
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

    /// Load a single thumbnail from disk cache if not already loaded.
    pub(super) fn load_thumbnail(&self, item_idx: usize) -> Task<Message> {
        use ferrocull_core::cache::{ThumbnailCache, cache_key_from_disk};

        let item = &self.items[item_idx];
        if self.loaded_thumbs.contains_key(&item.path) {
            return Task::none();
        }

        let path = item.path.clone();
        Task::perform(
            tokio::task::spawn_blocking(move || {
                let cache = ThumbnailCache::open().ok()?;
                let key = cache_key_from_disk(&path).ok()?;
                let jpeg = cache.load(&key).ok()??;
                Some((path, iced::widget::image::Handle::from_bytes(jpeg)))
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
