use ferrocull_core::media::{FilterMode, SortOrder};
use iced::Task;

use super::{Ferrocull, toggle_set};
use crate::{
    media_view::RebuildOutcome,
    messages::{Message, filters},
};

impl Ferrocull {
    /// Prune selection/focus to the current visible set and surface a status
    /// message when the prune hid the focused item or dropped selected ones.
    pub(super) fn reconcile_selection(&mut self) {
        let outcome = self
            .media
            .prune_hidden(&mut self.selected, &mut self.focused_index);
        self.report_focus_loss(outcome);
    }

    fn report_focus_loss(&mut self, outcome: RebuildOutcome) {
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

pub(super) fn update(state: &mut Ferrocull, msg: filters::Message) -> Task<Message> {
    // Every arm that reorders or refilters the view resets the grid to the top:
    // the pinned anchor is a display ordinal into the old order, so keeping it
    // would scroll to an arbitrary row after the reflow.
    match msg {
        filters::Message::SortChanged(order) => return state.handle_sort_changed(order),
        filters::Message::AscendingToggled => {
            state.config.ascending = !state.config.ascending;
            return state.reset_grid_scroll();
        }
        filters::Message::FilterChanged(mode) => {
            state.config.filter_mode = mode;
        }
        filters::Message::GroupRawJpegToggled => {
            state.config.group_raw_jpeg = !state.config.group_raw_jpeg;
        }
        filters::Message::GroupBurstsToggled => {
            // `rebuild` clears burst expansion when grouping is off.
            state.config.group_bursts = !state.config.group_bursts;
        }
        filters::Message::HideRejectedToggled => {
            state.config.hide_rejected = !state.config.hide_rejected;
        }
        filters::Message::RatingFilterToggled(rating) => {
            toggle_set(&mut state.config.selected_ratings, rating);
        }
        filters::Message::ColorLabelFilterToggled(label) => {
            toggle_set(&mut state.config.selected_color_labels, label);
        }
        filters::Message::DateToggled(selection) => {
            state.config.selected_dates = if state.config.selected_dates == Some(selection) {
                None
            } else {
                Some(selection)
            };
        }
        filters::Message::YearExpanded(year) => {
            toggle_set(&mut state.expanded_years, year);
            return Task::none();
        }
        filters::Message::MonthExpanded(year, month) => {
            toggle_set(&mut state.expanded_months, (year, month));
            return Task::none();
        }
        filters::Message::ClearAll => {
            state.config.filter_mode = FilterMode::default();
            state.config.hide_rejected = false;
            state.config.selected_dates = None;
            state.config.selected_ratings.clear();
            state.config.selected_color_labels.clear();
        }
    }
    state.rebuild_view();
    state.reset_grid_scroll()
}

impl Ferrocull {
    fn handle_sort_changed(&mut self, order: SortOrder) -> Task<Message> {
        if order == self.config.sort_order {
            return Task::none();
        }
        self.config.sort_order = order;
        self.rebuild_view();
        self.reset_grid_scroll()
    }

    /// Rebuild the whole derived view from the current settings, then reconcile
    /// selection/focus against the new visible set (surfacing a status message
    /// if anything was hidden).
    pub(super) fn rebuild_view(&mut self) {
        self.media.rebuild(&self.config.params());
        self.reconcile_selection();
    }
}
