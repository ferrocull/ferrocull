//! Per-file scan pipeline: EXIF extraction then thumbnail generation, fanned
//! out across a rayon pool.
//!
//! Ordering contract, per file: [`Event::ExifLoaded`] is emitted before
//! [`Event::ThumbnailReady`]. EXIF drives item creation upstream, so it must
//! land first.
//!
//! Cache hits ([`cache::cache_key_from_canonical`] + [`cache::ThumbnailCache`])
//! skip generation entirely.

use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Mutex, mpsc},
};

use chrono::{DateTime, Utc};
use ferrocull_media::FileCategory;

use crate::{
    cache::{self, ThumbnailCache},
    media::{CaptureSettings, CaptureTime},
    thumbnail::{
        self, ExifMetadata, Orientation, generate_photo_thumbnail, generate_raw_with_preread,
    },
    xmp,
};

/// Bytes pre-read from the head of each file, reused for EXIF and thumbnail.
///
/// Large enough to cover the embedded preview of a typical RAW file, small
/// enough to stay cheap: cold scans are I/O-bound and per-file time scales
/// with bytes read. Files whose preview lies deeper fall back to continuation
/// reads on the decode pool.
const INITIAL_READ: usize = 512 * 1024;

/// A file to scan. The concrete value is echoed back unchanged in
/// [`Event::ExifLoaded`] so the caller can build its own item from it.
pub trait Input {
    fn path(&self) -> &Path;
    fn category(&self) -> FileCategory;
    fn xmp_sidecar(&self) -> Option<&Path>;
}

/// Progress events emitted per file, in order (EXIF before thumbnail).
///
/// A file that fails to open or read its head is logged and dropped silently —
/// no event is emitted for it.
pub enum Event<T> {
    /// Capture time (persisted from cache, EXIF, or mtime fallback), capture
    /// settings, and XMP sidecar are available; the input file is handed back
    /// for item construction. `canonical_path` is the file's canonicalized path (or the
    /// raw path when canonicalization fails), resolved here so the caller does
    /// not repeat that I/O on its update loop.
    ExifLoaded {
        file: T,
        canonical_path: PathBuf,
        capture_time: CaptureTime,
        capture_settings: CaptureSettings,
        xmp: Option<xmp::Metadata>,
    },
    /// The thumbnail is cached (freshly generated or already present), or its
    /// generation failed. Carries no pixel data.
    ThumbnailReady {
        path: PathBuf,
        result: Result<(), thumbnail::Error>,
    },
}

/// Read the head of a file for EXIF/thumbnail, returning the buffer and the
/// still-open handle positioned right after it.
fn preread(path: &Path) -> io::Result<(Vec<u8>, File)> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let initial = usize::try_from(len).unwrap_or(usize::MAX).min(INITIAL_READ);
    let mut data = vec![0u8; initial];
    file.read_exact(&mut data)?;
    Ok((data, file))
}

/// Threads reading media file bodies concurrently. Cold-cache scans are
/// I/O-bound and storage throughput collapses when many reads interleave, so a
/// few near-sequential streams beat a full rayon fan-out — more so on SD cards,
/// where interleaving costs most.
const READ_CONCURRENCY: usize = 2;

/// Read-ahead budget between the reader pool and the decode pool: how many
/// pre-read files may sit in the channel awaiting decode. Sized to absorb
/// scheduling jitter without holding tens of full photo bodies in memory.
const READ_QUEUE_DEPTH: usize = 8;

/// A pre-read media file awaiting thumbnail generation on the decode pool:
/// the full body for photos, the head (plus the open handle to read more) for
/// RAW files.
struct ReadFile {
    path: PathBuf,
    category: FileCategory,
    data: Vec<u8>,
    handle: File,
    orientation: Orientation,
    capture_time: CaptureTime,
    capture_settings: CaptureSettings,
    key: Option<String>,
}

