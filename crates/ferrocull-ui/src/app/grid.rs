use std::{collections::HashSet, path::Path, rc::Rc};

use chrono::{DateTime, Utc};
use ferrocull_core::{
    ColorLabel,
    media::{Item, SortOrder},
};
use iced::{Task, widget::scrollable::AbsoluteOffset};

use super::{Ferrocull, PreviewState, ViewMode};
use crate::{
    messages::{Message, grid},
    undo,
    views::{self, GRID_SCROLLABLE_ID},
};

/// Status-echo wording for a rating value.
fn rating_desc(rating: i8) -> String {
    match rating {
        -1 => "Rejected".to_owned(),
        0 => "Unrated".to_owned(),
        n => format!("\u{2605}{n}"),
    }
}

/// Status-echo wording for a color label value.
fn label_desc(label: Option<ColorLabel>) -> String {
    label.map_or_else(
        || "Label cleared".to_owned(),
        |l| format!("Label {}", l.xmp_str()),
    )
}

/// The `(from, to)` value transition to echo for the target item of an
/// undo/redo. Undo reverses the forward move (`after → before`); redo replays it
/// (`before → after`).
fn target_transition<T: Copy>(
    changes: &[(usize, T, T)],
    target: usize,
    dir: undo::Direction,
) -> (T, T) {
    let &(_, before, after) = changes
        .iter()
        .find(|&&(member, _, _)| member == target)
        .expect("undo entry target missing from its recorded group");
    (dir.pick(after, before), dir.pick(before, after))
}

/// Status-echo suffix naming the burst/pair members an action fanned out to.
fn group_suffix(group_len: usize) -> String {
    if group_len > 1 {
        format!(" (+{} grouped)", group_len - 1)
    } else {
        String::new()
    }
}

/// Bulk tag echo: "Tagged/Untagged N file(s)".
fn count_echo(tag: bool, n: usize) -> String {
    format!(
        "{} {n} file{}",
        if tag { "Tagged" } else { "Untagged" },
        super::plural(n)
    )
}

