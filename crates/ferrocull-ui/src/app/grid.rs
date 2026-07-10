use std::{collections::HashSet, path::Path, rc::Rc};

use ferrocull_core::{
    ColorLabel,
    media::{Item, SortOrder},
};
use iced::{Task, widget::scrollable::AbsoluteOffset};

use super::{Ferrocull, PreviewState, ViewMode};
use crate::{
    messages::{Message, grid},
    views::{self, GRID_SCROLLABLE_ID},
};

pub(super) fn update(state: &mut Ferrocull, msg: grid::Message) -> Task<Message> {
    match msg {
        grid::Message::FileFocused(path) => {
            let idx = state
                .media
                .index_of(&path)
                .expect("a rendered grid path must resolve to a media item");
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
                .media
                .sorted_view()
                .values()
                .flat_map(|&idx| state.group_of(idx))
                .collect();
        }
        grid::Message::SelectNone => state.selected.clear(),
        grid::Message::RejectFile(path) => state.handle_reject_file(&path),
        grid::Message::BurstToggled(second) => {
            state
                .media
                .toggle_burst_expansion(second, &state.config.params());
            // Collapsing hides members → reconcile selection/focus.
            state.reconcile_selection();
        }
        grid::Message::ThumbnailHover(idx, is_entering) => {
            state.hovered_thumbnail = is_entering.then_some(idx);
            if !is_entering {
                state.hovered_star = None;
            }
        }
        grid::Message::StarHover(star) => state.hovered_star = star,
        grid::Message::FocusNext => {
            let idx = state.focused_index.map_or_else(
                || state.first_index(),
                |current| state.adjacent_index(current, true).or(Some(current)),
            );
            return state.reveal_focus(idx);
        }
        grid::Message::FocusPrev => {
            let idx = state.focused_index.map_or_else(
                || state.last_index(),
                |current| state.adjacent_index(current, false).or(Some(current)),
            );
            return state.reveal_focus(idx);
        }
        grid::Message::FocusDown => return state.move_focus_by_row(true),
        grid::Message::FocusUp => return state.move_focus_by_row(false),
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
        grid::Message::Scrolled {
            offset,
            grid_width,
            viewport_height,
            content_height,
        } => {
            return state.handle_grid_scrolled(offset, grid_width, viewport_height, content_height);
        }
        grid::Message::Wheel(delta) => return state.handle_grid_wheel(delta),
    }
    Task::none()
}

