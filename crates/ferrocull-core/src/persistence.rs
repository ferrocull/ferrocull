//! Persistent media database for download history, ratings, and color labels.

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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub ingest: IngestConfig,
    pub post_download_hooks: Vec<Hook>,
    /// Delete source files after successful download and checksum verification.
    pub delete_after_download: bool,
    /// App-level preferences edited via the Settings popup.
    pub preferences: Preferences,
    /// Durable grid view preferences (sort/filter/grouping). Selection sets are
    /// deliberately not persisted — they reference session-specific content.
    pub view: ViewPrefs,
    /// App-level rename patterns the user saved for reuse, most-recent first.
    pub saved_patterns: Vec<String>,
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
    pub hide_rejected: bool,
    pub group_raw_jpeg: bool,
    pub group_bursts: bool,
}

impl Default for ViewPrefs {
    fn default() -> Self {
        Self {
            sort_order: SortOrder::default(),
            ascending: true,
            filter_mode: FilterMode::default(),
            hide_rejected: false,
            group_raw_jpeg: true,
            group_bursts: true,
        }
    }
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
            "CREATE TABLE IF NOT EXISTS downloads (
                source_id TEXT PRIMARY KEY,
                checksum TEXT NOT NULL,
                dest_path TEXT NOT NULL,
                downloaded_at TEXT NOT NULL
            )",
            [],
        )
        .map_err(db_err("create downloads table"))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS ratings (
                source_id TEXT PRIMARY KEY,
                rating INTEGER NOT NULL,
                color_label INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            )",
            [],
        )
        .map_err(db_err("create ratings table"))?;

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

    /// Records a successful download.
    pub fn record_download(
        &mut self,
        source_id: &str,
        checksum: &str,
        dest: &Path,
    ) -> Result<(), Error> {
        let now = Utc::now().to_rfc3339();
        let dest_str = dest.to_string_lossy();

        self.conn
            .execute(
                "INSERT OR REPLACE INTO downloads (source_id, checksum, dest_path, downloaded_at)
             VALUES (?1, ?2, ?3, ?4)",
                params![source_id, checksum, dest_str, now],
            )
            .map(drop)
            .map_err(db_err("record download"))
    }

    /// Returns whether a file has been downloaded.
    pub fn is_downloaded(&self, source_id: &str) -> Result<bool, Error> {
        self.conn
            .query_row(
                "SELECT 1 FROM downloads WHERE source_id = ?1",
                params![source_id],
                |_| Ok(()),
            )
            .optional()
            .map(|row| row.is_some())
            .map_err(db_err("query download status"))
    }

    /// Sets the rating for a file. Valid range: `-1..=5` (`-1` rejected, `0` unrated, `1..=5` stars).
    pub fn set_rating(&mut self, source_id: &str, rating: i8) -> Result<(), Error> {
        let now = Utc::now().to_rfc3339();

        self.conn
            .execute(
                "INSERT INTO ratings (source_id, rating, color_label, updated_at)
             VALUES (
                 ?1,
                 ?2,
                 COALESCE((SELECT color_label FROM ratings WHERE source_id = ?1), 0),
                 ?3
             )
             ON CONFLICT(source_id) DO UPDATE SET rating = excluded.rating, updated_at = excluded.updated_at",
                params![source_id, rating, now],
            )
            .map(drop)
            .map_err(db_err("set rating"))
    }

    /// Sets the color label for a file.
    pub fn set_color_label(
        &mut self,
        source_id: &str,
        label: Option<ColorLabel>,
    ) -> Result<(), Error> {
        let db_value = label.map_or(0u8, u8::from);
        let now = Utc::now().to_rfc3339();

        self.conn
            .execute(
                "INSERT INTO ratings (source_id, rating, color_label, updated_at)
             VALUES (
                 ?1,
                 COALESCE((SELECT rating FROM ratings WHERE source_id = ?1), 0),
                 ?2,
                 ?3
             )
             ON CONFLICT(source_id) DO UPDATE SET color_label = excluded.color_label, updated_at = excluded.updated_at",
                params![source_id, db_value, now],
            )
            .map(drop)
            .map_err(db_err("set color label"))
    }

    /// Gets rating and color label for a file. Returns `None` if no metadata row exists.
    ///
    /// `None` distinguishes "user never set metadata" from `Some((0, None))` which means
    /// "user explicitly cleared rating and label".
    pub fn rating_and_color(
        &self,
        source_id: &str,
    ) -> Result<Option<(i8, Option<ColorLabel>)>, Error> {
        self.conn
            .query_row(
                "SELECT rating, color_label FROM ratings WHERE source_id = ?1",
                params![source_id],
                |row| {
                    let rating: i8 = row.get(0)?;
                    let color: u8 = row.get(1)?;
                    Ok((rating, ColorLabel::try_from(color).ok()))
                },
            )
            .optional()
            .map_err(db_err("query metadata"))
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
