use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};

/// A user-configured post-download hook (persisted in settings/profiles).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hook {
    pub name: String,
    pub command: String,
    pub enabled: bool,
}

/// Minimal hook spec for execution — no serde, no enabled flag.
/// The caller is responsible for filtering enabled hooks before calling `run_hooks`.
#[derive(Debug, Clone)]
pub struct Spec<'a> {
    pub name: &'a str,
    pub command: &'a str,
}

#[derive(Debug, Clone)]
pub struct Context {
    pub dest_dir: PathBuf,
    pub files_downloaded: Vec<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to create temp file for file list: {0}")]
    TempFile(io::Error),
    #[error("failed to spawn hook '{command}': {source}")]
    Spawn { command: String, source: io::Error },
    #[error("hook '{name}' exited with code {code:?}{}", if stderr.is_empty() { String::new() } else { format!(": {stderr}") })]
    NonZeroExit {
        name: String,
        code: Option<i32>,
        stderr: String,
    },
}

/// Create a temp file listing all downloaded paths, one per line.
fn create_file_list(ctx: &Context) -> Result<tempfile::NamedTempFile, Error> {
    let mut f = tempfile::NamedTempFile::new().map_err(Error::TempFile)?;
    for path in &ctx.files_downloaded {
        writeln!(f, "{}", path.display()).map_err(Error::TempFile)?;
    }
    Ok(f)
}

/// Runs a single hook with a pre-created file list temp file.
fn run_single(hook: &Spec<'_>, ctx: &Context, file_list_path: &Path) -> Result<(), Error> {
    let (shell, shell_arg) = if cfg!(target_os = "windows") {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };

    let output = Command::new(shell)
        .arg(shell_arg)
        .arg(hook.command)
        .env("FERROCULL_DEST_DIR", &ctx.dest_dir)
        .env(
            "FERROCULL_FILE_COUNT",
            ctx.files_downloaded.len().to_string(),
        )
        .env("FERROCULL_FILE_LIST", file_list_path)
        .output()
        .map_err(|source| Error::Spawn {
            command: hook.command.to_owned(),
            source,
        })?;

    if !output.status.success() {
        return Err(Error::NonZeroExit {
            name: hook.name.to_owned(),
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    Ok(())
}

/// Runs hooks sequentially, sharing a single temp file for the file list.
///
/// The caller is responsible for filtering to only enabled hooks.
/// Returns a result for each hook. Errors from one hook do not prevent subsequent hooks from running.
///
/// If the shared file list cannot be created, returns one `TempFile` error per hook.
#[must_use]
pub fn run_hooks(hooks: &[Spec<'_>], ctx: &Context) -> Vec<Result<(), Error>> {
    if hooks.is_empty() {
        return Vec::new();
    }

    let temp_file = match create_file_list(ctx) {
        Ok(f) => f,
        Err(Error::TempFile(io_err)) => {
            let msg = io_err.to_string();
            let kind = io_err.kind();
            return hooks
                .iter()
                .map(|_| Err(Error::TempFile(io::Error::new(kind, msg.clone()))))
                .collect();
        }
        Err(e) => return vec![Err(e)],
    };
    let temp_path = temp_file.into_temp_path();

    hooks
        .iter()
        .map(|h| run_single(h, ctx, &temp_path))
        .collect()
}
