//! The seam callers use for a media file's culling metadata (rating, color label).
//!
//! Owns the read precedence — the persistent database wins, an XMP sidecar is the
//! fallback — and the culling write policy: rating and label edits persist to the
//! database; the XMP sidecar is authored once at ingest ([ADR-0006]), not on every
//! edit. Wraps [`MediaDatabase`] and the [`crate::xmp`] module.
//!
//! [ADR-0006]: docs/adr/0006-xmp-at-ingest.md

use std::path::Path;

use crate::{
    MediaFile,
    media::ColorLabel,
    persistence::{AppSettings, MediaDatabase},
    profiles::{NamedProfile, Profile},
    xmp::Metadata,
};

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
    pub fn is_ingested(&self, fingerprint: &str) -> bool {
        self.db
            .is_ingested(fingerprint)
            .expect("is_ingested query failed")
    }

    pub fn record_ingest(&mut self, fingerprint: &str, checksum: &str, dest: &Path) {
        self.db
            .record_ingest(fingerprint, checksum, dest)
            .expect("record_ingest query failed");
    }

    #[must_use]
    pub fn profiles(&self) -> Vec<NamedProfile> {
        self.db.list_profiles().expect("list_profiles query failed")
    }

    pub fn save_profile(&mut self, name: &str, profile: &Profile) {
        self.db
            .save_profile(name, profile)
            .expect("save_profile query failed");
    }

    pub fn delete_profile(&mut self, name: &str) {
        self.db
            .delete_profile(name)
            .expect("delete_profile query failed");
    }

    #[must_use]
    pub fn job_code_history(&self) -> Vec<String> {
        self.db
            .job_code_history()
            .expect("job_code_history query failed")
    }

    pub fn set_job_code_history(&mut self, codes: &[String]) {
        self.db
            .set_job_code_history(codes)
            .expect("set_job_code_history query failed");
    }

    #[must_use]
    pub fn settings(&self) -> AppSettings {
        self.db.settings().expect("settings query failed")
    }

    pub fn set_settings(&mut self, settings: &AppSettings) {
        self.db
            .set_settings(settings)
            .expect("set_settings query failed");
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
                .expect("path has no filename")
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
    fn ingest_history_is_keyed_by_fingerprint_not_path() {
        use chrono::DateTime;

        use crate::{fingerprint::ingest_fingerprint, media::CaptureTime};

        let mut store = store();
        let capture = CaptureTime::new(
            DateTime::<Utc>::from_timestamp(1_700_000_000, 0).expect("valid timestamp"),
            0,
        );

        // The same frame re-inserted on a different card path shares a
        // fingerprint (basename + size + capture time); a genuinely different
        // frame reusing the camera name (differing size) does not.
        let re_inserted = ingest_fingerprint("IMG_0001.CR2", 24_000_000, capture);
        let different_frame = ingest_fingerprint("IMG_0001.CR2", 31_500_000, capture);

        assert!(!store.is_ingested(&re_inserted), "nothing ingested yet");

        store.record_ingest(
            &ingest_fingerprint("IMG_0001.CR2", 24_000_000, capture),
            "checksum",
            Path::new("/dest/2024/IMG_0001.CR2"),
        );

        assert!(
            store.is_ingested(&re_inserted),
            "same frame from another path reads as ingested"
        );
        assert!(
            !store.is_ingested(&different_frame),
            "a different frame reusing the camera name reads as new"
        );
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

    fn profile(photos: &str) -> Profile {
        Profile {
            ingest: crate::profiles::IngestConfig {
                photos_dest: PathBuf::from(photos),
                ..crate::profiles::IngestConfig::default()
            },
        }
    }

    #[test]
    fn profiles_save_list_load_delete_roundtrip() {
        let mut store = store();
        assert!(store.profiles().is_empty());

        store.save_profile("Wedding", &profile("/a"));
        store.save_profile("Studio", &profile("/b"));

        let by_name = store.profiles();
        // Ordered by name.
        assert_eq!(by_name.len(), 2);
        assert_eq!(by_name[0].name, "Studio");
        assert_eq!(by_name[1].name, "Wedding");
        assert_eq!(by_name[1].profile.ingest.photos_dest, PathBuf::from("/a"));

        // Save with existing name replaces the payload.
        store.save_profile("Wedding", &profile("/c"));
        let after_replace = store.profiles();
        assert_eq!(after_replace.len(), 2);
        assert_eq!(
            after_replace[1].profile.ingest.photos_dest,
            PathBuf::from("/c")
        );

        store.delete_profile("Studio");
        let after_delete = store.profiles();
        assert_eq!(after_delete.len(), 1);
        assert_eq!(after_delete[0].name, "Wedding");
    }

    #[test]
    fn job_code_history_roundtrips_with_order_and_dedup() {
        let mut store = store();
        assert!(store.job_code_history().is_empty());

        let mut history = crate::JobCodeHistory::from_codes(store.job_code_history());
        history.add("A");
        history.add("B");
        history.add("A"); // moves A to front, dedups
        store.set_job_code_history(history.codes());

        assert_eq!(
            store.job_code_history(),
            vec!["A".to_owned(), "B".to_owned()]
        );

        let reloaded = crate::JobCodeHistory::from_codes(store.job_code_history());
        assert_eq!(reloaded.codes(), ["A", "B"]);
    }

    #[test]
    fn settings_default_when_absent_then_roundtrip() {
        let mut store = store();
        let defaults = store.settings();
        assert!(!defaults.delete_after_ingest);
        assert!(defaults.post_ingest_hooks.is_empty());

        let settings = AppSettings {
            ingest: crate::profiles::IngestConfig {
                photo_pattern: String::from("{filename}.{ext}"),
                ..crate::profiles::IngestConfig::default()
            },
            post_ingest_hooks: vec![crate::Hook {
                name: String::from("notify"),
                command: String::from("echo done"),
                enabled: true,
            }],
            delete_after_ingest: true,
            ..AppSettings::default()
        };
        store.set_settings(&settings);

        let loaded = store.settings();
        assert!(loaded.delete_after_ingest);
        assert_eq!(loaded.ingest.photo_pattern, "{filename}.{ext}");
        assert_eq!(loaded.post_ingest_hooks.len(), 1);
        assert_eq!(loaded.post_ingest_hooks[0].command, "echo done");
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "panel widths round-trip through the store bit-for-bit; exact equality is the point"
    )]
    fn preferences_and_view_prefs_roundtrip() {
        use crate::{
            media::{FilterMode, SortOrder},
            persistence::{PanelWidths, Preferences, ThemePreference, ViewPrefs},
        };

        let mut store = store();

        // Defaults land on the documented values, not zero/false.
        let defaults = store.settings();
        assert_eq!(defaults.preferences.theme, ThemePreference::Auto);
        assert_eq!(defaults.preferences.thumbnail_size, 256);
        assert!(defaults.preferences.cache_dir.is_none());
        assert!(defaults.view.ascending);
        assert!(defaults.view.group_raw_jpeg);
        assert_eq!(defaults.panel_widths.left, 250.0);
        assert_eq!(defaults.panel_widths.right, 300.0);

        let settings = AppSettings {
            preferences: Preferences {
                theme: ThemePreference::Light,
                thumbnail_size: 512,
                cache_dir: Some(PathBuf::from("/tmp/ferro-cache")),
            },
            view: ViewPrefs {
                sort_order: SortOrder::Rating,
                ascending: false,
                filter_mode: FilterMode::RawOnly,
                new_only: true,
                hide_rejected: true,
                group_raw_jpeg: false,
                group_bursts: false,
                expand_bursts: true,
                date_tree_ascending: false,
            },
            panel_widths: PanelWidths {
                left: 320.0,
                right: 180.0,
            },
            ..AppSettings::default()
        };
        store.set_settings(&settings);

        let loaded = store.settings();
        assert_eq!(loaded.preferences.theme, ThemePreference::Light);
        assert_eq!(loaded.preferences.thumbnail_size, 512);
        assert_eq!(
            loaded.preferences.cache_dir,
            Some(PathBuf::from("/tmp/ferro-cache"))
        );
        assert_eq!(loaded.view.sort_order, SortOrder::Rating);
        assert!(!loaded.view.ascending);
        assert_eq!(loaded.view.filter_mode, FilterMode::RawOnly);
        assert!(loaded.view.hide_rejected);
        assert!(!loaded.view.group_raw_jpeg);
        assert_eq!(loaded.panel_widths.left, 320.0);
        assert_eq!(loaded.panel_widths.right, 180.0);
    }

    #[test]
    fn info_strip_preference_roundtrips() {
        let mut store = store();
        assert!(
            store.settings().info_strip_open,
            "the strip starts open in both compare and the preview"
        );

        store.set_settings(&AppSettings {
            info_strip_open: false,
            ..AppSettings::default()
        });

        assert!(
            !store.settings().info_strip_open,
            "closing the strip is persisted, not session-local"
        );
    }
}
