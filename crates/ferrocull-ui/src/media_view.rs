//! Owns the media item store and every index derived from it: the source of
//! truth for the loaded photos and the derived structures the grid,
//! navigation, and burst grouping depend on.
//!
//! Every mutation keeps all derived indices consistent and bumps the
//! render-cache version internally, so no caller can forget to update one map
//! or invalidate the grid cache.
//!
//! Data-oriented design (ADR-0003): columnar `items`, integer indices into it,
//! and free-standing transforms over slices — no per-item entity objects, no
//! `Rc`/`RefCell`.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::Bound::{Excluded, Unbounded},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use ferrocull_core::{
    ColorLabel,
    media::{
        CaptureTime, DateSelection, FilterMode, Item, SortKey, SortOrder, matches_date_filter,
    },
};

/// The user-chosen filter/sort/grouping configuration that defines what the
/// view shows, borrowed from the owning application state.
///
/// `ascending` is intentionally absent — the view is stored in ascending
/// sort-key order and direction is applied at read time, so it never affects
/// the derived indices. Burst expansion is likewise absent — `MediaView` owns
/// it, since only the burst re-keying logic can keep it consistent.
pub(crate) struct ViewParams<'a> {
    pub(crate) sort_order: SortOrder,
    pub(crate) filter_mode: FilterMode,
    pub(crate) hide_rejected: bool,
    pub(crate) group_raw_jpeg: bool,
    pub(crate) group_bursts: bool,
    pub(crate) selected_sources: &'a BTreeSet<PathBuf>,
    pub(crate) selected_dates: Option<DateSelection>,
    pub(crate) selected_ratings: &'a BTreeSet<i8>,
    pub(crate) selected_color_labels: &'a BTreeSet<Option<ColorLabel>>,
}

/// Result of pruning selection/focus against the current visible set.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RebuildOutcome {
    /// The previously-focused item is no longer visible.
    pub(crate) focused_lost: bool,
    /// Number of previously-selected items removed from the selection set.
    pub(crate) selection_pruned: usize,
}

#[derive(Default)]
pub(crate) struct MediaView {
    /// Append-only columnar store. Indices into this are stable for the session.
    items: Vec<Item>,
    /// Path → index into `items`. Every path in `items` has exactly one entry.
    item_index: HashMap<PathBuf, usize>,
    /// Visible items in ascending sort-key order (the display sequence).
    sorted_view: BTreeMap<SortKey, usize>,
    /// Reverse of `sorted_view` for O(1) key lookup / visibility test.
    /// Invariant: `sort_key_by_idx.contains_key(i)` iff `sorted_view` contains `i`.
    sort_key_by_idx: HashMap<usize, SortKey>,
    /// All passing (filter-visible, pre burst-collapse) items in capture-time
    /// order, for incremental burst maintenance. Key is `(capture_time, idx)`
    /// so equal timestamps break by index — matching the stable order a full
    /// rebuild would produce.
    burst_order: BTreeSet<(CaptureTime, usize)>,
    /// Burst key (first member's second) → members, in capture-time order.
    /// The badge count is `members.len()` — never duplicated per member.
    burst_map: HashMap<DateTime<Utc>, Vec<usize>>,
    /// Item index → its burst key, for every burst member.
    burst_of: HashMap<usize, DateTime<Utc>>,
    /// Bursts the user has expanded (shown un-collapsed), keyed by burst key.
    /// Owned here so burst re-keying can migrate expansion atomically.
    expanded_bursts: BTreeSet<DateTime<Utc>>,
    /// JPEG-side path of every known RAW+JPEG pair → the index of the RAW that
    /// hides it (its group representative). Rebuilt from scratch on
    /// [`Self::rebuild`].
    hidden_jpeg_paths: HashMap<PathBuf, usize>,
    /// Live count of items with a star rating (`rating >= 1`), maintained at
    /// every insertion and rating mutation so the status-bar tally never scans
    /// the store. Counts all loaded items, visible or not.
    rated_count: usize,
    /// Live count of rejected items (`rating == -1`), maintained alongside
    /// [`Self::rated_count`].
    rejected_count: usize,
    /// Monotonic counter for grid render-cache invalidation.
    version: u64,
}

/// Whether a rating counts as rated (`>= 1`) and/or rejected (`== -1`).
const fn rating_class(rating: i8) -> (bool, bool) {
    (rating >= 1, rating == -1)
}

impl MediaView {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub(crate) fn items(&self) -> &[Item] {
        &self.items
    }

    #[must_use]
    pub(crate) fn item(&self, idx: usize) -> &Item {
        &self.items[idx]
    }

    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Number of loaded items with a star rating (`rating >= 1`).
    #[must_use]
    pub(crate) fn rated_count(&self) -> usize {
        self.rated_count
    }

    /// Number of loaded items that are rejected (`rating == -1`).
    #[must_use]
    pub(crate) fn rejected_count(&self) -> usize {
        self.rejected_count
    }

    #[must_use]
    pub(crate) fn index_of(&self, path: &Path) -> Option<usize> {
        self.item_index.get(path).copied()
    }

    #[must_use]
    pub(crate) fn version(&self) -> u64 {
        self.version
    }

    #[must_use]
    pub(crate) fn sorted_view(&self) -> &BTreeMap<SortKey, usize> {
        &self.sorted_view
    }

    /// Item index → burst key, for the grid (badge toggle target).
    #[must_use]
    pub(crate) fn burst_of(&self) -> &HashMap<usize, DateTime<Utc>> {
        &self.burst_of
    }

    /// Burst key → members. The grid derives the badge count from the length.
    #[must_use]
    pub(crate) fn burst_map(&self) -> &HashMap<DateTime<Utc>, Vec<usize>> {
        &self.burst_map
    }

    /// Number of currently visible (post-filter, post burst-collapse) items.
    #[must_use]
    pub(crate) fn visible_len(&self) -> usize {
        self.sorted_view.len()
    }

    #[must_use]
    pub(crate) fn is_view_empty(&self) -> bool {
        self.sorted_view.is_empty()
    }

    /// Whether `idx` is part of the current visible sequence.
    #[must_use]
    pub(crate) fn is_visible(&self, idx: usize) -> bool {
        self.sort_key_by_idx.contains_key(&idx)
    }

    /// If `idx` is in a *collapsed* burst, the full member set to fan an action
    /// (rating, tagging) out over; otherwise `None`.
    #[must_use]
    pub(crate) fn collapsed_burst_members(
        &self,
        idx: usize,
        group_bursts: bool,
    ) -> Option<&[usize]> {
        if !group_bursts {
            return None;
        }
        let key = self.burst_of.get(&idx)?;
        if self.expanded_bursts.contains(key) {
            return None;
        }
        Some(&self.burst_map[key])
    }

