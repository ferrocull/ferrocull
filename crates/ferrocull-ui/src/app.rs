mod compare;
mod destination;
mod filters;
mod grid;
mod preview;
mod profile;
mod settings;
mod sources;

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

use ferrocull_core::{
    AppSettings, ColorLabel, FileCategory, Hook, IngestConfig, JobCodeHistory, NamedProfile,
    Preferences, ViewPrefs,
    cache::{PreviewCache, ThumbnailCache, default_cache_root},
    media::{DateSelection, FilterMode},
    metadata_store,
    persistence::{MediaDatabase, PanelWidths},
    scan,
};
use ferrocull_devices::{ScannedFile, Source};
use iced::{
    Element, Fill, Function, Length, Subscription, Task, Theme,
    futures::SinkExt,
    keyboard::{self, Event as KeyboardEvent, Key, Modifiers},
    widget::{
        Space, button, center, column, container, mouse_area, opaque, progress_bar, responsive,
        row, scrollable, stack, text, tooltip,
    },
};
use sipper::sipper;

use crate::{
    media_view::{MediaView, TagState, ViewParams},
    messages::{
        Message, Panel, ScanEvent, Section, compare as compare_msg, destination as destination_msg,
        filters as filters_msg, grid as grid_msg, preview as preview_msg, settings as settings_msg,
        sources as sources_msg,
    },
    styles,
    theme::spacing,
    undo,
    views::{self, collapsible_section},
    widgets::{Splitter, splitter},
};

/// Transient status-bar message. Errors render in the danger color; action
/// echoes (rating/label/tag/undo feedback) render in the normal text color.
enum StatusMessage {
    Error(String),
    Echo(String),
}

/// Tracks which config panel sections are expanded (present = expanded).
#[derive(Debug, Clone)]
struct SectionState(HashSet<Section>);

impl SectionState {
    fn is_expanded(&self, section: Section) -> bool {
        self.0.contains(&section)
    }

    fn toggle(&mut self, section: Section) {
        if !self.0.remove(&section) {
            self.0.insert(section);
        }
    }

    fn with_defaults() -> Self {
        Self(HashSet::from([Section::Destination, Section::Rename]))
    }
}

const PANEL_MIN_WIDTH: f32 = 150.0;
const PANEL_MAX_WIDTH: f32 = 600.0;

/// Pluralization suffix for a count: empty for exactly one, `"s"` otherwise.
fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Max scan events drained into one [`Message::ScanBatch`]. The pipeline emits
/// two events per file, so this caps a batch at ~128 files — large enough to
/// collapse the per-event rebuild storm, small enough to keep progress
/// responsive.
const SCAN_BATCH_LIMIT: usize = 256;

struct ThumbnailProgress {
    total: usize,
    completed: usize,
}

struct IngestProgress {
    total_files: usize,
    files_completed: usize,
}

/// State for full-screen preview mode. Created on enter, dropped on exit.
struct PreviewState {
    index: usize,
    /// Used to detect navigation on close (scroll grid to new position).
    opened_at: usize,
    view_state: crate::widgets::ViewState,
}

/// State for compare mode. Created on enter, dropped on exit.
struct CompareState {
    /// Left/top pane -- the "keeper".
    select_index: usize,
    /// Right/bottom pane -- the "challenger".
    candidate_index: usize,
    active_pane: compare_msg::Pane,
    lock_scroll: bool,
    select_view_state: crate::widgets::ViewState,
    candidate_view_state: crate::widgets::ViewState,
    layout: compare_msg::Layout,
}

enum ViewMode {
    Grid,
    Preview(PreviewState),
    Compare(CompareState),
}

/// State for the Settings popup overlay. Created on open, dropped on close.
/// Committed values live on [`Ferrocull`]; this holds only transient UI state
/// and the destructive changes staged for confirmation.
pub(crate) struct SettingsState {
    pub(crate) category: settings_msg::Category,
    /// Thumbnail resolution staged awaiting confirmation (destructive: clears
    /// and regenerates the thumbnail cache). `None` when nothing is staged.
    pub(crate) pending_thumbnail_size: Option<u32>,
    /// Cache directory staged awaiting confirmation (destructive: moves files).
    pub(crate) pending_cache_dir: Option<PathBuf>,
    /// A cache relocation is running; its confirm control stays disabled until
    /// the move settles.
    pub(crate) cache_move_in_flight: bool,
}

impl SettingsState {
    fn new() -> Self {
        Self {
            category: settings_msg::Category::default(),
            pending_thumbnail_size: None,
            pending_cache_dir: None,
            cache_move_in_flight: false,
        }
    }
}

/// The single active modal overlay, if any. Exactly one modal shows at a time,
/// so holding them in one `Option` makes that exclusivity structural rather
/// than a property of guard order in `handle_key_press`.
enum Modal {
    Settings(SettingsState),
    IngestFailures,
    Shortcuts,
}

/// The user's filter/sort/grouping choices — the source of truth for what the
/// grid shows. A distinct struct so `config.params()` borrows only these
/// fields, leaving `&mut self.media` free at rebuild/insert sites.
///
/// The durable subset (persisted across launches) lives in the embedded
/// [`ViewPrefs`]; the selection sets below are session-only and never
/// persisted. Burst *expansion* is not here — `MediaView` owns it, since only
/// its burst re-keying logic can keep it consistent.
struct ViewConfig {
    /// Durable prefs restored at startup and written back on change.
    view: ViewPrefs,
    selected_sources: BTreeSet<PathBuf>,
    selected_dates: Option<DateSelection>,
    selected_ratings: BTreeSet<i8>,
    selected_color_labels: BTreeSet<Option<ColorLabel>>,
}

impl ViewConfig {
    /// Seed the view from persisted durable prefs. Selection sets start empty —
    /// they reference session-specific content and are never persisted.
    fn from_prefs(view: ViewPrefs) -> Self {
        Self {
            view,
            selected_sources: BTreeSet::new(),
            selected_dates: None,
            selected_ratings: BTreeSet::new(),
            selected_color_labels: BTreeSet::new(),
        }
    }

