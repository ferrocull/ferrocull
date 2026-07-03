use std::{collections::HashSet, path::Path};

use ferrocull_core::{ColorLabel, media::Item};
use iced::Task;

use super::{Ferrocull, PreviewState, ViewMode, toggle_set};
use crate::messages::{Message, grid};

pub(super) fn update(state: &mut Ferrocull, msg: grid::Message) -> Task<Message> {
    match msg {
        grid::Message::Scrolled(viewport) => {
            state.grid_viewport_width = viewport.bounds().width;
        }
        grid::Message::ThumbnailVisible(item_idx) => {
            return state.load_thumbnail(item_idx);
        }
        grid::Message::ThumbnailHidden(item_idx) => {
            state.loaded_thumbs.remove(&state.items[item_idx].path);
        }
        grid::Message::FileFocused(path) => {
            let idx = *state
                .item_index
                .get(&path)
                .expect("rendered path is in item_index");
            state.focused_index = Some(idx);
        }
        grid::Message::FileSelectionToggled(path) => state.handle_file_toggled(&path),
        grid::Message::FileSelected(path) => state.handle_file_selected(&path),
        grid::Message::FileDeselected(path) => state.handle_file_deselected(&path),
        grid::Message::FileRated(path, rating) => state.update_rating(&path, rating),
        grid::Message::FileColorLabelSet(path, color_label) => {
            state.update_color_label(&path, color_label);
        }
        grid::Message::SelectAll => {
            state.selected = state
                .sorted_view
                .values()
                .flat_map(|&idx| state.target_indices(idx))
                .collect();
        }
        grid::Message::SelectNone => state.selected.clear(),
        grid::Message::RejectFile(path) => state.handle_reject_file(&path),
        grid::Message::BurstToggled(second) => {
            toggle_set(&mut state.expanded_bursts, second);
            let outcome = state.rebuild_sorted_view();
            state.report_focus_loss(outcome);
            state.item_version += 1;
        }
        grid::Message::ThumbnailHover(idx, is_entering) => {
            state.hovered_thumbnail = is_entering.then_some(idx);
            if !is_entering {
                state.hovered_star = None;
            }
        }
        grid::Message::StarHover(star) => state.hovered_star = star,
        grid::Message::FocusNext => {
            state.focused_index = state.focused_index.map_or_else(
                || state.first_index(),
                |current| state.adjacent_index(current, true).or(Some(current)),
            );
        }
        grid::Message::FocusPrev => {
            state.focused_index = state.focused_index.map_or_else(
                || state.last_index(),
                |current| state.adjacent_index(current, false).or(Some(current)),
            );
        }
        grid::Message::FocusOn(idx) => {
            state.focused_index = Some(idx);
        }
        grid::Message::OpenPreview(idx) => {
            state.preview_generation = state.preview_generation.wrapping_add(1);
            state.view_mode = ViewMode::Preview(PreviewState {
                index: idx,
                opened_at: idx,
                view_state: crate::widgets::ViewState::new(),
            });
            return state.load_preview_for_index(idx);
        }
    }
    Task::none()
}

impl Ferrocull {
    fn handle_file_toggled(&mut self, path: &Path) {
        let idx = *self
            .item_index
            .get(path)
            .expect("rendered path is in item_index");
        let is_selected = self.selected.contains(&idx);
        self.set_selection_for_file(idx, !is_selected);
    }

    fn handle_file_selected(&mut self, path: &Path) {
        let idx = *self
            .item_index
            .get(path)
            .expect("rendered path is in item_index");
        self.set_selection_for_file(idx, true);
    }

    fn handle_file_deselected(&mut self, path: &Path) {
        let idx = *self
            .item_index
            .get(path)
            .expect("rendered path is in item_index");
        self.set_selection_for_file(idx, false);
    }

    /// Set selection state for a file and its burst/RAW+JPEG pairs.
    fn set_selection_for_file(&mut self, idx: usize, select: bool) {
        for target_idx in self.target_indices(idx) {
            self.set_selection(target_idx, select);
        }
    }

    fn handle_reject_file(&mut self, path: &Path) {
        let idx = *self
            .item_index
            .get(path)
            .expect("rendered path is in item_index");
        let new_rating: i8 = if self.items[idx].rating == -1 { 0 } else { -1 };
        self.update_rating(path, new_rating);
    }

    /// Update rating for an item and its burst/pair members.
    fn update_rating(&mut self, path: &Path, rating: i8) {
        let idx = *self
            .item_index
            .get(path)
            .expect("rendered path is in item_index");

        if self.items[idx].rating == rating {
            return;
        }

        let targets = self.target_indices(idx);
        let source_ids = self.apply_to_targets(&targets, |item| {
            item.rating = rating;
        });
        // Rejecting takes a file out of the working set; un-rejecting does not put it back.
        if rating == -1 {
            for &target_idx in &targets {
                self.selected.remove(&target_idx);
            }
        }
        let outcome = self.rebuild_sorted_view();
        self.report_focus_loss(outcome);

        for source_id in source_ids {
            self.db
                .set_rating(&source_id, rating)
                .expect("set_rating query failed");
        }
    }

    /// Update color label for an item and its burst/pair members.
    fn update_color_label(&mut self, path: &Path, color_label: Option<ColorLabel>) {
        let idx = *self
            .item_index
            .get(path)
            .expect("rendered path is in item_index");

        if self.items[idx].color_label == color_label {
            return;
        }

        let targets = self.target_indices(idx);
        let source_ids = self.apply_to_targets(&targets, |item| {
            item.color_label = color_label;
        });
        let outcome = self.rebuild_sorted_view();
        self.report_focus_loss(outcome);

        for source_id in source_ids {
            self.db
                .set_color_label(&source_id, color_label)
                .expect("set_color_label query failed");
        }
    }

    /// Apply mutation to target items plus their JPEG pairs (when grouping is on).
    /// Returns unique `source_ids` of mutated items for persistence.
    fn apply_to_targets<F>(&mut self, targets: &[usize], mut mutate: F) -> Vec<String>
    where
        F: FnMut(&mut Item),
    {
        let mut to_mutate: Vec<usize> = Vec::with_capacity(targets.len() * 2);
        let mut seen: HashSet<usize> = HashSet::new();
        for &target_idx in targets {
            if seen.insert(target_idx) {
                to_mutate.push(target_idx);
            }
            let jpeg_idx = self
                .group_raw_jpeg
                .then(|| {
                    self.items[target_idx]
                        .jpeg_pair
                        .as_ref()
                        .and_then(|jpeg| self.item_index.get(jpeg).copied())
                })
                .flatten();
            if let Some(jpeg_idx) = jpeg_idx
                && seen.insert(jpeg_idx)
            {
                to_mutate.push(jpeg_idx);
            }
        }

        let source_ids = to_mutate
            .iter()
            .map(|&idx| {
                let item = &mut self.items[idx];
                mutate(item);
                item.source_id.clone()
            })
            .collect();

        if !to_mutate.is_empty() {
            self.item_version += 1;
        }
        source_ids
    }
}