    /// The deduplicated logical unit an action fans out over: `idx` plus its
    /// collapsed-burst members (an expanded burst does not fan out) plus each
    /// member's hidden JPEG sibling.
    #[must_use]
    pub(crate) fn group_of(
        &self,
        idx: usize,
        group_bursts: bool,
        group_raw_jpeg: bool,
    ) -> Vec<usize> {
        let members = self
            .collapsed_burst_members(idx, group_bursts)
            .unwrap_or_else(|| std::slice::from_ref(&idx));

        let mut group: Vec<usize> = Vec::with_capacity(members.len());
        for &member in members {
            if !group.contains(&member) {
                group.push(member);
            }
            if group_raw_jpeg
                && let Some(jpeg) = &self.items[member].jpeg_pair
                && let Some(jpeg_idx) = self.item_index.get(jpeg).copied()
                && !group.contains(&jpeg_idx)
            {
                group.push(jpeg_idx);
            }
        }
        group
    }

    /// First item index in display order.
    #[must_use]
    pub(crate) fn first_index(&self, ascending: bool) -> Option<usize> {
        if ascending {
            self.sorted_view.values().next().copied()
        } else {
            self.sorted_view.values().next_back().copied()
        }
    }

    /// Last item index in display order.
    #[must_use]
    pub(crate) fn last_index(&self, ascending: bool) -> Option<usize> {
        if ascending {
            self.sorted_view.values().next_back().copied()
        } else {
            self.sorted_view.values().next().copied()
        }
    }

    /// Adjacent item in display order, or `None` at a boundary. Falls back to a
    /// computed key when `current` is being viewed but filtered out of the view.
    #[must_use]
    pub(crate) fn adjacent_index(
        &self,
        current: usize,
        forward: bool,
        ascending: bool,
        sort_order: SortOrder,
    ) -> Option<usize> {
        let current_key = self
            .sort_key_by_idx
            .get(&current)
            .cloned()
            .unwrap_or_else(|| SortKey::from_item(&self.items[current], sort_order));

        if forward == ascending {
            self.sorted_view
                .range((Excluded(&current_key), Unbounded))
                .next()
                .map(|(_, &idx)| idx)
        } else {
            self.sorted_view
                .range((Unbounded, Excluded(&current_key)))
                .next_back()
                .map(|(_, &idx)| idx)
        }
    }

    /// Ordinal position (0-based) of `idx` within the display sequence, or
    /// `None` if `idx` is not currently visible (filtered out or collapsed).
    #[must_use]
    pub(crate) fn ordinal_position(&self, idx: usize, ascending: bool) -> Option<usize> {
        let key = self.sort_key_by_idx.get(&idx)?;
        Some(if ascending {
            self.sorted_view.range(..key).count()
        } else {
            self.sorted_view.range((Excluded(key), Unbounded)).count()
        })
    }

    /// Item index at display-order position `ordinal`, or `None` if out of
    /// range. The inverse of [`ordinal_position`](Self::ordinal_position).
    #[must_use]
    pub(crate) fn index_at_ordinal(&self, ordinal: usize, ascending: bool) -> Option<usize> {
        if ascending {
            self.sorted_view.values().nth(ordinal).copied()
        } else {
            self.sorted_view.values().rev().nth(ordinal).copied()
        }
    }

    /// Item indices for the `len` display-order positions starting at `start`.
    /// Used to resolve the grid's scroll-window rows to concrete items.
    #[must_use]
    pub(crate) fn indices_in_ordinal_range(
        &self,
        start: usize,
        len: usize,
        ascending: bool,
    ) -> Vec<usize> {
        if ascending {
            self.sorted_view
                .values()
                .skip(start)
                .take(len)
                .copied()
                .collect()
        } else {
            self.sorted_view
                .values()
                .rev()
                .skip(start)
                .take(len)
                .copied()
                .collect()
        }
    }

    /// Item indices between `a` and `b` in display order, inclusive of both
    /// endpoints, returned in display order. Both endpoints must be visible.
    #[must_use]
    pub(crate) fn indices_between(&self, a: usize, b: usize, ascending: bool) -> Vec<usize> {
        let oa = self
            .ordinal_position(a, ascending)
            .expect("range endpoint not visible");
        let ob = self
            .ordinal_position(b, ascending)
            .expect("range endpoint not visible");
        let (start, end) = if oa <= ob { (oa, ob) } else { (ob, oa) };
        self.indices_in_ordinal_range(start, end - start + 1, ascending)
    }

    /// Adjust the rated/rejected tallies for one item's rating change.
    fn apply_rating_delta(&mut self, prev: i8, next: i8) {
        let (was_rated, was_rejected) = rating_class(prev);
        let (now_rated, now_rejected) = rating_class(next);
        match (was_rated, now_rated) {
            (true, false) => self.rated_count -= 1,
            (false, true) => self.rated_count += 1,
            _ => {}
        }
        match (was_rejected, now_rejected) {
            (true, false) => self.rejected_count -= 1,
            (false, true) => self.rejected_count += 1,
            _ => {}
        }
    }

    /// Apply `mutate` to a single item and reconcile the derived view for it in
    /// place — O(log n) plus the affected burst run, never a full rebuild.
    ///
    /// `mutate` must not change `capture_time` (bursts are keyed on it);
    /// rating/color/ingest changes are all it is used for.
    pub(crate) fn mutate_item(
        &mut self,
        idx: usize,
        params: &ViewParams,
        mutate: impl FnOnce(&mut Item),
    ) {
        let capture_time = self.items[idx].capture_time;
        let was_passing = self.burst_order.contains(&(capture_time, idx));
        let was_shown = self.is_visible(idx);
        let prev_rating = self.items[idx].rating;

        mutate(&mut self.items[idx]);
        debug_assert_eq!(
            capture_time, self.items[idx].capture_time,
            "mutate_item must not change capture_time"
        );
        self.apply_rating_delta(prev_rating, self.items[idx].rating);
        self.version += 1;

        let now_passing = passes(&self.items[idx], params, &self.hidden_jpeg_paths);
        if was_passing == now_passing {
            // Membership and `capture_time` unchanged → bursts untouched; only
            // this item's own sort key can have moved, so reposition it.
            if was_shown {
                self.replace_sort_key(idx, params.sort_order);
            }
        } else if now_passing {
            self.admit(idx, params);
        } else {
            self.retract(idx, params);
        }
    }