    /// Borrow the config as [`ViewParams`] for a `MediaView` operation.
    fn params(&self) -> ViewParams<'_> {
        ViewParams {
            sort_order: self.view.sort_order,
            filter_mode: self.view.filter_mode,
            new_only: self.view.new_only,
            hide_rejected: self.view.hide_rejected,
            group_raw_jpeg: self.view.group_raw_jpeg,
            group_bursts: self.view.group_bursts,
            selected_sources: &self.selected_sources,
            selected_dates: self.selected_dates,
            selected_ratings: &self.selected_ratings,
            selected_color_labels: &self.selected_color_labels,
        }
    }

    /// The stackable filter axes. The empty-view indicator and Clear Filters
    /// must agree on what counts as a filter, so both lists live here —
    /// `clear_filters` resets exactly the axes this checks.
    fn has_active_filters(&self) -> bool {
        self.view.filter_mode != FilterMode::default()
            || self.view.new_only
            || self.view.hide_rejected
            || self.selected_dates.is_some()
            || !self.selected_ratings.is_empty()
            || !self.selected_color_labels.is_empty()
    }

    fn clear_filters(&mut self) {
        self.view.filter_mode = FilterMode::default();
        self.view.new_only = false;
        self.view.hide_rejected = false;
        self.selected_dates = None;
        self.selected_ratings.clear();
        self.selected_color_labels.clear();
    }
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "TEA pattern requires flat state struct with independent toggle flags"
)]
#[allow(
    clippy::multiple_inherent_impl,
    reason = "impl blocks split across feature child modules (grid, sources, filters, etc.)"
)]
struct Ferrocull {
    /// Owns the media items and every index derived from them (sorted view,
    /// bursts, burst expansion, RAW+JPEG hiding, render-cache version).
    media: MediaView,
    /// The user's filter/sort/grouping choices, passed to `MediaView` as
    /// [`ViewParams`].
    config: ViewConfig,
    /// The active modal overlay (Settings, ingest failures, or shortcuts),
    /// present only while one is open (rendered as a top overlay layer over the
    /// dimmed grid). At most one is ever open.
    modal: Option<Modal>,
    /// Committed theme preference (source of truth for persistence; mirrored
    /// into the render-time cache via `theme::set_preference`).
    theme_preference: ferrocull_core::ThemePreference,
    /// Committed grid thumbnail resolution (longest edge, px), fed into the
    /// thumbnail scan.
    thumbnail_size: u32,
    /// Committed cache root override. `None` uses the platform default
    /// (`cache::default_cache_root`); the resolved root is
    /// [`Self::cache_root`].
    cache_dir: Option<PathBuf>,
    selected: BTreeSet<usize>,
    sources: Vec<Source>,
    photos_dest: String,
    videos_dest: String,
    photo_pattern: String,
    video_pattern: String,
    /// App-level rename patterns the user saved for reuse, most-recent first.
    saved_patterns: Vec<String>,
    ingest_progress: Option<IngestProgress>,
    /// Per-file failures from the last ingest (shown in the status bar until
    /// the next ingest; expandable into a details popup with retry).
    last_ingest_failures: Vec<crate::messages::FailureInfo>,
    /// Transient status message shown in status bar: errors (profile save, DB)
    /// and action echoes (rating/label/tag/undo feedback).
    status_message: Option<StatusMessage>,
    /// Bounded stack of undoable grid metadata mutations (Ctrl+Z).
    undo_stack: undo::Stack,
    thumbnail_progress: Option<ThumbnailProgress>,
    scan_jobs_in_flight: usize,
    thumbnail_jobs_in_flight: usize,
    scanning: bool,
    job_code: String,
    job_code_history: JobCodeHistory,
    backup_destinations: Vec<PathBuf>,
    profiles: Vec<NamedProfile>,
    current_profile: Option<String>,
    profile_name_input: String,
    hooks: Vec<Hook>,
    delete_after_ingest: bool,
    /// The seam for culling metadata (rating, color label) and ingest history.
    /// Sync reads and writes are acceptable: `SQLite` WAL writes are sub-ms for
    /// local storage, well under iced's 16ms frame budget.
    metadata: metadata_store::Store,
    sections: SectionState,
    expanded_years: BTreeSet<i32>,
    expanded_months: BTreeSet<(i32, u32)>,
    /// Shared thumbnail disk cache, opened once at startup. Cloned into each
    /// blocking thumbnail-load task so no task re-opens the cache (a `dirs`
    /// lookup + `create_dir_all` per thumbnail otherwise).
    thumbnail_cache: Arc<ThumbnailCache>,
    /// Shared on-disk cache of extracted full-screen preview JPEGs, opened once
    /// at startup. Distinct from `preview_cache` (in-memory GPU allocations).
    preview_disk_cache: Arc<PreviewCache>,
    loaded_thumbs: HashMap<PathBuf, iced::widget::image::Handle>,
    /// Item indices whose thumbnails are currently in the load window (viewport
    /// plus overscan). Drives which thumbnails load and which are evicted as the
    /// grid scrolls — the update-side replacement for the old per-cell sensors.
    thumb_window: HashSet<usize>,
    /// Set when a scan batch reported freshly-cached thumbnails, so the next
    /// window reconcile retries loading every in-window thumbnail that is now on
    /// disk (not just cells that just entered the window).
    thumb_generation_dirty: bool,
    hovered_thumbnail: Option<usize>,
    hovered_star: Option<i8>,
    focused_index: Option<usize>,
    modifiers: Modifiers,
    left_panel_visible: bool,
    right_panel_visible: bool,
    panel_widths: PanelWidths,
    /// Whether compare mode and the preview show the info strip beneath the
    /// photo. One flag for both views, persisted, so a photographer who culls
    /// with settings visible never re-opens it.
    info_strip_open: bool,
    preview_cache: HashMap<PathBuf, iced::widget::image::Allocation>,
    /// In-flight preview requests, keyed by path and tagged with generation.
    preview_loading: HashMap<PathBuf, u64>,
    /// Monotonic generation to discard stale async preview loads.
    preview_generation: u64,
    view_mode: ViewMode,
    /// Current date for "Today"/"Yesterday" headers. Updated on each message.
    today: chrono::NaiveDate,
    /// OS window scale factor, tracked via `window::Event::Rescaled` — the
    /// thumbnail grid floors cell widths to whole physical pixels with it.
    window_scale: f32,
    /// Tracked absolute y offset of the thumbnail scrollable, kept in sync via
    /// the scrollable's `on_scroll` (drags, keyboard, and programmatic scrolls).
    grid_scroll_y: f32,
    /// Last measured grid content width (`None` until the first layout). Drives
    /// row math and resize re-anchoring.
    grid_area_width: Option<f32>,
    /// Display ordinal of the card whose row is pinned at the viewport top.
    /// Updated when the user scrolls; reflows re-anchor to it unchanged, so a
    /// resize drag keeps pinning the same card (and resizing back restores it).
    grid_anchor: usize,
    /// Last seen scrollable viewport height, to tell user scrolls from clamps.
    grid_viewport_height: f32,
    /// Last seen scrollable content height, to tell user scrolls from clamps.
    grid_content_height: f32,
    /// Fractional carry for hi-res wheels: whole line steps move a row, the
    /// remainder accumulates toward the next.
    grid_wheel_lines: f32,
    /// Memoized grid row model. The row starts only change when the media view,
    /// sort/grouping, or derived column geometry change, but `on_scroll` fires
    /// many times per second — this caches the `O(items)` `section_counts` walk
    /// so a scroll frame doesn't rebuild it. Held behind an `Rc` so the hot path
    /// hands out a cheap refcount bump instead of cloning the row vector.
    grid_rows_cache: Option<(GridRowsKey, Rc<[views::thumbnails::RowStart]>)>,
}

/// Invalidation key for [`Ferrocull::grid_rows`]'s memoized row model. Captures
/// everything the row starts depend on: the media view (`media_version`), the
/// section layout (`ascending`, `grouped`), and the column geometry
/// (`width_bits`, `scale_bits`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GridRowsKey {
    media_version: u64,
    ascending: bool,
    grouped: bool,
    width_bits: u32,
    scale_bits: u32,
}

impl Default for Ferrocull {
    fn default() -> Self {
        let data_dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Ferrocull");
        let db_path = data_dir.join("ferrocull.db");

        let db = match MediaDatabase::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                tracing::error!(
                    path = %db_path.display(),
                    "cannot open database: {e} — the file may be corrupt, move or delete it then relaunch"
                );
                panic!("cannot open database at {}: {e}", db_path.display());
            }
        };
        let metadata = metadata_store::Store::new(db);
        let profiles = metadata.profiles();
        let job_code_history = JobCodeHistory::from_codes(metadata.job_code_history());
        let settings = metadata.settings();

        // Apply the persisted theme preference before the first frame so it
        // opens with the correct appearance rather than the OS-detected one.
        let theme_preference = settings.preferences.theme;
        crate::theme::set_preference(theme_preference);

        let thumbnail_size = settings.preferences.thumbnail_size;
        let cache_dir = settings.preferences.cache_dir.clone();
        let cache_root = cache_dir
            .clone()
            .or_else(default_cache_root)
            .expect("cache root unresolved");

        // Opened once here and shared (via `Arc`) into every blocking load task.
        // A failure to create the cache directory would leave thumbnails
        // unloadable for the whole session, so fail loudly at boot like the DB
        // rather than degrade to a permanently blank grid.
        let thumbnail_cache = Arc::new(
            ThumbnailCache::open_in_root(&cache_root)
                .unwrap_or_else(|e| panic!("cannot open thumbnail cache: {e}")),
        );
        let preview_disk_cache = Arc::new(
            PreviewCache::open_in_root(&cache_root)
                .unwrap_or_else(|e| panic!("cannot open preview cache: {e}")),
        );

        Self {
            media: MediaView::new(),
            config: ViewConfig::from_prefs(settings.view),
            modal: None,
            theme_preference,
            thumbnail_size,
            cache_dir,
            selected: BTreeSet::new(),
            sources: Vec::new(),
            photos_dest: settings.ingest.photos_dest.to_string_lossy().into_owned(),
            videos_dest: settings.ingest.videos_dest.to_string_lossy().into_owned(),
            photo_pattern: settings.ingest.photo_pattern,
            video_pattern: settings.ingest.video_pattern,
            saved_patterns: settings.saved_patterns,
            ingest_progress: None,
            last_ingest_failures: Vec::new(),
            status_message: None,
            undo_stack: undo::Stack::default(),
            thumbnail_progress: None,
            scan_jobs_in_flight: 0,
            thumbnail_jobs_in_flight: 0,
            scanning: false,
            job_code: String::new(),
            job_code_history,
            backup_destinations: settings.ingest.backup_destinations,
            profiles,
            current_profile: None,
            profile_name_input: String::new(),
            hooks: settings.post_ingest_hooks,
            delete_after_ingest: settings.delete_after_ingest,
            metadata,
            sections: SectionState::with_defaults(),
            expanded_years: BTreeSet::new(),
            expanded_months: BTreeSet::new(),
            thumbnail_cache,
            preview_disk_cache,
            loaded_thumbs: HashMap::new(),
            thumb_window: HashSet::new(),
            thumb_generation_dirty: false,
            hovered_thumbnail: None,
            hovered_star: None,
            focused_index: None,
            modifiers: Modifiers::default(),
            left_panel_visible: true,
            right_panel_visible: true,
            panel_widths: settings.panel_widths,
            info_strip_open: settings.info_strip_open,
            preview_cache: HashMap::new(),
            preview_loading: HashMap::new(),
            preview_generation: 0,
            view_mode: ViewMode::Grid,
            today: chrono::Local::now().date_naive(),
            window_scale: 1.0,
            grid_scroll_y: 0.0,
            grid_area_width: None,
            grid_anchor: 0,
            grid_viewport_height: 0.0,
            grid_content_height: 0.0,
            grid_wheel_lines: 0.0,
            grid_rows_cache: None,
        }
    }
}

fn theme(_state: &Ferrocull) -> Theme {
    crate::theme::resolve_theme()
}

fn toggle_set<T: Ord>(set: &mut BTreeSet<T>, item: T) {
    if !set.remove(&item) {
        set.insert(item);
    }
}

impl Ferrocull {
    fn first_index(&self) -> Option<usize> {
        self.media.first_index(self.config.view.ascending)
    }

    fn last_index(&self) -> Option<usize> {
        self.media.last_index(self.config.view.ascending)
    }

    fn adjacent_index(&self, current: usize, forward: bool) -> Option<usize> {
        self.media.adjacent_index(
            current,
            forward,
            self.config.view.ascending,
            self.config.view.sort_order,
        )
    }

    fn ordinal_position(&self, item_idx: usize) -> Option<usize> {
        self.media
            .ordinal_position(item_idx, self.config.view.ascending)
    }

