//! Persistent media database for download history, ratings, and color labels.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};

use crate::media::ColorLabel;

/// Errors from database operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error during {operation}: {source}")]
    Db {
        operation: &'static str,
        source: rusqlite::Error,
    },
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
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
        .map(drop)
        .map_err(db_err("create ratings table"))
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
}
