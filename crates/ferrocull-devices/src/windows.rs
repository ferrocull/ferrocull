//! Windows device detection through the Win32 volume functions.
//!
//! Removable storage is read from the drive letters Windows has assigned, one
//! volume per letter, so a card whose filesystem Windows cannot read is listed
//! alongside the ones it can. Cameras are detected only when they mount as mass
//! storage.
//!
//! TODO: Proper WPD (Windows Portable Devices) integration for PTP/MTP cameras.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use tokio::sync::mpsc::UnboundedSender;

use crate::{
    Camera, DeviceEvent, Filesystem, MountError, MountOptions, ScanError, StorageDevice,
    UnmountError, UnmountOptions, diff, win32,
};

/// Delay between two polls of the removable drive list.
const SCAN_INTERVAL: Duration = Duration::from_secs(2);

/// Every removable volume Windows has a drive letter for, whether or not its
/// filesystem can be read.
///
/// Letters come back in alphabetical order, so the device list is ordered by
/// drive letter.
pub(crate) fn scan_storage() -> Result<Vec<StorageDevice>, ScanError> {
    Ok(win32::removable_letters()?
        .into_iter()
        .filter_map(|letter| device(letter, win32::read_volume(letter)))
        .collect())
}

/// The card in the drive at `letter`, or `None` when the slot is empty.
///
/// A removable drive keeps its letter while its slot is empty, and a multi-slot
/// card reader takes a letter per slot, so a letter on its own does not mean a
/// card is present.
///
/// Media whose filesystem Windows cannot read carries no mount point, since it
/// offers no usable filesystem path. A volume query that fails for any other
/// reason says nothing about the media, so the drive is dropped for this poll
/// and the next one picks it up again.
fn device(letter: char, volume: win32::Volume) -> Option<StorageDevice> {
    match volume {
        win32::Volume::Empty => None,
        win32::Volume::Failed { code } => {
            tracing::warn!(%letter, code, "volume query failed; skipping drive this poll");
            None
        }
        win32::Volume::Unreadable => Some(StorageDevice {
            name: unlabelled_name(letter),
            filesystem: Filesystem::Unreadable,
            device_path: device_path(letter),
            total_bytes: None,
            used_bytes: None,
        }),
        win32::Volume::Readable { label, disk_space } => {
            let (total_bytes, used_bytes) = disk_space.unzip();

            Some(StorageDevice {
                name: label.unwrap_or_else(|| unlabelled_name(letter)),
                filesystem: Filesystem::Mounted(PathBuf::from(format!("{letter}:\\"))),
                device_path: device_path(letter),
                total_bytes,
                used_bytes,
            })
        }
    }
}

/// Display name for a volume that carries no label of its own.
fn unlabelled_name(letter: char) -> String {
    format!("Removable ({letter}:)")
}

/// Device-namespace path of the drive at `letter`, the identity the watcher
/// keys devices by. The path is derived from the drive letter, so a card
/// swapped into the same letter keeps the key and reports nothing.
fn device_path(letter: char) -> PathBuf {
    PathBuf::from(format!(r"\\.\{letter}:"))
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
/// A scan that loses the whole drive-letter enumeration is reported as an error
/// and skipped, so the previous drive set stands. A card whose filesystem
/// Windows cannot read is reported with no mount point, so reformatting it in
/// the same slot arrives as a `Mounted`. A drive whose own volume query fails
/// is left out of the scan, so a transient failure on a working card reads as a
/// `Removed` followed by an `Inserted`.
///
/// The future stays pending for as long as the watch lasts, and dropping it
/// tears the watch down.
///
/// TODO: Replace polling with `RegisterDeviceNotificationW` for event-driven detection.
pub(crate) async fn watch(tx: UnboundedSender<DeviceEvent>) {
    let mut known = crate::run_blocking(scan_storage)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "initial drive scan failed; starting with no known drives");
            Vec::new()
        });

    // Drives present when the watch starts are replayed so one that mounted
    // before the first scan completes is still reported. The consumer treats
    // events as refresh triggers, so a replay for an already known drive is
    // harmless.
    for device in &known {
        if tx.send(DeviceEvent::Inserted(device.clone())).is_err() {
            return;
        }
    }

    loop {
        tokio::time::sleep(SCAN_INTERVAL).await;

        let Ok(current) = crate::run_blocking(scan_storage)
            .await
            .inspect_err(|error| {
                tracing::warn!(%error, "drive scan failed; keeping the previous drive set");
            })
        else {
            continue;
        };

        let device_events = events(&known, &current);
        known = current;

        for device_event in device_events {
            if tx.send(device_event).is_err() {
                return;
            }
        }
    }
}

