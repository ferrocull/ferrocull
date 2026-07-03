mod compare;
mod destination;
mod filters;
mod grid;
mod preview;
mod profile;
mod sources;

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::PathBuf,
};

use chrono::{DateTime, Utc};
use ferrocull_core::{
    ColorLabel, FileCategory, Hook, JobCodeHistory, NamedProfile, load_profiles,
    media::{CaptureTime, DateSelection, FilterMode, SortOrder},
    persistence::MediaDatabase,
    profiles_dir,
    thumbnail::parse_exif_from_bytes,
    xmp::{self, Metadata},
};
use ferrocull_devices::{ScannedFile, Source};
use iced::{
    Color, Element, Fill, Function, Length, Subscription, Task, Theme,
    futures::SinkExt,
    keyboard::{self, Event as KeyboardEvent, Key, Modifiers},
    widget::{
        Space, button, column, container, progress_bar, row, scrollable, stack, text, tooltip,
    },
};
use sipper::sipper;

use crate::{
    media_view::{MediaView, ViewParams},
    messages::{
        Message, Panel, Section, compare as compare_msg, destination as destination_msg,
        filters as filters_msg, grid as grid_msg, preview as preview_msg, profile as profile_msg,
        sources as sources_msg,
    },
    styles,
    theme::spacing,
    views::{self, GRID_SCROLLABLE_ID, GridCacheKey, collapsible_section},
};

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

const LEFT_PANEL_WIDTH: f32 = 250.0;
const RIGHT_PANEL_WIDTH: f32 = 300.0;

const DEFAULT_PATTERN: &str = "{YYYY}/{MM}/{DD}/{filename}.{ext}";
const THUMBNAIL_SIZE: u32 = 256;

struct ThumbnailProgress {
    total: usize,
    completed: usize,
}

struct DownloadProgress {
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

/// The user's filter/sort/grouping choices — the source of truth for what the
/// grid shows. A distinct struct so `config.params()` borrows only these
/// fields, leaving `&mut self.media` free at rebuild/insert sites.
///
/// Burst *expansion* is not here — `MediaView` owns it, since only its burst
/// re-keying logic can keep it consistent.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent user-toggle flags for the view configuration"
)]
struct ViewConfig {
    sort_order: SortOrder,
    /// Display direction (applied at read time, so not part of `ViewParams`).
    ascending: bool,
    filter_mode: FilterMode,
    hide_rejected: bool,
    group_raw_jpeg: bool,
    group_bursts: bool,
    selected_sources: BTreeSet<PathBuf>,
    selected_dates: Option<DateSelection>,
    selected_ratings: BTreeSet<i8>,
    selected_color_labels: BTreeSet<Option<ColorLabel>>,
}

impl ViewConfig {
    fn with_defaults() -> Self {
        Self {
            sort_order: SortOrder::default(),
            ascending: true,
            filter_mode: FilterMode::default(),
            hide_rejected: false,
            group_raw_jpeg: true,
            group_bursts: true,
            selected_sources: BTreeSet::new(),
            selected_dates: None,
            selected_ratings: BTreeSet::new(),
            selected_color_labels: BTreeSet::new(),
        }
    }

    /// Borrow the config as [`ViewParams`] for a `MediaView` operation.
    fn params(&self) -> ViewParams<'_> {
        ViewParams {
            sort_order: self.sort_order,
            filter_mode: self.filter_mode,
            hide_rejected: self.hide_rejected,
            group_raw_jpeg: self.group_raw_jpeg,
            group_bursts: self.group_bursts,
            selected_sources: &self.selected_sources,
            selected_dates: self.selected_dates,
            selected_ratings: &self.selected_ratings,
            selected_color_labels: &self.selected_color_labels,
        }
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
    selected: BTreeSet<usize>,
    sources: Vec<Source>,
    photos_dest: String,
    videos_dest: String,
    photo_pattern: String,
    video_pattern: String,
    download_progress: Option<DownloadProgress>,
    /// Failure count from last download (shown in status bar until next action).
    last_download_failures: usize,
    /// Transient status message shown in status bar (e.g. profile save errors, DB errors).
    status_message: Option<String>,
    thumbnail_progress: Option<ThumbnailProgress>,
    scan_jobs_in_flight: usize,
    thumbnail_jobs_in_flight: usize,
    scanning: bool,
    job_code: String,
    job_code_history: JobCodeHistory,
    jobcode_path: PathBuf,
    backup_destinations: Vec<PathBuf>,
    profiles: Vec<NamedProfile>,
    current_profile: Option<String>,
    profile_name_input: String,
    hooks: Vec<Hook>,
    delete_after_download: bool,
    /// Persistent database connection for sync reads and writes. Sync is
    /// acceptable: `SQLite` WAL writes are sub-ms for local storage, well under
    /// iced's 16ms frame budget.
    db: MediaDatabase,
    sections: SectionState,
    expanded_years: BTreeSet<i32>,
    expanded_months: BTreeSet<(i32, u32)>,
    loaded_thumbs: HashMap<PathBuf, iced::widget::image::Handle>,
    grid_viewport_width: f32,
    hovered_thumbnail: Option<usize>,
    hovered_star: Option<i8>,
    focused_index: Option<usize>,
    modifiers: Modifiers,
    left_panel_visible: bool,
    right_panel_visible: bool,
    preview_cache: HashMap<PathBuf, iced::widget::image::Allocation>,
    /// In-flight preview requests, keyed by path and tagged with generation.
    preview_loading: HashMap<PathBuf, u64>,
    /// Monotonic generation to discard stale async preview loads.
    preview_generation: u64,
    view_mode: ViewMode,
    /// Current date for "Today"/"Yesterday" headers. Updated on each message.
    today: chrono::NaiveDate,
}

