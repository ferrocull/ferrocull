use std::{
    fs::{self, File},
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

/// Chunk size for copy and hash passes. Sized to keep syscall count low on
/// USB card readers, where small reads dominate the per-chunk cost.
const BUFFER_SIZE: usize = 1024 * 1024;

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

/// Outcome of a successful tee copy: the primary destination is written and
/// synced; individual backup destinations may still have failed.
#[derive(Debug)]
pub(crate) struct Tee {
    /// Hex-encoded SHA-256 of the source stream, shared by every copy.
    pub checksum: String,
    pub backup_failures: Vec<(PathBuf, Error)>,
}

/// A backup destination writer that is still alive, or the error that killed it.
type BackupSlot = Result<(PathBuf, BufWriter<File>), (PathBuf, Error)>;

fn open_writer(src: &Path, dest: &Path) -> Result<BufWriter<File>, Error> {
    let map_io = |source: io::Error| Error::Io {
        src: src.to_path_buf(),
        dest: dest.to_path_buf(),
        source,
    };

    let parent = dest.parent().expect("dest has a parent directory");
    fs::create_dir_all(parent).map_err(map_io)?;

    let file = File::create_new(dest).map_err(|e| {
        if e.kind() == io::ErrorKind::AlreadyExists {
            Error::DestinationExists {
                path: dest.to_path_buf(),
            }
        } else {
            map_io(e)
        }
    })?;
    Ok(BufWriter::with_capacity(BUFFER_SIZE, file))
}

/// Remove a partially written destination file, logging a failure to do so:
/// a stray partial file blocks a later retry with `DestinationExists`.
fn remove_partial(path: &Path) {
    if let Err(e) = fs::remove_file(path) {
        tracing::warn!(
            path = %path.display(),
            error = %e,
            "failed to remove partial destination file"
        );
    }
}

/// Flush the writer and fsync the destination file.
fn finish_writer(writer: BufWriter<File>) -> io::Result<()> {
    writer
        .into_inner()
        .map_err(io::IntoInnerError::into_error)?
        .sync_all()
}

/// Copies a file from `src` to `dest` while computing SHA-256 in a single
/// pass, teeing the stream to every backup destination along the way.
///
/// The primary copy is all-or-nothing: any error on the source or `dest`
/// removes every partial file and fails the call. A backup destination that
/// fails (open, write, or sync) has its partial file removed and is reported
/// in [`Tee::backup_failures`] without affecting the other copies.
///
/// `progress_fn` receives the byte count of each chunk as it is copied.
///
/// # Errors
///
/// Returns `Error::DestinationExists` if `dest` already exists.
/// Returns `Error::Io` on source or primary-destination I/O failures.
pub(crate) fn copy_with_checksum(
    src: &Path,
    dest: &Path,
    backups: &[PathBuf],
    mut progress_fn: impl FnMut(u64),
) -> Result<Tee, Error> {
    let map_io = |source: io::Error| Error::Io {
        src: src.to_path_buf(),
        dest: dest.to_path_buf(),
        source,
    };

    let src_file = File::open(src).map_err(map_io)?;
    let mut reader = BufReader::with_capacity(BUFFER_SIZE, src_file);

    let mut primary = open_writer(src, dest)?;
    let mut backup_slots: Vec<BackupSlot> = backups
        .iter()
        .map(|path| match open_writer(src, path) {
            Ok(writer) => Ok((path.clone(), writer)),
            Err(e) => Err((path.clone(), e)),
        })
        .collect();

    let result = (|| {
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; BUFFER_SIZE].into_boxed_slice();

        loop {
            let bytes_read = reader.read(&mut buffer).map_err(map_io)?;
            if bytes_read == 0 {
                break;
            }

            let chunk = &buffer[..bytes_read];
            hasher.update(chunk);
            primary.write_all(chunk).map_err(map_io)?;
            write_to_backups(&mut backup_slots, src, chunk);

            progress_fn(bytes_read as u64);
        }

        finish_writer(primary).map_err(map_io)?;
        Ok(hex::encode(hasher.finalize()))
    })();

    match result {
        Ok(checksum) => Ok(Tee {
            checksum,
            backup_failures: finish_backups(backup_slots, src),
        }),
        Err(e) => {
            remove_partial(dest);
            for (path, writer) in backup_slots.into_iter().flatten() {
                drop(writer);
                remove_partial(&path);
            }
            Err(e)
        }
    }
}

/// Write a chunk to every live backup writer. A writer that fails is killed:
/// its partial file is removed and the slot holds the error from then on.
fn write_to_backups(slots: &mut [BackupSlot], src: &Path, chunk: &[u8]) {
    for slot in slots {
        let Ok((path, writer)) = slot else { continue };
        let Err(e) = writer.write_all(chunk) else {
            continue;
        };
        let path = path.clone();
        let error = Error::Io {
            src: src.to_path_buf(),
            dest: path.clone(),
            source: e,
        };
        // Assigning drops the dead writer, releasing the file before removal.
        *slot = Err((path.clone(), error));
        remove_partial(&path);
    }
}

/// Flush and fsync every surviving backup writer, converting late failures
/// into removed partial files. Returns the accumulated failures.
fn finish_backups(slots: Vec<BackupSlot>, src: &Path) -> Vec<(PathBuf, Error)> {
    slots
        .into_iter()
        .filter_map(|slot| match slot {
            Ok((path, writer)) => match finish_writer(writer) {
                Ok(()) => None,
                Err(e) => {
                    remove_partial(&path);
                    let error = Error::Io {
                        src: src.to_path_buf(),
                        dest: path.clone(),
                        source: e,
                    };
                    Some((path, error))
                }
            },
            Err(failure) => Some(failure),
        })
        .collect()
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
