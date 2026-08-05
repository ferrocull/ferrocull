pub(crate) mod compare {
    use crate::widgets;

    /// Compare mode layout orientation.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub(crate) enum Layout {
        /// Side-by-side (H key)
        #[default]
        Horizontal,
        /// Stacked vertically (V key)
        Vertical,
    }

    /// Which pane is active for navigation.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub(crate) enum Pane {
        /// Left/Top pane - the "keeper"
        #[default]
        Select,
        /// Right/Bottom pane - the "challenger"
        Candidate,
    }

    /// Messages for compare mode interactions.
    #[derive(Debug, Clone, Copy)]
    pub(crate) enum Message {
        /// Enter compare mode with horizontal layout (H key)
        EnterHorizontal,
        /// Enter compare mode with vertical layout (V key)
        EnterVertical,
        /// Exit compare mode (O or Escape)
        Exit,
        /// Promote candidate to select, find new candidate (G key)
        Promote,
        /// Toggle synchronized zoom/pan (L key)
        ToggleLockScroll,
        /// Active pane changed (for ratings/actions)
        ActivePaneChanged(Pane),
        /// Navigate candidate to next image (arrows)
        CandidateNext,
        /// Navigate candidate to previous image (arrows)
        CandidatePrev,
        CandidateNavigateTo(usize),
        /// View state changed (zoom/pan from `Viewer`) for a pane.
        ViewStateChanged(Pane, widgets::Event),
        /// Reset zoom to fit (Z key)
        ResetZoom,
    }
}

pub(crate) mod grid {
    use std::path::PathBuf;

    use chrono::{DateTime, Utc};
    use ferrocull_core::ColorLabel;

    /// Focus-movement direction for arrow-key navigation.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Direction {
        Left,
        Right,
        Up,
        Down,
    }

    /// Messages for grid and media item interactions.
    #[derive(Debug, Clone)]
    pub(crate) enum Message {
        /// Click on thumbnail: sets focus to this item.
        FileFocused(PathBuf),
        /// Cmd/Ctrl+Click on thumbnail: toggles the tag.
        FileTagToggled(PathBuf),
        /// Shift+Click on thumbnail: tags the contiguous range from the focused
        /// item to this one, inclusive, and moves focus here.
        RangeTagTo(PathBuf),
        /// Keyboard +: explicitly tag file.
        FileTagged(PathBuf),
        /// Keyboard -: explicitly untag file.
        FileUntagged(PathBuf),
        FileRated(PathBuf, i8),
        FileColorLabelSet(PathBuf, Option<ColorLabel>),
        TagAll,
        UntagAll,
        RejectFile(PathBuf),
        BurstToggled(DateTime<Utc>),
        ThumbnailHover(usize, bool),
        StarHover(Option<i8>),
        FocusNext,
        FocusPrev,
        FocusUp,
        FocusDown,
        /// Shift+Arrow: move focus like the plain arrow and tag every item
        /// between the old and new focus, inclusive.
        ExtendFocus(Direction),
        /// Move focus one viewport's worth of rows.
        FocusPageDown,
        FocusPageUp,
        /// Move focus to the first item in the loaded view.
        FocusHome,
        /// Move focus to the last item in the loaded view.
        FocusEnd,
        FocusOn(usize),
        /// `B` key: toggle collapse/expand of the focused item's burst.
        ToggleFocusedBurst,
        /// Ctrl+Z: revert the most recent metadata mutation.
        Undo,
        /// Ctrl+Shift+Z / Ctrl+Y: re-apply the most recently undone mutation.
        Redo,
        OpenPreview(usize),
        /// Wheel scrolled over the grid — snap row-by-row.
        Wheel(iced::mouse::ScrollDelta),
        /// Viewport report: absolute y offset, the grid's available width, and
        /// the viewport/content heights. A width change re-anchors the top row;
        /// height changes mark offset moves as clamps rather than user scrolls.
        Scrolled {
            offset: f32,
            grid_width: f32,
            viewport_height: f32,
            content_height: f32,
        },
    }
}

pub(crate) mod sources {
    use std::path::PathBuf;

    /// Messages for source selection.
    #[derive(Debug, Clone)]
    pub(crate) enum Message {
        SourceToggled(PathBuf),
        MountStorage(PathBuf),
        UnmountStorage(PathBuf),
        AddDirectoryClicked,
        RefreshSources,
        SourceDirectoryPicked(Option<PathBuf>),
    }
}