impl Default for Ferrocull {
    fn default() -> Self {
        let profiles = profiles_dir()
            .and_then(|dir| load_profiles(&dir))
            .unwrap_or_else(|e| {
                tracing::warn!("failed to load profiles: {e}");
                Vec::new()
            });
        let sources = Vec::new();
        let data_dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Ferrocull");
        let db_path = data_dir.join("ferrocull.db");
        let jobcode_path = data_dir.join("jobcodes.json");

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
        let jobcode_history = JobCodeHistory::load(&jobcode_path).unwrap_or_else(|e| {
            tracing::warn!("failed to load jobcode history: {e}");
            JobCodeHistory::default()
        });

        Self {
            media: MediaView::new(),
            config: ViewConfig::with_defaults(),
            selected: BTreeSet::new(),
            sources,
            photos_dest: dirs::picture_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .to_string_lossy()
                .into_owned(),
            videos_dest: dirs::video_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .to_string_lossy()
                .into_owned(),
            photo_pattern: String::from(DEFAULT_PATTERN),
            video_pattern: String::from(DEFAULT_PATTERN),
            download_progress: None,
            last_download_failures: 0,
            status_message: None,
            thumbnail_progress: None,
            scan_jobs_in_flight: 0,
            thumbnail_jobs_in_flight: 0,
            scanning: false,
            job_code: String::new(),
            job_code_history: jobcode_history,
            jobcode_path,
            backup_destinations: Vec::new(),
            profiles,
            current_profile: None,
            profile_name_input: String::new(),
            hooks: Vec::new(),
            delete_after_download: false,
            db,
            sections: SectionState::with_defaults(),
            expanded_years: BTreeSet::new(),
            expanded_months: BTreeSet::new(),
            loaded_thumbs: HashMap::new(),
            grid_viewport_width: 0.0,
            hovered_thumbnail: None,
            hovered_star: None,
            focused_index: None,
            modifiers: Modifiers::default(),
            left_panel_visible: true,
            right_panel_visible: true,
            preview_cache: HashMap::new(),
            preview_loading: HashMap::new(),
            preview_generation: 0,
            view_mode: ViewMode::Grid,
            today: chrono::Local::now().date_naive(),
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
        self.media.first_index(self.config.ascending)
    }

    fn last_index(&self) -> Option<usize> {
        self.media.last_index(self.config.ascending)
    }

    /// Produce a task that scrolls the grid so `target_idx` is visible.
    fn scroll_grid_to_item(&self, target_idx: usize) -> Task<Message> {
        let Some(position) = self.ordinal_position(target_idx) else {
            return Task::none();
        };
        let len = self.media.visible_len();
        #[expect(
            clippy::cast_precision_loss,
            reason = "scroll position as float is fine"
        )]
        // Divide by the last index, not the count, so the final item snaps to the
        // viewport bottom (offset 1.0) rather than being clipped off it.
        let fraction = if len <= 1 {
            0.0
        } else {
            position as f32 / (len - 1) as f32
        };
        iced::widget::operation::snap_to(
            GRID_SCROLLABLE_ID,
            scrollable::RelativeOffset {
                x: 0.0,
                y: fraction,
            },
        )
    }

    fn adjacent_index(&self, current: usize, forward: bool) -> Option<usize> {
        self.media.adjacent_index(
            current,
            forward,
            self.config.ascending,
            self.config.sort_order,
        )
    }

    fn ordinal_position(&self, item_idx: usize) -> Option<usize> {
        self.media.ordinal_position(item_idx, self.config.ascending)
    }

    /// Indices to operate on: burst members if collapsed, else just `idx`.
    fn target_indices(&self, idx: usize) -> Vec<usize> {
        self.media
            .collapsed_burst_members(idx, self.config.group_bursts)
            .map_or_else(|| vec![idx], <[usize]>::to_vec)
    }

    /// Set selection state for an item and its JPEG pair if grouping is enabled.
    fn set_selection(&mut self, idx: usize, select: bool) {
        if select {
            self.selected.insert(idx);
        } else {
            self.selected.remove(&idx);
        }

        if !self.config.group_raw_jpeg {
            return;
        }

        if let Some(jpeg_idx) = self
            .media
            .item(idx)
            .jpeg_pair
            .as_ref()
            .and_then(|jpeg| self.media.index_of(jpeg))
        {
            if select {
                self.selected.insert(jpeg_idx);
            } else {
                self.selected.remove(&jpeg_idx);
            }
        }
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
    fn handle_key_press(&mut self, key: &Key, modifiers: Modifiers) -> Task<Message> {
        use keyboard::key::Named;

        match key {
            Key::Character(c) => {
                return self.handle_character_key(c, modifiers);
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
                return handle_arrow_key(key, in_preview);
            }
            Key::Named(Named::Space | Named::Enter) => {
                if matches!(self.view_mode, ViewMode::Grid)
                    && let Some(idx) = self.focused_index
                {
                    return Task::done(Message::Grid(grid_msg::Message::OpenPreview(idx)));
                }
            }
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

    fn handle_character_key(&self, c: &str, modifiers: Modifiers) -> Task<Message> {
        let Some(ch) = c.chars().next() else {
            return Task::none();
        };

        let target_idx = match self.view_mode {
            ViewMode::Compare(ref cmp) => Some(match cmp.active_pane {
                compare_msg::Pane::Select => cmp.select_index,
                compare_msg::Pane::Candidate => cmp.candidate_index,
            }),
            ViewMode::Preview(ref p) => Some(p.index),
            ViewMode::Grid => self.focused_index,
        };

        // Selection keys: +/- work regardless of shift state
        match ch {
            '+' | '=' => {
                return self.action_on_target(
                    target_idx,
                    |path| Message::Grid(grid_msg::Message::FileSelected(path)),
                    false,
                );
            }
            '-' | '_' => {
                return self.action_on_target(
                    target_idx,
                    |path| Message::Grid(grid_msg::Message::FileDeselected(path)),
                    false,
                );
            }
            _ => {}
        }

        // Cmd+0-7 (Mac) / Ctrl+0-7 (Win/Linux): color label
        if modifiers.command() {
            match ch {
                '0'..='7' => {
                    let digit = ch as u8 - b'0';
                    let label = ColorLabel::try_from(digit).ok();
                    return self.action_on_target(
                        target_idx,
                        |path| Message::Grid(grid_msg::Message::FileColorLabelSet(path, label)),
                        false,
                    );
                }
                'a' | 'A' => return Task::done(Message::Grid(grid_msg::Message::SelectAll)),
                'd' | 'D' => return Task::done(Message::Grid(grid_msg::Message::SelectNone)),
                _ => {}
            }
        } else if !modifiers.shift() && !modifiers.alt() {
            return self.handle_unmodified_char(ch, target_idx);
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
            'z' | 'Z' if in_preview => {
                Task::done(Message::Preview(preview_msg::Message::ResetZoom))
            }
            // 0-5: star rating (Photo Mechanic convention)
            '0'..='5' => {
                #[expect(
                    clippy::cast_possible_wrap,
                    reason = "'0'..='5' yields 0..=5, well within i8 range"
                )]
                let rating = (ch as u8 - b'0') as i8;
                self.action_on_target(
                    target_idx,
                    |path| Message::Grid(grid_msg::Message::FileRated(path, rating)),
                    rating > 0,
                )
            }
            // T: tag toggle (selection toggle, Photo Mechanic convention)
            't' | 'T' => self.action_on_target(
                target_idx,
                |path| Message::Grid(grid_msg::Message::FileSelectionToggled(path)),
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

    let forward = match key {
        Key::Named(Named::ArrowRight | Named::ArrowDown) => true,
        Key::Named(Named::ArrowLeft | Named::ArrowUp) => false,
        _ => return Task::none(),
    };

    let msg = match (in_preview, forward) {
        (true, true) => Message::Preview(preview_msg::Message::Next),
        (true, false) => Message::Preview(preview_msg::Message::Prev),
        (false, true) => Message::Grid(grid_msg::Message::FocusNext),
        (false, false) => Message::Grid(grid_msg::Message::FocusPrev),
    };
    Task::done(msg)
}

/// Result type for sipper: either EXIF data or disk cache signal.
enum SipperResult {
    Exif(ScannedFile, CaptureTime, Option<Metadata>),
    ThumbnailCached(PathBuf, Result<(), String>),
}

/// Spawn sipper that extracts EXIF first (creating items), then generates thumbnails.
fn spawn_thumbnail_sipper(files: Vec<ScannedFile>) -> Task<Message> {
    use std::io::Read;

    use ferrocull_core::{
        cache::{ThumbnailCache, cache_key_from_disk},
        thumbnail::{generate_raw_with_preread, generate_thumbnail_from_bytes},
    };
    use rayon::prelude::*;

    const INITIAL_READ: usize = 2 * 1024 * 1024;

    let thumb_sipper = sipper(move |mut sender| async move {
        let cache = ThumbnailCache::open().ok();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        rayon::spawn(move || {
            files.into_par_iter().for_each(|scanned| {
                let path = scanned.path.clone();

                let file_result = (|| -> Result<(Vec<u8>, std::fs::File), std::io::Error> {
                    let mut file = std::fs::File::open(&path)?;
                    let len = file.metadata()?.len();
                    let initial = usize::try_from(len)
                        .unwrap_or(usize::MAX)
                        .min(INITIAL_READ);
                    let mut data = vec![0u8; initial];
                    file.read_exact(&mut data)?;
                    Ok((data, file))
                })();

                let (data, mut file) = match file_result {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::error!(path = %path.display(), error = %e, "failed to read file, skipping");
                        return;
                    }
                };

                // Fall back to file modification time if EXIF has no date.
                let capture_time = parse_exif_from_bytes(&data).unwrap_or_else(|| {
                    let mtime = file.metadata()
                        .expect("file already opened")
                        .modified()
                        .expect("modification time available");
                    CaptureTime::new(DateTime::<Utc>::from(mtime), 0)
                });
                let xmp_metadata = scanned
                    .xmp_sidecar
                    .as_ref()
                    .and_then(|xmp_path| xmp::read_sidecar(xmp_path).ok());
                let media_type = scanned.media_type;

                drop(tx.send(SipperResult::Exif(
                    scanned,
                    capture_time,
                    xmp_metadata,
                )));

                let key = match cache_key_from_disk(&path) {
                    Ok(k) => Some(k),
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "cache key derivation failed, bypassing cache");
                        None
                    }
                };
                if let Some(ref k) = key
                    && let Some(ref c) = cache
                    && let Ok(Some(_)) = c.load(k)
                {
                    drop(tx.send(SipperResult::ThumbnailCached(path, Ok(()))));
                    return;
                }

                let mut data = data;
                let thumb_result = match media_type {
                    FileCategory::Photo => {
                        if let Err(e) = file.read_to_end(&mut data) {
                            Err(e.to_string())
                        } else {
                            generate_thumbnail_from_bytes(&data, &path, THUMBNAIL_SIZE)
                                .map(|r| r.jpeg)
                                .map_err(|e| e.to_string())
                        }
                    }
                    FileCategory::Raw => {
                        generate_raw_with_preread(data, &mut file, THUMBNAIL_SIZE, &path)
                            .map_err(|e| e.to_string())
                    }
                    _ => Err("unsupported format".to_owned()),
                };

                if let Ok(ref img) = thumb_result
                    && let Some(ref k) = key
                    && let Some(ref c) = cache
                {
                    drop(c.put(k, img));
                }

                // Signal completion — no pixel data sent to UI
                drop(tx.send(SipperResult::ThumbnailCached(
                    path,
                    thumb_result.map(|_| ()),
                )));
            });
        });

        while let Some(item) = rx.recv().await {
            sender.send(item).await;
        }
    });

    Task::sip(
        thumb_sipper,
        |result| match result {
            SipperResult::Exif(scanned, time, xmp) => Message::ExifLoaded(scanned, time, xmp),
            SipperResult::ThumbnailCached(path, res) => Message::ThumbnailCached(path, res),
        },
        |()| Message::ThumbnailsComplete,
    )
}

