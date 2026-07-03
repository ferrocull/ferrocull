use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

const MAX_HISTORY: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("failed to parse job code history: {0}")]
    Parse(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JobCodeHistory {
    codes: Vec<String>,
}

impl JobCodeHistory {
    /// Add a job code to history. Moves it to front if already present.
    /// Trims to [`MAX_HISTORY`] items.
    pub fn add(&mut self, code: &str) {
        self.codes.retain(|c| c != code);
        self.codes.insert(0, code.to_owned());
        self.codes.truncate(MAX_HISTORY);
    }

    #[must_use]
    pub fn codes(&self) -> &[String] {
        &self.codes
    }

    /// Load from JSON file. Returns empty history if file doesn't exist.
    ///
    /// # Errors
    /// Returns error if file exists but cannot be read or parsed.
    pub fn load(path: &Path) -> Result<Self, Error> {
        match fs::read_to_string(path) {
            Ok(contents) => Ok(serde_json::from_str(&contents)?),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::Io {
                path: path.to_owned(),
                source: e,
            }),
        }
    }

    /// Save to JSON file. Creates parent directories if needed.
    ///
    /// # Errors
    /// Returns error if file cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        let parent = path.parent().expect("save path has a parent directory");
        fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_owned(),
            source,
        })?;
        let contents = serde_json::to_string_pretty(self)?;
        fs::write(path, contents).map_err(|source| Error::Io {
            path: path.to_owned(),
            source,
        })
    }
}
