use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::hooks::Hook;

/// Shared ingest configuration embedded in both `Profile` and `Settings`.
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
pub struct Settings {
    pub ingest: IngestConfig,
    pub post_download_hooks: Vec<Hook>,
    /// Delete source files after successful download and checksum verification.
    pub delete_after_download: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("failed to parse settings: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("failed to serialize settings: {0}")]
    Serialize(#[from] toml::ser::Error),
}

impl Settings {
    /// Loads settings from the given TOML file.
    ///
    /// # Errors
    /// Returns `Error::Io` if the file cannot be read (except for not found),
    /// or `Error::Parse` if the TOML is malformed.
    pub fn load(path: &Path) -> Result<Self, Error> {
        match fs::read_to_string(path) {
            Ok(contents) => Ok(toml::from_str(&contents)?),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::Io {
                path: path.to_owned(),
                source: e,
            }),
        }
    }

    /// Saves settings to the given TOML file.
    ///
    /// # Errors
    /// Returns `Error::Io` if the file cannot be written,
    /// or `Error::Serialize` if serialization fails.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        let parent = path.parent().expect("settings path has a parent directory");
        fs::create_dir_all(parent).map_err(|e| Error::Io {
            path: parent.to_owned(),
            source: e,
        })?;
        let contents = toml::to_string_pretty(self)?;
        fs::write(path, contents).map_err(|e| Error::Io {
            path: path.to_owned(),
            source: e,
        })?;
        Ok(())
    }
}
