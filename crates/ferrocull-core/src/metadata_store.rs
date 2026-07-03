//! The seam callers use for a media file's culling metadata (rating, color label).
//!
//! Owns the read precedence — the persistent database wins, an XMP sidecar is the
//! fallback — and the culling write policy: rating and label edits persist to the
//! database; the XMP sidecar is authored once at ingest ([ADR-0006]), not on every
//! edit. Wraps [`MediaDatabase`] and the [`crate::xmp`] module.
//!
//! [ADR-0006]: docs/adr/0006-xmp-at-ingest.md

use std::path::Path;

use crate::{MediaFile, media::ColorLabel, persistence::MediaDatabase, xmp::Metadata};

/// Reads and writes culling metadata across the database, XMP sidecars, and memory.
///
/// Database-backed operations are infallible: [`MediaDatabase::open`] validates the
/// connection at startup, so a mid-session query failure is a broken invariant, not a
/// runtime error.
pub struct Store {
    db: MediaDatabase,
}

impl Store {
    #[must_use]
    pub fn new(db: MediaDatabase) -> Self {
        Self { db }
    }

    /// Rating and color label for a file, preferring the database over `xmp_fallback`.
    ///
    /// `xmp_fallback` is the sidecar metadata parsed at scan time; the store never
    /// re-reads it. Returns `(0, None)` when neither source has metadata.
    #[must_use]
    pub fn load(
        &self,
        source_id: &str,
        xmp_fallback: Option<&Metadata>,
    ) -> (i8, Option<ColorLabel>) {
        self.db
            .rating_and_color(source_id)
            .expect("rating_and_color query failed")
            .unwrap_or_else(|| xmp_fallback.map_or((0, None), |x| (x.rating, x.color_label)))
    }

    pub fn set_rating(&mut self, source_id: &str, rating: i8) {
        self.db
            .set_rating(source_id, rating)
            .expect("set_rating query failed");
    }

    pub fn set_color_label(&mut self, source_id: &str, label: Option<ColorLabel>) {
        self.db
            .set_color_label(source_id, label)
            .expect("set_color_label query failed");
    }

    #[must_use]
    pub fn is_downloaded(&self, source_id: &str) -> bool {
        self.db
            .is_downloaded(source_id)
            .expect("is_downloaded query failed")
    }

    pub fn record_download(&mut self, source_id: &str, checksum: &str, dest: &Path) {
        self.db
            .record_download(source_id, checksum, dest)
            .expect("record_download query failed");
    }
}

/// The XMP payload authored at ingest from a file's current session state.
///
/// Returns `None` when the file carries nothing worth preserving (unrated, no label),
/// so ingest skips writing a sidecar of defaults.
#[must_use]
pub fn ingest_payload(media_file: &MediaFile) -> Option<Metadata> {
    if media_file.rating == 0 && media_file.color_label.is_none() {
        return None;
    }

    Some(Metadata {
        rating: media_file.rating,
        color_label: media_file.color_label,
        original_filename: Some(
            media_file
                .path
                .file_name()
                .expect("scanned file has filename")
                .to_string_lossy()
                .into_owned(),
        ),
        capture_date: Some(media_file.datetime),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::Utc;
    use ferrocull_media::FileCategory;

    use super::*;
    use crate::xmp::{self, write_sidecar};

    fn store() -> Store {
        Store::new(MediaDatabase::open_in_memory().expect("open in-memory database"))
    }

    fn media_file(rating: i8, color_label: Option<ColorLabel>) -> MediaFile {
        MediaFile {
            path: PathBuf::from("/cards/DCIM/IMG_0001.CR2"),
            datetime: Utc::now(),
            media_type: FileCategory::Raw,
            paired_files: Vec::new(),
            sidecars: Vec::new(),
            xmp_sidecar: None,
            rating,
            color_label,
            rendered_dest: None,
        }
    }

    #[test]
    fn database_row_beats_xmp_fallback() {
        let mut store = store();
        store.set_rating("id", 5);
        store.set_color_label("id", Some(ColorLabel::Green));

        let fallback = Metadata {
            rating: 1,
            color_label: Some(ColorLabel::Red),
            ..Metadata::default()
        };

        assert_eq!(
            store.load("id", Some(&fallback)),
            (5, Some(ColorLabel::Green))
        );
    }

    #[test]
    fn xmp_fallback_used_when_no_database_row() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let image = dir.path().join("IMG_0002.CR2");
        write_sidecar(
            &image,
            &Metadata {
                rating: 3,
                color_label: Some(ColorLabel::Blue),
                ..Metadata::default()
            },
        )
        .expect("write sidecar");
        let sidecar = xmp::sidecar_path_for(&image);
        let parsed = xmp::read_sidecar(&sidecar).expect("read sidecar");

        assert_eq!(
            store().load("id", Some(&parsed)),
            (3, Some(ColorLabel::Blue))
        );
    }

    #[test]
    fn defaults_when_neither_source_has_metadata() {
        assert_eq!(store().load("id", None), (0, None));
    }

    #[test]
    fn write_then_load_roundtrips() {
        let mut store = store();
        store.set_rating("id", -1);
        store.set_color_label("id", Some(ColorLabel::Purple));

        assert_eq!(store.load("id", None), (-1, Some(ColorLabel::Purple)));

        store.set_color_label("id", None);
        assert_eq!(store.load("id", None), (-1, None));
    }

    #[test]
    fn ingest_payload_reflects_session_mutations() {
        let file = media_file(4, Some(ColorLabel::Yellow));
        let payload = ingest_payload(&file).expect("rated file yields a payload");

        assert_eq!(payload.rating, 4);
        assert_eq!(payload.color_label, Some(ColorLabel::Yellow));
        assert_eq!(payload.original_filename.as_deref(), Some("IMG_0001.CR2"));
    }

    #[test]
    fn ingest_payload_skipped_for_default_metadata() {
        assert!(ingest_payload(&media_file(0, None)).is_none());
    }
}
