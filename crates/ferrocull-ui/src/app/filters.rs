use std::collections::HashSet;

use ferrocull_core::media::{
    CaptureTime, FilterMode, Item, SortKey, SortOrder, matches_date_filter,
};
use iced::Task;

use super::{BurstInfo, Ferrocull, toggle_set};

/// Side-effects of a `rebuild_sorted_view` call. Callers that change filters
/// should check this and surface to the user; callers that just added items
/// or refreshed sources can safely ignore.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct RebuildOutcome {
    /// The previously-focused item is no longer visible.
    pub(super) focused_lost: bool,
    /// Number of previously-selected items removed from the selection set.
    pub(super) selection_pruned: usize,
}

impl Ferrocull {
    /// Surface a notification when rebuild hid the focused item or removed
    /// items from the selection set. Sets `status_message` only when something
    /// was actually pruned.
    pub(super) fn report_focus_loss(&mut self, outcome: RebuildOutcome) {
        let message = match (outcome.focused_lost, outcome.selection_pruned) {
            (false, 0) => return,
            (true, 0) => "Focused item hidden".to_owned(),
            (false, n) => format!("{n} selected item{} hidden", if n == 1 { "" } else { "s" }),
            (true, n) => format!(
                "Focused item and {n} selected item{} hidden",
                if n == 1 { "" } else { "s" }
            ),
        };
        self.status_message = Some(message);
    }
}
use crate::{
    messages::{Message, filters},
    views::BurstDisplayInfo,
};

pub(super) fn update(state: &mut Ferrocull, msg: filters::Message) -> Task<Message> {
    match msg {
        filters::Message::SortChanged(order) => state.handle_sort_changed(order),
        filters::Message::AscendingToggled => state.ascending = !state.ascending,
        filters::Message::FilterChanged(mode) => {
            state.filter_mode = mode;
            let outcome = state.rebuild_sorted_view();
            state.report_focus_loss(outcome);
        }
        filters::Message::GroupRawJpegToggled => {
            state.group_raw_jpeg = !state.group_raw_jpeg;
            let outcome = state.rebuild_sorted_view();
            state.report_focus_loss(outcome);
        }
        filters::Message::GroupBurstsToggled => {
            state.group_bursts = !state.group_bursts;
            if !state.group_bursts {
                state.expanded_bursts.clear();
            }
            let outcome = state.rebuild_sorted_view();
            state.report_focus_loss(outcome);
        }
        filters::Message::HideRejectedToggled => {
            state.hide_rejected = !state.hide_rejected;
            let outcome = state.rebuild_sorted_view();
            state.report_focus_loss(outcome);
        }
        filters::Message::RatingFilterToggled(rating) => {
            toggle_set(&mut state.selected_ratings, rating);
            let outcome = state.rebuild_sorted_view();
            state.report_focus_loss(outcome);
        }
        filters::Message::ColorLabelFilterToggled(label) => {
            toggle_set(&mut state.selected_color_labels, label);
            let outcome = state.rebuild_sorted_view();
            state.report_focus_loss(outcome);
        }
        filters::Message::DateToggled(selection) => {
            state.selected_dates = if state.selected_dates == Some(selection) {
                None
            } else {
                Some(selection)
            };
            let outcome = state.rebuild_sorted_view();
            state.report_focus_loss(outcome);
        }
        filters::Message::YearExpanded(year) => {
            toggle_set(&mut state.expanded_years, year);
        }
        filters::Message::MonthExpanded(year, month) => {
            toggle_set(&mut state.expanded_months, (year, month));
        }
        filters::Message::ClearAll => {
            state.filter_mode = FilterMode::default();
            state.hide_rejected = false;
            state.selected_dates = None;
            state.selected_ratings.clear();
            state.selected_color_labels.clear();
            let outcome = state.rebuild_sorted_view();
            state.report_focus_loss(outcome);
        }
    }
    Task::none()
}

impl Ferrocull {
    fn handle_sort_changed(&mut self, order: SortOrder) {
        if order == self.sort_order {
            return;
        }
        self.sort_order = order;
        // Sort order doesn't change visibility, so focused/selected can't be pruned.
        let _ = self.rebuild_sorted_view();
    }

