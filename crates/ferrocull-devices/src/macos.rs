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
    UnmountOptions, WatchError, diff,
};

/// Directory macOS mounts removable volumes under, one child per volume.
const VOLUMES_DIR: &str = "/Volumes";

/// Consecutive rescans a volume may go missing from while its `/Volumes`
/// directory survives before it counts as gone.
const MISSED_SCAN_LIMIT: u8 = 3;

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

/// Volumes from `known` to keep in the current scan, with `misses` advanced to
/// the number of consecutive rescans each carried mount point has been absent
/// from. `mount_exists` reports whether a mount point is still a directory.
///
/// macOS removes a volume's directory under `/Volumes` when it unmounts, so a
/// directory that outlives its scan entry means one of two things: a `diskutil`
/// query that failed for this rescan, which recovers within
/// [`MISSED_SCAN_LIMIT`] rescans, or a leftover directory from an abnormal
/// unmount, which never recovers. The cap bounds the second case so the volume
/// is eventually reported removed.
fn carried_over(
    known: &[StorageDevice],
    current: &[StorageDevice],
    misses: &mut HashMap<PathBuf, u8>,
    mount_exists: impl Fn(&Path) -> bool,
) -> Vec<StorageDevice> {
    let scanned: HashSet<&Path> = current.iter().map(diff::mount_point).collect();

    let carried = known
        .iter()
        .filter(|device| {
            let path = diff::mount_point(device);
            if scanned.contains(path) || !mount_exists(path) {
                return false;
            }

            let count = misses.entry(path.to_path_buf()).or_default();
            *count += 1;
            *count <= MISSED_SCAN_LIMIT
        })
        .cloned()
        .collect();

    let tracked: HashSet<&Path> = known.iter().map(diff::mount_point).collect();
    misses.retain(|mount_point, _| {
        tracked.contains(mount_point.as_path()) && !scanned.contains(mount_point.as_path())
    });

    carried
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
    let mut misses: HashMap<PathBuf, u8> = HashMap::new();

    // Volumes present when the watch starts are replayed so one that mounted
    // before the baseline scan completes is still reported. The consumer treats
    // events as refresh triggers, so a replay for an already known volume is
    // harmless.
    for device in &known {
        if tx.send(DeviceEvent::Inserted(device.clone())).is_err() {
            return;
        }
    }

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

        current.extend(carried_over(&known, &current, &mut misses, Path::is_dir));

        let device_events = diff::events(&known, &current);
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
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
    };

    use crate::StorageDevice;

    fn device(name: &str) -> StorageDevice {
        StorageDevice {
            name: name.to_owned(),
            mount_point: Some(PathBuf::from(format!("/Volumes/{name}"))),
            device_path: PathBuf::from(format!("/dev/disk-{name}")),
            total_bytes: Some(64_000_000_000),
            used_bytes: Some(1_000_000),
        }
    }

    /// Stands in for a mount point directory that outlives its volume.
    fn directory_survives(_path: &Path) -> bool {
        true
    }

    #[test]
    fn volume_missing_from_a_scan_is_carried_over_up_to_the_limit() {
        let known = [device("EOS_DIGITAL")];
        let mut misses = HashMap::new();

        for miss in 1..=super::MISSED_SCAN_LIMIT {
            let carried = super::carried_over(&known, &[], &mut misses, directory_survives);
            assert_eq!(carried.len(), 1, "dropped on missed scan {miss}");
        }

        let carried = super::carried_over(&known, &[], &mut misses, directory_survives);
        assert!(carried.is_empty());
    }

    #[test]
    fn volume_back_in_a_scan_starts_its_missed_scan_count_over() {
        let known = [device("EOS_DIGITAL")];
        let mut misses = HashMap::new();

        for _ in 1..=super::MISSED_SCAN_LIMIT {
            super::carried_over(&known, &[], &mut misses, directory_survives);
        }
        super::carried_over(&known, &known, &mut misses, directory_survives);

        for miss in 1..=super::MISSED_SCAN_LIMIT {
            let carried = super::carried_over(&known, &[], &mut misses, directory_survives);
            assert_eq!(carried.len(), 1, "dropped on missed scan {miss}");
        }
    }

    #[test]
    fn volume_whose_directory_is_gone_is_not_carried_over() {
        let known = [device("EOS_DIGITAL")];
        let mut misses = HashMap::new();

        let carried = super::carried_over(&known, &[], &mut misses, |_| false);

        assert!(carried.is_empty());
        assert!(misses.is_empty());
    }
}
