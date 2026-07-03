use std::{
    fs::{self, File},
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

const BUFFER_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub bytes_copied: u64,
    pub total_bytes: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error copying {} to {}: {source}", src.display(), dest.display())]
    Io {
        src: PathBuf,
        dest: PathBuf,
        source: io::Error,
    },
    #[error("destination already exists: {}", path.display())]
    DestinationExists { path: PathBuf },
}

/// Copies a file from src to dest while computing SHA256 in a single pass.
///
/// Returns the hex-encoded SHA256 checksum on success.
///
/// # Errors
///
/// Returns `Error::DestinationExists` if dest already exists.
/// Returns `Error::Io` on I/O failures.
pub(crate) fn copy_with_checksum(
    src: &Path,
    dest: &Path,
    mut progress_fn: impl FnMut(Progress),
) -> Result<String, Error> {
    let map_io = |source: io::Error| Error::Io {
        src: src.to_path_buf(),
        dest: dest.to_path_buf(),
        source,
    };

    let src_file = File::open(src).map_err(&map_io)?;
    let total_bytes = src_file.metadata().map_err(&map_io)?.len();
    let mut reader = BufReader::with_capacity(BUFFER_SIZE, src_file);

    let parent = dest.parent().expect("dest has a parent directory");
    fs::create_dir_all(parent).map_err(&map_io)?;

    let dest_file = File::create_new(dest).map_err(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            Error::DestinationExists {
                path: dest.to_path_buf(),
            }
        } else {
            map_io(e)
        }
    })?;
    let result = (|| {
        let mut writer = BufWriter::with_capacity(BUFFER_SIZE, dest_file);
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; BUFFER_SIZE].into_boxed_slice();
        let mut bytes_copied: u64 = 0;

        loop {
            let bytes_read = reader.read(&mut buffer).map_err(&map_io)?;
            if bytes_read == 0 {
                break;
            }

            let chunk = &buffer[..bytes_read];
            hasher.update(chunk);
            writer.write_all(chunk).map_err(&map_io)?;

            bytes_copied += bytes_read as u64;
            progress_fn(Progress {
                bytes_copied,
                total_bytes,
            });
        }

        writer.flush().map_err(&map_io)?;
        writer
            .into_inner()
            .map_err(|e| map_io(e.into_error()))?
            .sync_all()
            .map_err(&map_io)?;

        Ok(hex::encode(hasher.finalize()))
    })();

    if result.is_err() {
        drop(fs::remove_file(dest));
    }

    result
}

/// Hashes a file with SHA256 in a single buffered pass.
pub(crate) fn hash_file(path: &Path) -> Result<[u8; 32], io::Error> {
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(BUFFER_SIZE, file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; BUFFER_SIZE].into_boxed_slice();

    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(hasher.finalize().into())
}
