//! Bounded undo/redo stacks for grid metadata mutations (rating, color label,
//! tag) and the burst collapse/expand view toggle.
//!
//! Each [`Entry`] captures both the previous and the applied value of every
//! item one user action touched (burst members and RAW+JPEG pairs included), so
//! the same entry reverses in either direction: undo restores `before`, redo
//! restores `after`. Applying an entry is the owner's job — the stacks only
//! record entries and hand them back in LIFO order, moving each one between the
//! undo and redo halves as it is applied.

use chrono::{DateTime, Utc};
use ferrocull_core::ColorLabel;

/// Most entries retained per stack; pushing beyond this drops the oldest.
const LIMIT: usize = 32;

/// Which way an [`Entry`] is being applied — undo restores the recorded
/// `before` values, redo restores the `after` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Undo,
    Redo,
}

impl Direction {
    /// Status-echo prefix for this direction.
    pub(crate) fn verb(self) -> &'static str {
        match self {
            Self::Undo => "Undo",
            Self::Redo => "Redo",
        }
    }

    /// Pick the value this direction restores from a `(before, after)` pair.
    pub(crate) fn pick<T>(self, before: T, after: T) -> T {
        match self {
            Self::Undo => before,
            Self::Redo => after,
        }
    }
}

/// One undoable action: the `(idx, before, after)` value triples of every item
/// it touched. Item indices are stable for the session (the store is
/// append-only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    Rating {
        /// `(item, before, after)` rating of every item the action touched.
        changes: Vec<(usize, i8, i8)>,
        /// Items the forward action removed from the working set (rejecting
        /// untags): undo re-tags them, redo untags them again.
        selection_removed: Vec<usize>,
    },
    ColorLabel {
        /// `(item, before, after)` label of every item the action touched.
        changes: Vec<(usize, Option<ColorLabel>, Option<ColorLabel>)>,
    },
    Tag {
        /// Items whose tag state the action flipped; every one landed on
        /// `tagged` (from `!tagged`), so one flag replays the whole entry in
        /// either direction.
        members: Vec<usize>,
        tagged: bool,
    },
    /// A burst collapse/expand toggle. The explicit expansion state and focus to
    /// restore in each direction are stored, so replay sets state absolutely
    /// rather than re-toggling — a stale re-toggle would desync after a rebuild
    /// reset the burst's expansion independently of the undo stack.
    Burst {
        key: DateTime<Utc>,
        expanded_before: bool,
        expanded_after: bool,
        focus_before: Option<usize>,
        focus_after: Option<usize>,
    },
}

/// One recorded user action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    /// The item the user acted on — focus returns here on undo/redo (except
    /// [`Action::Burst`], which restores its own recorded focus).
    pub(crate) target: usize,
    pub(crate) action: Action,
}

fn push_bounded(stack: &mut Vec<Entry>, entry: Entry) {
    if stack.len() == LIMIT {
        stack.remove(0);
    }
    stack.push(entry);
}

/// Paired LIFO undo/redo stacks, each bounded at [`LIMIT`].
#[derive(Debug, Default)]
pub(crate) struct Stack {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
}

impl Stack {
    /// Record a fresh user action. Clears the redo stack — a new forward
    /// mutation invalidates any undone-then-not-redone future.
    pub(crate) fn record(&mut self, entry: Entry) {
        push_bounded(&mut self.undo, entry);
        self.redo.clear();
    }

    /// Take the most recent undoable action, if any. The caller applies it in
    /// the [`Direction::Undo`] direction, then hands it to [`Self::push_redo`].
    pub(crate) fn take_undo(&mut self) -> Option<Entry> {
        self.undo.pop()
    }

    /// Take the most recent redoable action, if any. The caller applies it in
    /// the [`Direction::Redo`] direction, then hands it to [`Self::push_undo`].
    pub(crate) fn take_redo(&mut self) -> Option<Entry> {
        self.redo.pop()
    }

    /// Return an applied entry to the redo stack (after an undo).
    pub(crate) fn push_redo(&mut self, entry: Entry) {
        push_bounded(&mut self.redo, entry);
    }

    /// Return an applied entry to the undo stack (after a redo).
    pub(crate) fn push_undo(&mut self, entry: Entry) {
        push_bounded(&mut self.undo, entry);
    }

    /// Number of actions currently undoable — drives the status-bar hint.
    pub(crate) fn undo_len(&self) -> usize {
        self.undo.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, Entry, LIMIT, Stack};

    fn entry(target: usize) -> Entry {
        Entry {
            target,
            action: Action::Tag {
                members: vec![target],
                tagged: true,
            },
        }
    }

    #[test]
    fn take_undo_returns_entries_in_lifo_order() {
        let mut stack = Stack::default();
        stack.record(entry(1));
        stack.record(entry(2));
        assert_eq!(stack.take_undo().unwrap().target, 2);
        assert_eq!(stack.take_undo().unwrap().target, 1);
        assert_eq!(stack.take_undo(), None);
    }

    #[test]
    fn recording_past_the_bound_drops_the_oldest() {
        let mut stack = Stack::default();
        for i in 0..LIMIT + 5 {
            stack.record(entry(i));
        }
        // The newest LIMIT entries survive: LIMIT+4 down to 5.
        for expected in (5..LIMIT + 5).rev() {
            assert_eq!(stack.take_undo().unwrap().target, expected);
        }
        assert_eq!(stack.take_undo(), None);
    }

    #[test]
    fn undo_then_redo_round_trips_through_both_stacks() {
        let mut stack = Stack::default();
        stack.record(entry(7));

        let undone = stack.take_undo().unwrap();
        assert_eq!(undone.target, 7);
        stack.push_redo(undone);
        assert_eq!(stack.undo_len(), 0);

        let redone = stack.take_redo().unwrap();
        assert_eq!(redone.target, 7);
        stack.push_undo(redone);
        assert_eq!(stack.undo_len(), 1);
    }

    #[test]
    fn recording_a_new_action_clears_the_redo_stack() {
        let mut stack = Stack::default();
        stack.record(entry(1));
        // Undo it, populating the redo stack.
        let e = stack.take_undo().unwrap();
        stack.push_redo(e);

        // A fresh action after an undo discards the redo future.
        stack.record(entry(2));
        assert_eq!(stack.take_redo(), None);
    }
}
