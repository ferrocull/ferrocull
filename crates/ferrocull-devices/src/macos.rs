use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::Receiver,
    thread::{self, JoinHandle},
};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    Camera, DeviceEvent, MountError, MountOptions, ScanError, StorageDevice, UnmountError,
    UnmountOptions, WatchError,
};

/// Directory macOS mounts removable volumes under, one child per volume.
const VOLUMES_DIR: &str = "/Volumes";

#[expect(
    clippy::disallowed_methods,
    reason = "runs on the blocking pool through run_blocking, so std::fs cannot stall the runtime"
)]
pub(crate) fn scan_storage() -> Result<Vec<StorageDevice>, ScanError> {
    let entries = std::fs::read_dir(VOLUMES_DIR)?;

    let devices = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let mount_point = entry.path();

            if !mount_point.is_dir() {
                return None;
            }

            let info = disk_info(&mount_point)?;

            let removable = info.removable_media.or_else(|| {
                tracing::error!(
                    path = %mount_point.display(),
                    "diskutil plist missing RemovableMedia; skipping"
                );
                None
            })?;
            let ejectable = info.ejectable.or_else(|| {
                tracing::error!(
                    path = %mount_point.display(),
                    "diskutil plist missing Ejectable; skipping"
                );
                None
            })?;
            if !removable && !ejectable {
                return None;
            }

            let name = entry.file_name().to_string_lossy().into_owned();

            let device_path = info
                .device_node
                .map_or_else(|| mount_point.clone(), PathBuf::from);

            let (total_bytes, used_bytes) = crate::statvfs::disk_space(&mount_point).unzip();

            Some(StorageDevice {
                name,
                mount_point: Some(mount_point),
                device_path,
                total_bytes,
                used_bytes,
            })
        })
        .collect();

    Ok(devices)
}

/// Parsed subset of `diskutil info -plist` output. A volume counts as removable
/// media when it reports either `RemovableMedia` or `Ejectable`.
#[derive(serde::Deserialize)]
struct DiskInfo {
    #[serde(rename = "DeviceNode")]
    device_node: Option<String>,
    #[serde(rename = "RemovableMedia")]
    removable_media: Option<bool>,
    #[serde(rename = "Ejectable")]
    ejectable: Option<bool>,
}

fn disk_info(mount_point: &Path) -> Option<DiskInfo> {
    let output = Command::new("diskutil")
        .args(["info", "-plist"])
        .arg(mount_point)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    plist::from_bytes(&output.stdout).ok()
}

/// TODO: Implement `ImageCaptureCore` integration for PTP cameras.
/// For now, cameras that mount as mass storage are detected by `scan_storage()`.
#[must_use]
pub const fn scan_cameras() -> Vec<Camera> {
    Vec::new()
}

/// Mount point of a scanned volume.
fn mount_point(device: &StorageDevice) -> &Path {
    device
        .mount_point
        .as_deref()
        .expect("storage device has no mount point")
}

fn by_mount_point(devices: &[StorageDevice]) -> HashMap<&Path, &StorageDevice> {
    devices
        .iter()
        .map(|device| (mount_point(device), device))
        .collect()
}

/// Events implied by moving from the `known` volume set to `current`.
///
/// Removals read their device path from `known`, which is the only place it
/// survives once the volume is gone. A mount point held by a different device in
/// each set is a card swapped under a reused volume name, two cards carrying the
/// factory label `EOS_DIGITAL` for instance, and reports as the removal of the
/// old card followed by the insertion of the new one.
fn diff_devices(known: &[StorageDevice], current: &[StorageDevice]) -> Vec<DeviceEvent> {
    let known_by_mount = by_mount_point(known);
    let current_by_mount = by_mount_point(current);

    let appeared =
        current
            .iter()
            .flat_map(|device| match known_by_mount.get(mount_point(device)) {
                None => vec![DeviceEvent::Inserted(device.clone())],
                Some(previous) if previous.device_path != device.device_path => vec![
                    DeviceEvent::Removed {
                        device_path: previous.device_path.clone(),
                    },
                    DeviceEvent::Inserted(device.clone()),
                ],
                Some(_) => Vec::new(),
            });

    let vanished = known
        .iter()
        .filter(|device| !current_by_mount.contains_key(mount_point(device)))
        .map(|device| DeviceEvent::Removed {
            device_path: device.device_path.clone(),
        });

    appeared.chain(vanished).collect()
}

/// Whether any of `paths` is `/Volumes` itself or a direct child of it.
///
/// `FSEvents` is recursive in the kernel and `notify` applies
/// [`RecursiveMode::NonRecursive`] in userspace, so writes deep inside a mounted
/// card arrive here too. Only the volume directories themselves signal a mount
/// or an unmount, and the depth check keeps a card being read from triggering a
/// `diskutil` rescan per file. `/Volumes` itself counts because `notify`
/// delivers events on the watched root, and a dropped-event rescan notification
/// carries the root path rather than the volume that changed.
fn touches_volume_root(paths: &[PathBuf]) -> bool {
    let volumes = Path::new(VOLUMES_DIR);
    paths
        .iter()
        .any(|path| path == volumes || path.parent() == Some(volumes))
}