impl Ferrocull {
    /// Snap the grid one or more rows per wheel notch (line deltas), or scroll
    /// smoothly for touchpad pixel deltas. No-op until the grid width is known.
    fn handle_grid_wheel(&mut self, delta: iced::mouse::ScrollDelta) -> Task<Message> {
        let Some(width) = self.grid_area_width else {
            return Task::none();
        };
        match delta {
            iced::mouse::ScrollDelta::Pixels { y, .. } => iced::widget::operation::scroll_by(
                GRID_SCROLLABLE_ID,
                AbsoluteOffset { x: 0.0, y: -y },
            ),
            iced::mouse::ScrollDelta::Lines { y: dy, .. } => {
                // A direction reversal discards the fractional carry — hi-res
                // wheels would otherwise swallow the first notch of the new
                // direction paying off the old remainder.
                if self.grid_wheel_lines * -dy < 0.0 {
                    self.grid_wheel_lines = 0.0;
                }
                self.grid_wheel_lines += -dy;
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "wheel line accumulation stays far within i32"
                )]
                let steps = self.grid_wheel_lines.trunc() as i32;
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "the carried remainder is a small whole line count"
                )]
                let consumed = steps as f32;
                self.grid_wheel_lines -= consumed;
                if steps == 0 {
                    return Task::none();
                }
                let rows = self.grid_rows(width);
                let Some(target) = views::thumbnails::step_row(&rows, self.grid_scroll_y, steps)
                else {
                    return Task::none();
                };
                let offset = rows[target].offset;
                self.grid_scroll_y = offset;
                self.grid_anchor = rows[target].ordinal;
                iced::widget::operation::scroll_to(
                    GRID_SCROLLABLE_ID,
                    AbsoluteOffset { x: 0.0, y: offset },
                )
            }
        }
    }

    /// Interpret the scrollable's viewport report and keep the anchor card
    /// pinned to the viewport top.
    ///
    /// iced funnels scrolls, window resizes, and content growth through this one
    /// channel and clamps the offset against reflowed content before reporting,
    /// so the offset alone is ambiguous. [`views::thumbnails::scroll_reaction`]
    /// disambiguates from the geometry deltas: a reflow or a clamp re-pins the
    /// stored anchor; only a pure offset move at unchanged geometry is a user
    /// scroll that moves the anchor.
    fn handle_grid_scrolled(
        &mut self,
        offset: f32,
        grid_width: f32,
        viewport_height: f32,
        content_height: f32,
    ) -> Task<Message> {
        let prev = self
            .grid_area_width
            .map(|width| views::thumbnails::GridGeometry {
                width,
                viewport_height: self.grid_viewport_height,
                content_height: self.grid_content_height,
                scroll_y: self.grid_scroll_y,
            });
        let reaction = views::thumbnails::scroll_reaction(
            prev,
            offset,
            grid_width,
            viewport_height,
            content_height,
        );

        self.grid_area_width = Some(grid_width);
        self.grid_viewport_height = viewport_height;
        self.grid_content_height = content_height;

        match reaction {
            views::thumbnails::ScrollReaction::Reanchor => self.reanchor_grid(grid_width),
            views::thumbnails::ScrollReaction::AdoptOffset => {
                let rows = self.grid_rows(grid_width);
                self.pin_anchor(&rows, offset);
                self.grid_scroll_y = offset;
                Task::none()
            }
            views::thumbnails::ScrollReaction::Idle => {
                self.grid_scroll_y = offset;
                Task::none()
            }
        }
    }

    /// Scroll so the row holding the anchor card sits at the viewport top under
    /// the current geometry for `grid_width`. The stored anchor is left
    /// untouched, so chained reflows keep pinning the same card. The target is
    /// clamped to the scrollable's own range: when the anchor sits in the final
    /// screenful its row cannot reach the very top, and matching iced's clamp
    /// here keeps the follow-up report from reading the clamp as a user scroll.
    pub(super) fn reanchor_grid(&mut self, grid_width: f32) -> Task<Message> {
        let rows = self.grid_rows(grid_width);
        if rows.is_empty() {
            return Task::none();
        }
        let target = views::thumbnails::row_for_ordinal(&rows, self.grid_anchor)
            .expect("ordinal 0 always maps to the first row");
        let y = rows[target].offset.min(self.max_grid_offset());
        self.grid_scroll_y = y;
        iced::widget::operation::scroll_to(GRID_SCROLLABLE_ID, AbsoluteOffset { x: 0.0, y })
    }

    /// Reset the grid to the top and drop the pinned anchor. Called whenever the
    /// view model changes (sort, filter, grouping, ascending, ...): the anchor
    /// is a display ordinal into the *old* order, so re-pinning it after the
    /// reflow would scroll to an arbitrary row in the new order.
    pub(super) fn reset_grid_scroll(&mut self) -> Task<Message> {
        self.grid_anchor = 0;
        self.grid_scroll_y = 0.0;
        self.grid_wheel_lines = 0.0;
        iced::widget::operation::scroll_to(GRID_SCROLLABLE_ID, AbsoluteOffset { x: 0.0, y: 0.0 })
    }

    /// Largest scroll offset the grid can reach: content minus viewport, floored
    /// at zero when the content is shorter than the viewport.
    fn max_grid_offset(&self) -> f32 {
        (self.grid_content_height - self.grid_viewport_height).max(0.0)
    }

    /// Pin the anchor to the row occupying the viewport top at `offset`.
    fn pin_anchor(&mut self, rows: &[views::thumbnails::RowStart], offset: f32) {
        if let Some(row) = views::thumbnails::anchor_row(rows, offset) {
            self.grid_anchor = rows[row].ordinal;
        }
    }

    /// Scroll anchors for every grid row under the current view and the
    /// geometry derived from `grid_width`. Empty when no items are visible.
    ///
    /// Memoized on [`GridRowsKey`]: `on_scroll` calls this on every frame, but
    /// the row model only changes with the media view, sort/grouping, or column
    /// geometry — so a plain scroll hands out a cheap `Rc` clone instead of
    /// re-walking every item in `section_counts`.
    fn grid_rows(&mut self, grid_width: f32) -> Rc<[views::thumbnails::RowStart]> {
        let grouped = self.config.view.sort_order == SortOrder::Time;
        let key = super::GridRowsKey {
            media_version: self.media.version(),
            ascending: self.config.view.ascending,
            grouped,
            width_bits: grid_width.to_bits(),
            scale_bits: self.window_scale.to_bits(),
        };
        if let Some((cached_key, rows)) = &self.grid_rows_cache
            && *cached_key == key
        {
            return Rc::clone(rows);
        }
        let counts = views::thumbnails::section_counts(
            self.media.items(),
            self.media.sorted_view(),
            self.config.view.ascending,
            grouped,
        );
        let (cols, cell_width) = views::thumbnails::grid_metrics(grid_width, self.window_scale);
        let rows: Rc<[views::thumbnails::RowStart]> =
            views::thumbnails::row_starts(&counts, cols, cell_width, grouped).into();
        self.grid_rows_cache = Some((key, Rc::clone(&rows)));
        rows
    }

    /// Item indices whose rows fall in the current thumbnail load window
    /// (viewport plus [`GRID_OVERSCAN`](views::thumbnails::GRID_OVERSCAN)).
    fn window_item_indices(&mut self, grid_width: f32) -> HashSet<usize> {
        let rows = self.grid_rows(grid_width);
        let Some((first, last)) = views::thumbnails::visible_row_window(
            &rows,
            self.grid_scroll_y,
            self.grid_viewport_height,
            views::thumbnails::GRID_OVERSCAN,
        ) else {
            return HashSet::new();
        };
        let start = rows[first].ordinal;
        let end = rows
            .get(last + 1)
            .map_or_else(|| self.media.visible_len(), |r| r.ordinal);
        self.media
            .indices_in_ordinal_range(start, end - start, self.config.view.ascending)
            .into_iter()
            .collect()
    }

    /// Reconcile which thumbnails are loaded with the current scroll window:
    /// evict thumbnails whose cells left the window and spawn loads for cells
    /// that just entered it. When a scan batch reported freshly-cached
    /// thumbnails ([`Ferrocull::thumb_generation_dirty`]), also retry every
    /// in-window cell that is still unloaded, so a thumbnail that finished
    /// generating while its cell sat in view gets picked up.
    ///
    /// Replaces the per-cell `sensor` `on_show`/`on_hide` the virtualized grid
    /// can no longer host (offscreen cells are not built). Called after every
    /// `update`; cheap when the window did not move.
    pub(super) fn reconcile_thumbnail_window(&mut self) -> Task<Message> {
        let new_window = match self.grid_area_width {
            Some(width) => self.window_item_indices(width),
            // No scroll report yet — iced suppresses them while the content fits
            // the viewport, so the visible set is bounded: load all of it.
            None => self.media.sorted_view().values().copied().collect(),
        };

        // Evict thumbnails whose items left the window (bounds memory).
        let leaving: Vec<std::path::PathBuf> = self
            .loaded_thumbs
            .keys()
            .filter(|path| {
                self.media
                    .index_of(path)
                    .is_none_or(|idx| !new_window.contains(&idx))
            })
            .cloned()
            .collect();
        for path in leaving {
            self.loaded_thumbs.remove(&path);
        }

        let retry_all = std::mem::take(&mut self.thumb_generation_dirty);
        let loads: Vec<Task<Message>> = new_window
            .iter()
            .filter(|&&idx| {
                let entered = !self.thumb_window.contains(&idx);
                (entered || retry_all)
                    && !self.loaded_thumbs.contains_key(&self.media.item(idx).path)
            })
            .map(|&idx| self.load_thumbnail(idx))
            .collect();

        self.thumb_window = new_window;
        Task::batch(loads)
    }

    /// Move focus to `idx` (if any) and scroll the grid the minimum needed to
    /// keep the newly focused card on screen.
    fn reveal_focus(&mut self, idx: Option<usize>) -> Task<Message> {
        self.focused_index = idx;
        idx.map_or_else(Task::none, |i| self.scroll_focus_into_view(i))
    }

    /// Scroll so the row holding card `idx` is fully visible, aligning to
    /// whichever viewport edge it fell past; a no-op when the card is already
    /// visible. Uses the same anchor bookkeeping as the wheel handler so the
    /// next viewport report does not read the move as a user scroll.
    pub(super) fn scroll_focus_into_view(&mut self, idx: usize) -> Task<Message> {
        let Some(width) = self.grid_area_width else {
            return Task::none();
        };
        let rows = self.grid_rows(width);
        let ordinal = self
            .ordinal_position(idx)
            .expect("no ordinal for focused index");
        let target =
            views::thumbnails::row_for_ordinal(&rows, ordinal).expect("no row for focused ordinal");
        // Row anchors double as content-space row tops (monotonic, gap-adjusted).
        let row_top = rows[target].offset;
        let row_bot = rows
            .get(target + 1)
            .map_or(self.grid_content_height, |r| r.offset);
        let view_top = self.grid_scroll_y;
        let view_bot = view_top + self.grid_viewport_height;

        let y = if row_top < view_top {
            row_top
        } else if row_bot > view_bot {
            // Align the row's bottom to the viewport, but never past its top (a
            // row taller than the viewport aligns to the top instead).
            (row_bot - self.grid_viewport_height).min(row_top)
        } else {
            return Task::none();
        };
        let y = y.clamp(0.0, self.max_grid_offset());

        self.grid_scroll_y = y;
        self.pin_anchor(&rows, y);
        iced::widget::operation::scroll_to(GRID_SCROLLABLE_ID, AbsoluteOffset { x: 0.0, y })
    }

    /// Move focus one grid row up or down, staying in the same column, then
    /// reveal it. Clamps `col` into a shorter partial target row and stays put
    /// at the top/bottom edge. Reuses the section-aware row model so the column
    /// stays aligned across date-section breaks.
    fn move_focus_by_row(&mut self, down: bool) -> Task<Message> {
        let Some(width) = self.grid_area_width else {
            return Task::none();
        };
        let rows = self.grid_rows(width);
        if rows.is_empty() {
            return Task::none();
        }
        let Some(current) = self.focused_index else {
            let idx = if down {
                self.first_index()
            } else {
                self.last_index()
            };
            return self.reveal_focus(idx);
        };
        let ordinal = self
            .ordinal_position(current)
            .expect("no ordinal for focused index");
        let row =
            views::thumbnails::row_for_ordinal(&rows, ordinal).expect("no row for focused ordinal");
        let col = ordinal - rows[row].ordinal;
        let target_row = if down {
            if row + 1 >= rows.len() {
                return Task::none();
            }
            row + 1
        } else {
            if row == 0 {
                return Task::none();
            }
            row - 1
        };
        let row_end = rows
            .get(target_row + 1)
            .map_or_else(|| self.media.visible_len(), |r| r.ordinal);
        let target_ordinal = (rows[target_row].ordinal + col).min(row_end - 1);
        let target_idx = self
            .media
            .index_at_ordinal(target_ordinal, self.config.view.ascending)
            .expect("no visible item at target ordinal");
        self.reveal_focus(Some(target_idx))
    }

    fn handle_file_toggled(&mut self, path: &Path) {
        let idx = self
            .media
            .index_of(path)
            .expect("a rendered grid path must resolve to a media item");
        let is_selected = self.selected.contains(&idx);
        self.set_selection_for_file(idx, !is_selected);
    }

    fn handle_file_selected(&mut self, path: &Path) {
        let idx = self
            .media
            .index_of(path)
            .expect("a rendered grid path must resolve to a media item");
        self.set_selection_for_file(idx, true);
    }

    fn handle_file_deselected(&mut self, path: &Path) {
        let idx = self
            .media
            .index_of(path)
            .expect("a rendered grid path must resolve to a media item");
        self.set_selection_for_file(idx, false);
    }

    /// Set selection state for a file and its whole logical group (burst members
    /// + RAW+JPEG siblings).
    fn set_selection_for_file(&mut self, idx: usize, select: bool) {
        for member in self.group_of(idx) {
            if select {
                self.selected.insert(member);
            } else {
                self.selected.remove(&member);
            }
        }
    }

    fn handle_reject_file(&mut self, path: &Path) {
        let idx = self
            .media
            .index_of(path)
            .expect("a rendered grid path must resolve to a media item");
        let new_rating: i8 = if self.media.item(idx).rating == -1 {
            0
        } else {
            -1
        };
        self.update_rating(path, new_rating);
    }

    /// Update rating for an item and its burst/pair members.
    fn update_rating(&mut self, path: &Path, rating: i8) {
        let idx = self
            .media
            .index_of(path)
            .expect("a rendered grid path must resolve to a media item");

        if self.media.item(idx).rating == rating {
            return;
        }

        let group = self.group_of(idx);
        let source_ids = self.apply_to_group(&group, |item| {
            item.rating = rating;
        });
        // Rejecting takes a file out of the working set; un-rejecting does not put it back.
        if rating == -1 {
            for &member in &group {
                self.selected.remove(&member);
            }
        }
        // Reconcile selection/focus in case the rating change hid an item.
        self.reconcile_selection();

        for source_id in source_ids {
            self.metadata.set_rating(&source_id, rating);
        }
    }

    /// Update color label for an item and its burst/pair members.
    fn update_color_label(&mut self, path: &Path, color_label: Option<ColorLabel>) {
        let idx = self
            .media
            .index_of(path)
            .expect("a rendered grid path must resolve to a media item");

        if self.media.item(idx).color_label == color_label {
            return;
        }

        let group = self.group_of(idx);
        let source_ids = self.apply_to_group(&group, |item| {
            item.color_label = color_label;
        });
        self.reconcile_selection();

        for source_id in source_ids {
            self.metadata.set_color_label(&source_id, color_label);
        }
    }

    /// Apply `mutate` to every member of `group`, reconciling the view per
    /// item. Returns the mutated `source_ids`, for persistence.
    fn apply_to_group<F>(&mut self, group: &[usize], mut mutate: F) -> Vec<String>
    where
        F: FnMut(&mut Item),
    {
        let params = self.config.params();
        group
            .iter()
            .map(|&idx| {
                self.media.mutate_item(idx, &params, &mut mutate);
                self.media.item(idx).source_id.clone()
            })
            .collect()
    }
}
