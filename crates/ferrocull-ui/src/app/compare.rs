use std::{collections::HashSet, path::PathBuf};

use iced::Task;

use super::{CompareState, Ferrocull, PreviewState, ViewMode};
use crate::messages::{Message, compare};

pub(super) fn update(state: &mut Ferrocull, msg: compare::Message) -> Task<Message> {
    match msg {
        compare::Message::EnterHorizontal => {
            state.enter_compare_mode(compare::Layout::Horizontal);
            state.load_compare_previews()
        }
        compare::Message::EnterVertical => {
            state.enter_compare_mode(compare::Layout::Vertical);
            state.load_compare_previews()
        }
        compare::Message::Exit => {
            state.exit_compare_mode();
            if let ViewMode::Preview(ref p) = state.view_mode {
                state.load_preview_for_index(p.index)
            } else {
                Task::none()
            }
        }
        compare::Message::Promote => {
            state.promote_candidate();
            state.load_compare_previews()
        }
        compare::Message::ToggleLockScroll => {
            if let ViewMode::Compare(ref mut cmp) = state.view_mode {
                cmp.lock_scroll = !cmp.lock_scroll;
                if cmp.lock_scroll {
                    let synced = match cmp.active_pane {
                        compare::Pane::Select => cmp.select_view_state,
                        compare::Pane::Candidate => cmp.candidate_view_state,
                    };
                    cmp.select_view_state = synced;
                    cmp.candidate_view_state = synced;
                }
            }
            Task::none()
        }
        compare::Message::ActivePaneChanged(pane) => {
            if let ViewMode::Compare(ref mut cmp) = state.view_mode {
                cmp.active_pane = pane;
            }
            Task::none()
        }
        compare::Message::CandidateNext | compare::Message::CandidatePrev => {
            state.navigate_candidate(matches!(msg, compare::Message::CandidateNext));
            state.load_compare_previews()
        }
        compare::Message::CandidateNavigateTo(idx) => {
            if let ViewMode::Compare(ref mut cmp) = state.view_mode {
                cmp.candidate_index = idx;
            }
            state.load_compare_previews()
        }
        compare::Message::ViewStateChanged(pane, event) => {
            if let ViewMode::Compare(ref mut cmp) = state.view_mode {
                use crate::widgets::Event;
                let target = match pane {
                    compare::Pane::Select => &mut cmp.select_view_state,
                    compare::Pane::Candidate => &mut cmp.candidate_view_state,
                };
                match event {
                    Event::Zoomed { scale, offset } => {
                        target.scale = scale;
                        target.offset = offset;
                    }
                    Event::Panned { offset } => {
                        target.offset = offset;
                    }
                }

                if cmp.lock_scroll {
                    let synced = *target;
                    cmp.select_view_state = synced;
                    cmp.candidate_view_state = synced;
                }
            }
            Task::none()
        }
        compare::Message::ToggleInfoStrip => {
            state.info_strip_open = !state.info_strip_open;
            state.persist_settings();
            Task::none()
        }
        compare::Message::ResetZoom => {
            if let ViewMode::Compare(ref mut cmp) = state.view_mode {
                match (cmp.lock_scroll, cmp.active_pane) {
                    (true, _) => {
                        cmp.select_view_state.toggle_zoom();
                        cmp.candidate_view_state = cmp.select_view_state;
                    }
                    (false, compare::Pane::Select) => cmp.select_view_state.toggle_zoom(),
                    (false, compare::Pane::Candidate) => cmp.candidate_view_state.toggle_zoom(),
                }
            }
            Task::none()
        }
    }
}

impl Ferrocull {
    /// Enter compare mode with the given layout.
    /// Uses current preview/focused index as select, next image as candidate.
    fn enter_compare_mode(&mut self, layout: compare::Layout) {
        if let ViewMode::Compare(ref mut cmp) = self.view_mode {
            cmp.layout = layout;
            return;
        }

        let select_idx = match self.view_mode {
            ViewMode::Preview(ref p) => Some(p.index),
            _ => self.focused_index,
        };
        let Some(select) = select_idx else {
            return;
        };

        let candidate = self.adjacent_index(select, true).unwrap_or(select);

        self.preview_generation = self.preview_generation.wrapping_add(1);

        self.view_mode = ViewMode::Compare(CompareState {
            select_index: select,
            candidate_index: candidate,
            active_pane: compare::Pane::Select,
            lock_scroll: true,
            select_view_state: crate::widgets::ViewState::new(),
            candidate_view_state: crate::widgets::ViewState::new(),
            layout,
        });
    }

    /// Exit compare mode, returning to single preview with the select image.
    fn exit_compare_mode(&mut self) {
        if let ViewMode::Compare(ref cmp) = self.view_mode {
            let select_index = cmp.select_index;
            self.preview_generation = self.preview_generation.wrapping_add(1);
            self.view_mode = ViewMode::Preview(PreviewState {
                index: select_index,
                opened_at: select_index,
                view_state: crate::widgets::ViewState::new(),
            });
        }
    }

    /// Promote candidate to select, find new candidate.
    fn promote_candidate(&mut self) {
        let candidate = match self.view_mode {
            ViewMode::Compare(ref cmp) => cmp.candidate_index,
            _ => return,
        };
        let new_candidate = self.adjacent_index(candidate, true).unwrap_or(candidate);

        if let ViewMode::Compare(ref mut cmp) = self.view_mode {
            cmp.select_index = candidate;
            cmp.candidate_index = new_candidate;
            cmp.active_pane = compare::Pane::Select;
        }
    }

    /// Navigate the candidate pane (select stays as reference).
    fn navigate_candidate(&mut self, forward: bool) {
        let current_idx = match self.view_mode {
            ViewMode::Compare(ref cmp) => cmp.candidate_index,
            _ => return,
        };

        if let Some(new_idx) = self.adjacent_index(current_idx, forward)
            && let ViewMode::Compare(ref mut cmp) = self.view_mode
        {
            cmp.candidate_index = new_idx;
        }
    }

    /// Load previews for compare mode (both panes).
    fn load_compare_previews(&mut self) -> Task<Message> {
        let ViewMode::Compare(ref cmp) = self.view_mode else {
            return Task::none();
        };

        let paths: HashSet<PathBuf> = [cmp.select_index, cmp.candidate_index]
            .into_iter()
            .map(|i| self.media.item(i).path.clone())
            .collect();

        self.load_previews_for_paths(paths)
    }
}
