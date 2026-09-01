//! Windows device detection through `sysinfo`.
//!
//! Removable storage is read from the system disk list; cameras are detected
//! only when they mount as mass storage.
//!
//! TODO: Proper WPD (Windows Portable Devices) integration for PTP/MTP cameras.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    thread::{self, JoinHandle},
    time::Duration,
};

use sysinfo::{DiskRefreshKind, Disks};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    Camera, DeviceEvent, MountError, MountOptions, StorageDevice, UnmountError, UnmountOptions,
};

/// Drive letter of a drive-root mount point, uppercased. Only a bare root
/// (`E:` or `E:\`) names a removable volume, so anything else yields `None`:
/// network shares and GUID volume paths carry no letter, and a folder-mounted
/// volume (`C:\mnt\card`) sits on another drive's filesystem, where reporting
/// it by its first character would misattribute it to that host drive.
fn drive_letter(mount_point: &Path) -> Option<char> {
    let raw = mount_point.to_string_lossy();
    let root = raw.strip_suffix('\\').unwrap_or(&raw);

    let mut chars = root.chars();
    let letter = chars.next()?;
    (letter.is_ascii_alphabetic() && chars.next() == Some(':') && chars.next().is_none())
        .then_some(letter.to_ascii_uppercase())
}

pub(crate) fn scan_storage() -> Vec<StorageDevice> {
    let disks = Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_storage());

    disks
        .iter()
        .filter(|disk| disk.is_removable())
        .filter_map(|disk| {
            let letter = drive_letter(disk.mount_point())?;

            let volume_name = disk.name().to_string_lossy();
            let name = if volume_name.is_empty() {
                format!("Removable ({letter}:)")
            } else {
                volume_name.into_owned()
            };

            // sysinfo leaves the sizes at zero when the size query fails, so a
            // zero total means unknown rather than empty.
            let (total_bytes, used_bytes) = match disk.total_space() {
                0 => (None, None),
                total => (
                    Some(total),
                    Some(total.saturating_sub(disk.available_space())),
                ),
            };

            Some(StorageDevice {
                name,
                mount_point: Some(PathBuf::from(format!("{letter}:\\"))),
                device_path: PathBuf::from(format!(r"\\.\{letter}:")),
                total_bytes,
                used_bytes,
            })
        })
        .collect()
}

/// TODO: Implement WPD (Windows Portable Devices) for PTP/MTP camera detection.
/// For now, cameras that mount as mass storage are detected by `scan_storage()`.
#[must_use]
pub const fn scan_cameras() -> Vec<Camera> {
    Vec::new()
}

/// Polls the removable drive list and reports the difference between successive
/// scans.
///
/// `sysinfo` drops a volume whose information query fails, so a transient
/// per-volume failure during one poll surfaces as a spurious `Removed` followed
/// by an `Inserted` on the next poll.
///
/// TODO: Replace polling with `RegisterDeviceNotificationW` for event-driven detection.
pub(crate) fn watch(tx: UnboundedSender<DeviceEvent>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut known_drives = scan_current_removable_drives();

        loop {
            thread::sleep(Duration::from_secs(2));

            let current_drives = scan_current_removable_drives();

            for (letter, device) in &current_drives {
                if !known_drives.contains_key(letter)
                    && tx.send(DeviceEvent::Inserted(device.clone())).is_err()
                {
                    return;
                }
            }

            for (letter, device) in &known_drives {
                if !current_drives.contains_key(letter)
                    && tx
                        .send(DeviceEvent::Removed {
                            device_path: device.device_path.clone(),
                        })
                        .is_err()
                {
                    return;
                }
            }

            known_drives = current_drives;
        }
    })
}

fn scan_current_removable_drives() -> HashMap<char, StorageDevice> {
    scan_storage()
        .into_iter()
        .map(|d| {
            let mount_point = d
                .mount_point
                .as_ref()
                .expect("storage device has no mount point");
            let letter = drive_letter(mount_point).expect("mount point has no drive letter");
            (letter, d)
        })
        .collect()
}

/// Windows auto-mounts devices -- manual mount is not supported.
pub(crate) fn mount(
    _device: &StorageDevice,
    _options: &MountOptions,
) -> Result<PathBuf, MountError> {
    Err(MountError::Failed(String::from(
        "Windows auto-mounts devices — manual mount not supported",
    )))
}

/// Unmount via `mountvol /P` (dismounts the volume and takes it offline).
pub(crate) fn unmount(
    device: &StorageDevice,
    _options: &UnmountOptions,
) -> Result<(), UnmountError> {
    let target = mountvol_target(&device.device_path);
    let output = Command::new("mountvol")
        .arg(&target)
        .arg("/P")
        .output()
        .map_err(|e| UnmountError::Failed(format!("failed to execute mountvol: {e}")))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(parse_unmount_error(&stderr))
}

/// Convert a `\\.\X:` device path to the `X:\` form `mountvol` expects.
/// `mountvol` rejects the Win32 device-namespace prefix and requires a trailing
/// backslash on drive-letter paths.
fn mountvol_target(device_path: &Path) -> PathBuf {
    let raw = device_path.to_string_lossy();
    if let Some(drive) = raw.strip_prefix(r"\\.\")
        && drive.len() == 2
        && drive.ends_with(':')
    {
        return PathBuf::from(format!(r"{drive}\"));
    }

    device_path.to_path_buf()
}

fn parse_unmount_error(stderr: &str) -> UnmountError {
    let lower = stderr.to_lowercase();
    if lower.contains("in use") || lower.contains("being used") {
        UnmountError::Busy
    } else if lower.contains("access is denied") || lower.contains("permission") {
        UnmountError::PermissionDenied
    } else if lower.contains("not found") || lower.contains("invalid") {
        UnmountError::NotMounted
    } else {
        UnmountError::Failed(stderr.trim().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn drive_letter_uppercases_and_accepts_trimmed_roots() {
        assert_eq!(super::drive_letter(Path::new(r"E:\")), Some('E'));
        assert_eq!(super::drive_letter(Path::new("e:")), Some('E'));
    }

    #[test]
    fn drive_letter_rejects_paths_without_one() {
        assert_eq!(super::drive_letter(Path::new(r"\\server\share")), None);
        assert_eq!(super::drive_letter(Path::new("")), None);
    }

    #[test]
    fn drive_letter_rejects_folder_mounts() {
        assert_eq!(super::drive_letter(Path::new(r"C:\mnt\card")), None);
    }
}