    /// Rebuild `sorted_view` from scratch with current filter and sort settings.
    ///
    /// Silently prunes `selected` and `focused_index` to the new visible set —
    /// the returned `RebuildOutcome` lets callers surface that to the user (e.g.
    /// status message when the focused item is hidden by a new filter).
    #[must_use]
    pub(super) fn rebuild_sorted_view(&mut self) -> RebuildOutcome {
        self.item_version += 1;
        self.sorted_view.clear();
        self.sort_key_by_idx.clear();
        self.burst_map.clear();
        self.burst_membership.clear();
        self.burst_display.clear();

        self.hidden_jpeg_paths = if self.group_raw_jpeg {
            self.items
                .iter()
                .filter_map(|item| item.jpeg_pair.clone())
                .collect()
        } else {
            HashSet::new()
        };

        let passing_indices: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| self.passes_filters(item))
            .map(|(idx, _)| idx)
            .collect();

        if self.group_bursts {
            self.compute_bursts(&passing_indices);
            self.burst_display = self
                .burst_map
                .iter()
                .flat_map(|(&burst_key, info)| {
                    let display_info = BurstDisplayInfo {
                        count: info.count,
                        burst_key,
                    };
                    info.members.iter().map(move |&idx| (idx, display_info))
                })
                .collect();
        }

        for &idx in &passing_indices {
            if self.should_show_in_grid(idx) {
                let key = SortKey::from_item(&self.items[idx], self.sort_order);
                self.sorted_view.insert(key.clone(), idx);
                self.sort_key_by_idx.insert(idx, key);
            }
        }

        let visible: HashSet<usize> = self.sorted_view.values().copied().collect();
        let selection_before = self.selected.len();
        self.selected.retain(|idx| visible.contains(idx));
        let selection_pruned = selection_before - self.selected.len();
        let focused_lost = self.focused_index.is_some_and(|i| !visible.contains(&i));
        self.focused_index = self.focused_index.filter(|idx| visible.contains(idx));

        RebuildOutcome {
            focused_lost,
            selection_pruned,
        }
    }

    /// Build burst map using delta-based grouping: consecutive photos ≤1 second apart.
    fn compute_bursts(&mut self, passing_indices: &[usize]) {
        let mut timed_items: Vec<(usize, CaptureTime)> = passing_indices
            .iter()
            .map(|&idx| (idx, self.items[idx].capture_time))
            .collect();

        timed_items.sort_by_key(|(_, ct)| *ct);

        if timed_items.is_empty() {
            return;
        }

        let mut current_group: Vec<usize> = vec![timed_items[0].0];
        let mut prev_time = timed_items[0].1;

        for &(idx, capture_time) in &timed_items[1..] {
            if !prev_time.within_burst_threshold(&capture_time) {
                self.register_burst_if_valid(std::mem::take(&mut current_group));
            }
            current_group.push(idx);
            prev_time = capture_time;
        }

        self.register_burst_if_valid(current_group);
    }

    /// Register a group as a burst if it has 3+ members.
    fn register_burst_if_valid(&mut self, group: Vec<usize>) {
        if group.len() < 3 {
            return;
        }
        let burst_key = self.items[group[0]].capture_time.second;
        for &member_idx in &group {
            self.burst_membership.insert(member_idx, burst_key);
        }
        self.burst_map.insert(
            burst_key,
            BurstInfo {
                count: group.len(),
                members: group,
            },
        );
    }

    /// Check if an item should appear in the grid (handles burst collapsing).
    fn should_show_in_grid(&self, idx: usize) -> bool {
        if !self.group_bursts {
            return true;
        }
        let Some(&burst_key) = self.burst_membership.get(&idx) else {
            return true;
        };
        if self.expanded_bursts.contains(&burst_key) {
            return true;
        }
        // Only show the first member (representative) when collapsed
        self.burst_map[&burst_key].members.first() == Some(&idx)
    }

    /// Check if item passes all current filters.
    pub(super) fn passes_filters(&self, item: &Item) -> bool {
        let from_source = self
            .selected_sources
            .iter()
            .any(|s| item.path.starts_with(s));
        let passes_mode = self.filter_mode.matches(item);
        let not_rejected = !(self.hide_rejected && item.rating == -1);
        let not_hidden = !self.group_raw_jpeg || !self.hidden_jpeg_paths.contains(&item.path);
        let passes_date = matches_date_filter(item, self.selected_dates);

        let passes_rating =
            self.selected_ratings.is_empty() || self.selected_ratings.contains(&item.rating);

        let passes_color = self.selected_color_labels.is_empty()
            || self.selected_color_labels.contains(&item.color_label);

        from_source
            && passes_mode
            && not_rejected
            && not_hidden
            && passes_date
            && passes_rating
            && passes_color
    }
}
