//! Persistent media database for ingest history and per-frame culling state.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::{
    hooks::Hook,
    media::{ColorLabel, FilterMode, SortOrder},
    profiles::{IngestConfig, Profile},
};

/// The app's persisted working settings, restored at startup and written on change.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub ingest: IngestConfig,
    pub post_ingest_hooks: Vec<Hook>,
    /// Delete source files after successful ingest and checksum verification.
    pub delete_after_ingest: bool,
    /// App-level preferences edited via the Settings popup.
    pub preferences: Preferences,
    /// Durable grid view preferences (sort/filter/grouping). Selection sets are
    /// deliberately not persisted — they reference session-specific content.
    pub view: ViewPrefs,
    /// App-level rename patterns the user saved for reuse, most-recent first.
    pub saved_patterns: Vec<String>,
    /// User-adjusted sidebar panel widths, restored on startup.
    pub panel_widths: PanelWidths,
    /// Whether the info strip is open. One flag for compare and the preview.
    pub info_strip_open: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            ingest: IngestConfig::default(),
            post_ingest_hooks: Vec::new(),
            delete_after_ingest: false,
            preferences: Preferences::default(),
            view: ViewPrefs::default(),
            saved_patterns: Vec::new(),
            panel_widths: PanelWidths::default(),
            // Open by default: capture settings are part of judging a frame,
            // not an occasional lookup.
            info_strip_open: true,
        }
    }
}

/// Theme preference. `Auto` follows the OS dark-mode setting; `Dark`/`Light`
/// force a fixed appearance. The UI resolves this into a concrete theme.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemePreference {
    #[default]
    Auto,
    Dark,
    Light,
}

impl ThemePreference {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Dark, Self::Light];
}

impl std::fmt::Display for ThemePreference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Auto => "Auto",
            Self::Dark => "Dark",
            Self::Light => "Light",
        })
    }
}

/// App-level preferences: appearance and storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub theme: ThemePreference,
    /// Grid thumbnail resolution in pixels (longest edge).
    pub thumbnail_size: u32,
    /// Cache root holding the `thumbnails/` and `previews/` namespaces. `None`
    /// resolves to the platform default (`cache::default_cache_root`).
    pub cache_dir: Option<PathBuf>,
}

/// Default grid thumbnail resolution (longest edge, in pixels).
pub const DEFAULT_THUMBNAIL_SIZE: u32 = 256;

impl Default for Preferences {
    fn default() -> Self {
        Self {
            theme: ThemePreference::default(),
            thumbnail_size: DEFAULT_THUMBNAIL_SIZE,
            cache_dir: None,
        }
    }
}

/// Durable grid view preferences restored at startup and written on change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent durable view-toggle flags, mirroring the UI's ViewConfig"
)]
pub struct ViewPrefs {
    pub sort_order: SortOrder,
    pub ascending: bool,
    pub filter_mode: FilterMode,
    /// Independent, stackable "show only not-yet-ingested items" toggle. ANDs
    /// with `filter_mode` rather than being a member of it. `#[serde(default)]`
    /// on the struct covers absence in older persisted prefs.
    pub new_only: bool,
    pub hide_rejected: bool,
    pub group_raw_jpeg: bool,
    pub group_bursts: bool,
    /// Whether a burst shows its members by default. Qualifies `group_bursts`
    /// and means nothing while that is off. Per-burst folds made by hand are
    /// session-only and never persisted.
    pub expand_bursts: bool,
    pub date_tree_ascending: bool,
}

impl Default for ViewPrefs {
    fn default() -> Self {
        Self {
            sort_order: SortOrder::default(),
            ascending: true,
            filter_mode: FilterMode::default(),
            new_only: false,
            hide_rejected: false,
            group_raw_jpeg: true,
            group_bursts: true,
            expand_bursts: false,
            date_tree_ascending: true,
        }
    }
}

/// User-adjusted sidebar panel widths, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelWidths {
    pub left: f32,
    pub right: f32,
}

impl Default for PanelWidths {
    fn default() -> Self {
        Self {
            left: 250.0,
            right: 300.0,
        }
    }
}

/// One frame's stored culling row, as written.
///
/// Rating and color label are absent until the photographer sets them, which is
/// what keeps an XMP sidecar speaking for a field nobody has touched. The store
/// resolves the row into a [`CullingState`].
#[derive(Debug, Clone, Copy, Default)]
pub struct CullingRow {
    pub rating: Option<i8>,
    /// `None`: never set, an XMP sidecar may still answer. `Some(None)`: the
    /// photographer explicitly cleared the label.
    pub color_label: Option<Option<ColorLabel>>,
    pub tagged: bool,
}

/// Errors from database operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error during {operation}: {source}")]
    Db {
        operation: &'static str,
        source: rusqlite::Error,
    },
    #[error("serialization error during {operation}: {source}")]
    Serde {
        operation: &'static str,
        source: serde_json::Error,
    },
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
}

fn serde_err(operation: &'static str) -> impl FnOnce(serde_json::Error) -> Error {
    move |source| Error::Serde { operation, source }
}

