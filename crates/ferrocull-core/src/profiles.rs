use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::settings::IngestConfig;

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

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("failed to parse profile: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("failed to serialize profile: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("invalid profile name: {0}")]
    InvalidName(String),
    #[error("could not determine config directory")]
    NoConfigDir,
}

/// Validates a profile name for filesystem safety without rewriting it.
/// Internal spaces are allowed. Leading/trailing whitespace is rejected.
fn validate_name(name: &str) -> Result<(), Error> {
    if name.is_empty() {
        return Err(Error::InvalidName(
            "profile name must not be empty".to_owned(),
        ));
    }
    if name != name.trim() {
        return Err(Error::InvalidName(
            "leading or trailing whitespace is not allowed".to_owned(),
        ));
    }
    if name == "." || name == ".." {
        return Err(Error::InvalidName(
            "'.' and '..' are not valid profile names".to_owned(),
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(Error::InvalidName(
            "control characters are not allowed".to_owned(),
        ));
    }
    if name
        .chars()
        .any(|c| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return Err(Error::InvalidName(
            "name contains filesystem-reserved characters".to_owned(),
        ));
    }
    Ok(())
}

/// Returns the platform-appropriate profiles directory path.
///
/// - Linux: `~/.config/Ferrocull/profiles/`
/// - macOS: `~/Library/Application Support/Ferrocull/profiles/`
/// - Windows: `%APPDATA%/Ferrocull/profiles/`
pub fn profiles_dir() -> Result<PathBuf, Error> {
    let config_dir = dirs::config_dir().ok_or(Error::NoConfigDir)?;
    Ok(config_dir.join("Ferrocull").join("profiles"))
}

/// Loads all profiles from the given directory.
///
/// Malformed profile files are skipped with a warning rather than
/// failing the entire load.
pub fn load_all(dir: &Path) -> Result<Vec<NamedProfile>, Error> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(Error::Io {
                path: dir.to_owned(),
                source: e,
            });
        }
    };

    let mut profiles: Vec<NamedProfile> = entries
        .filter_map(|entry| match entry {
            Ok(e) => Some(e.path()),
            Err(e) => {
                tracing::warn!(error = %e, "failed to read profile directory entry");
                None
            }
        })
        .filter(|p| p.extension().is_some_and(|ext| ext == "toml"))
        .filter_map(|p| {
            let Some(name) = p.file_stem().and_then(|s| s.to_str()) else {
                tracing::warn!(path = %p.display(), "invalid profile filename");
                return None;
            };

            if let Err(e) = validate_name(name) {
                tracing::warn!(
                    path = %p.display(),
                    error = %e,
                    "invalid profile name from filename"
                );
                return None;
            }

            let contents = match fs::read_to_string(&p) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(path = %p.display(), error = %e, "failed to read profile file");
                    return None;
                }
            };
            match toml::from_str::<Profile>(&contents) {
                Ok(profile) => Some(NamedProfile {
                    name: name.to_owned(),
                    profile,
                }),
                Err(e) => {
                    tracing::warn!(path = %p.display(), error = %e, "failed to parse profile file");
                    None
                }
            }
        })
        .collect();

    profiles.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    Ok(profiles)
}

/// Saves a profile to a TOML file in the given directory.
///
/// # Errors
/// Returns error if name is empty/invalid or file cannot be written.
pub fn save(name: &str, profile: &Profile, dir: &Path) -> Result<(), Error> {
    validate_name(name)?;

    fs::create_dir_all(dir).map_err(|e| Error::Io {
        path: dir.to_owned(),
        source: e,
    })?;

    let filename = format!("{name}.toml");
    let path = dir.join(filename);

    let contents = toml::to_string_pretty(profile)?;
    fs::write(&path, contents).map_err(|e| Error::Io { path, source: e })?;
    Ok(())
}

/// Deletes a profile by name from the given directory.
///
/// # Errors
/// Returns error if name is empty/invalid or file cannot be deleted.
pub fn delete(name: &str, dir: &Path) -> Result<(), Error> {
    validate_name(name)?;

    let filename = format!("{name}.toml");
    let path = dir.join(filename);

    fs::remove_file(&path).map_err(|e| Error::Io { path, source: e })?;
    Ok(())
}