/// Scan `files` as a two-stage pipeline. A small reader pool
/// ([`READ_CONCURRENCY`] threads) walks the list in order, resolves the cache,
/// and reads media bodies; the rayon pool decodes, resizes, encodes, and
/// caches. Splitting the stages keeps disk access limited to a few
/// near-sequential streams while decode still uses every core; `on_event`
/// fires from both pools. Blocks until every file is processed.
pub fn run<T, F>(files: Vec<T>, thumbnail_size: u32, cache: Option<&ThumbnailCache>, on_event: F)
where
    T: Input + Send,
    F: Fn(Event<T>) + Sync + Send,
{
    use rayon::prelude::*;

    let queue = Mutex::new(files.into_iter());
    let (tx, rx) = mpsc::sync_channel::<ReadFile>(READ_QUEUE_DEPTH);

    std::thread::scope(|scope| {
        for _ in 0..READ_CONCURRENCY {
            let tx = tx.clone();
            let queue = &queue;
            let on_event = &on_event;
            scope.spawn(move || {
                // Keep the lock inside the closure: a `queue.lock()...next()`
                // scrutinee holds the guard across the whole loop body, which
                // serializes the readers.
                let next_file = || queue.lock().expect("reader queue lock poisoned").next();
                while let Some(file) = next_file() {
                    if let Some(read) = read_stage(file, cache, on_event) {
                        tx.send(read).expect("decode pool hung up");
                    }
                }
            });
        }
        // Readers own the remaining senders; the channel closes when the last
        // reader finishes, ending the decode loop below.
        drop(tx);

        rx.into_iter().par_bridge().for_each(|read| {
            decode_stage(read, thumbnail_size, cache, &on_event);
        });
    });
}

/// Reader-pool stage: resolve the cache (hit emits both events and ends the
/// file's pipeline), otherwise read the media body and hand it to the decode
/// pool. Emits [`Event::ExifLoaded`]; a file that fails to read its head is
/// logged and dropped without emitting any event.
fn read_stage<T, F>(file: T, cache: Option<&ThumbnailCache>, on_event: &F) -> Option<ReadFile>
where
    T: Input,
    F: Fn(Event<T>),
{
    let path = file.path().to_path_buf();
    let category = file.category();

    // The XMP sidecar is a tiny separate file read on both the hit and miss
    // paths, since external ratings can change between sessions.
    let xmp = file
        .xmp_sidecar()
        .and_then(|sidecar| xmp::read_sidecar(sidecar).ok());

    // Canonicalize once here (metadata I/O, not a body read): the result
    // feeds both the cache key and the event, so the caller needn't repeat
    // it on its update loop. On failure, fall back to the raw path and
    // bypass the cache.
    let (canonical_path, key) = match path.canonicalize() {
        Ok(canonical) => {
            let key = match cache::cache_key_from_canonical(&canonical) {
                Ok(k) => Some(k),
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "cache key derivation failed, bypassing cache");
                    None
                }
            };
            (canonical, key)
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "canonicalize failed, bypassing cache");
            (path.clone(), None)
        }
    };

    // Cache hit: the thumbnail and its persisted capture metadata are all
    // present, so we skip every read of the media body.
    if let Some(ref k) = key
        && let Some(c) = cache
        && let Ok(Some(entry)) = c.load(k)
    {
        tracing::debug!(path = %path.display(), "thumbnail cache hit");
        on_event(Event::ExifLoaded {
            file,
            canonical_path,
            capture_time: entry.capture_time,
            capture_settings: entry.capture_settings,
            xmp,
        });
        on_event(Event::ThumbnailReady {
            path,
            result: Ok(()),
        });
        return None;
    }

    tracing::debug!(path = %path.display(), "thumbnail cache miss, generating");
    let (mut data, mut handle) = match preread(&path) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(path = %path.display(), error = %e, "failed to read file, skipping");
            return None;
        }
    };

    // Orientation rides through to the decode pool so EXIF is parsed once per
    // file. Fall back to file modification time if EXIF carries no date.
    let exif = ExifMetadata::parse(&data);
    let capture_time = exif.capture_time.unwrap_or_else(|| {
        let mtime = handle
            .metadata()
            .expect("fstat failed on an open file")
            .modified()
            .expect("file mtime unavailable");
        CaptureTime::new(DateTime::<Utc>::from(mtime), 0)
    });

    on_event(Event::ExifLoaded {
        file,
        canonical_path,
        capture_time,
        capture_settings: exif.capture_settings.clone(),
        xmp,
    });

    // A photo's body IS the image, so the whole read belongs on the reader
    // pool. RAW files usually decode from the pre-read head; the rare
    // continuation reads happen on the decode pool instead.
    if category == FileCategory::Photo
        && let Err(source) = handle.read_to_end(&mut data)
    {
        let result = Err(thumbnail::Error::Io {
            path: path.clone(),
            source,
        });
        on_event(Event::ThumbnailReady { path, result });
        return None;
    }

    Some(ReadFile {
        path,
        category,
        data,
        handle,
        orientation: exif.orientation,
        capture_time,
        capture_settings: exif.capture_settings,
        key,
    })
}