    /// Insert a freshly-scanned item, keeping every derived index correct
    /// (including bursts and burst expansion) incrementally.
    ///
    /// The caller guarantees the path is fresh. Selection/focus are
    /// deliberately *not* pruned here: a scan streams items in and must never
    /// wipe a selection the user built. Filter/sort changes go through
    /// [`Self::rebuild`], which does prune.
    pub(crate) fn insert(&mut self, item: Item, params: &ViewParams) {
        debug_assert!(
            !self.item_index.contains_key(&item.path),
            "insert() requires a fresh path; the caller must dedup"
        );

        let idx = self.items.len();
        let jpeg_pair = item.jpeg_pair.clone();
        let path = item.path.clone();
        let (rated, rejected) = rating_class(item.rating);
        self.rated_count += usize::from(rated);
        self.rejected_count += usize::from(rejected);
        self.items.push(item);
        self.item_index.insert(path, idx);
        self.version += 1;

        // A RAW hides its JPEG sibling. If that sibling was already scanned and
        // is currently shown, retract it now that grouping hides it.
        if params.group_raw_jpeg
            && let Some(jpeg) = jpeg_pair
            && !self.hidden_jpeg_paths.contains_key(&jpeg)
        {
            let sibling = self.item_index.get(&jpeg).copied();
            self.hidden_jpeg_paths.insert(jpeg, idx);
            if let Some(jidx) = sibling {
                let jt = self.items[jidx].capture_time;
                if self.burst_order.contains(&(jt, jidx))
                    && !passes(&self.items[jidx], params, &self.hidden_jpeg_paths)
                {
                    self.retract(jidx, params);
                }
            }
        }

        if passes(&self.items[idx], params, &self.hidden_jpeg_paths) {
            self.admit(idx, params);
        }
    }

    /// Rebuild every derived index from scratch for the given view params.
    ///
    /// Selection/focus are not touched here — call [`Self::prune_hidden`]
    /// afterwards to reconcile them against the new visible set.
    pub(crate) fn rebuild(&mut self, params: &ViewParams) {
        self.version += 1;
        self.sorted_view.clear();
        self.sort_key_by_idx.clear();
        self.burst_order.clear();
        self.burst_map.clear();
        self.burst_of.clear();

        // Only the JPEG-grouping path consults this map; skip the clones when off.
        self.hidden_jpeg_paths = if params.group_raw_jpeg {
            self.items
                .iter()
                .enumerate()
                .filter_map(|(idx, item)| item.jpeg_pair.clone().map(|jpeg| (jpeg, idx)))
                .collect()
        } else {
            HashMap::new()
        };

        let passing: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| passes(item, params, &self.hidden_jpeg_paths))
            .map(|(idx, _)| idx)
            .collect();

        for &idx in &passing {
            self.burst_order.insert((self.items[idx].capture_time, idx));
        }

        if params.group_bursts {
            // `burst_order` already yields capture-time order — no separate sort.
            let ordered: Vec<usize> = self.burst_order.iter().map(|&(_, idx)| idx).collect();
            for burst in self.group_bursts_in(&ordered) {
                self.register_burst(burst);
            }
            // Drop expanded keys that no longer name a live burst.
            self.expanded_bursts
                .retain(|key| self.burst_map.contains_key(key));
        } else {
            self.expanded_bursts.clear();
        }