pub(super) fn update(state: &mut Ferrocull, msg: grid::Message) -> Task<Message> {
    match msg {
        grid::Message::FileFocused(path) => {
            let idx = state
                .media
                .index_of(&path)
                .expect("a rendered grid path must resolve to a media item");
            state.focused_index = Some(idx);
        }
        grid::Message::FileTagToggled(path) => state.handle_file_tag_toggled(&path),
        grid::Message::FileTagged(path) => state.handle_file_tagged(&path),
        grid::Message::FileUntagged(path) => state.handle_file_untagged(&path),
        grid::Message::RangeTagTo(path) => {
            let idx = state
                .media
                .index_of(&path)
                .expect("a rendered grid path must resolve to a media item");
            // Without an anchor a range has no start: behave like a plain click.
            match state.focused_index {
                Some(anchor) => return state.range_tag(anchor, idx),
                None => state.focused_index = Some(idx),
            }
        }
        grid::Message::FileRated(path, rating) => state.update_rating(&path, rating),
        grid::Message::FileColorLabelSet(path, color_label) => {
            state.update_color_label(&path, color_label);
        }
        grid::Message::TagAll => {
            let members: Vec<usize> = state
                .media
                .sorted_view()
                .values()
                .flat_map(|&idx| state.group_of(idx))
                .collect();
            state.tag_members(&members, true, state.focused_index, |n| count_echo(true, n));
        }
        grid::Message::UntagAll => {
            let members: Vec<usize> = state.selected.iter().copied().collect();
            state.tag_members(&members, false, state.focused_index, |n| {
                count_echo(false, n)
            });
        }
        grid::Message::RejectFile(path) => state.handle_reject_file(&path),
        grid::Message::BurstToggled(key) => {
            let target = state.media.burst_map()[&key][0];
            return state.toggle_burst(key, target);
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
        grid::Message::ExtendFocus(direction) => return state.extend_focus(direction),
        grid::Message::FocusPageDown => return state.move_focus_by_page(true),
        grid::Message::FocusPageUp => return state.move_focus_by_page(false),
        grid::Message::FocusHome => {
            let idx = state.first_index();
            return state.reveal_focus(idx);
        }
        grid::Message::FocusEnd => {
            let idx = state.last_index();
            return state.reveal_focus(idx);
        }
        grid::Message::FocusOn(idx) => {
            state.focused_index = Some(idx);
        }
        grid::Message::ToggleFocusedBurst => return state.toggle_focused_burst(),
        grid::Message::Undo => return state.handle_undo(),
        grid::Message::Redo => return state.handle_redo(),
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
            .expect("grid anchor maps to no row");
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
    /// reveal it. Clamps into a shorter partial target row and stays put at
    /// the top/bottom edge.
    fn move_focus_by_row(&mut self, down: bool) -> Task<Message> {
        let Some(current) = self.focused_index else {
            let idx = if down {
                self.first_index()
            } else {
                self.last_index()
            };
            return self.reveal_focus(idx);
        };
        self.row_step_target(current, down)
            .map_or_else(Task::none, |target| self.reveal_focus(Some(target)))
    }

    /// Move focus one viewport's worth of rows up or down, staying in the same
    /// column, then reveal it.
    fn move_focus_by_page(&mut self, down: bool) -> Task<Message> {
        let Some(current) = self.focused_index else {
            let idx = if down {
                self.first_index()
            } else {
                self.last_index()
            };
            return self.reveal_focus(idx);
        };
        self.page_step_target(current, down)
            .map_or_else(Task::none, |target| self.reveal_focus(Some(target)))
    }

    /// Item one grid row up/down from `current`, same column; `None` at the
    /// top/bottom edge or before the first layout.
    fn row_step_target(&mut self, current: usize, down: bool) -> Option<usize> {
        self.focus_row_target(current, |rows, row| {
            if down {
                (row + 1 < rows.len()).then_some(row + 1)
            } else {
                row.checked_sub(1)
            }
        })
    }

    /// Item one viewport page up/down from `current`, same column; `None` at
    /// the edges or before the first layout.
    fn page_step_target(&mut self, current: usize, down: bool) -> Option<usize> {
        let viewport = self.grid_viewport_height;
        self.focus_row_target(current, |rows, row| {
            views::thumbnails::page_row(rows, row, viewport, down)
        })
    }

    /// Resolve a row-based focus move: find `current`'s row and column, pick a
    /// target row via `target_row`, and clamp the column into it (a shorter
    /// partial row clamps to its last card). Reuses the section-aware row model
    /// so the column stays aligned across date-section breaks.
    fn focus_row_target(
        &mut self,
        current: usize,
        target_row: impl FnOnce(&[views::thumbnails::RowStart], usize) -> Option<usize>,
    ) -> Option<usize> {
        let width = self.grid_area_width?;
        let rows = self.grid_rows(width);
        if rows.is_empty() {
            return None;
        }
        let ordinal = self
            .ordinal_position(current)
            .expect("no ordinal for focused index");
        let row =
            views::thumbnails::row_for_ordinal(&rows, ordinal).expect("no row for focused ordinal");
        let target = target_row(&rows, row)?;
        let col = ordinal - rows[row].ordinal;
        let row_end = rows
            .get(target + 1)
            .map_or_else(|| self.media.visible_len(), |r| r.ordinal);
        let target_ordinal = (rows[target].ordinal + col).min(row_end - 1);
        Some(
            self.media
                .index_at_ordinal(target_ordinal, self.config.view.ascending)
                .expect("no visible item at target ordinal"),
        )
    }

    /// Shift+Arrow: move focus like the plain arrow and tag every item between
    /// the old and new focus, inclusive. At an edge the range collapses to the
    /// current item, which is still tagged.
    fn extend_focus(&mut self, direction: grid::Direction) -> Task<Message> {
        let Some(current) = self.focused_index else {
            // No anchor: land like the plain arrow and tag the landed item.
            let idx = match direction {
                grid::Direction::Right | grid::Direction::Down => self.first_index(),
                grid::Direction::Left | grid::Direction::Up => self.last_index(),
            };
            return idx.map_or_else(Task::none, |i| self.range_tag(i, i));
        };
        let target = match direction {
            grid::Direction::Right => self.adjacent_index(current, true),
            grid::Direction::Left => self.adjacent_index(current, false),
            grid::Direction::Down => self.row_step_target(current, true),
            grid::Direction::Up => self.row_step_target(current, false),
        }
        .unwrap_or(current);
        self.range_tag(current, target)
    }

    /// Tag the contiguous display-order range from `anchor` to `target`
    /// (inclusive, group-propagated) as one undo unit, then move focus to
    /// `target` and reveal it.
    fn range_tag(&mut self, anchor: usize, target: usize) -> Task<Message> {
        let range = self
            .media
            .indices_between(anchor, target, self.config.view.ascending);
        let members: Vec<usize> = range.iter().flat_map(|&idx| self.group_of(idx)).collect();
        self.tag_members(&members, true, Some(target), |n| count_echo(true, n));
        self.reveal_focus(Some(target))
    }

    /// `B` key: toggle collapse/expand of the focused item's burst. No-op when
    /// nothing is focused or the focused item is not in a burst.
    fn toggle_focused_burst(&mut self) -> Task<Message> {
        let Some(idx) = self.focused_index else {
            return Task::none();
        };
        let Some(&key) = self.media.burst_of().get(&idx) else {
            return Task::none();
        };
        self.toggle_burst(key, idx)
    }

    /// Collapse/expand burst `key` as one undoable view-state entry, repairing
    /// focus onto the burst's visible representative when a collapse hides the
    /// focused member. Shared by the `B` key and the badge-click path so both
    /// are undoable and repair focus identically. `key` must name a live burst;
    /// `target` is the acted-on item, used for the status echo.
    fn toggle_burst(&mut self, key: DateTime<Utc>, target: usize) -> Task<Message> {
        let focus_before = self.focused_index;
        let expanded_before = self.media.is_burst_expanded(key);
        self.media
            .toggle_burst_expansion(key, &self.config.params());
        let expanded_after = self.media.is_burst_expanded(key);
        // A collapse can hide the focused member (only this burst's members
        // change visibility); keep the cursor on the burst's representative.
        if self
            .focused_index
            .is_some_and(|idx| !self.media.is_visible(idx))
        {
            self.focused_index = Some(self.media.burst_map()[&key][0]);
        }
        self.reconcile_selection();
        let focus_after = self.focused_index;
        self.undo_stack.record(undo::Entry {
            target,
            action: undo::Action::Burst {
                key,
                expanded_before,
                expanded_after,
                focus_before,
                focus_after,
            },
        });
        // Reveal the toggled burst, not the focused card: a badge click does
        // not move focus, so the stored focus can sit far off-screen and
        // revealing it would yank the viewport there.
        self.scroll_focus_into_view(self.media.burst_map()[&key][0])
    }

    /// Ctrl+Z: revert the most recent recorded mutation, moving it onto the redo
    /// stack.
    fn handle_undo(&mut self) -> Task<Message> {
        let Some(entry) = self.undo_stack.take_undo() else {
            self.echo("Nothing to undo".to_owned());
            return Task::none();
        };
        let task = self.apply_entry(&entry, undo::Direction::Undo);
        self.undo_stack.push_redo(entry);
        task
    }

    /// Ctrl+Shift+Z / Ctrl+Y: re-apply the most recently undone mutation, moving
    /// it back onto the undo stack.
    fn handle_redo(&mut self) -> Task<Message> {
        let Some(entry) = self.undo_stack.take_redo() else {
            self.echo("Nothing to redo".to_owned());
            return Task::none();
        };
        let task = self.apply_entry(&entry, undo::Direction::Redo);
        self.undo_stack.push_undo(entry);
        task
    }

    /// Apply a recorded entry in `dir`: undo restores each item's `before`
    /// value, redo restores `after`. Runs through the same mutation/persistence
    /// paths the original action used, then returns focus to the acted-on card.
    fn apply_entry(&mut self, entry: &undo::Entry, dir: undo::Direction) -> Task<Message> {
        let name = self.display_name(entry.target);
        match &entry.action {
            undo::Action::Rating {
                changes,
                selection_removed,
            } => {
                let assignments: Vec<(usize, i8)> = changes
                    .iter()
                    .map(|&(m, b, a)| (m, dir.pick(b, a)))
                    .collect();
                self.set_rating_values(&assignments);
                // The forward action untagged these; undo re-tags, redo untags.
                match dir {
                    undo::Direction::Undo => {
                        self.selected.extend(selection_removed.iter().copied());
                    }
                    undo::Direction::Redo => {
                        for member in selection_removed {
                            self.selected.remove(member);
                        }
                    }
                }
                let (from, to) = target_transition(changes, entry.target, dir);
                self.echo(format!(
                    "{}: {} → {} — {name}",
                    dir.verb(),
                    rating_desc(from),
                    rating_desc(to)
                ));
            }
            undo::Action::ColorLabel { changes } => {
                let assignments: Vec<(usize, Option<ColorLabel>)> = changes
                    .iter()
                    .map(|&(m, b, a)| (m, dir.pick(b, a)))
                    .collect();
                self.set_color_label_values(&assignments);
                let (from, to) = target_transition(changes, entry.target, dir);
                self.echo(format!(
                    "{}: {} → {} — {name}",
                    dir.verb(),
                    label_desc(from),
                    label_desc(to)
                ));
            }
            undo::Action::Tag { changes } => {
                for &(member, before, after) in changes {
                    if dir.pick(before, after) {
                        self.selected.insert(member);
                    } else {
                        self.selected.remove(&member);
                    }
                }
                let n = changes.len();
                self.echo(format!(
                    "{}: tag change — {n} file{}",
                    dir.verb(),
                    super::plural(n)
                ));
            }
            undo::Action::Burst {
                key,
                expanded_before,
                expanded_after,
                focus_before,
                focus_after,
            } => {
                // Set the recorded state absolutely, not by re-toggling: a
                // rebuild (filter/sort/grouping change, scan batch) can reset a
                // burst's expansion or dissolve it entirely without touching the
                // undo stack, so a blind toggle would flip the wrong way.
                let expanded = dir.pick(*expanded_before, *expanded_after);
                if !self
                    .media
                    .set_burst_expansion(*key, expanded, &self.config.params())
                {
                    // The burst no longer exists; nothing to restore.
                    self.echo(format!("{}: burst no longer available", dir.verb()));
                    return Task::none();
                }
                self.focused_index = dir.pick(*focus_before, *focus_after);
                self.echo(format!("{}: burst — {name}", dir.verb()));
            }
        }
        self.reconcile_selection();

        // Metadata/tag actions return to the acted-on card; a burst restored its
        // own focus above.
        let focus = match &entry.action {
            undo::Action::Burst { .. } => self.focused_index,
            _ => Some(entry.target),
        };
        match focus {
            Some(target) if self.media.is_visible(target) => {
                self.focused_index = Some(target);
                self.scroll_focus_into_view(target)
            }
            _ => Task::none(),
        }
    }

    fn handle_file_tag_toggled(&mut self, path: &Path) {
        let idx = self
            .media
            .index_of(path)
            .expect("a rendered grid path must resolve to a media item");
        let is_tagged = self.selected.contains(&idx);
        self.set_tag_for_file(idx, !is_tagged);
    }

    fn handle_file_tagged(&mut self, path: &Path) {
        let idx = self
            .media
            .index_of(path)
            .expect("a rendered grid path must resolve to a media item");
        self.set_tag_for_file(idx, true);
    }

    fn handle_file_untagged(&mut self, path: &Path) {
        let idx = self
            .media
            .index_of(path)
            .expect("a rendered grid path must resolve to a media item");
        self.set_tag_for_file(idx, false);
    }

    /// Tag or untag a file and its whole logical group (burst members and
    /// RAW+JPEG siblings), recording one undo unit and echoing to the status
    /// bar when anything changed.
    fn set_tag_for_file(&mut self, idx: usize, tag: bool) {
        let group = self.group_of(idx);
        let msg = format!(
            "{} → {}{}",
            if tag { "Tagged" } else { "Untagged" },
            self.display_name(idx),
            group_suffix(group.len())
        );
        self.tag_members(&group, tag, Some(idx), |_| msg);
    }

    /// Set the tag state of `members` as one undo unit and echo `message(n)`,
    /// where `n` is how many members actually changed. Undo returns focus to
    /// `target` (falling back to the first changed member). A no-op when no
    /// member changed state.
    fn tag_members(
        &mut self,
        members: &[usize],
        tag: bool,
        target: Option<usize>,
        message: impl FnOnce(usize) -> String,
    ) {
        let changed = self.set_tags(members, tag);
        if changed.is_empty() {
            return;
        }
        let n = changed.len();
        let target = target.unwrap_or(changed[0].0);
        self.undo_stack.record(undo::Entry {
            target,
            action: undo::Action::Tag { changes: changed },
        });
        self.echo(message(n));
    }

    /// Set the tag state of every member, returning the `(idx, before, after)`
    /// state of each member whose state actually changed (the undo payload).
    fn set_tags(&mut self, members: &[usize], tag: bool) -> Vec<(usize, bool, bool)> {
        members
            .iter()
            .filter_map(|&member| {
                let was_tagged = self.selected.contains(&member);
                if was_tagged == tag {
                    return None;
                }
                if tag {
                    self.selected.insert(member);
                } else {
                    self.selected.remove(&member);
                }
                Some((member, was_tagged, tag))
            })
            .collect()
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
        let changes: Vec<(usize, i8, i8)> = group
            .iter()
            .map(|&member| (member, self.media.item(member).rating, rating))
            .collect();
        let assignments: Vec<(usize, i8)> = group.iter().map(|&member| (member, rating)).collect();
        self.set_rating_values(&assignments);
        // Rejecting takes a file out of the working set; un-rejecting does not put it back.
        let mut selection_removed = Vec::new();
        if rating == -1 {
            for &member in &group {
                if self.selected.remove(&member) {
                    selection_removed.push(member);
                }
            }
        }
        self.undo_stack.record(undo::Entry {
            target: idx,
            action: undo::Action::Rating {
                changes,
                selection_removed,
            },
        });
        let msg = format!(
            "{} → {}{} \u{b7} saved",
            rating_desc(rating),
            self.display_name(idx),
            group_suffix(group.len())
        );
        self.echo(msg);
        // Reconcile selection/focus in case the rating change hid an item.
        self.reconcile_selection();
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
        let changes: Vec<(usize, Option<ColorLabel>, Option<ColorLabel>)> = group
            .iter()
            .map(|&member| (member, self.media.item(member).color_label, color_label))
            .collect();
        let assignments: Vec<(usize, Option<ColorLabel>)> =
            group.iter().map(|&member| (member, color_label)).collect();
        self.set_color_label_values(&assignments);
        self.undo_stack.record(undo::Entry {
            target: idx,
            action: undo::Action::ColorLabel { changes },
        });
        let msg = format!(
            "{} → {}{} \u{b7} saved",
            label_desc(color_label),
            self.display_name(idx),
            group_suffix(group.len())
        );
        self.echo(msg);
        self.reconcile_selection();
    }

    /// Apply each `(member, rating)` assignment: mutate the member's group and
    /// synchronously persist the new rating to metadata. Shared by the forward
    /// rating handler and undo/redo replay so both run identical mutate+persist
    /// code, and so persistence always completes before the caller's "· saved"
    /// echo (metadata writes are synchronous and crash on failure).
    fn set_rating_values(&mut self, assignments: &[(usize, i8)]) {
        for &(member, rating) in assignments {
            for source_id in self.apply_to_group(&[member], |item| item.rating = rating) {
                self.metadata.set_rating(&source_id, rating);
            }
        }
    }

    /// Apply each `(member, label)` assignment: mutate the member's group and
    /// synchronously persist the new color label to metadata. Shared by the
    /// forward label handler and undo/redo replay.
    fn set_color_label_values(&mut self, assignments: &[(usize, Option<ColorLabel>)]) {
        for &(member, label) in assignments {
            for source_id in self.apply_to_group(&[member], |item| item.color_label = label) {
                self.metadata.set_color_label(&source_id, label);
            }
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
