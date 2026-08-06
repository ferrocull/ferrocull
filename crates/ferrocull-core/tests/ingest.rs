// An integration test exercises the crate's public API; the package's other
// dependencies are linked but never imported here.
#![expect(unused_crate_dependencies)]

//! Filesystem-level tests of the ingest contract: sources are only deleted
//! after every copy verifies, backup failures are reported rather than
//! swallowed, and a re-run repairs a partially failed ingest in place
//! instead of duplicating or destroying verified copies.

use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{TimeZone, Utc};
use ferrocull_core::{
    FileCategory, MediaFile,
    ingest::{self, Job, Tracker, execute_ingest},
};
use tempfile::TempDir;

const CONTENT: &[u8] = b"raw sensor payload";
const RENDERED: &str = "2024/IMG_0001.CR2";

struct Fixture {
    _root: TempDir,
    card: PathBuf,
    dest: PathBuf,
    backup: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().expect("failed to create temp dir");
        let card = root.path().join("card");
        let dest = root.path().join("dest");
        let backup = root.path().join("backup");
        for dir in [&card, &dest, &backup] {
            fs::create_dir(dir).expect("failed to create fixture dir");
        }
        fs::write(card.join("IMG_0001.CR2"), CONTENT).expect("failed to write source");
        Self {
            _root: root,
            card,
            dest,
            backup,
        }
    }

    fn source(&self) -> PathBuf {
        self.card.join("IMG_0001.CR2")
    }

    fn primary(&self) -> PathBuf {
        self.dest.join(RENDERED)
    }

    fn backup_copy(&self) -> PathBuf {
        self.backup.join(RENDERED)
    }

    fn media_file(&self) -> MediaFile {
        MediaFile {
            path: self.source(),
            datetime: Utc
                .with_ymd_and_hms(2024, 5, 1, 10, 14, 22)
                .single()
                .expect("unambiguous test timestamp"),
            media_type: FileCategory::Raw,
            paired_files: Vec::new(),
            sidecars: Vec::new(),
            xmp_sidecar: None,
            rating: 0,
            color_label: None,
            rendered_dest: Some(RENDERED.to_owned()),
        }
    }

    fn job(&self, delete_after_ingest: bool) -> Job {
        Job {
            files: vec![self.media_file()],
            dest_base: self.dest.clone(),
            videos_dest: self.dest.clone(),
            backup_destinations: vec![self.backup.clone()],
            delete_after_ingest,
        }
    }
}

fn run(job: &Job) -> Vec<ingest::FileResult> {
    execute_ingest(job, &Tracker::default())
}

fn single_success(mut results: Vec<ingest::FileResult>) -> ingest::Success {
    assert_eq!(results.len(), 1, "one file in, one result out");
    results
        .pop()
        .expect("results is empty")
        .expect("expected the file to ingest successfully")
}

fn file_count(dir: &Path) -> usize {
    fs::read_dir(dir)
        .expect("failed to read dir")
        .map(|entry| entry.expect("failed to read dir entry"))
        .map(|entry| {
            if entry.file_type().expect("failed to stat entry").is_dir() {
                file_count(&entry.path())
            } else {
                1
            }
        })
        .sum()
}

#[test]
fn clean_run_copies_everywhere_and_deletes_the_source() {
    let fx = Fixture::new();

    let success = single_success(run(&fx.job(true)));

    assert!(success.backup_failures.is_empty());
    assert!(success.source_deleted);
    assert!(!fx.source().exists(), "source stays on the card");
    assert_eq!(fs::read(fx.primary()).expect("primary missing"), CONTENT);
    assert_eq!(fs::read(fx.backup_copy()).expect("backup missing"), CONTENT);
}

#[test]
fn backup_failure_is_reported_and_keeps_the_source() {
    let fx = Fixture::new();
    // A file where the backup needs a directory makes every copy into that
    // backup fail while leaving the primary untouched.
    fs::write(fx.backup.join("2024"), b"not a directory").expect("failed to write blocker");

    let success = single_success(run(&fx.job(true)));

    assert_eq!(success.backup_failures.len(), 1);
    assert!(!success.source_deleted);
    assert!(fx.source().exists(), "source must survive a backup failure");
    assert_eq!(fs::read(fx.primary()).expect("primary missing"), CONTENT);
}