fn boot() -> (Ferrocull, Task<Message>) {
    // Seed the theme cache synchronously so the first frame uses the correct
    // OS preference rather than the Light fallback.
    crate::theme::set_os_is_dark(crate::theme::detect_os_is_dark());

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
        KeyboardEvent::KeyPressed { key, modifiers, .. } => {
            Some(Message::KeyPressed(key, modifiers))
        }
        KeyboardEvent::ModifiersChanged(modifiers) => Some(Message::ModifiersChanged(modifiers)),
        KeyboardEvent::KeyReleased { .. } => None,
    });
    let tick = iced::time::every(std::time::Duration::from_mins(1)).map(|_| Message::Tick);
    let devices = Subscription::run(device_events);
    Subscription::batch([keys, tick, devices])
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

            // The Linux watcher is an async task; the macOS/Windows watchers run on
            // their own OS thread and return immediately.
            #[cfg(target_os = "linux")]
            let _watcher = tokio::spawn(async move {
                if let Err(e) = ferrocull_devices::watch(tx).await {
                    tracing::error!("device watch ended: {e}");
                }
            });
            #[cfg(not(target_os = "linux"))]
            if let Err(e) = ferrocull_devices::watch(tx) {
                tracing::error!("device watch failed to start: {e}");
            }

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