    /// The logical group to fan out to: collapsed-burst members plus RAW+JPEG siblings.
    fn group_of(&self, idx: usize) -> Vec<usize> {
        self.media.group_of(
            idx,
            self.config.view.group_bursts,
            self.config.view.group_raw_jpeg,
        )
    }

    /// Tag state of what a grid cell stands for, over the same group the tag
    /// keystroke fans out to.
    fn tag_state(&self, idx: usize) -> TagState {
        self.media.tag_state(
            idx,
            &self.selected,
            self.config.view.group_bursts,
            self.config.view.group_raw_jpeg,
        )
    }

    /// Show an action echo (rating/label/tag/undo feedback) in the status bar.
    fn echo(&mut self, message: String) {
        self.status_message = Some(StatusMessage::Echo(message));
    }

    /// Show an error in the status bar.
    fn error(&mut self, message: String) {
        self.status_message = Some(StatusMessage::Error(message));
    }

    /// Filename of item `idx` for status echoes.
    fn display_name(&self, idx: usize) -> String {
        self.media
            .item(idx)
            .path
            .file_name()
            .expect("item path has no filename")
            .to_string_lossy()
            .into_owned()
    }

    /// Snapshots the persisted working settings and writes them to the store.
    /// Called at every mutation site of a persisted field.
    fn persist_settings(&mut self) {
        let settings = AppSettings {
            ingest: IngestConfig {
                photos_dest: PathBuf::from(&self.photos_dest),
                videos_dest: PathBuf::from(&self.videos_dest),
                photo_pattern: self.photo_pattern.clone(),
                video_pattern: self.video_pattern.clone(),
                backup_destinations: self.backup_destinations.clone(),
            },
            post_ingest_hooks: self.hooks.clone(),
            delete_after_ingest: self.delete_after_ingest,
            preferences: Preferences {
                theme: self.theme_preference,
                thumbnail_size: self.thumbnail_size,
                cache_dir: self.cache_dir.clone(),
            },
            view: self.config.view,
            saved_patterns: self.saved_patterns.clone(),
            panel_widths: self.panel_widths,
            info_strip_open: self.info_strip_open,
        };
        self.metadata.set_settings(&settings);
    }

    /// The open Settings popup's transient state, when Settings is the active
    /// modal.
    fn settings(&self) -> Option<&SettingsState> {
        match &self.modal {
            Some(Modal::Settings(s)) => Some(s),
            _ => None,
        }
    }

    /// Mutable access to the open Settings popup's transient state, when
    /// Settings is the active modal.
    fn settings_mut(&mut self) -> Option<&mut SettingsState> {
        match &mut self.modal {
            Some(Modal::Settings(s)) => Some(s),
            _ => None,
        }
    }

    /// The resolved cache root: the configured override, else the platform
    /// default. Used to display and relocate the cache.
    fn cache_root(&self) -> Option<PathBuf> {
        self.cache_dir.clone().or_else(default_cache_root)
    }

    /// A thumbnail scan is running (source scan or thumbnail regeneration),
    /// which holds cache handles and stalls destructive settings changes.
    fn scan_in_flight(&self) -> bool {
        self.scanning || self.thumbnail_jobs_in_flight > 0
    }

    fn handle_thumbnail_cached(&mut self) {
        if let Some(ref mut progress) = self.thumbnail_progress {
            progress.completed += 1;
            if progress.completed >= progress.total && self.thumbnail_jobs_in_flight == 0 {
                self.thumbnail_progress = None;
            }
        }
    }
}

impl Ferrocull {
    fn handle_key_press(
        &mut self,
        key: &Key,
        modified_key: &Key,
        modifiers: Modifiers,
    ) -> Task<Message> {
        use keyboard::key::Named;

        // A modal swallows every global shortcut so grid/rating keys can't fire
        // behind the scrim. Esc dismisses any modal; the shortcut overlay also
        // closes on `?`/F1, since those keys toggle it.
        if let Some(ref modal) = self.modal {
            let closes = match modal {
                Modal::Shortcuts => {
                    matches!(key, Key::Named(Named::Escape | Named::F1))
                        || matches!(modified_key, Key::Character(c) if c == "?")
                }
                Modal::Settings(_) | Modal::IngestFailures => {
                    matches!(key, Key::Named(Named::Escape))
                }
            };
            if closes {
                self.modal = None;
            }
            return Task::none();
        }

        // `?` is matched on the modified key: it lives on a different physical
        // key per layout (Shift+/ on QWERTY, Shift+, on AZERTY, ...), and only
        // the modified key resolves what the press actually typed.
        if matches!(modified_key, Key::Character(c) if c == "?") {
            return Task::done(Message::ToggleShortcuts);
        }

        match key {
            Key::Character(c) => {
                return self.handle_character_key(c, modified_key, modifiers);
            }
            Key::Named(
                Named::ArrowRight | Named::ArrowLeft | Named::ArrowDown | Named::ArrowUp,
            ) => {
                if matches!(self.view_mode, ViewMode::Compare(_)) {
                    // Arrows always move candidate pane (select stays as reference)
                    let forward = matches!(key, Key::Named(Named::ArrowRight | Named::ArrowDown));
                    return Task::done(if forward {
                        Message::Compare(compare_msg::Message::CandidateNext)
                    } else {
                        Message::Compare(compare_msg::Message::CandidatePrev)
                    });
                }
                let in_preview = matches!(self.view_mode, ViewMode::Preview(_));
                if !in_preview && modifiers.shift() {
                    let direction = match key {
                        Key::Named(Named::ArrowRight) => grid_msg::Direction::Right,
                        Key::Named(Named::ArrowLeft) => grid_msg::Direction::Left,
                        Key::Named(Named::ArrowDown) => grid_msg::Direction::Down,
                        _ => grid_msg::Direction::Up,
                    };
                    return Task::done(Message::Grid(grid_msg::Message::ExtendFocus(direction)));
                }
                return handle_arrow_key(key, in_preview);
            }
            Key::Named(Named::PageDown | Named::PageUp | Named::Home | Named::End) => {
                if matches!(self.view_mode, ViewMode::Grid) {
                    let msg = match key {
                        Key::Named(Named::PageDown) => grid_msg::Message::FocusPageDown,
                        Key::Named(Named::PageUp) => grid_msg::Message::FocusPageUp,
                        Key::Named(Named::Home) => grid_msg::Message::FocusHome,
                        _ => grid_msg::Message::FocusEnd,
                    };
                    return Task::done(Message::Grid(msg));
                }
            }
            Key::Named(Named::Space | Named::Enter) => {
                if matches!(self.view_mode, ViewMode::Grid)
                    && let Some(idx) = self.focused_index
                {
                    return Task::done(Message::Grid(grid_msg::Message::OpenPreview(idx)));
                }
            }
            // F1 mirrors `?` (Shift+/) for the shortcut overlay.
            Key::Named(Named::F1) => return Task::done(Message::ToggleShortcuts),
            Key::Named(Named::Tab) => {
                if let ViewMode::Compare(ref c) = self.view_mode {
                    let new_pane = match c.active_pane {
                        compare_msg::Pane::Select => compare_msg::Pane::Candidate,
                        compare_msg::Pane::Candidate => compare_msg::Pane::Select,
                    };
                    return Task::done(Message::Compare(compare_msg::Message::ActivePaneChanged(
                        new_pane,
                    )));
                }
            }
            Key::Named(Named::Escape) => match self.view_mode {
                ViewMode::Compare(_) => {
                    return Task::done(Message::Compare(compare_msg::Message::Exit));
                }
                ViewMode::Preview(_) => {
                    return Task::done(Message::Preview(preview_msg::Message::Close));
                }
                ViewMode::Grid => {
                    self.focused_index = None;
                }
            },
            _ => {}
        }

        Task::none()
    }