#[test]
fn rerun_after_backup_failure_repairs_in_place() {
    let fx = Fixture::new();
    fs::write(fx.backup.join("2024"), b"not a directory").expect("failed to write blocker");
    let job = fx.job(true);
    let first = single_success(run(&job));
    assert_eq!(first.backup_failures.len(), 1);

    fs::remove_file(fx.backup.join("2024")).expect("failed to remove blocker");
    let second = single_success(run(&job));

    assert!(second.backup_failures.is_empty());
    assert_eq!(second.checksum, first.checksum);
    assert!(second.source_deleted);
    assert!(!fx.source().exists());
    assert_eq!(fs::read(fx.backup_copy()).expect("backup missing"), CONTENT);
    assert_eq!(
        file_count(&fx.dest),
        1,
        "the matched primary must not be duplicated"
    );
}

#[test]
fn matching_existing_backup_is_a_completed_copy() {
    let fx = Fixture::new();
    fs::create_dir(fx.backup.join("2024")).expect("failed to create backup dir");
    fs::write(fx.backup_copy(), CONTENT).expect("failed to pre-fill backup");

    let success = single_success(run(&fx.job(true)));

    assert!(success.backup_failures.is_empty());
    assert!(success.source_deleted);
}

#[test]
fn mismatched_existing_backup_fails_and_is_not_overwritten() {
    let fx = Fixture::new();
    fs::create_dir(fx.backup.join("2024")).expect("failed to create backup dir");
    fs::write(fx.backup_copy(), b"someone else's photo").expect("failed to pre-fill backup");

    let success = single_success(run(&fx.job(true)));

    assert_eq!(success.backup_failures.len(), 1);
    assert!(!success.source_deleted);
    assert!(fx.source().exists());
    assert_eq!(
        fs::read(fx.backup_copy()).expect("backup missing"),
        b"someone else's photo",
        "a mismatched backup must never be overwritten"
    );
}

#[test]
fn mismatched_existing_primary_is_a_collision() {
    let fx = Fixture::new();
    fs::create_dir(fx.dest.join("2024")).expect("failed to create dest dir");
    fs::write(fx.primary(), b"someone else's photo").expect("failed to pre-fill primary");

    let mut results = run(&fx.job(true));
    let failure = results
        .pop()
        .expect("results is empty")
        .expect_err("a mismatched primary must fail the file");

    assert!(matches!(failure.error, ingest::Error::Copy(_)));
    assert!(fx.source().exists());
    assert_eq!(
        fs::read(fx.primary()).expect("primary missing"),
        b"someone else's photo",
        "a mismatched primary must never be overwritten"
    );
}

#[test]
fn xmp_failure_rolls_back_fresh_copies_but_spares_the_matched_primary() {
    let fx = Fixture::new();
    // The primary already matches, as after a run whose backup failed.
    fs::create_dir(fx.dest.join("2024")).expect("failed to create dest dir");
    fs::write(fx.primary(), CONTENT).expect("failed to pre-fill primary");
    // A rating forces an XMP sidecar write; a directory squatting on the
    // sidecar path makes that write fail after the copies land.
    fs::create_dir_all(fx.dest.join("2024/IMG_0001.CR2.xmp"))
        .expect("failed to create sidecar blocker");
    let mut job = fx.job(true);
    job.files[0].rating = 3;

    let mut results = run(&job);
    let failure = results
        .pop()
        .expect("results is empty")
        .expect_err("a failed XMP write must fail the file");

    assert!(matches!(failure.error, ingest::Error::Xmp { .. }));
    assert!(fx.source().exists());
    assert_eq!(
        fs::read(fx.primary()).expect("primary missing"),
        CONTENT,
        "a rollback must spare the pre-existing matched primary"
    );
    assert!(
        !fx.backup_copy().exists(),
        "a rollback must remove the backup copied during the failed run"
    );
}
