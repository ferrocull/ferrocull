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
    use iced::widget::scrollable;

    /// Messages for grid and media item interactions.
    #[derive(Debug, Clone)]
    pub(crate) enum Message {
        /// Scroll position changed — records viewport width for grid cache key.
        Scrolled(scrollable::Viewport),
        /// Click on thumbnail: sets focus to this item.
        FileFocused(PathBuf),
        /// Cmd/Ctrl+Click on thumbnail: toggles selection.
        FileSelectionToggled(PathBuf),
        /// Keyboard +: explicitly select file.
        FileSelected(PathBuf),
        /// Keyboard -: explicitly deselect file.
        FileDeselected(PathBuf),
        FileRated(PathBuf, i8),
        FileColorLabelSet(PathBuf, Option<ColorLabel>),
        SelectAll,
        SelectNone,
        RejectFile(PathBuf),
        BurstToggled(DateTime<Utc>),
        ThumbnailVisible(usize),
        ThumbnailHidden(usize),
        ThumbnailHover(usize, bool),
        StarHover(Option<i8>),
        FocusNext,
        FocusPrev,
        FocusOn(usize),
        OpenPreview(usize),
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
        BrowsePhotosDest,
        BrowseVideosDest,
        PhotosDestPicked(Option<PathBuf>),
        VideosDestPicked(Option<PathBuf>),
        JobCodeChanged(String),
        JobCodeSelected(String),
        AddBackupClicked,
        RemoveBackup(usize),
        BackupDestPicked(Option<PathBuf>),
        DeleteAfterDownloadToggled,
        StartDownload,
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
        GroupRawJpegToggled,
        GroupBurstsToggled,
        HideRejectedToggled,
        RatingFilterToggled(i8),
        ColorLabelFilterToggled(Option<ColorLabel>),
        DateToggled(DateSelection),
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

use std::path::PathBuf;

use ferrocull_core::{media::CaptureTime, xmp::Metadata};
use ferrocull_devices::ScannedFile;
use iced::keyboard::{Key, Modifiers};

/// Result of a download operation.
#[derive(Debug, Clone)]
pub(crate) struct DownloadResult {
    pub successes: Vec<SuccessInfo>,
    pub failure_count: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct SuccessInfo {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub checksum: String,
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

    ToggleSection(Section),
    TogglePanel(Panel),
    KeyPressed(Key, Modifiers),
    ModifiersChanged(Modifiers),

    ExifLoaded(ScannedFile, CaptureTime, Option<Metadata>),
    ThumbnailCached(PathBuf, Result<(), String>),
    ScanComplete(Vec<ScannedFile>),
    ThumbnailsComplete,
    ThumbnailLoaded(PathBuf, iced::widget::image::Handle),
    DownloadProgressUpdate(usize),
    DownloadComplete(DownloadResult),
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
    ProfileSaved(Result<(String, Vec<ferrocull_core::NamedProfile>), String>),
    ProfileDeleted(Result<(Option<String>, Vec<ferrocull_core::NamedProfile>), String>),
    Tick,
    OsThemeDetected(bool),
    /// No-op message — produced by `spawn_blocking` panic fallbacks where the
    /// task failure has already been logged and the caller has nothing to do.
    Noop,
}