    fn handle_character_key(
        &self,
        c: &str,
        modified_key: &Key,
        modifiers: Modifiers,
    ) -> Task<Message> {
        let Some(ch) = c.chars().next() else {
            return Task::none();
        };

        // Digits are matched on the modified key: layouts like classic AZERTY
        // put digits behind Shift, so only the typed character identifies
        // them. On direct-digit layouts the modified key equals the base key,
        // and on QWERTY Shift+1 types '!' — not a digit — so shifted symbol
        // presses keep falling through unchanged.
        let typed_digit = match modified_key {
            Key::Character(m) => m.chars().next().filter(char::is_ascii_digit),
            _ => None,
        };

        let target_idx = match self.view_mode {
            ViewMode::Compare(ref cmp) => Some(match cmp.active_pane {
                compare_msg::Pane::Select => cmp.select_index,
                compare_msg::Pane::Candidate => cmp.candidate_index,
            }),
            ViewMode::Preview(ref p) => Some(p.index),
            ViewMode::Grid => self.focused_index,
        };

        // Tag/untag keyed on the modified key, not the base: what the press
        // actually typed decides. On classic AZERTY the '-'/'_' base keys carry
        // digits 6/8 under Shift, so Ctrl+Shift there must fall through to the
        // color-label branch below instead of silently untagging.
        if let Key::Character(m) = modified_key {
            match m.chars().next() {
                Some('+' | '=') => {
                    return self.action_on_target(
                        target_idx,
                        |path| Message::Grid(grid_msg::Message::FileTagged(path)),
                        false,
                    );
                }
                Some('-' | '_') => {
                    return self.action_on_target(
                        target_idx,
                        |path| Message::Grid(grid_msg::Message::FileUntagged(path)),
                        false,
                    );
                }
                _ => {}
            }
        }

        // Cmd+0-7 (Mac) / Ctrl+0-7 (Win/Linux): color label
        if modifiers.command() {
            if let Some(digit @ '0'..='7') = typed_digit {
                let label = ColorLabel::try_from(digit as u8 - b'0').ok();
                return self.action_on_target(
                    target_idx,
                    |path| Message::Grid(grid_msg::Message::FileColorLabelSet(path, label)),
                    false,
                );
            }
            match ch {
                'a' | 'A' => return Task::done(Message::Grid(grid_msg::Message::TagAll)),
                'd' | 'D' => return Task::done(Message::Grid(grid_msg::Message::UntagAll)),
                // Ctrl+Shift+Z redoes; plain Ctrl+Z undoes.
                'z' | 'Z' if modifiers.shift() => {
                    return Task::done(Message::Grid(grid_msg::Message::Redo));
                }
                'z' | 'Z' => return Task::done(Message::Grid(grid_msg::Message::Undo)),
                'y' | 'Y' => return Task::done(Message::Grid(grid_msg::Message::Redo)),
                ',' => return Task::done(Message::Settings(settings_msg::Message::Open)),
                _ => {}
            }
        } else if !modifiers.alt() {
            // 0-5: star rating — deliberate divergence from Photo Mechanic,
            // where unmodified digits set the color class (ADR-0001 mandates
            // PM shortcuts; this one is an intentional exception).
            // Shift is allowed only when the press
            // actually typed the digit (AZERTY-family layouts); a shifted
            // press that typed something else falls through to the
            // shift-gated handling below.
            if let Some(digit @ '0'..='5') = typed_digit {
                #[expect(
                    clippy::cast_possible_wrap,
                    reason = "'0'..='5' yields 0..=5, well within i8 range"
                )]
                let rating = (digit as u8 - b'0') as i8;
                return self.action_on_target(
                    target_idx,
                    |path| Message::Grid(grid_msg::Message::FileRated(path, rating)),
                    rating > 0,
                );
            }
            if !modifiers.shift() {
                return self.handle_unmodified_char(ch, target_idx);
            }
        }

        Task::none()
    }

    fn handle_unmodified_char(&self, ch: char, target_idx: Option<usize>) -> Task<Message> {
        let in_preview = matches!(self.view_mode, ViewMode::Preview(_));
        let in_compare = matches!(self.view_mode, ViewMode::Compare(_));
        match ch {
            // Compare mode keys (Photo Mechanic conventions)
            'h' | 'H' if in_preview || in_compare || self.focused_index.is_some() => {
                Task::done(Message::Compare(compare_msg::Message::EnterHorizontal))
            }
            'v' | 'V' if in_preview || in_compare || self.focused_index.is_some() => {
                Task::done(Message::Compare(compare_msg::Message::EnterVertical))
            }
            'o' | 'O' if in_compare => Task::done(Message::Compare(compare_msg::Message::Exit)),
            'g' | 'G' if in_compare => Task::done(Message::Compare(compare_msg::Message::Promote)),
            'l' | 'L' if in_compare => {
                Task::done(Message::Compare(compare_msg::Message::ToggleLockScroll))
            }
            'z' | 'Z' if in_compare => {
                Task::done(Message::Compare(compare_msg::Message::ResetZoom))
            }
            // I: Photo Mechanic's Info binding, here the info strip.
            'i' | 'I' if in_compare || in_preview => Task::done(Message::ToggleInfoStrip),
            'z' | 'Z' if in_preview => {
                Task::done(Message::Preview(preview_msg::Message::ResetZoom))
            }
            // B: toggle collapse/expand of the focused item's burst.
            'b' | 'B' if !in_preview && !in_compare => {
                Task::done(Message::Grid(grid_msg::Message::ToggleFocusedBurst))
            }
            // T: tag toggle (selection toggle, Photo Mechanic convention)
            't' | 'T' => self.action_on_target(
                target_idx,
                |path| Message::Grid(grid_msg::Message::FileTagToggled(path)),
                false,
            ),
            'x' | 'X' => self.action_on_target(
                target_idx,
                |path| Message::Grid(grid_msg::Message::RejectFile(path)),
                true,
            ),
            _ => Task::none(),
        }
    }

    /// Apply an action to the target item, optionally advancing to next.
    fn action_on_target<F>(
        &self,
        target_idx: Option<usize>,
        make_msg: F,
        advance: bool,
    ) -> Task<Message>
    where
        F: FnOnce(PathBuf) -> Message,
    {
        let Some(idx) = target_idx else {
            return Task::none();
        };
        let action = Task::done(make_msg(self.media.item(idx).path.clone()));

        if advance {
            let Some(next_idx) = self.adjacent_index(idx, true) else {
                return action;
            };
            let advance_msg = match self.view_mode {
                ViewMode::Compare(_) => {
                    Message::Compare(compare_msg::Message::CandidateNavigateTo(next_idx))
                }
                ViewMode::Preview(_) => {
                    Message::Preview(preview_msg::Message::NavigateTo(next_idx))
                }
                ViewMode::Grid => Message::Grid(grid_msg::Message::FocusOn(next_idx)),
            };
            Task::batch([action, Task::done(advance_msg)])
        } else {
            action
        }
    }
}

/// Open folder picker and map result to message.
fn pick_folder<F>(mapper: F) -> Task<Message>
where
    F: FnOnce(Option<PathBuf>) -> Message + Send + 'static,
{
    Task::perform(
        async { rfd::AsyncFileDialog::new().pick_folder().await },
        move |result| mapper(result.map(|h| h.path().to_path_buf())),
    )
}

fn handle_arrow_key(key: &Key, in_preview: bool) -> Task<Message> {
    use keyboard::key::Named;

    // Preview steps one image either way; the grid navigates in two dimensions.
    let msg = match (in_preview, key) {
        (true, Key::Named(Named::ArrowRight | Named::ArrowDown)) => {
            Message::Preview(preview_msg::Message::Next)
        }
        (true, Key::Named(Named::ArrowLeft | Named::ArrowUp)) => {
            Message::Preview(preview_msg::Message::Prev)
        }
        (false, Key::Named(Named::ArrowRight)) => Message::Grid(grid_msg::Message::FocusNext),
        (false, Key::Named(Named::ArrowLeft)) => Message::Grid(grid_msg::Message::FocusPrev),
        (false, Key::Named(Named::ArrowDown)) => Message::Grid(grid_msg::Message::FocusDown),
        (false, Key::Named(Named::ArrowUp)) => Message::Grid(grid_msg::Message::FocusUp),
        _ => return Task::none(),
    };
    Task::done(msg)
}

/// Adapts a [`ScannedFile`] to the core scan pipeline's input contract and
/// carries it back through [`scan::Event::ExifLoaded`] for item construction.
struct ScanFile(ScannedFile);

impl scan::Input for ScanFile {
    fn path(&self) -> &std::path::Path {
        &self.0.path
    }

    fn category(&self) -> FileCategory {
        self.0.media_type
    }

    fn xmp_sidecar(&self) -> Option<&std::path::Path> {
        self.0.xmp_sidecar.as_deref()
    }
}

/// Spawn sipper that extracts EXIF first (creating items), then generates
/// thumbnails, writing them through the shared [`ThumbnailCache`].
fn spawn_thumbnail_sipper(
    files: Vec<ScannedFile>,
    thumbnail_size: u32,
    cache: Arc<ThumbnailCache>,
) -> Task<Message> {
    let thumb_sipper = sipper(move |mut sender| async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        rayon::spawn(move || {
            let inputs = files.into_iter().map(ScanFile).collect();
            scan::run(inputs, thumbnail_size, Some(cache.as_ref()), |event| {
                drop(tx.send(event));
            });
        });

        // The pipeline fires two events per file across many rayon threads.
        // Draining the channel in batches and forwarding one message per drain
        // collapses that firehose into a single `update` pass per batch, so the
        // grid rebuilds O(batches) times during ingest instead of O(events).
        let mut buf = Vec::new();
        while rx.recv_many(&mut buf, SCAN_BATCH_LIMIT).await > 0 {
            let batch: Vec<ScanEvent> = buf
                .drain(..)
                .map(|event| match event {
                    scan::Event::ExifLoaded {
                        file,
                        canonical_path,
                        capture_time,
                        capture_settings,
                        xmp,
                    } => ScanEvent::ExifLoaded {
                        file: Box::new(file.0),
                        canonical_path,
                        capture_time,
                        capture_settings,
                        xmp,
                    },
                    scan::Event::ThumbnailReady { path, result } => {
                        ScanEvent::ThumbnailCached(path, result)
                    }
                })
                .collect();
            sender.send(batch).await;
        }
    });

    Task::sip(thumb_sipper, Message::ScanBatch, |()| {
        Message::ThumbnailsComplete
    })
}

fn boot() -> (Ferrocull, Task<Message>) {
    // `Ferrocull::default` applies the persisted theme preference (via
    // `theme::set_preference`), seeding the theme cache synchronously so the
    // first frame opens with the correct appearance.
    let state = Ferrocull::default();
    let task = sources::scan_storage_task();
    (state, task)
}

/// # Errors
/// Returns an error if the iced application fails to initialize or run.
pub fn run() -> iced::Result {
    iced::application(boot, update, view)
        .title("Ferrocull")
        .theme(theme)
        .subscription(subscription)
        .run()
}

