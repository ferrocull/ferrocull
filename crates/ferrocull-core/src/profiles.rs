use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Shared ingest configuration embedded in both [`Profile`] and the app settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IngestConfig {
    pub photos_dest: PathBuf,
    pub videos_dest: PathBuf,
    pub photo_pattern: String,
    pub video_pattern: String,
    pub backup_destinations: Vec<PathBuf>,
}

impl Default for IngestConfig {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| {
            tracing::warn!("could not determine home directory, using '.' as fallback");
            PathBuf::from(".")
        });
        let pictures = dirs::picture_dir().unwrap_or_else(|| home.join("Pictures"));
        let videos = dirs::video_dir().unwrap_or_else(|| home.join("Videos"));

        Self {
            photos_dest: pictures,
            videos_dest: videos,
            photo_pattern: String::from("{YYYY}/{MM}/{DD}/{filename}.{ext}"),
            video_pattern: String::from("{YYYY}/{MM}/{DD}/{filename}.{ext}"),
            backup_destinations: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub ingest: IngestConfig,
}

#[derive(Debug, Clone)]
pub struct NamedProfile {
    pub name: String,
    pub profile: Profile,
}