pub(crate) mod destination {
    use std::path::PathBuf;

    /// Messages for destination, transfer, and backup configuration.
    #[derive(Debug, Clone)]
    pub(crate) enum Message {
        PhotosDestChanged(String),
        VideosDestChanged(String),
        PhotoPatternChanged(String),
        VideoPatternChanged(String),
        /// Toggle membership of a pattern string in the saved-patterns list.
        PatternSaveToggled(String),
        BrowsePhotosDest,
        BrowseVideosDest,
        PhotosDestPicked(Option<PathBuf>),
        VideosDestPicked(Option<PathBuf>),
        JobCodeChanged(String),
        JobCodeSelected(String),
        AddBackupClicked,
        RemoveBackup(usize),
        BackupDestPicked(Option<PathBuf>),
        DeleteAfterIngestToggled,
        StartIngest,
        /// Toggle the ingest-failure details popup.
        ToggleIngestFailures,
        /// Re-run ingest for exactly the files that failed last time.
        RetryFailedIngest,
    }
}

pub(crate) mod filters {
    use ferrocull_core::{
        ColorLabel,
        media::{DateSelection, FilterMode, SortOrder},
    };

    /// Messages for filter, sort, and grouping controls.
    #[derive(Debug, Clone, Copy)]
    pub(crate) enum Message {
        SortChanged(SortOrder),
        AscendingToggled,
        FilterChanged(FilterMode),
        NewOnlyToggled,
        GroupRawJpegToggled,
        GroupBurstsToggled,
        ExpandBurstsToggled,
        HideRejectedToggled,
        RatingFilterToggled(i8),
        ColorLabelFilterToggled(Option<ColorLabel>),
        DateToggled(DateSelection),
        DateSortToggled,
        YearExpanded(i32),
        MonthExpanded(i32, u32),
        ClearAll,
    }
}

pub(crate) mod preview {
    use crate::widgets;

    /// Messages for preview mode.
    #[derive(Debug, Clone, Copy)]
    pub(crate) enum Message {
        Close,
        Prev,
        Next,
        NavigateTo(usize),
        /// Collapse/expand the burst the previewed frame belongs to (`B` key or
        /// a click on the info strip's burst badge).
        ToggleBurst,
        /// View state changed (zoom/pan from `Viewer`)
        ViewStateChanged(widgets::Event),
        /// Reset zoom to fit / toggle zoom (Z key)
        ResetZoom,
    }
}

pub(crate) mod profile {
    /// Messages for profile and hook management.
    #[derive(Debug, Clone)]
    pub(crate) enum Message {
        ProfileSelected(String),
        SaveRequested,
        DeleteRequested(String),
        NameChanged(String),
        HookAddRequested,
        HookRemoved(usize),
        HookToggled(usize),
        HookCommandEdited(usize, String),
    }
}

pub(crate) mod settings {
    use std::{path::PathBuf, sync::Arc};

    use ferrocull_core::{ThemePreference, cache};

    /// Settings popup category, shown as the left rail.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub(crate) enum Category {
        #[default]
        Appearance,
        Storage,
    }

    impl Category {
        pub(crate) const ALL: [Self; 2] = [Self::Appearance, Self::Storage];

        pub(crate) fn label(self) -> &'static str {
            match self {
                Self::Appearance => "Appearance",
                Self::Storage => "Storage",
            }
        }
    }

    /// Messages for the Settings popup.
    #[derive(Debug, Clone)]
    pub(crate) enum Message {
        Open,
        Close,
        SelectCategory(Category),
        /// Theme applies live (no confirmation).
        ThemeChanged(ThemePreference),
        /// Stage a new thumbnail resolution awaiting confirmation (destructive:
        /// clears and regenerates the thumbnail cache).
        ThumbnailSizeSelected(u32),
        ConfirmThumbnailSize,
        CancelThumbnailSize,
        /// Open the folder picker for a new cache location.
        BrowseCacheDir,
        /// Folder picker result; `Some` stages the move awaiting confirmation.
        CacheDirChosen(Option<PathBuf>),
        ConfirmCacheDir,
        CancelCacheDir,
        /// Cache relocation finished: the new resolved root, or the relocation
        /// error (shared for `Clone` — `cache::Error` wraps a non-`Clone`
        /// `io::Error`).
        CacheMoved(Result<PathBuf, Arc<cache::Error>>),
    }
}

use std::path::PathBuf;