fn subscription(_state: &Ferrocull) -> Subscription<Message> {
    let keys = keyboard::listen().filter_map(|event| match event {
        KeyboardEvent::KeyPressed {
            key,
            modified_key,
            modifiers,
            ..
        } => Some(Message::KeyPressed {
            key,
            modified_key,
            modifiers,
        }),
        KeyboardEvent::ModifiersChanged(modifiers) => Some(Message::ModifiersChanged(modifiers)),
        KeyboardEvent::KeyReleased { .. } => None,
    });
    let tick = iced::time::every(std::time::Duration::from_mins(1)).map(|_| Message::Tick);
    let devices = Subscription::run(device_events);
    let window = iced::window::events().filter_map(|(id, event)| match event {
        iced::window::Event::Opened { .. } => Some(Message::WindowOpened(id)),
        iced::window::Event::Rescaled(scale) => Some(Message::WindowScaleChanged(scale)),
        _ => None,
    });
    Subscription::batch([keys, tick, devices, window])
}

/// Subscription that turns storage hotplug events into source rescans,
/// replacing the periodic poll. The device watcher emits an event on every
/// plug, unplug, mount, or unmount; each collapses into a single
/// `RefreshSources`. The rescan reads the authoritative, per-drive-deduped
/// device list, so the event payload itself is unused — only the
/// "something changed" signal matters.
fn device_events() -> impl iced::futures::Stream<Item = Message> {
    iced::stream::channel(
        16,
        |mut output: iced::futures::channel::mpsc::Sender<Message>| async move {
            let (tx, mut rx) =
                tokio::sync::mpsc::unbounded_channel::<ferrocull_devices::DeviceEvent>();

            let _watcher = tokio::spawn(async move {
                if let Err(e) = ferrocull_devices::watch(tx).await {
                    tracing::error!("device watch ended: {e}");
                }
            });

            while rx.recv().await.is_some() {
                if output
                    .send(Message::Sources(sources_msg::Message::RefreshSources))
                    .await
                    .is_err()
                {
                    break; // application closed
                }
            }
        },
    )
}

fn update(state: &mut Ferrocull, message: Message) -> Task<Message> {
    let task = dispatch(state, message);
    // Every message may have moved the scroll offset, resized the grid, or
    // changed the visible set, so reconcile which thumbnails should be loaded
    // after the state has settled. It is a cheap no-op when nothing relevant
    // changed (no cells entered or left the window).
    let sync = state.reconcile_thumbnail_window();
    Task::batch([task, sync])
}

#[expect(
    clippy::too_many_lines,
    reason = "TEA dispatch: delegates to sub-functions, remaining arms are async result handlers"
)]
fn dispatch(state: &mut Ferrocull, message: Message) -> Task<Message> {
    match message {
        Message::Compare(msg) => compare::update(state, msg),
        Message::Grid(msg) => grid::update(state, msg),
        Message::Sources(msg) => sources::update(state, msg),
        Message::Destination(msg) => destination::update(state, msg),
        Message::Filters(msg) => filters::update(state, msg),
        Message::Preview(msg) => preview::update(state, msg),
        Message::Profile(msg) => profile::update(state, msg),
        Message::Settings(msg) => settings::update(state, msg),

        Message::ToggleSection(section) => {
            state.sections.toggle(section);
            Task::none()
        }
        Message::ToggleShortcuts => {
            state.modal = if matches!(state.modal, Some(Modal::Shortcuts)) {
                None
            } else {
                Some(Modal::Shortcuts)
            };
            Task::none()
        }
        Message::ToggleInfoStrip => {
            state.info_strip_open = !state.info_strip_open;
            state.persist_settings();
            Task::none()
        }
        Message::TogglePanel(panel) => {
            match panel {
                Panel::Left => state.left_panel_visible = !state.left_panel_visible,
                Panel::Right => state.right_panel_visible = !state.right_panel_visible,
            }
            Task::none()
        }
        Message::PanelResized(panel, width) => {
            let clamped = width.clamp(PANEL_MIN_WIDTH, PANEL_MAX_WIDTH);
            match panel {
                Panel::Left => state.panel_widths.left = clamped,
                Panel::Right => state.panel_widths.right = clamped,
            }
            Task::none()
        }
        Message::PanelResizeEnd => {
            state.persist_settings();
            Task::none()
        }
        Message::KeyPressed {
            ref key,
            ref modified_key,
            modifiers,
        } => state.handle_key_press(key, modified_key, modifiers),
        Message::ModifiersChanged(modifiers) => {
            state.modifiers = modifiers;
            Task::none()
        }

        Message::ScanBatch(events) => {
            for event in events {
                match event {
                    ScanEvent::ExifLoaded {
                        file,
                        canonical_path,
                        capture_time,
                        capture_settings,
                        xmp,
                    } => {
                        state.handle_exif_loaded(
                            *file,
                            &canonical_path,
                            capture_time,
                            capture_settings,
                            xmp.as_ref(),
                        );
                    }
                    ScanEvent::ThumbnailCached(_path, _result) => {
                        state.handle_thumbnail_cached();
                        // A newly-cached thumbnail is now loadable from disk, so
                        // the next reconcile must retry every in-window thumbnail.
                        state.thumb_generation_dirty = true;
                    }
                }
            }
            Task::none()
        }
        Message::ScanComplete(files) => state.handle_scan_complete(files),
        Message::ThumbnailsComplete => {
            state.thumbnail_jobs_in_flight = state.thumbnail_jobs_in_flight.saturating_sub(1);
            if state.thumbnail_jobs_in_flight == 0 {
                state.thumbnail_progress = None;
            }
            state.rebuild_view();
            // Seed focus onto the first card once a scan settles so the first
            // arrow press moves rather than being swallowed to seed focus.
            if state.focused_index.is_none()
                && let Some(first) = state.first_index()
            {
                return Task::done(Message::Grid(grid_msg::Message::FocusOn(first)));
            }
            Task::none()
        }
        Message::IngestProgressUpdate(n) => {
            if let Some(ref mut dl) = state.ingest_progress {
                dl.files_completed = n;
            }
            Task::none()
        }
        Message::IngestComplete(result) => state.handle_ingest_complete(&result),
        Message::PreviewLoaded(generation, path, result) => {
            // Generation mismatch means we navigated away; discard stale result.
            if generation != state.preview_generation {
                state.preview_loading.remove(&path);
                return Task::none();
            }
            match result {
                Ok(jpeg) => {
                    let handle = iced::widget::image::Handle::from_bytes(jpeg);
                    iced::widget::image::allocate(handle).map(move |alloc| {
                        Message::PreviewAllocated(generation, path.clone(), alloc)
                    })
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), "preview extraction failed: {e}");
                    state.preview_loading.remove(&path);
                    Task::none()
                }
            }
        }
        Message::PreviewAllocated(generation, path, result) => {
            state.preview_loading.remove(&path);
            if generation != state.preview_generation {
                return Task::none();
            }
            match result {
                Ok(allocation) => {
                    state.preview_cache.insert(path, allocation);
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), "preview GPU allocation failed: {e}");
                }
            }
            Task::none()
        }
        Message::ThumbnailLoaded(path, handle) => {
            state.loaded_thumbs.insert(path, handle);
            Task::none()
        }
        Message::Tick => {
            state.today = chrono::Local::now().date_naive();
            // Storage device changes arrive via the `device_events`
            // subscription; the tick only refreshes the date and OS theme.
            Task::perform(
                tokio::task::spawn_blocking(crate::theme::detect_os_is_dark),
                |r| Message::OsThemeDetected(r.unwrap_or(false)),
            )
        }
        Message::OsThemeDetected(is_dark) => {
            crate::theme::set_os_is_dark(is_dark);
            Task::none()
        }
        Message::WindowOpened(id) => {
            iced::window::scale_factor(id).map(Message::WindowScaleChanged)
        }
        Message::WindowScaleChanged(scale) => {
            // Scale changes cell width at constant grid width, so the same
            // re-anchor keeps the top row pinned.
            match state.grid_area_width {
                Some(width) if (scale - state.window_scale).abs() > f32::EPSILON => {
                    state.window_scale = scale;
                    state.reanchor_grid(width)
                }
                _ => {
                    state.window_scale = scale;
                    Task::none()
                }
            }
        }
        Message::HooksComplete | Message::Noop => Task::none(),
        Message::SourcesRefreshed(storage_devices) => {
            state.handle_sources_refreshed(storage_devices);
            Task::none()
        }
        Message::MountResult(device_path, result) => {
            state.handle_mount_result(&device_path, result);
            Task::none()
        }
        Message::UnmountResult(device_path, result) => {
            state.handle_unmount_result(&device_path, result);
            Task::none()
        }
    }
}

fn view(state: &Ferrocull) -> Element<'_, Message> {
    let mut main_row = row![].height(Fill);

    if state.left_panel_visible {
        main_row =
            main_row
                .push(sources_panel(state))
                .push(panel_edge_handle(state, Panel::Left, true));
    } else {
        main_row = main_row.push(panel_edge_handle(state, Panel::Left, false));
    }

    main_row = main_row.push(thumbnails_panel(state));

    if state.right_panel_visible {
        main_row = main_row
            .push(panel_edge_handle(state, Panel::Right, true))
            .push(config_panel(state));
    } else {
        main_row = main_row.push(panel_edge_handle(state, Panel::Right, false));
    }

    let main_content = column![main_row, status_bar(state)];

    // Always root the tree in a stack! so the root widget type stays consistent
    // across all modes. Without this, switching between stack![main, overlay]
    // and bare main_content changes the root widget type, which makes iced
    // discard the entire widget state tree — including the grid's scroll
    // position.
    let mut root = stack![main_content];
    match state.view_mode {
        ViewMode::Compare(ref cmp) => root = root.push(compare_overlay(state, cmp)),
        ViewMode::Preview(ref p) => root = root.push(preview_overlay(state, p)),
        ViewMode::Grid => {}
    }
    if let Some(ref modal) = state.modal {
        let overlay = match modal {
            Modal::Settings(s) => settings_overlay(state, s),
            Modal::IngestFailures => ingest_failures_overlay(state),
            Modal::Shortcuts => shortcuts_overlay(),
        };
        root = root.push(overlay);
    }
    root.into()
}