#[expect(
    clippy::too_many_lines,
    reason = "TEA dispatch: delegates to sub-functions, remaining arms are async result handlers"
)]
fn update(state: &mut Ferrocull, message: Message) -> Task<Message> {
    match message {
        Message::Compare(msg) => compare::update(state, msg),
        Message::Grid(msg) => grid::update(state, msg),
        Message::Sources(msg) => sources::update(state, msg),
        Message::Destination(msg) => destination::update(state, msg),
        Message::Filters(msg) => filters::update(state, msg),
        Message::Preview(msg) => preview::update(state, msg),
        Message::Profile(msg) => profile::update(state, msg),

        Message::ToggleSection(section) => {
            state.sections.toggle(section);
            Task::none()
        }
        Message::TogglePanel(panel) => {
            match panel {
                Panel::Left => state.left_panel_visible = !state.left_panel_visible,
                Panel::Right => state.right_panel_visible = !state.right_panel_visible,
            }
            Task::none()
        }
        Message::KeyPressed(ref key, modifiers) => state.handle_key_press(key, modifiers),
        Message::ModifiersChanged(modifiers) => {
            state.modifiers = modifiers;
            Task::none()
        }

        Message::ExifLoaded(scanned, time, xmp) => {
            state.handle_exif_loaded(scanned, time, xmp.as_ref());
            Task::none()
        }
        Message::ThumbnailCached(_path, _result) => {
            state.handle_thumbnail_cached();
            // Dirty the grid so its sensors re-fire for newly cached thumbs.
            state.media.mark_dirty();
            Task::none()
        }
        Message::ScanComplete(files) => state.handle_scan_complete(files),
        Message::ThumbnailsComplete => {
            state.thumbnail_jobs_in_flight = state.thumbnail_jobs_in_flight.saturating_sub(1);
            if state.thumbnail_jobs_in_flight == 0 {
                state.thumbnail_progress = None;
            }
            state.rebuild_view();
            Task::none()
        }
        Message::DownloadProgressUpdate(n) => {
            if let Some(ref mut dl) = state.download_progress {
                dl.files_completed = n;
            }
            Task::none()
        }
        Message::DownloadComplete(result) => state.handle_download_complete(&result),
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
            state.media.mark_dirty();
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
        Message::HooksComplete | Message::Noop => Task::none(),
        Message::ProfileSaved(result) => {
            state.handle_profile_saved(result);
            Task::none()
        }
        Message::ProfileDeleted(result) => {
            state.handle_profile_deleted(result);
            Task::none()
        }
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
        main_row = main_row
            .push(sources_panel(state))
            .push(panel_edge_handle(Panel::Left, true));
    } else {
        main_row = main_row.push(panel_edge_handle(Panel::Left, false));
    }

    main_row = main_row.push(thumbnails_panel(state));

    if state.right_panel_visible {
        main_row = main_row
            .push(panel_edge_handle(Panel::Right, true))
            .push(config_panel(state));
    } else {
        main_row = main_row.push(panel_edge_handle(Panel::Right, false));
    }

    let main_content = column![main_row, status_bar(state)];

    // Always use stack! so the widget tree root type is consistent across all
    // modes. Without this, switching between stack![main, overlay] and bare
    // main_content changes the root widget type, which makes iced discard the
    // entire widget state tree — including the grid's scroll position.
    match state.view_mode {
        ViewMode::Compare(ref cmp) => stack![main_content, compare_overlay(state, cmp)].into(),
        ViewMode::Preview(ref p) => stack![main_content, preview_overlay(state, p)].into(),
        ViewMode::Grid => stack![main_content].into(),
    }
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
    .map(views::compare::Event::Item.with(active_path));

    let top = views::compare::top_bar(
        select_item,
        candidate_item,
        state.ordinal_position(cmp.select_index),
        state.media.visible_len(),
        active_pane,
        cmp.lock_scroll,
    );
    let select_pane = views::compare::image_pane(
        state
            .preview_cache
            .get(&select_item.path)
            .map(iced::widget::image::Allocation::handle),
        active_pane == compare_msg::Pane::Select,
        cmp.select_view_state,
        "SELECT",
    );
    let candidate_pane = views::compare::image_pane(
        state
            .preview_cache
            .get(&candidate_item.path)
            .map(iced::widget::image::Allocation::handle),
        active_pane == compare_msg::Pane::Candidate,
        cmp.candidate_view_state,
        "CANDIDATE",
    );
    let bottom = views::compare::bottom_bar(cmp.layout, item_ctrl);

    views::compare::compose(cmp.layout, top, select_pane, candidate_pane, bottom).map(|event| {
        match event {
            views::compare::Event::Close => Message::Compare(compare_msg::Message::Exit),
            views::compare::Event::ToggleLock => {
                Message::Compare(compare_msg::Message::ToggleLockScroll)
            }
            views::compare::Event::Prev => Message::Compare(compare_msg::Message::CandidatePrev),
            views::compare::Event::Next => Message::Compare(compare_msg::Message::CandidateNext),
            views::compare::Event::Promote => Message::Compare(compare_msg::Message::Promote),
            views::compare::Event::SwitchHorizontal => {
                Message::Compare(compare_msg::Message::EnterHorizontal)
            }
            views::compare::Event::SwitchVertical => {
                Message::Compare(compare_msg::Message::EnterVertical)
            }
            views::compare::Event::SetActivePane(pane) => {
                Message::Compare(compare_msg::Message::ActivePaneChanged(pane))
            }
            views::compare::Event::ViewStateChanged(pane, e) => {
                Message::Compare(compare_msg::Message::ViewStateChanged(pane, e))
            }
            views::compare::Event::Item(path, item_event) => map_item_event(path, item_event),
        }
    })
}