/// Decode-pool stage: generate the thumbnail from the pre-read bytes, cache
/// it, and emit [`Event::ThumbnailReady`].
fn decode_stage<T, F>(
    read: ReadFile,
    thumbnail_size: u32,
    cache: Option<&ThumbnailCache>,
    on_event: &F,
) where
    F: Fn(Event<T>),
{
    let ReadFile {
        path,
        category,
        data,
        mut handle,
        orientation,
        capture_time,
        capture_settings,
        key,
    } = read;

    let thumb_result = match category {
        FileCategory::Photo => generate_photo_thumbnail(&data, orientation, thumbnail_size),
        FileCategory::Raw => {
            generate_raw_with_preread(data, &mut handle, orientation, thumbnail_size, &path)
        }
        _ => Err(thumbnail::Error::UnsupportedFormat { path: path.clone() }),
    };

    if let Ok(ref img) = thumb_result
        && let Some(ref k) = key
        && let Some(c) = cache
    {
        // A failed cache write only costs a regeneration next scan, so the
        // file's own pipeline carries on.
        if let Err(e) = c.put(k, img, capture_time, &capture_settings) {
            tracing::warn!(path = %path.display(), error = %e, "caching thumbnail failed");
        }
    }

    on_event(Event::ThumbnailReady {
        path,
        result: thumb_result.map(|_| ()),
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct TestFile {
        path: PathBuf,
    }

    impl Input for TestFile {
        fn path(&self) -> &Path {
            &self.path
        }

        fn category(&self) -> FileCategory {
            FileCategory::Photo
        }

        fn xmp_sidecar(&self) -> Option<&Path> {
            None
        }
    }

    /// A recorded event, flattened so it can be collected without the generic payload.
    enum Rec {
        Exif(CaptureTime, CaptureSettings),
        Thumbnail(Result<(), String>),
    }

    /// Solid-color JPEG bytes, without any EXIF segment.
    fn jpeg_bytes() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(64, 48, image::Rgb([120, 90, 60]));
        let dynamic = image::DynamicImage::ImageRgb8(img);
        let mut buf = Vec::new();
        dynamic
            .write_to(&mut io::Cursor::new(&mut buf), image::ImageFormat::Jpeg)
            .expect("encode test jpeg");
        buf
    }

    /// Encode a solid-color JPEG (no EXIF, so capture time falls back to mtime).
    fn write_jpeg(path: &Path) {
        std::fs::write(path, jpeg_bytes()).expect("write test jpeg");
    }

    /// The settings baked into [`write_jpeg_with_exif`]: 1/500s at f/2.8,
    /// ISO 400, 50mm, on a Canon EOS R5 — the camera identity stripped of the
    /// padding the fixture writes it with.
    fn fixture_settings() -> CaptureSettings {
        CaptureSettings {
            exposure_time: Some(1.0 / 500.0),
            aperture: Some(2.8),
            iso: Some(400),
            focal_length: Some(50.0),
            make: Some(String::from("Canon")),
            model: Some(String::from("Canon EOS R5")),
        }
    }

    /// Camera identity as firmware pads it: trailing spaces before the
    /// terminating NUL.
    const FIXTURE_MAKE: &[u8] = b"Canon  \0";
    const FIXTURE_MODEL: &[u8] = b"Canon EOS R5 \0";

    /// Encode a solid-color JPEG carrying an EXIF APP1 segment with
    /// [`fixture_settings`] and a `DateTimeOriginal` of 2024:05:01 10:14:22.
    ///
    /// Hand-built rather than written with an EXIF writer: the scan must read
    /// the same byte layout a camera emits, and one fixture is cheaper than a
    /// dependency.
    fn write_jpeg_with_exif(path: &Path) {
        let jpeg = jpeg_bytes();
        let mut out = Vec::with_capacity(jpeg.len() + 256);
        out.extend_from_slice(&jpeg[..2]); // SOI
        out.extend_from_slice(&app1_exif());
        out.extend_from_slice(&jpeg[2..]);
        std::fs::write(path, &out).expect("write test jpeg");
    }

    /// An `APP1` segment holding a big-endian TIFF block: IFD0 carrying the
    /// camera identity and a pointer to an Exif sub-IFD with the fields the
    /// info strip reads.
    fn app1_exif() -> Vec<u8> {
        /// Byte offset of the Exif sub-IFD from the TIFF header start: 8-byte
        /// header, then IFD0 (three entries).
        const EXIF_IFD: u32 = 8 + 2 + 12 * 3 + 4;
        /// Byte offset of the value area, after the Exif sub-IFD's six entries.
        const VALUES: u32 = EXIF_IFD + 2 + 12 * 6 + 4;
        /// Offset of the `Make` string, past the Exif sub-IFD's own values:
        /// two rationals, a 20-byte timestamp, and a third rational.
        const MAKE: u32 = VALUES + 8 + 8 + 20 + 8;

        let ascii_count =
            |bytes: &[u8]| u32::try_from(bytes.len()).expect("fixture string length fits u32");
        let model_offset = MAKE + ascii_count(FIXTURE_MAKE);

        let mut tiff = vec![0x4D, 0x4D, 0x00, 0x2A];
        tiff.extend_from_slice(&8u32.to_be_bytes());

        let entry = |tag: u16, format: u16, count: u32, value: [u8; 4], out: &mut Vec<u8>| {
            out.extend_from_slice(&tag.to_be_bytes());
            out.extend_from_slice(&format.to_be_bytes());
            out.extend_from_slice(&count.to_be_bytes());
            out.extend_from_slice(&value);
        };

        // IFD0: the camera identity, then the Exif sub-IFD pointer.
        tiff.extend_from_slice(&3u16.to_be_bytes());
        entry(
            0x010F,
            2,
            ascii_count(FIXTURE_MAKE),
            MAKE.to_be_bytes(),
            &mut tiff,
        );
        entry(
            0x0110,
            2,
            ascii_count(FIXTURE_MODEL),
            model_offset.to_be_bytes(),
            &mut tiff,
        );
        entry(0x8769, 4, 1, EXIF_IFD.to_be_bytes(), &mut tiff);
        tiff.extend_from_slice(&0u32.to_be_bytes());

        // Exif sub-IFD, entries in ascending tag order as the spec requires.
        tiff.extend_from_slice(&6u16.to_be_bytes());
        entry(0x829A, 5, 1, VALUES.to_be_bytes(), &mut tiff); // ExposureTime
        entry(0x829D, 5, 1, (VALUES + 8).to_be_bytes(), &mut tiff); // FNumber
        entry(0x8827, 3, 1, [0x01, 0x90, 0, 0], &mut tiff); // ISO 400, inline
        entry(0x9003, 2, 20, (VALUES + 16).to_be_bytes(), &mut tiff); // DateTimeOriginal
        entry(0x920A, 5, 1, (VALUES + 36).to_be_bytes(), &mut tiff); // FocalLength
        entry(0x9291, 2, 3, [b'4', b'5', 0, 0], &mut tiff); // SubSecTimeOriginal, inline
        tiff.extend_from_slice(&0u32.to_be_bytes());

        let rational = |num: u32, den: u32, out: &mut Vec<u8>| {
            out.extend_from_slice(&num.to_be_bytes());
            out.extend_from_slice(&den.to_be_bytes());
        };
        rational(1, 500, &mut tiff);
        rational(28, 10, &mut tiff);
        tiff.extend_from_slice(b"2024:05:01 10:14:22\0");
        rational(50, 1, &mut tiff);
        tiff.extend_from_slice(FIXTURE_MAKE);
        tiff.extend_from_slice(FIXTURE_MODEL);

        let mut app1 = vec![0xFF, 0xE1];
        let payload_len = 2 + 6 + tiff.len();
        app1.extend_from_slice(
            &u16::try_from(payload_len)
                .expect("exif segment fits a jpeg marker")
                .to_be_bytes(),
        );
        app1.extend_from_slice(b"Exif\0\0");
        app1.extend_from_slice(&tiff);
        app1
    }

    fn scan_recording(files: Vec<TestFile>, cache: Option<&ThumbnailCache>) -> Vec<Rec> {
        let recorded = Mutex::new(Vec::new());
        run(files, 256, cache, |event| {
            let rec = match event {
                Event::ExifLoaded {
                    capture_time,
                    capture_settings,
                    ..
                } => Rec::Exif(capture_time, capture_settings),
                Event::ThumbnailReady { result, .. } => {
                    Rec::Thumbnail(result.map_err(|e| e.to_string()))
                }
            };
            recorded.lock().expect("lock recorder").push(rec);
        });
        recorded.into_inner().expect("unwrap recorder")
    }

    #[test]
    fn emits_exif_before_thumbnail() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("shot.jpg");
        write_jpeg(&path);

        let recs = scan_recording(vec![TestFile { path }], None);

        assert_eq!(recs.len(), 2, "one exif and one thumbnail event");
        assert!(matches!(recs[0], Rec::Exif(..)), "exif comes first");
        assert!(
            matches!(recs[1], Rec::Thumbnail(Ok(()))),
            "thumbnail comes second and succeeds"
        );
    }

    #[test]
    fn falls_back_to_mtime_without_exif() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("shot.jpg");
        write_jpeg(&path);

        let mtime = std::fs::metadata(&path)
            .expect("stat test jpeg")
            .modified()
            .expect("mtime available");
        let expected = DateTime::<Utc>::from(mtime);

        let recs = scan_recording(vec![TestFile { path }], None);

        let Rec::Exif(capture_time, _) = &recs[0] else {
            panic!("first event is exif");
        };
        assert_eq!(
            capture_time.second, expected,
            "capture time is the file mtime"
        );
        assert_eq!(
            capture_time.subsec_nanos, 0,
            "mtime fallback has no subseconds"
        );
    }

    #[test]
    fn cache_hit_skips_regeneration() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let cache_dir = dir.path().join("cache");
        let cache = ThumbnailCache::open_at(cache_dir).expect("open cache");

        let path = dir.path().join("shot.jpg");
        write_jpeg(&path);
        let mtime = std::fs::metadata(&path)
            .expect("stat test jpeg")
            .modified()
            .expect("mtime available");
        let len = std::fs::metadata(&path).expect("stat test jpeg").len();

        let first = scan_recording(vec![TestFile { path: path.clone() }], Some(&cache));
        assert!(
            matches!(first[1], Rec::Thumbnail(Ok(()))),
            "first run generates and caches the thumbnail"
        );

        // Corrupt the pixels but preserve length + mtime so the cache key is
        // identical: a cache miss would try to decode this and fail, so a
        // succeeding second run proves the cached thumbnail was reused.
        let corrupt = vec![0u8; usize::try_from(len).expect("length fits usize")];
        std::fs::write(&path, &corrupt).expect("overwrite pixels");
        File::open(&path)
            .expect("reopen for mtime")
            .set_modified(mtime)
            .expect("restore mtime");

        let second = scan_recording(vec![TestFile { path }], Some(&cache));
        assert!(
            matches!(second[1], Rec::Thumbnail(Ok(()))),
            "second run yields the cached thumbnail without decoding"
        );

        // The second run recovers capture time from the persisted sidecar,
        // never reading the now-corrupt body.
        let (Rec::Exif(first_time, _), Rec::Exif(second_time, _)) = (&first[0], &second[0]) else {
            panic!("each run emits exif first");
        };
        assert_eq!(
            first_time, second_time,
            "capture time is recovered from cache metadata, matching the first run"
        );
    }

    #[test]
    fn reads_capture_settings_from_exif() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("shot.jpg");
        write_jpeg_with_exif(&path);

        let recs = scan_recording(vec![TestFile { path }], None);

        let Rec::Exif(capture_time, settings) = &recs[0] else {
            panic!("first event is exif");
        };
        assert_eq!(settings, &fixture_settings(), "every setting is parsed");
        assert_eq!(
            capture_time.subsec_nanos, 450_000_000,
            "subseconds come from SubSecTimeOriginal"
        );
    }

    #[test]
    fn capture_settings_are_absent_without_exif() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let path = dir.path().join("shot.jpg");
        write_jpeg(&path);

        let recs = scan_recording(vec![TestFile { path }], None);

        let Rec::Exif(_, settings) = &recs[0] else {
            panic!("first event is exif");
        };
        assert_eq!(
            settings,
            &CaptureSettings::default(),
            "a file with no EXIF carries no settings"
        );
    }

    #[test]
    fn cache_hit_recovers_capture_settings() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let cache = ThumbnailCache::open_at(dir.path().join("cache")).expect("open cache");

        let path = dir.path().join("shot.jpg");
        write_jpeg_with_exif(&path);
        let stat = std::fs::metadata(&path).expect("stat test jpeg");
        let mtime = stat.modified().expect("mtime available");

        drop(scan_recording(
            vec![TestFile { path: path.clone() }],
            Some(&cache),
        ));

        // Blank the body while preserving length + mtime: the second run's
        // settings can only come from the cache sidecar.
        let corrupt = vec![0u8; usize::try_from(stat.len()).expect("length fits usize")];
        std::fs::write(&path, &corrupt).expect("overwrite pixels");
        File::open(&path)
            .expect("reopen for mtime")
            .set_modified(mtime)
            .expect("restore mtime");

        let second = scan_recording(vec![TestFile { path }], Some(&cache));

        let Rec::Exif(_, settings) = &second[0] else {
            panic!("second run emits exif first");
        };
        assert_eq!(
            settings,
            &fixture_settings(),
            "warm scan resurfaces the settings without reading the body"
        );
    }
}