        for &idx in &passing {
            if self.should_show(idx, params) {
                self.set_shown(idx, true, params.sort_order);
            }
        }
    }

    /// Whether burst `key` is currently expanded.
    #[must_use]
    pub(crate) fn is_burst_expanded(&self, key: DateTime<Utc>) -> bool {
        self.expanded_bursts.contains(&key)
    }

    /// Toggle whether a burst is expanded, updating only its members'
    /// visibility. A no-op when `key` no longer names a live burst.
    pub(crate) fn toggle_burst_expansion(&mut self, key: DateTime<Utc>, params: &ViewParams) {
        let expanded = !self.expanded_bursts.contains(&key);
        self.set_burst_expansion(key, expanded, params);
    }

    /// Set whether burst `key` is expanded to an explicit state, updating only
    /// its members' visibility. Returns `false` without mutating anything when
    /// `key` no longer names a live burst — an undo/redo replaying a toggle of a
    /// burst a later rebuild has since dissolved or re-keyed.
    pub(crate) fn set_burst_expansion(
        &mut self,
        key: DateTime<Utc>,
        expanded: bool,
        params: &ViewParams,
    ) -> bool {
        if !self.burst_map.contains_key(&key) {
            return false;
        }
        if expanded {
            self.expanded_bursts.insert(key);
        } else {
            self.expanded_bursts.remove(&key);
        }
        self.version += 1;
        for m in self.burst_map[&key].clone() {
            let show = self.should_show(m, params);
            if show != self.is_visible(m) {
                self.set_shown(m, show, params.sort_order);
            }
        }
        true
    }

    /// Prune `selected` and `focused` against the current visible set, reporting
    /// what was removed.
    ///
    /// A selected item that is *invisible* is nonetheless kept when it is a
    /// group member whose visible representative is still selected — a collapsed
    /// burst's first member, or a hidden JPEG's RAW sibling — so a deliberate
    /// selection of a whole pair or burst ingests together. Focus has no group
    /// semantics, so it is pruned on plain visibility.
    pub(crate) fn prune_hidden(
        &self,
        selected: &mut BTreeSet<usize>,
        focused: &mut Option<usize>,
    ) -> RebuildOutcome {
        let before = selected.len();
        // The potential representatives are exactly the selected *visible* items
        // (a representative is visible+selected by definition). A hidden member
        // survives iff its representative is one of them.
        let anchors: BTreeSet<usize> = selected
            .iter()
            .copied()
            .filter(|&idx| self.is_visible(idx))
            .collect();
        selected.retain(|&idx| {
            self.is_visible(idx)
                || self
                    .group_representative(idx)
                    .is_some_and(|rep| anchors.contains(&rep))
        });
        let selection_pruned = before - selected.len();

        let focused_lost = focused.is_some_and(|idx| !self.is_visible(idx));
        *focused = focused.filter(|idx| self.is_visible(*idx));

        RebuildOutcome {
            focused_lost,
            selection_pruned,
        }
    }

    /// The visible representative of a possibly-hidden group member: a collapsed
    /// burst's first member, or a hidden JPEG's owning RAW. `None` if `idx` is
    /// not a hidden group member.
    fn group_representative(&self, idx: usize) -> Option<usize> {
        if let Some(key) = self.burst_of.get(&idx) {
            return self.burst_map[key].first().copied();
        }
        self.hidden_jpeg_paths.get(&self.items[idx].path).copied()
    }

    /// Admit a passing item into the view, maintaining bursts.
    fn admit(&mut self, idx: usize, params: &ViewParams) {
        self.burst_order.insert((self.items[idx].capture_time, idx));

        if !params.group_bursts {
            self.set_shown(idx, true, params.sort_order);
            return;
        }

        let region = self.run_members_containing(idx);
        self.resync_region(&region, params);
    }

    /// Retract an item that no longer passes filters, maintaining bursts.
    fn retract(&mut self, idx: usize, params: &ViewParams) {
        let key = (self.items[idx].capture_time, idx);

        if !params.group_bursts {
            self.burst_order.remove(&key);
            self.set_shown(idx, false, params.sort_order);
            return;
        }

        // The old run still contains `idx`; capture it, then drop `idx` and
        // regroup the rest — removal can split or dissolve a burst.
        let region = self.run_members_containing(idx);
        self.burst_order.remove(&key);
        self.burst_of.remove(&idx);
        self.set_shown(idx, false, params.sort_order);
        let remaining: Vec<usize> = region.into_iter().filter(|&i| i != idx).collect();
        self.resync_region(&remaining, params);
    }

    /// Recompute burst grouping + visibility for one affected time region
    /// (capture-time ordered, all currently in `burst_order`). Diff-based: only
    /// members whose burst key or visibility actually changed are written, and
    /// burst expansion is migrated across any re-keying. `group_bursts` on.
    fn resync_region(&mut self, region: &[usize], params: &ViewParams) {
        let old_keys: BTreeSet<DateTime<Utc>> = region
            .iter()
            .filter_map(|i| self.burst_of.get(i).copied())
            .collect();
        let region_was_expanded = old_keys.iter().any(|k| self.expanded_bursts.contains(k));

        let new_bursts = self.group_bursts_in(region);
        let mut new_key_of: HashMap<usize, DateTime<Utc>> = HashMap::new();
        let mut new_keys: BTreeSet<DateTime<Utc>> = BTreeSet::new();
        for burst in &new_bursts {
            let key = self.items[burst[0]].capture_time.second;
            new_keys.insert(key);
            for &m in burst {
                new_key_of.insert(m, key);
            }
        }

        // Update `burst_of` only for members whose key changed.
        for &m in region {
            let old = self.burst_of.get(&m).copied();
            let new = new_key_of.get(&m).copied();
            if old != new {
                match new {
                    Some(k) => self.burst_of.insert(m, k),
                    None => self.burst_of.remove(&m),
                };
            }
        }

        // Update `burst_map`: drop vanished bursts, (re)insert the current ones.
        for key in &old_keys {
            if !new_keys.contains(key) {
                self.burst_map.remove(key);
            }
        }
        for burst in new_bursts {
            let key = self.items[burst[0]].capture_time.second;
            self.burst_map.insert(key, burst);
        }

        // Migrate expansion: drop keys that vanished; carry a still-open cluster
        // onto its new key(s).
        for key in &old_keys {
            if !new_keys.contains(key) {
                self.expanded_bursts.remove(key);
            }
        }
        if region_was_expanded {
            self.expanded_bursts.extend(new_keys.iter().copied());
        }

        // Flip only the members whose visibility actually changed.
        for &m in region {
            let show = self.should_show(m, params);
            if show != self.is_visible(m) {
                self.set_shown(m, show, params.sort_order);
            }
        }
    }

    /// The maximal run of passing items connected to `idx` by ≤threshold gaps,
    /// in capture-time order. `idx` must already be in `burst_order`.
    ///
    /// Walks outward with a single directional iterator per side, stopping at
    /// the first over-threshold gap: O(run length + log n).
    fn run_members_containing(&self, idx: usize) -> Vec<usize> {
        let anchor = (self.items[idx].capture_time, idx);

        let mut start = anchor;
        for &prev in self.burst_order.range(..anchor).rev() {
            if prev.0.within_burst_threshold(&start.0) {
                start = prev;
            } else {
                break;
            }
        }

        let mut end = anchor;
        for &next in self.burst_order.range((Excluded(anchor), Unbounded)) {
            if end.0.within_burst_threshold(&next.0) {
                end = next;
            } else {
                break;
            }
        }

        self.burst_order
            .range(start..=end)
            .map(|&(_, i)| i)
            .collect()
    }

    /// Set an item's presence in the sorted view. The sort key is stable across
    /// burst recomputation (item fields don't change), so a currently-shown item
    /// keeps its key.
    fn set_shown(&mut self, idx: usize, show: bool, sort_order: SortOrder) {
        let currently = self.sort_key_by_idx.contains_key(&idx);
        if show {
            if !currently {
                let key = SortKey::from_item(&self.items[idx], sort_order);
                self.sorted_view.insert(key.clone(), idx);
                self.sort_key_by_idx.insert(idx, key);
            }
        } else if let Some(key) = self.sort_key_by_idx.remove(&idx) {
            self.sorted_view.remove(&key);
        }
    }

    /// Re-position a shown item after a field change moved its sort key.
    fn replace_sort_key(&mut self, idx: usize, sort_order: SortOrder) {
        if let Some(old) = self.sort_key_by_idx.remove(&idx) {
            self.sorted_view.remove(&old);
        }
        let key = SortKey::from_item(&self.items[idx], sort_order);
        self.sorted_view.insert(key.clone(), idx);
        self.sort_key_by_idx.insert(idx, key);
    }

    /// Split a capture-time-ordered slice of passing indices into burst runs:
    /// maximal ≤threshold chains of length ≥3 (per `CONTEXT.md`).
    fn group_bursts_in(&self, ordered: &[usize]) -> Vec<Vec<usize>> {
        let mut bursts: Vec<Vec<usize>> = Vec::new();
        let mut run: Vec<usize> = Vec::new();
        let mut prev: Option<CaptureTime> = None;
        for &idx in ordered {
            let capture_time = self.items[idx].capture_time;
            if let Some(p) = prev
                && !p.within_burst_threshold(&capture_time)
            {
                if run.len() >= 3 {
                    bursts.push(std::mem::take(&mut run));
                } else {
                    run.clear();
                }
            }
            run.push(idx);
            prev = Some(capture_time);
        }
        if run.len() >= 3 {
            bursts.push(run);
        }
        bursts
    }

    /// Record a burst (≥3 members) in `burst_map` and `burst_of`.
    fn register_burst(&mut self, burst: Vec<usize>) {
        let key = self.items[burst[0]].capture_time.second;
        for &member in &burst {
            self.burst_of.insert(member, key);
        }
        self.burst_map.insert(key, burst);
    }

    /// Whether an item should appear in the grid (collapsed bursts show only
    /// their first member). Assumes `idx` passes filters.
    fn should_show(&self, idx: usize, params: &ViewParams) -> bool {
        if !params.group_bursts {
            return true;
        }
        let Some(key) = self.burst_of.get(&idx) else {
            return true;
        };
        if self.expanded_bursts.contains(key) {
            return true;
        }
        self.burst_map[key].first() == Some(&idx)
    }
}