fn preview_overlay(state: &Ferrocull, p: &PreviewState) -> Element<'static, Message> {
    let item = state.media.item(p.index);
    let item_path = item.path.clone();

    let item_ctrl = views::rating::item_controls(item.rating, item.color_label, state.hovered_star)
        .map(views::preview::Event::Item.with(item_path));

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
    );
    let bottom = views::preview::bottom_bar(item_ctrl);

    views::preview::compose(top, image, bottom).map(|event| match event {
        views::preview::Event::Close => Message::Preview(preview_msg::Message::Close),
        views::preview::Event::Prev => Message::Preview(preview_msg::Message::Prev),
        views::preview::Event::Next => Message::Preview(preview_msg::Message::Next),
        views::preview::Event::ViewStateChanged(e) => {
            Message::Preview(preview_msg::Message::ViewStateChanged(e))
        }
        views::preview::Event::Item(path, item_event) => map_item_event(path, item_event),
    })
}

/// Clickable edge handle for collapsing/expanding panels.
fn panel_edge_handle(panel: Panel, expanded: bool) -> Element<'static, Message> {
    let palette = crate::theme::palette();

    let icon = match (panel, expanded) {
        (Panel::Left, true) | (Panel::Right, false) => "«",
        (Panel::Left, false) | (Panel::Right, true) => "»",
    };

    let width = if expanded { 8.0 } else { 14.0 };

    let content = container(text(icon).size(9).color(palette.background.strong.text))
        .width(width)
        .height(Fill)
        .center_x(width)
        .center_y(Fill);

    let label = match (panel, expanded) {
        (Panel::Left, true) => "Hide sources",
        (Panel::Left, false) => "Show sources",
        (Panel::Right, true) => "Hide config",
        (Panel::Right, false) => "Show config",
    };

    let btn = button(content)
        .padding(0)
        .style(styles::panel_handle(expanded))
        .on_press(Message::TogglePanel(panel));

    tooltip(btn, text(label).size(10), tooltip::Position::Right)
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
        views::sources::sources_panel(&state.sources, &state.config.selected_sources).map(|e| {
            match e {
                views::sources::Event::Toggle(path) => {
                    Message::Sources(sources_msg::Message::SourceToggled(path))
                }
                views::sources::Event::Mount(path) => {
                    Message::Sources(sources_msg::Message::MountStorage(path))
                }
                views::sources::Event::Unmount(path) => {
                    Message::Sources(sources_msg::Message::UnmountStorage(path))
                }
                views::sources::Event::AddDirectory => {
                    Message::Sources(sources_msg::Message::AddDirectoryClicked)
                }
                views::sources::Event::Refresh => {
                    Message::Sources(sources_msg::Message::RefreshSources)
                }
            }
        });

    let dates = views::date_tree::date_tree(
        state.media.items(),
        state.media.version(),
        state.config.selected_dates,
        &state.expanded_years,
        &state.expanded_months,
    )
    .map(|e| match e {
        views::date_tree::Event::DateToggled(sel) => {
            Message::Filters(filters_msg::Message::DateToggled(sel))
        }
        views::date_tree::Event::YearExpanded(year) => {
            Message::Filters(filters_msg::Message::YearExpanded(year))
        }
        views::date_tree::Event::MonthExpanded(year, month) => {
            Message::Filters(filters_msg::Message::MonthExpanded(year, month))
        }
    });

    let content = column![
        header,
        scrollable(
            column![sources_view, Space::new().height(spacing::MD), dates]
                .padding([0.0, spacing::SM])
        )
        .height(Fill),
    ];

    container(content)
        .width(LEFT_PANEL_WIDTH)
        .height(Fill)
        .style(styles::panel)
        .into()
}