/// Wrap a modal's centered content in the dimmed, click-to-dismiss scrim shared
/// by every overlay. The outer `opaque` blocks pointer events from reaching the
/// grid; `on_dismiss` fires when the scrim (but not the card) is clicked.
fn modal_scrim<'a>(
    content: impl Into<Element<'a, Message>>,
    on_dismiss: Message,
) -> Element<'a, Message> {
    opaque(
        mouse_area(
            container(content.into())
                .width(Fill)
                .height(Fill)
                .style(styles::scrim),
        )
        .on_press(on_dismiss),
    )
}

/// Details popup for the last ingest's failures: per-file error text plus a
/// retry for exactly those files.
fn ingest_failures_overlay(state: &Ferrocull) -> Element<'_, Message> {
    use crate::theme::colors;
    let palette = crate::theme::palette();

    let rows = state.last_ingest_failures.iter().map(|failure| {
        let name = failure
            .source
            .file_name()
            .expect("item path has no filename")
            .to_string_lossy();
        column![
            text(name).size(12).color(palette.background.base.text),
            text(&failure.error).size(11).color(colors::DANGER),
        ]
        .spacing(2)
        .into()
    });

    let list = scrollable(column(rows).spacing(spacing::SM)).height(Fill);

    let retry_btn = button(text("Retry failed").size(12))
        .padding([6, 16])
        .style(styles::primary_button);
    let retry_btn = if state.ingest_progress.is_none() {
        retry_btn.on_press(Message::Destination(
            destination_msg::Message::RetryFailedIngest,
        ))
    } else {
        retry_btn
    };
    let close_btn = button(text("Close").size(12))
        .padding([6, 16])
        .style(styles::secondary_button)
        .on_press(Message::Destination(
            destination_msg::Message::ToggleIngestFailures,
        ));

    let n = state.last_ingest_failures.len();
    let interior = column![
        text(format!("{n} file{} failed to ingest", plural(n)))
            .size(16)
            .color(palette.background.base.text),
        list,
        row![retry_btn, close_btn].spacing(spacing::SM),
    ]
    .spacing(spacing::LG);

    let card = container(interior)
        .width(Length::Fixed(520.0))
        .height(Length::Fixed(440.0))
        .padding(spacing::LG)
        .style(styles::settings_card);

    modal_scrim(
        center(opaque(card)),
        Message::Destination(destination_msg::Message::ToggleIngestFailures),
    )
}

/// A key-cap pill carrying mono key text for the shortcut overlay.
fn keycap(label: &'static str) -> Element<'static, Message> {
    container(text(label).size(11).font(iced::Font::MONOSPACE))
        .padding([1, 5])
        .style(styles::keycap)
        .into()
}

/// One shortcut line: a fixed-width run of key caps followed by its description.
fn shortcut_row(keys: &[&'static str], desc: &'static str) -> Element<'static, Message> {
    let palette = crate::theme::palette();
    let caps = row(keys.iter().map(|k| keycap(k))).spacing(spacing::XS);
    row![
        container(caps).width(Length::Fixed(120.0)),
        text(desc).size(12).color(palette.background.base.text),
    ]
    .spacing(spacing::SM)
    .align_y(iced::Alignment::Center)
    .into()
}

/// A titled group of shortcut lines for one column of the overlay.
fn shortcut_group(
    title: &'static str,
    rows: Vec<Element<'static, Message>>,
) -> Element<'static, Message> {
    let palette = crate::theme::palette();
    let mut items: Vec<Element<'static, Message>> = Vec::with_capacity(rows.len() + 1);
    items.push(
        text(title)
            .size(11)
            .color(palette.background.weak.text)
            .into(),
    );
    items.extend(rows);
    column(items).spacing(spacing::XS).into()
}

/// Keyboard-shortcut reference overlay (`?` / F1). Documents the real current
/// bindings — every key here is verified against the handlers in
/// `handle_key_press`/`handle_character_key`.
fn shortcuts_overlay() -> Element<'static, Message> {
    let palette = crate::theme::palette();

    let navigate = shortcut_group(
        "NAVIGATE",
        vec![
            shortcut_row(
                &["\u{2190}", "\u{2192}", "\u{2191}", "\u{2193}"],
                "Move focus",
            ),
            shortcut_row(&["PgUp", "PgDn"], "Move focus a page"),
            shortcut_row(&["Home", "End"], "First / last item"),
            shortcut_row(&["Wheel"], "Scroll the grid"),
            shortcut_row(&["Space", "Enter"], "Open preview"),
        ],
    );

    let rate = shortcut_group(
        "RATE & LABEL",
        vec![
            shortcut_row(&["0", "\u{2013}", "5"], "Set star rating"),
            shortcut_row(&["X"], "Reject / un-reject"),
            shortcut_row(&["Ctrl", "0", "\u{2013}", "7"], "Set color label"),
            text(
                "digits rate — in Photo Mechanic they set color class; \
                 Ctrl+0–7 set color labels here",
            )
            .size(10)
            .color(palette.background.weak.text)
            .into(),
        ],
    );

    let tag = shortcut_group(
        "TAG",
        vec![
            shortcut_row(&["T"], "Toggle tag"),
            shortcut_row(&["+", "\u{2212}"], "Tag / untag"),
            shortcut_row(&["Shift", "Click"], "Range-tag to here"),
            shortcut_row(&["Shift", "\u{2190}\u{2192}\u{2191}\u{2193}"], "Extend tag"),
            shortcut_row(&["Ctrl", "A", "/", "D"], "Tag all / untag all"),
            shortcut_row(&["Ctrl", "Click"], "Toggle one tag"),
        ],
    );

    let bursts = shortcut_group(
        "BURSTS",
        vec![shortcut_row(&["B"], "Collapse / expand burst")],
    );

    let compare = shortcut_group(
        "COMPARE & PREVIEW",
        vec![
            shortcut_row(&["H", "V"], "Compare side-by-side / stacked"),
            shortcut_row(&["G"], "Promote candidate"),
            shortcut_row(&["O"], "Exit compare"),
            shortcut_row(&["L"], "Lock synced zoom/pan"),
            shortcut_row(&["Z"], "Reset zoom"),
            shortcut_row(&["I"], "Show / hide the info strip"),
            shortcut_row(&["Tab"], "Switch active pane"),
        ],
    );

    let general = shortcut_group(
        "GENERAL",
        vec![
            shortcut_row(&["Ctrl", "Z"], "Undo"),
            shortcut_row(&["Ctrl", "Shift", "Z"], "Redo"),
            shortcut_row(&["Ctrl", ","], "Settings"),
            shortcut_row(&["?", "F1"], "This help"),
            shortcut_row(&["Esc"], "Close / clear focus"),
        ],
    );

    let left = column![navigate, rate, tag].spacing(spacing::LG);
    let right = column![bursts, compare, general].spacing(spacing::LG);
    let columns =
        row![container(left).width(Fill), container(right).width(Fill),].spacing(spacing::LG);

    let close_btn = button(text("Close").size(12))
        .padding([6, 16])
        .style(styles::secondary_button)
        .on_press(Message::ToggleShortcuts);

    let interior = column![
        text("Keyboard Shortcuts")
            .size(16)
            .color(palette.background.base.text),
        scrollable(columns).height(Fill),
        row![Space::new().width(Fill), close_btn],
    ]
    .spacing(spacing::LG);

    let card = container(interior)
        .width(Length::Fixed(720.0))
        .height(Length::Fixed(560.0))
        .padding(spacing::LG)
        .style(styles::settings_card);

    modal_scrim(center(opaque(card)), Message::ToggleShortcuts)
}

/// Map a rating/color/reject item event to the corresponding grid message.
fn map_item_event(path: PathBuf, event: views::rating::ItemEvent) -> Message {
    match event {
        views::rating::ItemEvent::Rated(r) => Message::Grid(grid_msg::Message::FileRated(path, r)),
        views::rating::ItemEvent::ColorLabelSet(label) => {
            Message::Grid(grid_msg::Message::FileColorLabelSet(path, label))
        }
        views::rating::ItemEvent::Rejected => Message::Grid(grid_msg::Message::RejectFile(path)),
        views::rating::ItemEvent::StarHover(star) => {
            Message::Grid(grid_msg::Message::StarHover(star))
        }
    }
}