fn db_err(operation: &'static str) -> impl FnOnce(rusqlite::Error) -> Error {
    move |source| Error::Db { operation, source }
}

pub struct MediaDatabase {
    conn: Connection,
}

impl MediaDatabase {
    /// Opens or creates a media database at the given path.
    pub fn open(path: &Path) -> Result<Self, Error> {
        let parent = path.parent().expect("database path has a parent directory");
        fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_owned(),
            source,
        })?;

        let conn = Connection::open(path).map_err(db_err("open"))?;

        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(db_err("set WAL mode"))?;

        Self::init_tables(&conn)?;

        Ok(Self { conn })
    }

    /// Opens an in-memory database with the schema initialized.
    #[cfg(test)]
    pub(crate) fn open_in_memory() -> Result<Self, Error> {
        let conn = Connection::open_in_memory().map_err(db_err("open"))?;
        Self::init_tables(&conn)?;
        Ok(Self { conn })
    }

    fn init_tables(conn: &Connection) -> Result<(), Error> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS ingests (
                fingerprint TEXT PRIMARY KEY,
                checksum TEXT NOT NULL,
                dest_path TEXT NOT NULL,
                ingested_at TEXT NOT NULL
            )",
            [],
        )
        .map_err(db_err("create ingests table"))?;

        // Rating and color label are nullable: a row written by tagging alone
        // must not claim the photographer cleared them, or it would silence the
        // XMP sidecar those fields still fall back to.
        conn.execute(
            "CREATE TABLE IF NOT EXISTS culling_state (
                fingerprint TEXT PRIMARY KEY,
                rating INTEGER,
                color_label INTEGER,
                tagged INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            )",
            [],
        )
        .map_err(db_err("create culling_state table"))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS profiles (
                name TEXT PRIMARY KEY,
                payload TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )",
            [],
        )
        .map_err(db_err("create profiles table"))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS jobcode_history (
                position INTEGER PRIMARY KEY,
                code TEXT NOT NULL
            )",
            [],
        )
        .map_err(db_err("create jobcode_history table"))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS settings (
                id INTEGER PRIMARY KEY CHECK (id = 0),
                payload TEXT NOT NULL
            )",
            [],
        )
        .map(drop)
        .map_err(db_err("create settings table"))
    }

    /// Records a successful ingest, keyed by the frame's ingest fingerprint.
    pub fn record_ingest(
        &mut self,
        fingerprint: &str,
        checksum: &str,
        dest: &Path,
    ) -> Result<(), Error> {
        let now = Utc::now().to_rfc3339();
        let dest_str = dest.to_string_lossy();

        self.conn
            .execute(
                "INSERT OR REPLACE INTO ingests (fingerprint, checksum, dest_path, ingested_at)
             VALUES (?1, ?2, ?3, ?4)",
                params![fingerprint, checksum, dest_str, now],
            )
            .map(drop)
            .map_err(db_err("record ingest"))
    }

    /// Returns whether a frame with this ingest fingerprint has been ingested.
    pub fn is_ingested(&self, fingerprint: &str) -> Result<bool, Error> {
        self.conn
            .query_row(
                "SELECT 1 FROM ingests WHERE fingerprint = ?1",
                params![fingerprint],
                |_| Ok(()),
            )
            .optional()
            .map(|row| row.is_some())
            .map_err(db_err("query ingest status"))
    }

    /// Sets the rating for a frame. Valid range: `-1..=5` (`-1` rejected, `0` unrated, `1..=5` stars).
    pub fn set_rating(&mut self, fingerprint: &str, rating: i8) -> Result<(), Error> {
        let now = Utc::now().to_rfc3339();

        self.conn
            .execute(
                "INSERT INTO culling_state (fingerprint, rating, color_label, tagged, updated_at)
             VALUES (?1, ?2, NULL, 0, ?3)
             ON CONFLICT(fingerprint) DO UPDATE SET rating = excluded.rating, updated_at = excluded.updated_at",
                params![fingerprint, rating, now],
            )
            .map(drop)
            .map_err(db_err("set rating"))
    }

    /// Sets the color label for a frame.
    pub fn set_color_label(
        &mut self,
        fingerprint: &str,
        label: Option<ColorLabel>,
    ) -> Result<(), Error> {
        let db_value = label.map_or(0u8, u8::from);
        let now = Utc::now().to_rfc3339();

        self.conn
            .execute(
                "INSERT INTO culling_state (fingerprint, rating, color_label, tagged, updated_at)
             VALUES (?1, NULL, ?2, 0, ?3)
             ON CONFLICT(fingerprint) DO UPDATE SET color_label = excluded.color_label, updated_at = excluded.updated_at",
                params![fingerprint, db_value, now],
            )
            .map(drop)
            .map_err(db_err("set color label"))
    }

    /// Sets the tagged flag for every frame in `fingerprints`, in one
    /// transaction so tagging a long range costs one commit rather than one per
    /// frame.
    pub fn set_tagged(&mut self, fingerprints: &[String], tagged: bool) -> Result<(), Error> {
        let now = Utc::now().to_rfc3339();
        let tx = self.conn.transaction().map_err(db_err("begin tag write"))?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO culling_state (fingerprint, rating, color_label, tagged, updated_at)
                 VALUES (?1, NULL, NULL, ?2, ?3)
                 ON CONFLICT(fingerprint) DO UPDATE SET tagged = excluded.tagged, updated_at = excluded.updated_at",
                )
                .map_err(db_err("prepare tag write"))?;
            for fingerprint in fingerprints {
                stmt.execute(params![fingerprint, tagged, now])
                    .map_err(db_err("set tagged"))?;
            }
        }
        tx.commit().map_err(db_err("commit tag write"))
    }

    /// The stored culling state of a frame. An absent row reads as the default
    /// row: nothing rated, nothing labelled, untagged.
    pub fn culling_state(&self, fingerprint: &str) -> Result<CullingRow, Error> {
        self.conn
            .query_row(
                "SELECT rating, color_label, tagged FROM culling_state WHERE fingerprint = ?1",
                params![fingerprint],
                |row| {
                    // A stored `0` is the explicit "no label" written by
                    // clearing; only `1..=7` name a color.
                    Ok(CullingRow {
                        rating: row.get(0)?,
                        color_label: row.get::<_, Option<u8>>(1)?.map(|stored| match stored {
                            0 => None,
                            labelled => Some(
                                ColorLabel::try_from(labelled)
                                    .expect("color label value out of range in culling_state"),
                            ),
                        }),
                        tagged: row.get(2)?,
                    })
                },
            )
            .optional()
            .map(Option::unwrap_or_default)
            .map_err(db_err("query culling state"))
    }

    /// All saved profiles, ordered by name.
    pub fn list_profiles(&self) -> Result<Vec<crate::profiles::NamedProfile>, Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, payload FROM profiles ORDER BY name")
            .map_err(db_err("prepare list profiles"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(db_err("query profiles"))?;

        let mut profiles = Vec::new();
        for row in rows {
            let (name, payload) = row.map_err(db_err("read profile row"))?;
            let profile: Profile =
                serde_json::from_str(&payload).map_err(serde_err("deserialize profile"))?;
            profiles.push(crate::profiles::NamedProfile { name, profile });
        }
        Ok(profiles)
    }

    /// Inserts or replaces a profile by name.
    pub fn save_profile(&mut self, name: &str, profile: &Profile) -> Result<(), Error> {
        let payload = serde_json::to_string(profile).map_err(serde_err("serialize profile"))?;
        let now = Utc::now().to_rfc3339();

        self.conn
            .execute(
                "INSERT INTO profiles (name, payload, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET payload = excluded.payload, updated_at = excluded.updated_at",
                params![name, payload, now],
            )
            .map(drop)
            .map_err(db_err("save profile"))
    }

    /// Deletes a profile by name. A no-op if the name is absent.
    pub fn delete_profile(&mut self, name: &str) -> Result<(), Error> {
        self.conn
            .execute("DELETE FROM profiles WHERE name = ?1", params![name])
            .map(drop)
            .map_err(db_err("delete profile"))
    }

    /// Job code history, ordered most-recent first.
    pub fn job_code_history(&self) -> Result<Vec<String>, Error> {
        let mut stmt = self
            .conn
            .prepare("SELECT code FROM jobcode_history ORDER BY position")
            .map_err(db_err("prepare job code history"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(db_err("query job code history"))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(db_err("read job code history"))
    }

    /// Replaces the job code history with `codes` (index 0 is most recent).
    pub fn set_job_code_history(&mut self, codes: &[String]) -> Result<(), Error> {
        let tx = self
            .conn
            .transaction()
            .map_err(db_err("begin job code history transaction"))?;
        tx.execute("DELETE FROM jobcode_history", [])
            .map_err(db_err("clear job code history"))?;
        for (position, code) in codes.iter().enumerate() {
            let position = i64::try_from(position).expect("job code history index fits i64");
            tx.execute(
                "INSERT INTO jobcode_history (position, code) VALUES (?1, ?2)",
                params![position, code],
            )
            .map_err(db_err("insert job code"))?;
        }
        tx.commit().map_err(db_err("commit job code history"))
    }

    /// The persisted app settings, or defaults when none are stored yet.
    pub fn settings(&self) -> Result<AppSettings, Error> {
        let payload: Option<String> = self
            .conn
            .query_row("SELECT payload FROM settings WHERE id = 0", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(db_err("query settings"))?;

        payload.map_or_else(
            || Ok(AppSettings::default()),
            |json| serde_json::from_str(&json).map_err(serde_err("deserialize settings")),
        )
    }

    /// Writes the app settings, replacing any prior row.
    pub fn set_settings(&mut self, settings: &AppSettings) -> Result<(), Error> {
        let payload = serde_json::to_string(settings).map_err(serde_err("serialize settings"))?;

        self.conn
            .execute(
                "INSERT INTO settings (id, payload) VALUES (0, ?1)
             ON CONFLICT(id) DO UPDATE SET payload = excluded.payload",
                params![payload],
            )
            .map(drop)
            .map_err(db_err("save settings"))
    }
}