/// Rescans `/Volumes` on every relevant filesystem event and forwards the
/// difference from the previous scan, until the receiver hangs up.
fn forward_volume_events(
    events: &Receiver<Result<notify::Event, notify::Error>>,
    tx: &UnboundedSender<DeviceEvent>,
) {
    let mut known = scan_storage().unwrap_or_else(|error| {
        tracing::warn!(%error, "initial volume scan failed; starting with no known volumes");
        Vec::new()
    });

    while let Ok(received) = events.recv() {
        match received {
            Ok(event) if !touches_volume_root(&event.paths) => continue,
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(%error, "FSEvents watch error");
                continue;
            }
        }

        // Mounting a volume emits a burst of events; draining them costs one rescan.
        while events.try_recv().is_ok() {}

        let Ok(mut current) = scan_storage().inspect_err(|error| {
            tracing::warn!(%error, "volume scan failed; keeping the previous volume set");
        }) else {
            continue;
        };

        // macOS removes a volume's directory under /Volumes when it unmounts, so
        // a known volume whose directory survives a scan that lost it is still
        // mounted and its `diskutil` query failed.
        let still_mounted: Vec<StorageDevice> = {
            let scanned: HashSet<&Path> = current.iter().map(mount_point).collect();
            known
                .iter()
                .filter(|device| {
                    let path = mount_point(device);
                    !scanned.contains(path) && path.is_dir()
                })
                .cloned()
                .collect()
        };
        current.extend(still_mounted);

        let device_events = diff_devices(&known, &current);
        known = current;

        for device_event in device_events {
            if tx.send(device_event).is_err() {
                return;
            }
        }
    }
}

/// Watches `/Volumes` through `FSEvents` and reports volumes appearing and
/// disappearing by diffing successive [`scan_storage`] results.
pub(crate) fn watch(tx: UnboundedSender<DeviceEvent>) -> Result<JoinHandle<()>, WatchError> {
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(event_tx)
        .map_err(|e| WatchError::Backend(format!("failed to create the FSEvents watcher: {e}")))?;
    watcher
        .watch(Path::new(VOLUMES_DIR), RecursiveMode::NonRecursive)
        .map_err(|e| WatchError::Backend(format!("failed to watch {VOLUMES_DIR}: {e}")))?;

    Ok(thread::spawn(move || {
        forward_volume_events(&event_rx, &tx);
        // Delivery stops when the watcher drops, so it outlives the event loop.
        drop(watcher);
    }))
}

/// macOS auto-mounts devices -- manual mount is not supported.
pub(crate) fn mount(
    _device: &StorageDevice,
    _options: &MountOptions,
) -> Result<PathBuf, MountError> {
    Err(MountError::Failed(String::from(
        "macOS auto-mounts devices — manual mount not supported",
    )))
}

/// Unmount via `diskutil unmount [force]`.
pub(crate) fn unmount(
    device: &StorageDevice,
    options: &UnmountOptions,
) -> Result<(), UnmountError> {
    let mut cmd = Command::new("diskutil");
    cmd.arg("unmount");
    if options.force.unwrap_or(false) {
        cmd.arg("force");
    }
    cmd.arg(&device.device_path);

    let output = cmd
        .output()
        .map_err(|e| UnmountError::Failed(format!("failed to execute diskutil: {e}")))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stderr}{stdout}");

    Err(parse_unmount_error(&combined))
}

fn parse_unmount_error(output: &str) -> UnmountError {
    let lower = output.to_lowercase();
    if lower.contains("resource busy") || lower.contains("in use") {
        UnmountError::Busy
    } else if lower.contains("permission") || lower.contains("not permitted") {
        UnmountError::PermissionDenied
    } else if lower.contains("not a mount point")
        || lower.contains("could not find")
        || lower.contains("no such")
    {
        UnmountError::NotMounted
    } else {
        UnmountError::Failed(output.trim().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::{DeviceEvent, StorageDevice};

    fn device(name: &str) -> StorageDevice {
        StorageDevice {
            name: name.to_owned(),
            mount_point: Some(PathBuf::from(format!("/Volumes/{name}"))),
            device_path: PathBuf::from(format!("/dev/disk-{name}")),
            total_bytes: Some(64_000_000_000),
            used_bytes: Some(1_000_000),
        }
    }

    fn device_on(name: &str, device_path: &str) -> StorageDevice {
        StorageDevice {
            device_path: PathBuf::from(device_path),
            ..device(name)
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
    fn appearing_volume_is_reported_as_inserted() {
        let events = super::diff_devices(&[], &[device("EOS_DIGITAL")]);

        assert_eq!(inserted_names(&events), vec!["EOS_DIGITAL".to_owned()]);
        assert_eq!(removed_paths(&events), Vec::<PathBuf>::new());
    }

    #[test]
    fn vanished_volume_is_reported_as_removed() {
        let events = super::diff_devices(&[device("EOS_DIGITAL")], &[]);

        assert_eq!(
            removed_paths(&events),
            vec![PathBuf::from("/dev/disk-EOS_DIGITAL")]
        );
        assert_eq!(inserted_names(&events), Vec::<String>::new());
    }

    #[test]
    fn unchanged_volume_set_emits_nothing() {
        let events = super::diff_devices(&[device("EOS_DIGITAL")], &[device("EOS_DIGITAL")]);

        assert!(events.is_empty());
    }

    #[test]
    fn swapped_card_is_reported_as_both_removed_and_inserted() {
        let events = super::diff_devices(&[device("EOS_DIGITAL")], &[device("NIKON D850")]);

        assert_eq!(inserted_names(&events), vec!["NIKON D850".to_owned()]);
        assert_eq!(
            removed_paths(&events),
            vec![PathBuf::from("/dev/disk-EOS_DIGITAL")]
        );
    }

    #[test]
    fn card_swapped_under_a_reused_volume_name_is_removed_then_inserted() {
        let known = [device_on("EOS_DIGITAL", "/dev/disk2s1")];
        let current = [device_on("EOS_DIGITAL", "/dev/disk4s1")];

        let events = super::diff_devices(&known, &current);

        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            DeviceEvent::Removed { device_path } if device_path == Path::new("/dev/disk2s1")
        ));
        assert!(matches!(
            &events[1],
            DeviceEvent::Inserted(device) if device.device_path == Path::new("/dev/disk4s1")
        ));
    }
}
