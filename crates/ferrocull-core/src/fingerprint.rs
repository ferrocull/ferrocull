//! Location-independent ingest fingerprint.
//!
//! The fingerprint is a *provenance* identity for a frame, hashed from its
//! basename, file size, and capture time with the shared `XxHash3_128` → 32-hex
//! convention. It answers "have I ever ingested this frame on this machine?"
//! regardless of where the frame currently lives, so a re-inserted card (or a
//! moved file) judges newness correctly.
//!
//! It must not be confused with the thumbnail/preview cache key
//! ([`crate::cache::cache_key_from_disk`]), which is deliberately path- and
//! mtime-bound: a validity token for a derived artifact. The two identities have
//! contravariant requirements — each includes exactly what the other excludes —
//! and share only the hash helper.

use twox_hash::XxHash3_128;

use crate::media::CaptureTime;

/// Ingest fingerprint for a frame, from its `basename`, file `size`, and
/// `capture_time`.
///
/// Pure: the same three inputs always yield the same key, and no location
/// (path, mtime) enters — so two frames differing only in source path share a
/// fingerprint, while a differing size or capture time yields a different one.
///
/// Basename cuts collisions among frames sharing size + capture-second (bursts
/// get distinct camera names). When `capture_time` was derived from mtime
/// (a file carrying no capture metadata), the fingerprint is not perfectly
/// location-independent; that degrades only toward re-offering an
/// already-ingested frame, which is the accepted-in-alpha direction.
#[must_use]
pub fn ingest_fingerprint(basename: &str, size: u64, capture_time: CaptureTime) -> String {
    let mut key_bytes = basename.as_bytes().to_vec();
    key_bytes.extend_from_slice(&size.to_le_bytes());
    key_bytes.extend_from_slice(&capture_time.second.timestamp().to_le_bytes());
    key_bytes.extend_from_slice(&capture_time.second.timestamp_subsec_nanos().to_le_bytes());
    key_bytes.extend_from_slice(&capture_time.subsec_nanos.to_le_bytes());

    let hash = XxHash3_128::oneshot(&key_bytes);
    format!("{hash:032x}")
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::*;

    fn capture(secs: i64, subsec_nanos: u32) -> CaptureTime {
        CaptureTime::new(
            DateTime::<Utc>::from_timestamp(secs, 0).expect("valid timestamp"),
            subsec_nanos,
        )
    }

    #[test]
    fn same_frame_from_different_paths_shares_a_fingerprint() {
        // Two items differing only in source path — the path never enters the
        // fingerprint, so a re-inserted card judges the frame as the same one.
        let capture = capture(1_700_000_000, 500);
        assert_eq!(
            ingest_fingerprint("IMG_0001.CR2", 24_000_000, capture),
            ingest_fingerprint("IMG_0001.CR2", 24_000_000, capture),
        );
    }

    #[test]
    fn a_different_size_yields_a_different_fingerprint() {
        let capture = capture(1_700_000_000, 500);
        assert_ne!(
            ingest_fingerprint("IMG_0001.CR2", 24_000_000, capture),
            ingest_fingerprint("IMG_0001.CR2", 24_000_001, capture),
        );
    }

    #[test]
    fn a_different_capture_time_yields_a_different_fingerprint() {
        assert_ne!(
            ingest_fingerprint("IMG_0001.CR2", 24_000_000, capture(1_700_000_000, 0)),
            ingest_fingerprint("IMG_0001.CR2", 24_000_000, capture(1_700_000_001, 0)),
            "differing capture second"
        );
        assert_ne!(
            ingest_fingerprint("IMG_0001.CR2", 24_000_000, capture(1_700_000_000, 0)),
            ingest_fingerprint("IMG_0001.CR2", 24_000_000, capture(1_700_000_000, 1)),
            "differing capture subsecond"
        );
    }

    #[test]
    fn a_reused_camera_name_for_a_different_frame_is_distinct() {
        // Reformatted card reuses IMG_0001 for a genuinely different shot: a
        // different size (or capture time) must read as a different frame.
        assert_ne!(
            ingest_fingerprint("IMG_0001.CR2", 24_000_000, capture(1_700_000_000, 0)),
            ingest_fingerprint("IMG_0001.CR2", 31_500_000, capture(1_800_000_000, 0)),
        );
    }

    #[test]
    fn the_key_is_32_lowercase_hex_chars() {
        let key = ingest_fingerprint("IMG_0001.CR2", 24_000_000, capture(1, 0));
        assert_eq!(key.len(), 32);
        assert!(
            key.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
}