fn thumbnails_panel(state: &Ferrocull) -> Element<'_, Message> {
    let palette = crate::theme::palette();

    let filters_view = views::filters::filter_bar(
        views::filters::sort_controls(state.config.sort_order, state.config.ascending),
        views::filters::filter_mode_controls(state.config.filter_mode),
        views::filters::grouping_controls(
            state.config.group_raw_jpeg,
            state.config.group_bursts,
            state.config.hide_rejected,
        ),
        views::filters::rating_filter(&state.config.selected_ratings),
        views::filters::color_label_filter(&state.config.selected_color_labels),
    )
    .map(|event| match event {
        views::filters::Event::SortChanged(order) => {
            Message::Filters(filters_msg::Message::SortChanged(order))
        }
        views::filters::Event::AscendingToggled => {
            Message::Filters(filters_msg::Message::AscendingToggled)
        }
        views::filters::Event::FilterChanged(mode) => {
            Message::Filters(filters_msg::Message::FilterChanged(mode))
        }
        views::filters::Event::GroupRawJpegToggled => {
            Message::Filters(filters_msg::Message::GroupRawJpegToggled)
        }
        views::filters::Event::GroupBurstsToggled => {
            Message::Filters(filters_msg::Message::GroupBurstsToggled)
        }
        views::filters::Event::HideRejectedToggled => {
            Message::Filters(filters_msg::Message::HideRejectedToggled)
        }
        views::filters::Event::RatingFilterToggled(rating) => {
            Message::Filters(filters_msg::Message::RatingFilterToggled(rating))
        }
        views::filters::Event::ColorLabelFilterToggled(label) => {
            Message::Filters(filters_msg::Message::ColorLabelFilterToggled(label))
        }
    });

    let header = container(filters_view)
        .padding([spacing::SM, spacing::MD])
        .width(Fill);

    let grid = thumbnail_grid(state);

    let has_filters_active = !state.config.selected_ratings.is_empty()
        || !state.config.selected_color_labels.is_empty()
        || state.config.selected_dates.is_some()
        || state.config.filter_mode != FilterMode::default()
        || state.config.hide_rejected;

    let content: Element<'_, Message> =
        if state.media.is_view_empty() && !state.media.is_empty() && has_filters_active {
            let empty_state = column![
                text("No photos match current filters")
                    .size(14)
                    .color(palette.background.strong.text),
                button(text("Clear Filters").size(12).color(Color::WHITE))
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

    let cache_key = GridCacheKey {
        viewport_width_bits: state.grid_viewport_width.to_bits(),
        item_count: state.media.len(),
        item_content_version: state.media.version(),
        selected: state.selected.clone(),
        filter_mode: state.config.filter_mode,
        group_raw_jpeg: state.config.group_raw_jpeg,
        hide_rejected: state.config.hide_rejected,
        sort_order: state.config.sort_order,
        ascending: state.config.ascending,
        selected_dates: state.config.selected_dates,
        selected_sources: state.config.selected_sources.clone(),
        group_bursts: state.config.group_bursts,
        expanded_bursts: state.media.expanded_bursts().clone(),
        selected_ratings: state.config.selected_ratings.clone(),
        selected_color_labels: state.config.selected_color_labels.clone(),
        hovered_thumbnail: state.hovered_thumbnail,
        hovered_star: state.hovered_star,
        focused_index: state.focused_index,
    };

    views::thumbnail_grid(
        state.media.items(),
        state.media.sorted_view(),
        &state.selected,
        &state.loaded_thumbs,
        state.media.burst_of(),
        state.media.burst_map(),
        state.today,
        cache_key,
    )
    .map(move |event| match event {
        views::thumbnails::Event::CellClicked(path) => {
            if command_held {
                Message::Grid(grid_msg::Message::FileSelectionToggled(path))
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
        views::thumbnails::Event::ThumbnailVisible(idx) => {
            Message::Grid(grid_msg::Message::ThumbnailVisible(idx))
        }
        views::thumbnails::Event::ThumbnailHidden(idx) => {
            Message::Grid(grid_msg::Message::ThumbnailHidden(idx))
        }
        views::thumbnails::Event::Scrolled(vp) => Message::Grid(grid_msg::Message::Scrolled(vp)),
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "TEA view: each panel needs its own .map() closure"
)]
fn config_panel(state: &Ferrocull) -> Element<'_, Message> {
    let palette = crate::theme::palette();

    let destination_content =
        views::destination::destination_panel(&state.photos_dest, &state.videos_dest).map(|e| {
            match e {
                views::destination::Event::PhotosDestChanged(s) => {
                    Message::Destination(destination_msg::Message::PhotosDestChanged(s))
                }
                views::destination::Event::VideosDestChanged(s) => {
                    Message::Destination(destination_msg::Message::VideosDestChanged(s))
                }
                views::destination::Event::BrowsePhotos => {
                    Message::Destination(destination_msg::Message::BrowsePhotosDest)
                }
                views::destination::Event::BrowseVideos => {
                    Message::Destination(destination_msg::Message::BrowseVideosDest)
                }
            }
        });
    let rename_content =
        views::rename::rename_panel(&state.photo_pattern, &state.video_pattern, state.today).map(
            |e| match e {
                views::rename::Event::PhotoPatternChanged(s) => {
                    Message::Destination(destination_msg::Message::PhotoPatternChanged(s))
                }
                views::rename::Event::VideoPatternChanged(s) => {
                    Message::Destination(destination_msg::Message::VideoPatternChanged(s))
                }
            },
        );
    let backup_content = views::backup::backup_panel(&state.backup_destinations).map(|e| match e {
        views::backup::Event::Add => {
            Message::Destination(destination_msg::Message::AddBackupClicked)
        }
        views::backup::Event::Remove(idx) => {
            Message::Destination(destination_msg::Message::RemoveBackup(idx))
        }
    });
    let hooks_content = views::hooks::hooks_panel(&state.hooks).map(|e| match e {
        views::hooks::Event::Add => Message::Profile(profile_msg::Message::HookAddRequested),
        views::hooks::Event::Remove(idx) => {
            Message::Profile(profile_msg::Message::HookRemoved(idx))
        }
        views::hooks::Event::Toggle(idx) => {
            Message::Profile(profile_msg::Message::HookToggled(idx))
        }
        views::hooks::Event::Edit(idx, cmd) => {
            Message::Profile(profile_msg::Message::HookCommandEdited(idx, cmd))
        }
    });

    let profiles_content = views::profiles::profiles_panel(
        &state.profiles,
        state.current_profile.as_deref(),
        &state.profile_name_input,
    )
    .map(|e| match e {
        views::profiles::Event::Load(name) => {
            Message::Profile(profile_msg::Message::ProfileSelected(name))
        }
        views::profiles::Event::Save => Message::Profile(profile_msg::Message::SaveRequested),
        views::profiles::Event::Delete(name) => {
            Message::Profile(profile_msg::Message::DeleteRequested(name))
        }
        views::profiles::Event::NameChanged(name) => {
            Message::Profile(profile_msg::Message::NameChanged(name))
        }
    });

    let jobcode_content =
        views::jobcode::jobcode_panel(&state.job_code, state.job_code_history.codes()).map(|e| {
            match e {
                views::jobcode::Event::Changed(s) => {
                    Message::Destination(destination_msg::Message::JobCodeChanged(s))
                }
                views::jobcode::Event::Selected(s) => {
                    Message::Destination(destination_msg::Message::JobCodeSelected(s))
                }
            }
        });

    let delete_content =
        views::delete::delete_panel(state.delete_after_download).map(|e| match e {
            views::delete::Event::Toggled => {
                Message::Destination(destination_msg::Message::DeleteAfterDownloadToggled)
            }
        });

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
        .width(RIGHT_PANEL_WIDTH)
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

    let left_text = if total_count == 0 {
        text("No files scanned")
            .size(12)
            .color(palette.background.strong.text)
    } else if visible_count < total_count {
        text(format!(
            "Showing {visible_count} of {total_count} — Selected: {selected_count}"
        ))
        .size(12)
        .color(palette.background.base.text)
    } else {
        text(format!("Selected: {selected_count} files"))
            .size(12)
            .color(palette.background.base.text)
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

    if let Some(ref dl) = state.download_progress {
        progress_items.push(progress_indicator(
            Some("Import"),
            dl.files_completed,
            dl.total_files,
        ));
    }

    if state.last_download_failures > 0 && state.download_progress.is_none() {
        let n = state.last_download_failures;
        progress_items.push(
            text(format!(
                "{n} file{} failed to import",
                if n == 1 { "" } else { "s" }
            ))
            .size(11)
            .color(colors::DANGER)
            .into(),
        );
    }

    // Transient status message (profile/DB errors)
    if let Some(ref msg) = state.status_message {
        progress_items.push(text(msg.as_str()).size(11).color(colors::DANGER).into());
    }

    let center: Element<'_, Message> = if progress_items.is_empty() {
        Space::new().width(0).into()
    } else {
        column(progress_items).spacing(2).into()
    };

    let import_btn = button(text("Start Import").size(12).color(Color::WHITE))
        .padding([6, 20])
        .style(styles::primary_button);
    let import_btn = if selected_count > 0 && state.download_progress.is_none() {
        import_btn.on_press(Message::Destination(
            destination_msg::Message::StartDownload,
        ))
    } else {
        import_btn
    };
    let import_tip = text(format!("Import {selected_count} selected photos")).size(11);
    let import_with_tip = tooltip(import_btn, import_tip, tooltip::Position::Top)
        .gap(4)
        .snap_within_viewport(true);

    container(
        row![
            left_text,
            Space::new().width(Fill),
            center,
            Space::new().width(Fill),
            import_with_tip,
        ]
        .align_y(iced::Alignment::Center),
    )
    .width(Fill)
    .height(Length::Fixed(52.0))
    .padding([spacing::MD, spacing::LG])
    .style(styles::status_bar)
    .into()
}
