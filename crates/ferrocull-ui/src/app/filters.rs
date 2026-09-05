use std::time::Duration;

use ferrocull_core::media::SortOrder;
use iced::Task;

use super::{Ferrocull, toggle_set};
use crate::{
    messages::{Message, filters},
    views,
};

/// How long a thumbnail size change waits for another one before counting as
/// settled. The slider's release settles it sooner; the timer covers every input
/// that never reports one: the keyboard, the wheel over the grid, and the
/// slider's own arrow keys and `Ctrl+Wheel`.
const THUMBNAIL_SIZE_SETTLE: Duration = Duration::from_millis(200);

impl Ferrocull {
    /// Move focus off an item the current visible set no longer shows, and say
    /// so. Tags are untouched: a filter is a lens, not a scope.
    pub(super) fn reconcile_focus(&mut self) {
        if self.media.prune_hidden_focus(&mut self.focused_index) {
            self.error("Focused item hidden".to_owned());
        }
    }
}

pub(super) fn update(state: &mut Ferrocull, msg: filters::Message) -> Task<Message> {
    // Every arm that reorders or refilters the view resets the grid to the top:
    // the pinned anchor is a display ordinal into the old order, so keeping it
    // would scroll to an arbitrary row after the reflow. The two burst arms are
    // exempt: they only change what is hidden, leaving the display sequence in
    // the same order, so the photographer keeps their place. So are the
    // thumbnail size arms: resizing reflows the rows without touching the
    // display sequence, and the anchor still names the same card.
    match msg {
        filters::Message::SortChanged(order) => return state.handle_sort_changed(order),
        filters::Message::AscendingToggled => {
            state.config.view.ascending = !state.config.view.ascending;
            state.persist_settings();
            return state.reset_grid_scroll();
        }
        filters::Message::FilterChanged(mode) => {
            state.config.view.filter_mode = mode;
            state.persist_settings();
        }
        filters::Message::NewOnlyToggled => {
            state.config.view.new_only = !state.config.view.new_only;
            state.persist_settings();
        }
        filters::Message::GroupRawJpegToggled => {
            state.config.view.group_raw_jpeg = !state.config.view.group_raw_jpeg;
            state.persist_settings();
        }
        filters::Message::GroupBurstsToggled => {
            // `rebuild` retires the per-burst exceptions when grouping is off,
            // so grouping comes back at whatever the expand preference says.
            state.config.view.group_bursts = !state.config.view.group_bursts;
            state.persist_settings();
            return state.rebuild_keeping_place();
        }
        filters::Message::ExpandBurstsToggled => {
            state.config.view.expand_bursts = !state.config.view.expand_bursts;
            state.persist_settings();
            return state.rebuild_keeping_place();
        }
        filters::Message::HideRejectedToggled => {
            state.config.view.hide_rejected = !state.config.view.hide_rejected;
            state.persist_settings();
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
        filters::Message::DateSortToggled => {
            state.config.view.date_tree_ascending = !state.config.view.date_tree_ascending;
            state.persist_settings();
            return Task::none();
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
            state.config.clear_filters();
            state.persist_settings();
        }
        filters::Message::ThumbnailSizeChanged(size) => {
            return state.set_thumbnail_size(size);
        }
        filters::Message::ThumbnailSizeReleased => {
            if state.thumbnail_size_pending.is_some() {
                state.settle_thumbnail_size();
            }
            return Task::none();
        }
        filters::Message::ThumbnailSizeSettled(generation) => {
            // A newer change owns the settle now, or the release beat the timer.
            if state.thumbnail_size_pending == Some(generation) {
                state.settle_thumbnail_size();
            }
            return Task::none();
        }
        filters::Message::ThumbnailSizeWheel(delta) => {
            return state.handle_thumbnail_size_wheel(delta);
        }
        filters::Message::ThumbnailSizeStepped(direction) => {
            let stepped =
                views::thumbnails::step_thumbnail_size(state.config.view.thumbnail_size, direction);
            if stepped == state.config.view.thumbnail_size {
                return Task::none();
            }
            return state.set_thumbnail_size(stepped);
        }
    }
    state.rebuild_view();
    state.reset_grid_scroll()
}

impl Ferrocull {
    /// Adopt a new thumbnail size, keep the photographer's place, and start the
    /// settle window.
    ///
    /// Every input runs through here, so the slider, the keyboard, and the wheel
    /// all defer the preference write and the thumbnail load window to the same
    /// settle. With no measured grid width there is no geometry to re-anchor
    /// against, so the value alone is stored.
    pub(super) fn set_thumbnail_size(&mut self, size: u32) -> Task<Message> {
        let reflow = self.reflow_thumbnail_size(size);
        Task::batch([reflow, self.start_thumbnail_size_settle()])
    }

    /// Mark the thumbnail size unsettled and start the quiet-window timer for
    /// this change. A later change supersedes the timer by bumping the
    /// generation it carries.
    fn start_thumbnail_size_settle(&mut self) -> Task<Message> {
        let generation = self.thumbnail_size_generation.wrapping_add(1);
        self.thumbnail_size_generation = generation;
        self.thumbnail_size_pending = Some(generation);
        Task::perform(tokio::time::sleep(THUMBNAIL_SIZE_SETTLE), move |()| {
            Message::Filters(filters::Message::ThumbnailSizeSettled(generation))
        })
    }

    /// Take the thumbnail size as final: write it to the preferences and let
    /// the thumbnail load window reconcile against the settled geometry.
    fn settle_thumbnail_size(&mut self) {
        self.thumbnail_size_pending = None;
        self.persist_settings();
    }

    fn handle_sort_changed(&mut self, order: SortOrder) -> Task<Message> {
        if order == self.config.view.sort_order {
            return Task::none();
        }
        self.config.view.sort_order = order;
        self.persist_settings();
        self.rebuild_view();
        self.reset_grid_scroll()
    }

    /// Rebuild the whole derived view from the current settings, then reconcile
    /// focus against the new visible set (surfacing a status message if the
    /// focused item was hidden).
    pub(super) fn rebuild_view(&mut self) {
        self.media.rebuild(&self.config.params());
        self.reconcile_focus();
    }

    /// Rebuild for a change that hides and reveals frames without reordering
    /// them, keeping the photographer where they were culling.
    ///
    /// The repair reads the pre-rebuild focus and runs before the prune, which
    /// would otherwise drop the focus and report it as hidden for a fold the
    /// photographer asked for. With nothing focused there is no anchor to keep,
    /// so the grid resets.
    fn rebuild_keeping_place(&mut self) -> Task<Message> {
        let focused_before = self.focused_index;
        self.media.rebuild(&self.config.params());
        if let Some(idx) = focused_before
            && let Some(representative) = self.media.folded_burst_representative(idx)
        {
            self.focused_index = Some(representative);
        }
        self.reconcile_focus();
        match self.focused_index {
            Some(idx) => self.anchor_grid_to(idx),
            None => self.reset_grid_scroll(),
        }
    }
}