fn compare_overlay(state: &Ferrocull, cmp: &CompareState) -> Element<'static, Message> {
    let select_item = state.media.item(cmp.select_index);
    let candidate_item = state.media.item(cmp.candidate_index);
    let active_pane = cmp.active_pane;
    let active_item = match active_pane {
        compare_msg::Pane::Select => select_item,
        compare_msg::Pane::Candidate => candidate_item,
    };
    let active_path = active_item.path.clone();

    let item_ctrl = views::rating::item_controls(
        active_item.rating,
        active_item.color_label,
        state.hovered_star,
    )
    .map(map_item_event.with(active_path));

    let top = views::compare::top_bar(
        select_item,
        candidate_item,
        state.ordinal_position(cmp.select_index),
        state.media.visible_len(),
        active_pane,
        cmp.lock_scroll,
    );
    // Both readouts are rendered together so each pane can emphasize exactly
    // the fields that differ from the other frame.
    let (select_info, candidate_info) = if state.info_strip_open {
        let select = views::info::readout(&select_item.capture_settings, select_item.capture_time);
        let candidate = views::info::readout(
            &candidate_item.capture_settings,
            candidate_item.capture_time,
        );
        let differing = views::info::differing(&select, &candidate);
        (
            Some(views::info::strip(&select, differing)),
            Some(views::info::strip(&candidate, differing)),
        )
    } else {
        (None, None)
    };

    let select_pane = views::compare::image_pane(
        state
            .preview_cache
            .get(&select_item.path)
            .map(iced::widget::image::Allocation::handle),
        active_pane == compare_msg::Pane::Select,
        cmp.select_view_state,
        "SELECT",
        select_item,
        state.selected.contains(&cmp.select_index),
        select_info,
    );
    let candidate_pane = views::compare::image_pane(
        state
            .preview_cache
            .get(&candidate_item.path)
            .map(iced::widget::image::Allocation::handle),
        active_pane == compare_msg::Pane::Candidate,
        cmp.candidate_view_state,
        "CANDIDATE",
        candidate_item,
        state.selected.contains(&cmp.candidate_index),
        candidate_info,
    );
    let bottom = views::compare::bottom_bar(cmp.layout, item_ctrl);

    views::compare::compose(cmp.layout, top, select_pane, candidate_pane, bottom)
}

fn preview_overlay(state: &Ferrocull, p: &PreviewState) -> Element<'static, Message> {
    let item = state.media.item(p.index);
    let item_path = item.path.clone();

    let item_ctrl = views::rating::item_controls(item.rating, item.color_label, state.hovered_star)
        .map(map_item_event.with(item_path));

    let top = views::preview::top_bar(
        item,
        state.ordinal_position(p.index),
        state.media.visible_len(),
    );
    let image = views::preview::image_area(
        state
            .preview_cache
            .get(&item.path)
            .map(iced::widget::image::Allocation::handle),
        p.view_state,
        item,
        state.selected.contains(&p.index),
    );
    // Emphasis means "differs from the other frame", so with only one frame on
    // screen every field stays in the muted tone.
    let info = state.info_strip_open.then(|| {
        let values = views::info::readout(&item.capture_settings, item.capture_time);
        views::info::strip(&values, [false; views::info::FIELD_COUNT])
    });
    let bottom = views::preview::bottom_bar(item_ctrl);

    views::preview::compose(top, image, info, bottom)
}

/// The Settings popup: a centered card (category rail + pane) over a dimmed
/// scrim. Click-outside or `Esc` dismiss it; the card is `opaque` so clicks on
/// it don't fall through to the scrim.
///
/// The card sizes as a ratio of the window, clamped to a min/max — big enough to
/// breathe on large monitors without sprawling, and shrinking to fit small
/// windows rather than overflowing.
fn settings_overlay<'a>(state: &'a Ferrocull, s: &'a SettingsState) -> Element<'a, Message> {
    use settings_msg::Category;

    let card = responsive(move |size| {
        let width = (size.width * 0.6).clamp(560.0, 900.0);
        let height = (size.height * 0.78).clamp(440.0, 680.0);

        let palette = crate::theme::palette();

        let pane = match s.category {
            Category::Appearance => views::settings::appearance_pane(state.theme_preference),
            Category::Storage => views::settings::storage_pane(
                s,
                state.thumbnail_size,
                state
                    .cache_root()
                    .expect("cache root unresolved")
                    .display()
                    .to_string(),
                state.scan_in_flight(),
            ),
        };

        let rail_divider = container(Space::new().width(1))
            .width(1)
            .height(Fill)
            .style(|theme: &Theme| container::Style {
                background: Some(theme.extended_palette().background.weaker.color.into()),
                ..Default::default()
            });

        let body = row![
            views::settings::rail(s.category),
            rail_divider,
            container(pane).width(Fill),
        ]
        .spacing(spacing::LG)
        .height(Fill);

        let interior = column![
            text("Settings")
                .size(16)
                .color(palette.background.base.text),
            body,
        ]
        .spacing(spacing::LG);

        let card = container(interior)
            .width(Length::Fixed(width))
            .height(Length::Fixed(height))
            .padding(spacing::LG)
            .style(styles::settings_card);

        center(opaque(Element::from(card).map(Message::Settings))).into()
    });

    modal_scrim(card, Message::Settings(settings_msg::Message::Close))
}

/// Clickable edge handle for collapsing/expanding panels.
fn panel_edge_handle(state: &Ferrocull, panel: Panel, expanded: bool) -> Element<'_, Message> {
    let palette = crate::theme::palette();

    let icon = match (panel, expanded) {
        (Panel::Left, true) | (Panel::Right, false) => "«",
        (Panel::Left, false) | (Panel::Right, true) => "»",
    };

    let width = if expanded { 8.0 } else { 14.0 };

    let label = match (panel, expanded) {
        (Panel::Left, true) => "Hide sources",
        (Panel::Left, false) => "Show sources",
        (Panel::Right, true) => "Hide config",
        (Panel::Right, false) => "Show config",
    };

    let content = container(text(icon).size(9).color(palette.background.strong.text))
        .width(width)
        .height(Fill)
        .center_x(width)
        .center_y(Fill);

    let handle: Element<'_, Message> = if expanded {
        let (panel_width, side) = match panel {
            Panel::Left => (state.panel_widths.left, splitter::Side::Left),
            Panel::Right => (state.panel_widths.right, splitter::Side::Right),
        };

        Splitter::new(
            content.style(styles::panel_handle_expanded),
            panel_width,
            side,
        )
        .on_resize(move |w| Message::PanelResized(panel, w))
        .on_resize_end(Message::PanelResizeEnd)
        .on_click(Message::TogglePanel(panel))
        .into()
    } else {
        button(content)
            .padding(0)
            .style(styles::panel_handle_collapsed)
            .on_press(Message::TogglePanel(panel))
            .into()
    };

    tooltip(handle, text(label).size(10), tooltip::Position::Right)
        .gap(4)
        .snap_within_viewport(true)
        .into()
}

fn sources_panel(state: &Ferrocull) -> Element<'_, Message> {
    let palette = crate::theme::palette();

    let header = container(text("SOURCES").size(11).color(palette.background.weak.text))
        .padding([spacing::SM, spacing::MD])
        .width(Fill);

    let sources_view =
        views::sources::sources_panel(&state.sources, &state.config.selected_sources)
            .map(Message::Sources);

    let dates = views::date_tree::date_tree(
        state.media.items(),
        state.media.version(),
        state.config.selected_dates,
        &state.expanded_years,
        &state.expanded_months,
        state.config.view.date_tree_ascending,
    )
    .map(Message::Filters);

    let content = column![
        header,
        scrollable(
            column![sources_view, Space::new().height(spacing::MD), dates]
                .padding([0.0, spacing::SM])
        )
        .height(Fill),
    ];

    container(content)
        .width(state.panel_widths.left)
        .height(Fill)
        .style(styles::panel)
        .into()
}

fn thumbnails_panel(state: &Ferrocull) -> Element<'_, Message> {
    let palette = crate::theme::palette();

    let filters_view = views::filters::filter_bar(
        views::filters::sort_controls(state.config.view.sort_order, state.config.view.ascending),
        views::filters::filter_mode_controls(state.config.view.filter_mode),
        views::filters::new_toggle(state.config.view.new_only),
        views::filters::grouping_controls(
            state.config.view.group_raw_jpeg,
            state.config.view.group_bursts,
            state.config.view.hide_rejected,
        ),
        views::filters::rating_filter(&state.config.selected_ratings),
        views::filters::color_label_filter(&state.config.selected_color_labels),
    )
    .map(Message::Filters);

    let settings_btn = button(text("\u{2699}").size(16))
        .padding(spacing::XS)
        .style(styles::icon_button)
        .on_press(Message::Settings(settings_msg::Message::Open));
    let settings_with_tip = tooltip(
        settings_btn,
        text("Settings").size(11),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .snap_within_viewport(true);

    let header = container(
        row![settings_with_tip, container(filters_view).width(Fill)]
            .spacing(spacing::SM)
            .align_y(iced::Alignment::Center),
    )
    .padding(iced::Padding::from([spacing::SM, spacing::MD]).left(spacing::XS))
    .width(Fill);

    let grid = thumbnail_grid(state);

    let content: Element<'_, Message> = if state.media.is_view_empty()
        && !state.media.is_empty()
        && state.config.has_active_filters()
    {
        let empty_state = column![
            text("No photos match current filters")
                .size(14)
                .color(palette.background.strong.text),
            button(text("Clear Filters").size(12))
                .padding([6, 16])
                .style(styles::secondary_button)
                .on_press(Message::Filters(filters_msg::Message::ClearAll)),
        ]
        .spacing(spacing::MD)
        .align_x(iced::Alignment::Center);

        container(empty_state)
            .width(Fill)
            .height(Fill)
            .center_x(Fill)
            .center_y(Fill)
            .style(styles::grid_background)
            .into()
    } else {
        container(grid)
            .width(Fill)
            .height(Fill)
            .style(styles::grid_background)
            .into()
    };

    container(column![header, content])
        .width(Fill)
        .height(Fill)
        .style(styles::panel)
        .into()
}