/// Whether an item passes the current filters. Free function so it borrows only
/// the hidden set, not all of `MediaView`, letting callers hold `&mut self`.
fn passes(item: &Item, params: &ViewParams, hidden_jpeg_paths: &HashMap<PathBuf, usize>) -> bool {
    let from_source = params
        .selected_sources
        .iter()
        .any(|s| item.path.starts_with(s));
    let passes_mode = params.filter_mode.matches(item);
    let not_rejected = !(params.hide_rejected && item.rating == -1);
    let not_hidden = !params.group_raw_jpeg || !hidden_jpeg_paths.contains_key(&item.path);
    let passes_date = matches_date_filter(item, params.selected_dates);
    let passes_rating =
        params.selected_ratings.is_empty() || params.selected_ratings.contains(&item.rating);
    let passes_color = params.selected_color_labels.is_empty()
        || params.selected_color_labels.contains(&item.color_label);

    from_source
        && passes_mode
        && not_rejected
        && not_hidden
        && passes_date
        && passes_rating
        && passes_color
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "unwrapping known-good fixtures keeps test intent readable"
)]
mod tests {
    use std::path::{Path, PathBuf};

    use chrono::{DateTime, TimeZone, Utc};
    use ferrocull_core::{
        FileCategory,
        media::{CaptureTime, Item, SortOrder},
    };

    use super::{MediaView, ViewParams};