use ferrocull_core::{
    media::{CaptureSettings, CaptureTime},
    xmp::Metadata,
};
use ferrocull_devices::ScannedFile;
use iced::keyboard::{Key, Modifiers};

/// One unit of scan progress, drained from the pipeline in batches (see
/// [`Message::ScanBatch`]). The pipeline emits two of these per file — EXIF
/// first, thumbnail second — and that per-file order is preserved within a
/// batch.
#[derive(Debug, Clone)]
pub(crate) enum ScanEvent {
    /// EXIF resolved; carries what item construction needs. `file` is boxed to
    /// keep the enum near the size of its `ThumbnailCached` variant
    /// (`clippy::large_enum_variant`): batches are mostly thumbnail events.
    ExifLoaded {
        file: Box<ScannedFile>,
        capture_time: CaptureTime,
        capture_settings: CaptureSettings,
        xmp: Option<Metadata>,
    },
    /// The file's thumbnail is on disk (freshly generated or a cache hit), or
    /// generation failed.
    ThumbnailCached(PathBuf, Result<(), String>),
}

/// Result of an ingest operation.
#[derive(Debug, Clone)]
pub(crate) struct IngestResult {
    pub successes: Vec<SuccessInfo>,
    pub failures: Vec<FailureInfo>,
}

#[derive(Debug, Clone)]
pub(crate) struct SuccessInfo {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub checksum: String,
}

/// Aggregate ingest progress, polled from the worker counters while the
/// ingest runs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct IngestSnapshot {
    pub files_completed: usize,
    pub bytes_copied: u64,
}

/// One failed file from an ingest run, kept for the details popup and retry.
#[derive(Debug, Clone)]
pub(crate) struct FailureInfo {
    pub source: PathBuf,
    pub error: String,
    /// Rendering the retry must reuse so it targets the primary copy left
    /// behind by the failed run; `None` re-renders with current settings.
    pub rendered_dest: Option<String>,
}

/// Config panel section identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Section {
    Destination,
    Rename,
    Backup,
    Hooks,
}

/// Panel identifiers for collapse/expand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Panel {
    Left,
    Right,
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
    Compare(compare::Message),
    Grid(grid::Message),
    Sources(sources::Message),
    Destination(destination::Message),
    Filters(filters::Message),
    Preview(preview::Message),
    Profile(profile::Message),
    Settings(settings::Message),

    ToggleSection(Section),
    /// Toggle the keyboard-shortcut reference overlay (`?` / F1).
    ToggleShortcuts,
    /// Show or hide the info strip (`I`). One flag backs compare and preview,
    /// so the readout is on or off everywhere at once.
    ToggleInfoStrip,
    TogglePanel(Panel),
    PanelResized(Panel, f32),
    PanelResizeEnd,
    /// Carries both the base key (layout key without modifiers, for
    /// letter/digit bindings) and the modified key (with modifiers applied,
    /// for characters like `?` that live on different keys per layout).
    KeyPressed {
        key: Key,
        modified_key: Key,
        modifiers: Modifiers,
    },
    ModifiersChanged(Modifiers),

    /// A drained batch of scan progress events, applied in one `update` pass so
    /// the grid rebuilds once per batch instead of once per event.
    ScanBatch(Vec<ScanEvent>),
    ScanComplete(Vec<ScannedFile>),
    ThumbnailsComplete,
    ThumbnailLoaded(PathBuf, iced::widget::image::Handle),
    IngestProgressUpdate(IngestSnapshot),
    IngestComplete(IngestResult),
    PreviewLoaded(u64, PathBuf, Result<Vec<u8>, String>),
    PreviewAllocated(
        u64,
        PathBuf,
        Result<iced::widget::image::Allocation, iced::widget::image::Error>,
    ),
    HooksComplete,
    SourcesRefreshed(Result<Vec<ferrocull_devices::StorageDevice>, ferrocull_devices::ScanError>),
    MountResult(PathBuf, Result<PathBuf, String>),
    UnmountResult(PathBuf, Result<(), String>),
    Tick,
    OsThemeDetected(bool),
    /// Main window opened — kicks off the initial scale-factor fetch.
    WindowOpened(iced::window::Id),
    /// Window scale factor (initial fetch or monitor change) — grid cell
    /// widths are floored to whole physical pixels with it.
    WindowScaleChanged(f32),
    /// No-op message — produced by `spawn_blocking` panic fallbacks where the
    /// task failure has already been logged and the caller has nothing to do.
    Noop,
}