/// Thumbnail grid element with all context and cache keys.
/// Click always emits `CellClicked(path)` — modifier-dependent behavior is applied here.
fn thumbnail_grid(state: &Ferrocull) -> Element<'_, Message> {
    let command_held = state.modifiers.command();
    let shift_held = state.modifiers.shift();

    views::thumbnail_grid(
        state.media.items(),
        state.media.sorted_view(),
        |idx| state.tag_state(idx),
        &state.loaded_thumbs,
        state.media.burst_of(),
        state.media.burst_map(),
        state.today,
        state.window_scale,
        state.config.view.sort_order,
        state.config.view.ascending,
        state.config.view.group_raw_jpeg,
        state.hovered_thumbnail,
        state.hovered_star,
        state.focused_index,
        state.grid_scroll_y,
        state.grid_viewport_height,
    )
    .map(move |event| match event {
        views::thumbnails::Event::CellClicked(path) => {
            if shift_held {
                Message::Grid(grid_msg::Message::RangeTagTo(path))
            } else if command_held {
                Message::Grid(grid_msg::Message::FileTagToggled(path))
            } else {
                Message::Grid(grid_msg::Message::FileFocused(path))
            }
        }
        views::thumbnails::Event::CellDoubleClicked(idx) => {
            Message::Grid(grid_msg::Message::OpenPreview(idx))
        }
        views::thumbnails::Event::CellHover(idx, hovering) => {
            Message::Grid(grid_msg::Message::ThumbnailHover(idx, hovering))
        }
        views::thumbnails::Event::Rated(path, rating) => {
            Message::Grid(grid_msg::Message::FileRated(path, rating))
        }
        views::thumbnails::Event::StarHover(star) => {
            Message::Grid(grid_msg::Message::StarHover(star))
        }
        views::thumbnails::Event::BurstToggle(key) => {
            Message::Grid(grid_msg::Message::BurstToggled(key))
        }
        views::thumbnails::Event::Wheel(delta) => Message::Grid(grid_msg::Message::Wheel(delta)),
        views::thumbnails::Event::Scrolled {
            offset,
            grid_width,
            viewport_height,
            content_height,
        } => Message::Grid(grid_msg::Message::Scrolled {
            offset,
            grid_width,
            viewport_height,
            content_height,
        }),
    })
}

fn config_panel(state: &Ferrocull) -> Element<'_, Message> {
    let palette = crate::theme::palette();

    let destination_content =
        views::destination::destination_panel(&state.photos_dest, &state.videos_dest)
            .map(Message::Destination);
    let rename_content = views::rename::rename_panel(
        &state.photo_pattern,
        &state.video_pattern,
        &state.saved_patterns,
        state.today,
    )
    .map(Message::Destination);
    let backup_content =
        views::backup::backup_panel(&state.backup_destinations).map(Message::Destination);
    let hooks_content = views::hooks::hooks_panel(&state.hooks).map(Message::Profile);

    let profiles_content = views::profiles::profiles_panel(
        &state.profiles,
        state.current_profile.as_deref(),
        &state.profile_name_input,
    )
    .map(Message::Profile);

    let jobcode_content =
        views::jobcode::jobcode_panel(&state.job_code, state.job_code_history.codes())
            .map(Message::Destination);

    let delete_content =
        views::delete::delete_panel(state.delete_after_ingest).map(Message::Destination);

    let scrollable_content = column![
        profiles_content,
        Space::new().height(spacing::LG),
        collapsible_section(
            "Destination",
            state.sections.is_expanded(Section::Destination),
            Message::ToggleSection(Section::Destination),
            destination_content,
        ),
        Space::new().height(spacing::SM),
        collapsible_section(
            "Rename",
            state.sections.is_expanded(Section::Rename),
            Message::ToggleSection(Section::Rename),
            rename_content,
        ),
        Space::new().height(spacing::SM),
        jobcode_content,
        Space::new().height(spacing::SM),
        collapsible_section(
            "Backup",
            state.sections.is_expanded(Section::Backup),
            Message::ToggleSection(Section::Backup),
            backup_content,
        ),
        Space::new().height(spacing::SM),
        collapsible_section(
            "Hooks",
            state.sections.is_expanded(Section::Hooks),
            Message::ToggleSection(Section::Hooks),
            hooks_content,
        ),
        Space::new().height(spacing::LG),
        delete_content,
    ]
    .padding([0.0, spacing::MD])
    .spacing(spacing::XS);

    let header = container(text("CONFIG").size(11).color(palette.background.weak.text))
        .padding([spacing::SM, spacing::MD])
        .width(Fill);

    let content = column![header, scrollable(scrollable_content).height(Fill)];

    container(content)
        .width(state.panel_widths.right)
        .height(Fill)
        .style(styles::panel)
        .into()
}

/// Renders a labeled progress bar for the status bar.
fn progress_indicator<'a>(
    label: Option<&str>,
    completed: usize,
    total: usize,
) -> Element<'a, Message> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "progress percentage needs float"
    )]
    let pct = completed as f32 / total as f32;
    let label_text = label.map_or_else(
        || format!("{completed}/{total}"),
        |l| format!("{l}: {completed}/{total}"),
    );
    row![
        text(label_text).size(11),
        container(progress_bar(0.0..=1.0, pct).style(styles::storage_progress))
            .width(Length::Fixed(120.0)),
    ]
    .spacing(spacing::SM)
    .align_y(iced::Alignment::Center)
    .into()
}

fn status_bar(state: &Ferrocull) -> Element<'_, Message> {
    use crate::theme::colors;
    let palette = crate::theme::palette();

    let selected_count = state.selected.len();

    let visible_count = state.media.visible_len();
    let total_count = state.media.len();

    // Always-on session tally: rated (★) and rejected (✗), derived from the
    // counters MediaView maintains at every rating mutation.
    let tally = format!(
        "  \u{b7}  \u{2605}{}  \u{b7}  \u{2717}{}",
        state.media.rated_count(),
        state.media.rejected_count()
    );

    let left_text = if total_count == 0 {
        text("No files scanned")
            .size(12)
            .color(palette.background.strong.text)
    } else if visible_count < total_count {
        text(format!(
            "Showing {visible_count} of {total_count} — Tagged: {selected_count}{tally}"
        ))
        .size(12)
        .color(palette.background.base.text)
    } else {
        text(format!("Tagged: {selected_count} of {total_count}{tally}"))
            .size(12)
            .color(palette.background.base.text)
    };

    // Quiet undo-depth hint: present only while there is something to undo.
    let undo_depth = state.undo_stack.undo_len();
    let left: Element<'_, Message> = if undo_depth > 0 {
        row![
            left_text,
            text(format!("\u{21a9} {undo_depth}"))
                .size(11)
                .color(palette.background.base.text),
        ]
        .spacing(spacing::SM)
        .align_y(iced::Alignment::Center)
        .into()
    } else {
        left_text.into()
    };

    let mut progress_items: Vec<Element<'_, Message>> = Vec::new();

    if state.scanning {
        progress_items.push(
            text("Scanning...")
                .size(11)
                .color(palette.background.weak.text)
                .into(),
        );
    }

    if let Some(ref thumb) = state.thumbnail_progress {
        progress_items.push(progress_indicator(None, thumb.completed, thumb.total));
    }

    if let Some(ref ingest) = state.ingest_progress {
        progress_items.push(progress_indicator(
            Some("Ingest"),
            ingest.files_completed,
            ingest.total_files,
        ));
    }

    if !state.last_ingest_failures.is_empty() && state.ingest_progress.is_none() {
        let n = state.last_ingest_failures.len();
        let label = format!("{n} file{} failed to ingest — details", plural(n));
        progress_items.push(
            button(text(label).size(11).color(colors::DANGER))
                .padding(0)
                .style(styles::icon_button)
                .on_press(Message::Destination(
                    destination_msg::Message::ToggleIngestFailures,
                ))
                .into(),
        );
    }

    // Transient status message: errors in danger, action echoes in normal ink.
    if let Some(ref msg) = state.status_message {
        let (content, color) = match msg {
            StatusMessage::Error(m) => (m, colors::DANGER),
            StatusMessage::Echo(m) => (m, palette.background.base.text),
        };
        progress_items.push(text(content.as_str()).size(11).color(color).into());
    }

    let center: Element<'_, Message> = if progress_items.is_empty() {
        Space::new().width(0).into()
    } else {
        column(progress_items).spacing(2).into()
    };

    let ingest_btn = button(text("Ingest").size(12))
        .padding([6, 20])
        .style(styles::primary_button);
    let ingest_btn = if selected_count > 0 && state.ingest_progress.is_none() {
        ingest_btn.on_press(Message::Destination(destination_msg::Message::StartIngest))
    } else {
        ingest_btn
    };
    let ingest_tip = text(format!("Ingest {selected_count} tagged photos")).size(11);
    let ingest_with_tip = tooltip(ingest_btn, ingest_tip, tooltip::Position::Top)
        .gap(4)
        .snap_within_viewport(true);

    container(
        row![
            left,
            Space::new().width(Fill),
            center,
            Space::new().width(Fill),
            ingest_with_tip,
        ]
        .align_y(iced::Alignment::Center),
    )
    .width(Fill)
    .height(Length::Fixed(52.0))
    .padding([spacing::MD, spacing::LG])
    .style(styles::status_bar)
    .into()
}
