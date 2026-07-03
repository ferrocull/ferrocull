use std::{
    collections::HashMap,
    ffi::OsStr,
    path::{Path, PathBuf},
};

use ferrocull_media::{FileCategory, categorize_extension, is_media_file};
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct ScannedFile {
    /// Primary file path (RAW takes precedence over JPEG in pairs).
    pub path: PathBuf,
    /// Media type determined at scan time (RAW > Video > Photo priority).
    pub media_type: FileCategory,
    /// Paired files (e.g., JPEG paired with RAW).
    pub paired: Vec<PathBuf>,
    /// Non-XMP sidecar files (THM, WAV, MP3).
    pub sidecars: Vec<PathBuf>,
    /// XMP sidecar file, if present.
    pub xmp_sidecar: Option<PathBuf>,
}

const CAMERA_FOLDERS: &[&str] = &["DCIM", "PRIVATE", "MP_ROOT"];

fn base_name(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
}

/// Scan a directory recursively for media files.
///
/// Returns files grouped by RAW+JPEG pairing and sidecar association.
/// RAW files take precedence as the primary file when paired with JPEG.
#[must_use]
pub fn scan_directory(root: &Path) -> Vec<ScannedFile> {
    let files: Vec<PathBuf> = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| match entry {
            Ok(e) => Some(e),
            Err(e) => {
                tracing::warn!(error = %e, "skipping directory entry during scan");
                None
            }
        })
        .filter(|e| e.file_type().is_file())
        .filter(|e| is_media_file(e.path()))
        .map(walkdir::DirEntry::into_path)
        .collect();

    let mut groups: HashMap<(PathBuf, String), Vec<PathBuf>> = HashMap::new();
    for path in files {
        let parent = path
            .parent()
            .expect("WalkDir file has parent")
            .to_path_buf();
        let Some(base) = base_name(&path) else {
            continue;
        };
        groups.entry((parent, base)).or_default().push(path);
    }

    groups.into_values().filter_map(scanned_file).collect()
}

/// Scan only standard camera folders (DCIM, PRIVATE, `MP_ROOT`) if they exist.
/// Falls back to full directory scan if no camera folders found.
#[must_use]
pub fn scan_camera_folders(root: &Path) -> Vec<ScannedFile> {
    let camera_dirs: Vec<PathBuf> = CAMERA_FOLDERS
        .iter()
        .map(|name| root.join(name))
        .filter(|p| p.is_dir())
        .collect();

    if camera_dirs.is_empty() {
        return scan_directory(root);
    }

    camera_dirs
        .into_iter()
        .flat_map(|dir| scan_directory(&dir))
        .collect()
}

fn scanned_file(paths: Vec<PathBuf>) -> Option<ScannedFile> {
    let (mut raws, mut photos, mut videos, mut sidecars) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());

    for path in paths {
        let ext = path
            .extension()
            .and_then(OsStr::to_str)
            .expect("media file has ASCII extension");
        match categorize_extension(ext).expect("media file has known category") {
            FileCategory::Raw => raws.push(path),
            FileCategory::Photo => photos.push(path),
            FileCategory::Video => videos.push(path),
            FileCategory::Sidecar => sidecars.push(path),
        }
    }

    raws.sort_unstable();
    photos.sort_unstable();
    videos.sort_unstable();
    sidecars.sort_unstable();

    // Priority: RAW > Video > Photo
    let (primary, media_type, mut paired) = if !raws.is_empty() {
        let primary = raws.remove(0);
        raws.extend(videos);
        raws.extend(photos);
        (primary, FileCategory::Raw, raws)
    } else if !videos.is_empty() {
        let primary = videos.remove(0);
        videos.extend(photos);
        (primary, FileCategory::Video, videos)
    } else if !photos.is_empty() {
        let primary = photos.remove(0);
        (primary, FileCategory::Photo, photos)
    } else {
        return None;
    };

    paired.sort_unstable();

    let xmp_pos = sidecars.iter().position(|p| {
        p.extension()
            .expect("sidecar has extension")
            .to_str()
            .expect("sidecar extension is ASCII")
            .eq_ignore_ascii_case("xmp")
    });
    let xmp_sidecar = xmp_pos.map(|i| sidecars.remove(i));

    Some(ScannedFile {
        path: primary,
        media_type,
        paired,
        sidecars,
        xmp_sidecar,
    })
}
