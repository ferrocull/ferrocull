//! Cold-cache scan pipeline benchmark.
//!
//! Usage: `cargo run --release --example scan_bench -- <dir> [count]`
//!
//! Scans media files under `<dir>` through `scan::run` with a throwaway
//! thumbnail cache, printing throughput. Evict the page cache first for a
//! cold-storage measurement.

// An example only exercises a slice of the crate; the unused-dependency lint
// is aimed at the library itself, and a bench reports through stdout/stderr
// rather than `tracing`.
#![expect(unused_crate_dependencies, reason = "example uses few crate deps")]
#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "bench output goes to the terminal"
)]

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

use ferrocull_core::scan;
use ferrocull_media::FileCategory;

struct BenchFile {
    path: PathBuf,
    category: FileCategory,
}

impl scan::Input for BenchFile {
    fn path(&self) -> &Path {
        &self.path
    }

    fn category(&self) -> FileCategory {
        self.category
    }

    fn xmp_sidecar(&self) -> Option<&Path> {
        None
    }
}

fn collect_media_files(dir: &Path, files: &mut Vec<BenchFile>) {
    for entry in std::fs::read_dir(dir).expect("read bench directory") {
        let entry = entry.expect("read bench directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_media_files(&path, files);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str())
            && let Some(category) = ferrocull_media::categorize_extension(ext)
            && matches!(category, FileCategory::Photo | FileCategory::Raw)
        {
            files.push(BenchFile { path, category });
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("usage: scan_bench <dir> [count]");
    let count: usize = args
        .next()
        .map_or(usize::MAX, |c| c.parse().expect("count must be a number"));

    let mut files = Vec::new();
    collect_media_files(Path::new(&dir), &mut files);
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.truncate(count);
    let total = files.len();

    let cache_dir = tempfile::tempdir().expect("create temp cache dir");
    let cache = ferrocull_core::cache::ThumbnailCache::open_at(cache_dir.path().to_path_buf())
        .expect("open thumbnail cache");

    let ok = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let start = Instant::now();
    scan::run(files, 512, Some(&cache), |event| {
        if let scan::Event::ThumbnailReady { path, result } = event {
            match result {
                Ok(()) => ok.fetch_add(1, Ordering::Relaxed),
                Err(e) => {
                    eprintln!("FAIL {}: {e}", path.display());
                    failed.fetch_add(1, Ordering::Relaxed)
                }
            };
        }
    });
    let elapsed = start.elapsed();

    #[expect(clippy::cast_precision_loss, reason = "file counts are far below 2^52")]
    let rate = total as f64 / elapsed.as_secs_f64();
    println!(
        "{total} files in {:.2}s = {rate:.1} files/s (ok {}, failed {})",
        elapsed.as_secs_f64(),
        ok.load(Ordering::Relaxed),
        failed.load(Ordering::Relaxed),
    );
}