    fn base_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0).unwrap()
    }

    /// An item captured `secs` seconds (+`subsec_nanos`) after `base_time`,
    /// living under `/src` so the default source filter admits it.
    fn item_at(name: &str, secs: i64, subsec_nanos: u32) -> Item {
        let second = base_time() + chrono::Duration::seconds(secs);
        Item {
            path: PathBuf::from(format!("/src/{name}")),
            source_id: name.to_owned(),
            media_type: FileCategory::Raw,
            capture_time: CaptureTime::new(second, subsec_nanos),
            is_ingested: false,
            jpeg_pair: None,
            paired: Vec::new(),
            sidecars: Vec::new(),
            xmp_sidecar: None,
            rating: 0,
            color_label: None,
        }
    }

    /// Owns the `BTreeSet`s so a `ViewParams` can borrow them.
    struct Params {
        sources: std::collections::BTreeSet<PathBuf>,
        ratings: std::collections::BTreeSet<i8>,
        colors: std::collections::BTreeSet<Option<ferrocull_core::ColorLabel>>,
        sort_order: SortOrder,
        group_bursts: bool,
        group_raw_jpeg: bool,
    }

    impl Params {
        fn new() -> Self {
            let mut sources = std::collections::BTreeSet::new();
            sources.insert(PathBuf::from("/src"));
            Self {
                sources,
                ratings: std::collections::BTreeSet::new(),
                colors: std::collections::BTreeSet::new(),
                sort_order: SortOrder::Time,
                group_bursts: true,
                group_raw_jpeg: true,
            }
        }

        fn view(&self) -> ViewParams<'_> {
            ViewParams {
                sort_order: self.sort_order,
                filter_mode: ferrocull_core::media::FilterMode::All,
                hide_rejected: false,
                group_raw_jpeg: self.group_raw_jpeg,
                group_bursts: self.group_bursts,
                selected_sources: &self.sources,
                selected_dates: None,
                selected_ratings: &self.ratings,
                selected_color_labels: &self.colors,
            }
        }
    }

    /// Build a view by inserting `items` in the given order (incremental path).
    fn build_incremental(items: &[Item], params: &Params) -> MediaView {
        let mut view = MediaView::new();
        for item in items {
            view.insert(item.clone(), &params.view());
        }
        view
    }

    /// Build a view by appending `items` then doing one full rebuild.
    fn build_rebuilt(items: &[Item], params: &Params) -> MediaView {
        let mut view = MediaView::new();
        for item in items {
            let idx = view.items.len();
            view.items.push(item.clone());
            view.item_index.insert(item.path.clone(), idx);
        }
        view.rebuild(&params.view());
        view
    }

    /// Assert two views agree on every derived index (ignoring `version`,
    /// which legitimately differs by construction path).
    fn assert_same_derived_state(a: &MediaView, b: &MediaView) {
        assert_eq!(a.sorted_view, b.sorted_view, "sorted_view differs");
        assert_eq!(
            a.sort_key_by_idx, b.sort_key_by_idx,
            "sort_key_by_idx differs"
        );
        assert_eq!(a.burst_order, b.burst_order, "burst_order differs");
        assert_eq!(a.burst_of, b.burst_of, "burst_of differs");
        assert_eq!(a.burst_map, b.burst_map, "burst_map differs");
        assert_eq!(
            a.expanded_bursts, b.expanded_bursts,
            "expanded_bursts differs"
        );
        assert_eq!(
            a.hidden_jpeg_paths, b.hidden_jpeg_paths,
            "hidden_jpeg_paths differs"
        );
    }

    fn visible_names(view: &MediaView) -> Vec<String> {
        view.sorted_view
            .values()
            .map(|&idx| {
                view.items[idx]
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    #[test]
    fn insertion_keeps_sorted_order_by_capture_time() {
        let params = Params::new();
        let items = [
            item_at("c.raw", 30, 0),
            item_at("a.raw", 10, 0),
            item_at("b.raw", 20, 0),
        ];
        let view = build_incremental(&items, &params);
        assert_eq!(visible_names(&view), ["a.raw", "b.raw", "c.raw"]);
    }

    #[test]
    fn three_shots_within_one_second_form_a_burst() {
        let params = Params::new();
        let items = [
            item_at("a.raw", 0, 0),
            item_at("b.raw", 0, 400_000_000),
            item_at("c.raw", 0, 800_000_000),
        ];
        let view = build_incremental(&items, &params);
        assert_eq!(view.burst_map.len(), 1, "expected exactly one burst");
        assert_eq!(view.burst_map.values().next().unwrap().len(), 3);
        assert_eq!(
            visible_names(&view),
            ["a.raw"],
            "collapsed burst shows first only"
        );
    }

    #[test]
    fn two_shots_do_not_form_a_burst() {
        let params = Params::new();
        let items = [item_at("a.raw", 0, 0), item_at("b.raw", 0, 500_000_000)];
        let view = build_incremental(&items, &params);
        assert!(view.burst_map.is_empty(), "two shots are not a burst");
        assert_eq!(visible_names(&view), ["a.raw", "b.raw"]);
    }

    #[test]
    fn gap_over_one_second_breaks_the_burst() {
        let params = Params::new();
        let items = [
            item_at("a.raw", 0, 0),
            item_at("b.raw", 0, 400_000_000),
            item_at("c.raw", 0, 800_000_000),
            item_at("d.raw", 3, 0),
        ];
        let view = build_incremental(&items, &params);
        assert_eq!(view.burst_map.len(), 1);
        assert_eq!(visible_names(&view), ["a.raw", "d.raw"]);
    }

    #[test]
    fn inserting_a_bridge_merges_two_runs_into_one_burst() {
        let params = Params::new();
        let a = item_at("a.raw", 0, 0);
        let b = item_at("b.raw", 0, 500_000_000);
        let c = item_at("c.raw", 2, 0);
        let d = item_at("d.raw", 2, 500_000_000);
        let bridge = item_at("bridge.raw", 1, 250_000_000);
        let items = [a, b, c, d, bridge];
        let view = build_incremental(&items, &params);
        assert_eq!(
            view.burst_map.len(),
            1,
            "bridge should merge into one burst"
        );
        assert_eq!(view.burst_map.values().next().unwrap().len(), 5);
        assert_eq!(visible_names(&view), ["a.raw"]);
    }

    #[test]
    fn incremental_matches_rebuild_for_many_orderings() {
        let pool = [
            item_at("s1.raw", 0, 0),
            item_at("burst1.raw", 10, 0),
            item_at("burst2.raw", 10, 300_000_000),
            item_at("burst3.raw", 10, 600_000_000),
            item_at("burst4.raw", 10, 900_000_000),
            item_at("s2.raw", 40, 0),
            item_at("pairA.raw", 60, 0),
            item_at("pairB.raw", 60, 400_000_000),
            item_at("s3.raw", 200, 0),
        ];
        let params = Params::new();

        let n = pool.len();
        for seed in 0u64..64 {
            let mut order: Vec<usize> = (0..n).collect();
            let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            for i in (1..n).rev() {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let j = (state >> 33) as usize % (i + 1);
                order.swap(i, j);
            }
            let shuffled: Vec<Item> = order.iter().map(|&i| pool[i].clone()).collect();

            let incremental = build_incremental(&shuffled, &params);
            let rebuilt = build_rebuilt(&shuffled, &params);
            assert_same_derived_state(&incremental, &rebuilt);
        }
    }

    #[test]
    fn incremental_matches_rebuild_with_raw_jpeg_pairs() {
        let params = Params::new();
        let mut raw = item_at("photo.raw", 5, 0);
        raw.jpeg_pair = Some(PathBuf::from("/src/photo.jpg"));
        let jpeg = item_at("photo.jpg", 5, 0);
        let other = item_at("other.raw", 50, 0);

        // JPEG arrives before its RAW → RAW must retract the shown JPEG.
        let jpeg_first = [jpeg.clone(), other.clone(), raw.clone()];
        let inc = build_incremental(&jpeg_first, &params);
        let reb = build_rebuilt(&jpeg_first, &params);
        assert_same_derived_state(&inc, &reb);
        assert_eq!(
            visible_names(&inc),
            ["photo.raw", "other.raw"],
            "jpeg sibling must be hidden, raw kept"
        );

        // RAW arrives first → JPEG never admitted.
        let raw_first = [raw, other, jpeg];
        let inc2 = build_incremental(&raw_first, &params);
        let reb2 = build_rebuilt(&raw_first, &params);
        assert_same_derived_state(&inc2, &reb2);
    }

    #[test]
    fn insert_does_not_hide_existing_group_members() {
        // A selected hidden JPEG sibling and collapsed-burst members must survive
        // an unrelated insert: `insert` never prunes, and only the new item's own
        // region changes. (The caller's selection set is therefore never wiped.)
        let params = Params::new();
        let mut raw = item_at("photo.raw", 5, 0);
        raw.jpeg_pair = Some(PathBuf::from("/src/photo.jpg"));
        let mut view = MediaView::new();
        view.insert(raw, &params.view());
        view.insert(item_at("photo.jpg", 5, 0), &params.view());
        // burst b,c,d collapsed → c,d hidden.
        view.insert(item_at("b.raw", 100, 0), &params.view());
        view.insert(item_at("c.raw", 100, 300_000_000), &params.view());
        view.insert(item_at("d.raw", 100, 600_000_000), &params.view());

        let jpeg_idx = view.index_of(Path::new("/src/photo.jpg")).unwrap();
        let c_idx = view.index_of(Path::new("/src/c.raw")).unwrap();
        assert!(!view.is_visible(jpeg_idx), "jpeg starts hidden");
        assert!(
            !view.is_visible(c_idx),
            "collapsed burst member starts hidden"
        );

        // Stream in an unrelated far-away item.
        view.insert(item_at("far.raw", 5000, 0), &params.view());

        // The pre-existing hidden members are untouched.
        assert!(!view.is_visible(jpeg_idx), "jpeg still hidden, not pruned");
        assert!(!view.is_visible(c_idx), "burst member still hidden");
        assert!(
            view.is_visible(view.index_of(Path::new("/src/far.raw")).unwrap()),
            "the new item is visible"
        );
    }

    #[test]
    fn re_keyed_burst_keeps_expansion() {
        // Expand a burst, then insert an earlier frame that joins it as the new
        // first member (re-keying the burst). The expansion must migrate to the
        // new key and the stale key must be dropped.
        let params = Params::new();
        let mut view = MediaView::new();
        for it in [
            item_at("b.raw", 10, 100_000_000),
            item_at("c.raw", 10, 400_000_000),
            item_at("d.raw", 10, 700_000_000),
        ] {
            view.insert(it, &params.view());
        }
        let old_key = *view.burst_map.keys().next().unwrap();
        view.toggle_burst_expansion(old_key, &params.view());
        assert!(view.expanded_bursts.contains(&old_key));
        assert_eq!(visible_names(&view).len(), 3, "expanded → all shown");

        // Earlier frame within 1s of b → joins as new first, re-keying the burst.
        view.insert(item_at("a.raw", 9, 800_000_000), &params.view());
        let new_key = base_time() + chrono::Duration::seconds(9);
        assert_ne!(new_key, old_key);
        assert!(
            view.expanded_bursts.contains(&new_key),
            "expansion carried to the new key"
        );
        assert!(
            !view.expanded_bursts.contains(&old_key),
            "stale key dropped"
        );
        assert_eq!(visible_names(&view).len(), 4, "still expanded → 4 shown");
    }

    #[test]
    fn mutate_item_incremental_matches_full_rebuild() {
        // A rating change that a rating filter excludes: the mutated item must
        // leave the view and its burst dissolve, matching a full rebuild.
        let mut params = Params::new();
        params.ratings.insert(0); // only unrated (0) pass
        let items = [
            item_at("s.raw", 0, 0),
            item_at("b.raw", 10, 0),
            item_at("b2.raw", 10, 300_000_000),
            item_at("b3.raw", 10, 600_000_000),
        ];
        let mut inc = build_incremental(&items, &params);
        // b3 (index 3) becomes 5-star → filtered out → burst of 3 becomes 2.
        inc.mutate_item(3, &params.view(), |item| item.rating = 5);

        let mut mutated = items.to_vec();
        mutated[3].rating = 5;
        let reb = build_rebuilt(&mutated, &params);
        assert_same_derived_state(&inc, &reb);
    }

    #[test]
    fn mutate_item_reorders_under_rating_sort() {
        // Under rating sort, raising an item's rating moves it earlier.
        let mut params = Params::new();
        params.sort_order = SortOrder::Rating;
        let items = [
            item_at("a.raw", 0, 0),
            item_at("b.raw", 10, 0),
            item_at("c.raw", 20, 0),
        ];
        let mut inc = build_incremental(&items, &params);
        inc.mutate_item(2, &params.view(), |item| item.rating = 5); // c → 5 stars

        let mut mutated = items.to_vec();
        mutated[2].rating = 5;
        let reb = build_rebuilt(&mutated, &params);
        assert_same_derived_state(&inc, &reb);
        // Rating sort: 5-star c first, then unrated by time.
        assert_eq!(visible_names(&inc), ["c.raw", "a.raw", "b.raw"]);
    }

    #[test]
    fn mutating_expanded_burst_member_keeps_expansion() {
        // Rating a member of an expanded burst (no filter change) must not
        // collapse it.
        let params = Params::new();
        let items = [
            item_at("a.raw", 0, 0),
            item_at("a2.raw", 0, 300_000_000),
            item_at("a3.raw", 0, 600_000_000),
        ];
        let mut view = build_incremental(&items, &params);
        let key = *view.burst_map.keys().next().unwrap();
        view.toggle_burst_expansion(key, &params.view());
        assert_eq!(visible_names(&view).len(), 3, "expanded");

        view.mutate_item(1, &params.view(), |item| item.rating = 4);

        assert!(
            view.expanded_bursts.contains(&key),
            "expansion survives rating a member"
        );
        assert_eq!(visible_names(&view).len(), 3, "still all shown");
    }

    #[test]
    fn prune_keeps_selected_hidden_group_members_of_visible_representative() {
        // A deliberate selection of a whole collapsed burst / a RAW+JPEG pair
        // must survive a prune — the hidden members ride along with their
        // selected+visible representative.
        let params = Params::new();
        let mut raw = item_at("photo.raw", 5, 0);
        raw.jpeg_pair = Some(PathBuf::from("/src/photo.jpg"));
        let mut view = MediaView::new();
        view.insert(raw, &params.view());
        view.insert(item_at("photo.jpg", 5, 0), &params.view());
        // Collapsed burst b,c,d (first = b visible, c/d hidden).
        for it in [
            item_at("b.raw", 100, 0),
            item_at("c.raw", 100, 300_000_000),
            item_at("d.raw", 100, 600_000_000),
        ] {
            view.insert(it, &params.view());
        }
        let raw_i = view.index_of(Path::new("/src/photo.raw")).unwrap();
        let jpeg_i = view.index_of(Path::new("/src/photo.jpg")).unwrap();
        let b = view.index_of(Path::new("/src/b.raw")).unwrap();
        let c = view.index_of(Path::new("/src/c.raw")).unwrap();
        let d = view.index_of(Path::new("/src/d.raw")).unwrap();

        // Select the RAW (+hidden JPEG) and the whole burst (first + hidden).
        let mut selected: std::collections::BTreeSet<usize> =
            [raw_i, jpeg_i, b, c, d].into_iter().collect();
        let mut focused = None;
        let outcome = view.prune_hidden(&mut selected, &mut focused);
        assert_eq!(
            outcome.selection_pruned, 0,
            "hidden JPEG and burst members ride along with their representatives"
        );
        assert_eq!(selected, [raw_i, jpeg_i, b, c, d].into_iter().collect());

        // Without the visible representative selected, the hidden members prune.
        let mut orphaned: std::collections::BTreeSet<usize> = [jpeg_i, c, d].into_iter().collect();
        let o2 = view.prune_hidden(&mut orphaned, &mut focused);
        assert_eq!(
            o2.selection_pruned, 3,
            "orphaned hidden members are dropped"
        );
        assert!(orphaned.is_empty());
    }

    #[test]
    fn ordinal_position_is_none_for_hidden_item() {
        // A filtered-out / collapsed item has no ordinal, reported explicitly
        // rather than as a misleading 0.
        let params = Params::new();
        let items = [
            item_at("a.raw", 0, 0),
            item_at("a2.raw", 0, 300_000_000),
            item_at("a3.raw", 0, 600_000_000),
        ];
        let view = build_incremental(&items, &params);
        assert_eq!(
            view.ordinal_position(0, true),
            Some(0),
            "visible first member"
        );
        assert_eq!(
            view.ordinal_position(1, true),
            None,
            "collapsed burst member has no ordinal position"
        );
    }

    #[test]
    fn indices_between_covers_the_inclusive_display_range_in_either_direction() {
        let params = Params::new();
        let items = [
            item_at("a.raw", 10, 0),
            item_at("b.raw", 20, 0),
            item_at("c.raw", 30, 0),
            item_at("d.raw", 40, 0),
        ];
        let view = build_incremental(&items, &params);
        let idx = |name: &str| view.index_of(Path::new(name)).unwrap();
        let (a, b, c, d) = (
            idx("/src/a.raw"),
            idx("/src/b.raw"),
            idx("/src/c.raw"),
            idx("/src/d.raw"),
        );

        // Anchor before target and after target yield the same inclusive range.
        assert_eq!(view.indices_between(b, d, true), vec![b, c, d]);
        assert_eq!(view.indices_between(d, b, true), vec![b, c, d]);
        // Single item range.
        assert_eq!(view.indices_between(c, c, true), vec![c]);
        // Descending display order returns the range in display order.
        assert_eq!(view.indices_between(d, b, false), vec![d, c, b]);
        assert_eq!(view.indices_between(a, a, false), vec![a]);
    }

    #[test]
    fn version_bumps_on_every_mutation() {
        let params = Params::new();
        let mut view = MediaView::new();
        let v0 = view.version();

        view.insert(item_at("a.raw", 0, 0), &params.view());
        let v1 = view.version();
        assert!(v1 > v0, "insert must bump version");

        view.rebuild(&params.view());
        let v2 = view.version();
        assert!(v2 > v1, "rebuild must bump version");

        view.mutate_item(0, &params.view(), |item| item.rating = 5);
        let v3 = view.version();
        assert!(v3 > v2, "mutate_item must bump version");
    }

    #[test]
    #[should_panic(expected = "fresh path")]
    fn insert_requires_a_fresh_path() {
        let params = Params::new();
        let mut view = MediaView::new();
        view.insert(item_at("a.raw", 0, 0), &params.view());
        view.insert(item_at("a.raw", 0, 0), &params.view());
    }

    #[test]
    fn prune_hidden_drops_invisible_selection_and_focus() {
        let params = Params::new();
        let items = [
            item_at("a.raw", 0, 0),
            item_at("b.raw", 100, 0),
            item_at("c.raw", 100, 300_000_000),
            item_at("d.raw", 100, 600_000_000),
        ];
        let view = build_incremental(&items, &params);

        // a=0, b=1, c=2, d=3. b,c,d are a collapsed burst → only b shows.
        let mut selected: std::collections::BTreeSet<usize> = [0, 2, 3].into_iter().collect();
        let mut focused = Some(3usize);
        let outcome = view.prune_hidden(&mut selected, &mut focused);

        assert_eq!(outcome.selection_pruned, 2, "c and d were hidden");
        assert!(outcome.focused_lost, "focused d was hidden");
        assert_eq!(selected, [0].into_iter().collect());
        assert_eq!(focused, None);
    }

    /// Sort `group_of`'s output so tests can compare as a set (order is an
    /// internal detail — callers treat the group as a set).
    fn sorted_group(view: &MediaView, idx: usize) -> Vec<usize> {
        let mut g = view.group_of(idx, true, true);
        g.sort_unstable();
        g
    }

    #[test]
    fn group_of_plain_item_is_itself() {
        let params = Params::new();
        let items = [item_at("a.raw", 0, 0), item_at("b.raw", 100, 0)];
        let view = build_incremental(&items, &params);
        let a = view.index_of(Path::new("/src/a.raw")).unwrap();
        assert_eq!(view.group_of(a, true, true), vec![a], "lone item = itself");
    }

    #[test]
    fn group_of_plain_item_includes_its_jpeg_pair() {
        let params = Params::new();
        let mut raw = item_at("photo.raw", 0, 0);
        raw.jpeg_pair = Some(PathBuf::from("/src/photo.jpg"));
        let items = [raw, item_at("photo.jpg", 0, 0)];
        let view = build_incremental(&items, &params);
        let raw_i = view.index_of(Path::new("/src/photo.raw")).unwrap();
        let jpeg_i = view.index_of(Path::new("/src/photo.jpg")).unwrap();
        assert_eq!(
            sorted_group(&view, raw_i),
            {
                let mut e = vec![raw_i, jpeg_i];
                e.sort_unstable();
                e
            },
            "a RAW fans out to its hidden JPEG sibling"
        );
    }

    #[test]
    fn group_of_collapsed_burst_member_fans_to_all_members_and_pairs() {
        let params = Params::new();
        // Three RAWs in a collapsed burst, each with a JPEG sibling.
        let mut a = item_at("a.raw", 0, 0);
        a.jpeg_pair = Some(PathBuf::from("/src/a.jpg"));
        let mut b = item_at("b.raw", 0, 300_000_000);
        b.jpeg_pair = Some(PathBuf::from("/src/b.jpg"));
        let mut c = item_at("c.raw", 0, 600_000_000);
        c.jpeg_pair = Some(PathBuf::from("/src/c.jpg"));
        let items = [
            a,
            b,
            c,
            item_at("a.jpg", 0, 0),
            item_at("b.jpg", 0, 300_000_000),
            item_at("c.jpg", 0, 600_000_000),
        ];
        let view = build_incremental(&items, &params);
        let idx = |name| view.index_of(Path::new(name)).unwrap();
        let expected = {
            let mut e = vec![
                idx("/src/a.raw"),
                idx("/src/b.raw"),
                idx("/src/c.raw"),
                idx("/src/a.jpg"),
                idx("/src/b.jpg"),
                idx("/src/c.jpg"),
            ];
            e.sort_unstable();
            e
        };
        // Fanning from any member yields the same whole logical unit.
        assert_eq!(sorted_group(&view, idx("/src/a.raw")), expected);
        assert_eq!(sorted_group(&view, idx("/src/b.raw")), expected);
    }

    #[test]
    fn group_of_expanded_burst_does_not_fan_out() {
        let params = Params::new();
        let mut a = item_at("a.raw", 0, 0);
        a.jpeg_pair = Some(PathBuf::from("/src/a.jpg"));
        let items = [
            a,
            item_at("b.raw", 0, 300_000_000),
            item_at("c.raw", 0, 600_000_000),
            item_at("a.jpg", 0, 0),
        ];
        let mut view = build_incremental(&items, &params);
        let key = *view.burst_map.keys().next().unwrap();
        view.toggle_burst_expansion(key, &params.view());

        let a_i = view.index_of(Path::new("/src/a.raw")).unwrap();
        let a_jpeg = view.index_of(Path::new("/src/a.jpg")).unwrap();
        let b_i = view.index_of(Path::new("/src/b.raw")).unwrap();
        // Expanded: no fan-out to burst siblings, but the item's own JPEG rides along.
        assert_eq!(
            sorted_group(&view, a_i),
            {
                let mut e = vec![a_i, a_jpeg];
                e.sort_unstable();
                e
            },
            "expanded burst member = itself + own pair only"
        );
        assert_eq!(
            view.group_of(b_i, true, true),
            vec![b_i],
            "expanded burst member with no pair = itself"
        );
    }

    #[test]
    fn group_of_dedups_when_burst_members_share_a_jpeg_sibling() {
        // Two RAWs with the same stem but different extensions (e.g. dual-format
        // capture) both pair the same `photo.jpg`. Both are burst members, so
        // fanning out would add the shared JPEG twice — the group must dedup it.
        let params = Params::new();
        let mut cr2 = item_at("photo.cr2", 0, 0);
        cr2.jpeg_pair = Some(PathBuf::from("/src/photo.jpg"));
        let mut nef = item_at("photo.nef", 0, 300_000_000);
        nef.jpeg_pair = Some(PathBuf::from("/src/photo.jpg"));
        let items = [
            cr2,
            nef,
            item_at("x.raw", 0, 600_000_000),
            item_at("photo.jpg", 0, 0),
        ];
        let view = build_incremental(&items, &params);
        let idx = |name| view.index_of(Path::new(name)).unwrap();
        let group = view.group_of(idx("/src/photo.cr2"), true, true);

        let mut unique = group.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            group.len(),
            unique.len(),
            "the shared JPEG must not be duplicated"
        );
        let mut sorted = group;
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            {
                let mut e = vec![
                    idx("/src/photo.cr2"),
                    idx("/src/photo.nef"),
                    idx("/src/x.raw"),
                    idx("/src/photo.jpg"),
                ];
                e.sort_unstable();
                e
            },
            "both RAWs, the third member, and the single shared JPEG"
        );
    }

    #[test]
    fn expanding_a_burst_shows_all_members() {
        let params = Params::new();
        let items = [
            item_at("a.raw", 0, 0),
            item_at("a2.raw", 0, 300_000_000),
            item_at("a3.raw", 0, 600_000_000),
        ];
        let mut view = build_incremental(&items, &params);
        assert_eq!(visible_names(&view), ["a.raw"], "collapsed");

        let key = *view.burst_map.keys().next().unwrap();
        view.toggle_burst_expansion(key, &params.view());
        assert_eq!(visible_names(&view), ["a.raw", "a2.raw", "a3.raw"]);

        view.toggle_burst_expansion(key, &params.view());
        assert_eq!(visible_names(&view), ["a.raw"], "collapsed again");
    }
}