/// Events implied by moving from the `known` device set to `current`.
///
/// Removals read their device path from `known`, which is the only place it
/// survives once the device is gone.
fn events(known: &[StorageDevice], current: &[StorageDevice]) -> Vec<DeviceEvent> {
    let known_by_path = by_device_path(known);
    let current_by_path = by_device_path(current);

    let appeared = current.iter().filter_map(|device| {
        diff::change(
            known_by_path.get(device.device_path.as_path()).copied(),
            device,
        )
    });

    let vanished = known
        .iter()
        .filter(|device| !current_by_path.contains_key(device.device_path.as_path()))
        .map(|device| DeviceEvent::Removed {
            device_path: device.device_path.clone(),
        });

    appeared.chain(vanished).collect()
}

/// The devices of a set, keyed by the device path they are identified by.
fn by_device_path(devices: &[StorageDevice]) -> HashMap<&Path, &StorageDevice> {
    devices
        .iter()
        .map(|device| (device.device_path.as_path(), device))
        .collect()
}

/// Windows auto-mounts devices -- manual mount is not supported.
pub(crate) fn mount(
    _device: &StorageDevice,
    _options: &MountOptions,
) -> Result<PathBuf, MountError> {
    Err(MountError::Failed(String::from(
        "Windows auto-mounts devices, manual mount not supported",
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
    use std::path::{Path, PathBuf};

    use crate::{DeviceEvent, Filesystem, StorageDevice, win32::Volume};

    fn mounted(name: &str) -> StorageDevice {
        StorageDevice {
            name: name.to_owned(),
            filesystem: Filesystem::Mounted(PathBuf::from(format!("/Volumes/{name}"))),
            device_path: PathBuf::from(format!("/dev/disk-{name}")),
            total_bytes: Some(64_000_000_000),
            used_bytes: Some(1_000_000),
        }
    }

    fn unmounted(name: &str) -> StorageDevice {
        StorageDevice {
            filesystem: Filesystem::Unmounted,
            total_bytes: None,
            used_bytes: None,
            ..mounted(name)
        }
    }

    fn device_on(name: &str, device_path: &str) -> StorageDevice {
        StorageDevice {
            device_path: PathBuf::from(device_path),
            ..mounted(name)
        }
    }

    fn unreadable(name: &str) -> StorageDevice {
        StorageDevice {
            filesystem: Filesystem::Unreadable,
            ..unmounted(name)
        }
    }

    fn inserted_names(events: &[DeviceEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                DeviceEvent::Inserted(device) => Some(device.name.clone()),
                _ => None,
            })
            .collect()
    }

    fn removed_paths(events: &[DeviceEvent]) -> Vec<PathBuf> {
        events
            .iter()
            .filter_map(|event| match event {
                DeviceEvent::Removed { device_path } => Some(device_path.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn labelled_volume_keeps_its_label_and_sizes() {
        let volume = Volume::Readable {
            label: Some(String::from("EOS_DIGITAL")),
            disk_space: Some((64_000_000_000, 1_000_000)),
        };

        let device = super::device('E', volume).expect("readable volume dropped");

        assert_eq!(device.name, "EOS_DIGITAL");
        assert_eq!(
            device.filesystem,
            Filesystem::Mounted(PathBuf::from(r"E:\"))
        );
        assert_eq!(device.device_path, PathBuf::from(r"\\.\E:"));
        assert_eq!(device.total_bytes, Some(64_000_000_000));
        assert_eq!(device.used_bytes, Some(1_000_000));
    }

    #[test]
    fn unlabelled_volume_is_named_after_its_drive_letter() {
        let volume = Volume::Readable {
            label: None,
            disk_space: Some((64_000_000_000, 1_000_000)),
        };

        let device = super::device('E', volume).expect("readable volume dropped");

        assert_eq!(device.name, "Removable (E:)");
    }

    #[test]
    fn volume_without_a_size_query_reads_as_unknown_sizes() {
        let volume = Volume::Readable {
            label: Some(String::from("EOS_DIGITAL")),
            disk_space: None,
        };

        let device = super::device('E', volume).expect("readable volume dropped");

        assert_eq!(device.total_bytes, None);
        assert_eq!(device.used_bytes, None);
        assert_eq!(
            device.filesystem,
            Filesystem::Mounted(PathBuf::from(r"E:\"))
        );
    }

    #[test]
    fn media_windows_cannot_read_is_surfaced_as_unreadable() {
        let device = super::device('E', Volume::Unreadable).expect("unreadable media dropped");

        assert_eq!(device.name, "Removable (E:)");
        assert_eq!(device.filesystem, Filesystem::Unreadable);
        assert_eq!(device.device_path, PathBuf::from(r"\\.\E:"));
        assert_eq!(device.total_bytes, None);
        assert_eq!(device.used_bytes, None);
    }

    #[test]
    fn empty_slot_yields_no_device() {
        assert!(super::device('E', Volume::Empty).is_none());
    }

    #[test]
    fn failed_volume_query_yields_no_device() {
        assert!(
            super::device(
                'E',
                Volume::Failed {
                    code: -2_147_024_891
                }
            )
            .is_none()
        );
    }

    #[test]
    fn appearing_volume_is_reported_as_inserted() {
        let events = super::events(&[], &[mounted("EOS_DIGITAL")]);

        assert_eq!(inserted_names(&events), vec!["EOS_DIGITAL".to_owned()]);
        assert_eq!(removed_paths(&events), Vec::<PathBuf>::new());
    }

    #[test]
    fn vanished_volume_is_reported_as_removed() {
        let events = super::events(&[mounted("EOS_DIGITAL")], &[]);

        assert_eq!(
            removed_paths(&events),
            vec![PathBuf::from("/dev/disk-EOS_DIGITAL")]
        );
        assert_eq!(inserted_names(&events), Vec::<String>::new());
    }

    #[test]
    fn unchanged_volume_set_emits_nothing() {
        let events = super::events(&[mounted("EOS_DIGITAL")], &[mounted("EOS_DIGITAL")]);

        assert!(events.is_empty());
    }

    #[test]
    fn swapped_card_is_reported_as_both_removed_and_inserted() {
        let events = super::events(&[mounted("EOS_DIGITAL")], &[mounted("NIKON D850")]);

        assert_eq!(inserted_names(&events), vec!["NIKON D850".to_owned()]);
        assert_eq!(
            removed_paths(&events),
            vec![PathBuf::from("/dev/disk-EOS_DIGITAL")]
        );
    }

    #[test]
    fn card_swapped_under_a_reused_volume_name_is_removed_and_inserted() {
        let known = [device_on("EOS_DIGITAL", "/dev/disk2s1")];
        let current = [device_on("EOS_DIGITAL", "/dev/disk4s1")];

        let events = super::events(&known, &current);

        assert_eq!(events.len(), 2);
        assert_eq!(removed_paths(&events), vec![PathBuf::from("/dev/disk2s1")]);
        assert_eq!(inserted_names(&events), vec!["EOS_DIGITAL".to_owned()]);
    }

    #[test]
    fn volume_losing_its_mount_point_is_reported_as_unmounted() {
        let events = super::events(&[mounted("EOS_DIGITAL")], &[unmounted("EOS_DIGITAL")]);

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            DeviceEvent::Unmounted { device_path }
                if device_path == Path::new("/dev/disk-EOS_DIGITAL")
        ));
    }

    #[test]
    fn volume_gaining_a_mount_point_is_reported_as_mounted() {
        let events = super::events(&[unmounted("EOS_DIGITAL")], &[mounted("EOS_DIGITAL")]);

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            DeviceEvent::Mounted {
                device_path,
                mount_point,
                total_bytes,
                used_bytes,
            } if device_path == Path::new("/dev/disk-EOS_DIGITAL")
                && mount_point == Path::new("/Volumes/EOS_DIGITAL")
                && *total_bytes == Some(64_000_000_000)
                && *used_bytes == Some(1_000_000)
        ));
    }

    #[test]
    fn volume_remounted_elsewhere_is_reported_as_mounted_at_the_new_path() {
        let known = [mounted("EOS_DIGITAL")];
        let current = [StorageDevice {
            filesystem: Filesystem::Mounted(PathBuf::from("/Volumes/EOS_DIGITAL 1")),
            ..mounted("EOS_DIGITAL")
        }];

        let events = super::events(&known, &current);

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            DeviceEvent::Mounted {
                device_path,
                mount_point,
                total_bytes,
                used_bytes,
            } if device_path == Path::new("/dev/disk-EOS_DIGITAL")
                && mount_point == Path::new("/Volumes/EOS_DIGITAL 1")
                && *total_bytes == Some(64_000_000_000)
                && *used_bytes == Some(1_000_000)
        ));
    }

    #[test]
    fn reformatted_card_in_the_same_slot_is_reported_as_mounted() {
        let events = super::events(&[unreadable("EOS_DIGITAL")], &[mounted("EOS_DIGITAL")]);

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            DeviceEvent::Mounted {
                device_path,
                mount_point,
                total_bytes,
                used_bytes,
            } if device_path == Path::new("/dev/disk-EOS_DIGITAL")
                && mount_point == Path::new("/Volumes/EOS_DIGITAL")
                && *total_bytes == Some(64_000_000_000)
                && *used_bytes == Some(1_000_000)
        ));
    }

    #[test]
    fn disk_appearing_unmounted_is_reported_as_inserted() {
        let events = super::events(&[], &[unmounted("NIKON_D850")]);

        assert_eq!(inserted_names(&events), vec!["NIKON_D850".to_owned()]);
    }
}
