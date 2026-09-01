use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
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

/// Consecutive rescans a device may go missing from while its mount point or
/// `/dev` node survives before it counts as gone.
const MISSED_SCAN_LIMIT: u8 = 3;

/// Removable volumes mounted under [`VOLUMES_DIR`], plus the removable disks
/// that are attached without a mount point.
pub(crate) fn scan_storage() -> Result<Vec<StorageDevice>, ScanError> {
    let mut devices = mounted_volumes()?;
    let mounted: HashSet<PathBuf> = devices
        .iter()
        .map(|device| device.device_path.clone())
        .collect();

    match unmounted_disks() {
        // A volume can unmount between the /Volumes walk and the diskutil
        // listing: the walk saw it mounted and the listing reports it as an
        // unmounted candidate, so without the filter the same device would be
        // listed twice. The stale mounted entry wins; the next rescan corrects
        // it.
        Ok(unmounted) => devices.extend(
            unmounted
                .into_iter()
                .filter(|device| !mounted.contains(&device.device_path)),
        ),
        Err(error) => {
            tracing::warn!(
                %error,
                "cannot list unmounted disks; the scan reports mounted volumes only"
            );
        }
    }

    Ok(devices)
}

#[expect(
    clippy::disallowed_methods,
    reason = "runs on the blocking pool through run_blocking, so std::fs cannot stall the runtime"
)]
fn mounted_volumes() -> Result<Vec<StorageDevice>, ScanError> {
    let entries = std::fs::read_dir(VOLUMES_DIR)?;

    let devices = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let mount_point = entry.path();

            if !mount_point.is_dir() {
                return None;
            }

            let info = disk_info(&mount_point)?;
            if !is_removable(&info, &mount_point.to_string_lossy())? {
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

/// Removable disks carrying a filesystem that is attached but not mounted, so
/// the user can pick one and mount it.
fn unmounted_disks() -> Result<Vec<StorageDevice>, ScanError> {
    let listing = disk_listing()?;

    Ok(unmounted_candidates(&listing)
        .into_iter()
        .filter_map(unmounted_device)
        .collect())
}

/// Entries of a listing that carry a filesystem with no mount point: the
/// unmounted partitions of a partitioned disk, and an unpartitioned disk
/// formatted end to end. A partition without a volume name holds no filesystem
/// `diskutil` recognizes, which rules out the Apple internal partitions. The
/// EFI System Partition of a GPT disk does carry one, so it is ruled out by its
/// `Content` instead; filtering on the partition type rather than on the name
/// keeps a card the user labeled `EFI` in the list.
fn unmounted_candidates(listing: &Listing) -> Vec<&ListedDisk> {
    listing
        .disks
        .iter()
        .flat_map(|disk| {
            disk.partitions
                .as_ref()
                .map_or_else(|| vec![disk], |partitions| partitions.iter().collect())
        })
        .filter(|entry| {
            entry.volume_name.is_some()
                && entry.mount_point.is_none()
                && entry.content.as_deref() != Some("EFI")
        })
        .collect()
}

/// The candidate as a storage device, or `None` when it is not removable media
/// or `diskutil` cannot describe it.
fn unmounted_device(candidate: &ListedDisk) -> Option<StorageDevice> {
    let info = disk_info(&candidate.device_identifier)?;
    if !is_removable(&info, &candidate.device_identifier)? {
        return None;
    }

    let name = candidate
        .volume_name
        .clone()
        .expect("candidate without a volume name");

    let device_path = info.device_node.map_or_else(
        || PathBuf::from(format!("/dev/{}", candidate.device_identifier)),
        PathBuf::from,
    );

    Some(StorageDevice {
        name,
        mount_point: None,
        device_path,
        total_bytes: candidate.size,
        used_bytes: None,
    })
}

/// Parsed subset of `diskutil list -plist` output.
#[derive(serde::Deserialize)]
struct Listing {
    #[serde(rename = "AllDisksAndPartitions")]
    disks: Vec<ListedDisk>,
}

/// A whole disk or one of its partitions. Both shapes carry the same keys; only
/// a whole disk carries `Partitions`, and a disk formatted without a partition
/// table carries the volume keys itself. `APFSVolumes` stays unparsed: APFS
/// never holds camera media, and skipping it keeps the internal system volumes
/// of an APFS container out of the scan.
#[derive(serde::Deserialize)]
struct ListedDisk {
    #[serde(rename = "DeviceIdentifier")]
    device_identifier: String,
    #[serde(rename = "Content")]
    content: Option<String>,
    #[serde(rename = "VolumeName")]
    volume_name: Option<String>,
    #[serde(rename = "MountPoint")]
    mount_point: Option<String>,
    #[serde(rename = "Size")]
    size: Option<u64>,
    #[serde(rename = "Partitions")]
    partitions: Option<Vec<Self>>,
}

fn disk_listing() -> Result<Listing, ScanError> {
    let output = Command::new("diskutil")
        .args(["list", "-plist"])
        .output()
        .map_err(|e| ScanError::Backend(format!("failed to execute diskutil list: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ScanError::Backend(format!(
            "diskutil list failed: {}",
            stderr.trim()
        )));
    }

    plist::from_bytes(&output.stdout)
        .map_err(|e| ScanError::Backend(format!("failed to parse diskutil list output: {e}")))
}

/// Parsed subset of `diskutil info -plist` output. A volume counts as removable
/// media when it reports either `RemovableMedia` or `Ejectable`.
#[derive(serde::Deserialize)]
struct DiskInfo {
    #[serde(rename = "DeviceNode")]
    device_node: Option<String>,
    #[serde(rename = "MountPoint")]
    mount_point: Option<String>,
    #[serde(rename = "RemovableMedia")]
    removable_media: Option<bool>,
    #[serde(rename = "Ejectable")]
    ejectable: Option<bool>,
}

impl DiskInfo {
    /// Where the volume is mounted. `diskutil` reports an unmounted volume with
    /// an empty mount point.
    fn mount_point(&self) -> Option<&str> {
        self.mount_point
            .as_deref()
            .filter(|mount_point| !mount_point.is_empty())
    }
}

/// Whether the disk is removable media, or `None` when `diskutil` omits a key
/// the decision rests on. `disk` names the disk in the log line.
fn is_removable(info: &DiskInfo, disk: &str) -> Option<bool> {
    let Some(removable) = info.removable_media else {
        tracing::error!(disk, "diskutil plist missing RemovableMedia; skipping");
        return None;
    };
    let Some(ejectable) = info.ejectable else {
        tracing::error!(disk, "diskutil plist missing Ejectable; skipping");
        return None;
    };

    Some(removable || ejectable)
}

/// Queries `diskutil` about a disk named by mount point, `/dev` node, or device
/// identifier, all of which it accepts.
fn disk_info(disk: impl AsRef<OsStr>) -> Option<DiskInfo> {
    let output = Command::new("diskutil")
        .args(["info", "-plist"])
        .arg(disk)
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

/// Path whose existence means the device is still attached: its mount point
/// while mounted, its `/dev` node otherwise. Both disappear when the disk is
/// detached.
fn existence_probe(device: &StorageDevice) -> &Path {
    device.mount_point.as_deref().unwrap_or(&device.device_path)
}

/// Devices from `known` to keep in the current scan, with `misses` advanced to
/// the number of consecutive rescans each carried device has been absent from.
/// `probe_exists` reports whether a device's [`existence_probe`] path is still
/// there.
///
/// macOS removes a volume's directory under `/Volumes` when it unmounts and its
/// `/dev` node when the disk is detached, so a probe path that outlives its scan
/// entry means one of two things: a `diskutil` query that failed for this
/// rescan, which recovers within [`MISSED_SCAN_LIMIT`] rescans, or a leftover
/// directory from an abnormal unmount, which never recovers. The cap bounds the
/// second case so the device is eventually reported removed.
fn carried_over(
    known: &[StorageDevice],
    current: &[StorageDevice],
    misses: &mut HashMap<PathBuf, u8>,
    probe_exists: impl Fn(&Path) -> bool,
) -> Vec<StorageDevice> {
    let scanned: HashSet<&Path> = current
        .iter()
        .map(|device| device.device_path.as_path())
        .collect();

    let carried = known
        .iter()
        .filter(|device| {
            let device_path = device.device_path.as_path();
            if scanned.contains(device_path) || !probe_exists(existence_probe(device)) {
                return false;
            }

            let count = misses.entry(device_path.to_path_buf()).or_default();
            *count += 1;
            *count <= MISSED_SCAN_LIMIT
        })
        .cloned()
        .collect();

    let tracked: HashSet<&Path> = known
        .iter()
        .map(|device| device.device_path.as_path())
        .collect();
    misses.retain(|device_path, _| {
        tracked.contains(device_path.as_path()) && !scanned.contains(device_path.as_path())
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

        current.extend(carried_over(&known, &current, &mut misses, Path::exists));

        let device_events = diff::events(&known, &current);
        known = current;

        for device_event in device_events {
            if tx.send(device_event).is_err() {
                return;
            }
        }
    }
}

/// Watches `/Volumes` through `FSEvents` and reports devices appearing and
/// disappearing by diffing successive [`scan_storage`] results.
///
/// `FSEvents` only observes `/Volumes`, while a scan also reports attached disks
/// that carry no mount point. Mounting and unmounting both create or remove a
/// volume directory, so those reach the watcher. Two changes to a disk that is
/// never mounted do not: attaching one that macOS declines to auto-mount, and
/// yanking one that is already unmounted. Both are rare, and the list catches up
/// on the next event or a manual refresh.
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

/// Mounts through `diskutil mount`, which picks the mount point and the
/// filesystem driver itself, so `options` carries nothing macOS can act on.
pub(crate) fn mount(
    device: &StorageDevice,
    _options: &MountOptions,
) -> Result<PathBuf, MountError> {
    let output = Command::new("diskutil")
        .arg("mount")
        .arg(&device.device_path)
        .output()
        .map_err(|e| MountError::Failed(format!("failed to execute diskutil: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(parse_mount_error(&format!("{stderr}{stdout}")));
    }

    // `diskutil mount` names the volume it mounted, not where it landed, so the
    // mount point comes from a follow-up query. A volume that mounts without one
    // being readable is reported as a failure; the next rescan reconciles the
    // list either way.
    disk_info(&device.device_path)
        .and_then(|info| info.mount_point().map(PathBuf::from))
        .ok_or_else(|| {
            MountError::Failed(format!(
                "mounted {} but could not read its mount point",
                device.device_path.display()
            ))
        })
}

fn parse_mount_error(output: &str) -> MountError {
    let lower = output.to_lowercase();
    if lower.contains("permission") || lower.contains("not permitted") {
        MountError::PermissionDenied
    } else {
        MountError::Failed(output.trim().to_owned())
    }
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

    fn unmounted(name: &str) -> StorageDevice {
        StorageDevice {
            mount_point: None,
            total_bytes: None,
            used_bytes: None,
            ..device(name)
        }
    }

    /// Stands in for a probe path that outlives its device.
    fn path_survives(_path: &Path) -> bool {
        true
    }

    /// Hand-written `diskutil list -plist` output covering the shapes the
    /// parser meets: an internal disk whose partitions carry no volume name, an
    /// APFS container with keys the parser skips, a mounted and an unmounted
    /// partition, an external GPT disk whose EFI System Partition carries a
    /// volume name, and an unpartitioned disk formatted end to end.
    const DISKUTIL_LISTING: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>AllDisksAndPartitions</key>
    <array>
        <dict>
            <key>Content</key><string>GUID_partition_scheme</string>
            <key>DeviceIdentifier</key><string>disk0</string>
            <key>OSInternal</key><false/>
            <key>Partitions</key>
            <array>
                <dict>
                    <key>Content</key><string>Apple_APFS_ISC</string>
                    <key>DeviceIdentifier</key><string>disk0s1</string>
                    <key>Size</key><integer>524288000</integer>
                </dict>
                <dict>
                    <key>Content</key><string>Apple_APFS</string>
                    <key>DeviceIdentifier</key><string>disk0s2</string>
                    <key>Size</key><integer>494384795648</integer>
                </dict>
            </array>
            <key>Size</key><integer>500277792768</integer>
        </dict>
        <dict>
            <key>APFSPhysicalStores</key>
            <array>
                <dict><key>DeviceIdentifier</key><string>disk0s2</string></dict>
            </array>
            <key>APFSVolumes</key>
            <array>
                <dict>
                    <key>DeviceIdentifier</key><string>disk3s3</string>
                    <key>Size</key><integer>1342177280</integer>
                    <key>VolumeName</key><string>Recovery</string>
                </dict>
            </array>
            <key>Content</key><string>Apple_APFS</string>
            <key>DeviceIdentifier</key><string>disk3</string>
            <key>OSInternal</key><true/>
            <key>Partitions</key><array/>
            <key>Size</key><integer>494384795648</integer>
        </dict>
        <dict>
            <key>Content</key><string>FDisk_partition_scheme</string>
            <key>DeviceIdentifier</key><string>disk4</string>
            <key>OSInternal</key><false/>
            <key>Partitions</key>
            <array>
                <dict>
                    <key>Content</key><string>DOS_FAT_32</string>
                    <key>DeviceIdentifier</key><string>disk4s1</string>
                    <key>MountPoint</key><string>/Volumes/EOS_DIGITAL</string>
                    <key>Size</key><integer>67108352</integer>
                    <key>VolumeName</key><string>EOS_DIGITAL</string>
                </dict>
            </array>
            <key>Size</key><integer>67108864</integer>
        </dict>
        <dict>
            <key>Content</key><string>GUID_partition_scheme</string>
            <key>DeviceIdentifier</key><string>disk5</string>
            <key>OSInternal</key><false/>
            <key>Partitions</key>
            <array>
                <dict>
                    <key>Content</key><string>Microsoft Basic Data</string>
                    <key>DeviceIdentifier</key><string>disk5s1</string>
                    <key>Size</key><integer>65011712</integer>
                    <key>VolumeName</key><string>NIKON_D850</string>
                </dict>
            </array>
            <key>Size</key><integer>67108864</integer>
        </dict>
        <dict>
            <key>Content</key><string>GUID_partition_scheme</string>
            <key>DeviceIdentifier</key><string>disk6</string>
            <key>OSInternal</key><false/>
            <key>Partitions</key>
            <array>
                <dict>
                    <key>Content</key><string>EFI</string>
                    <key>DeviceIdentifier</key><string>disk6s1</string>
                    <key>Size</key><integer>209715200</integer>
                    <key>VolumeName</key><string>EFI</string>
                </dict>
                <dict>
                    <key>Content</key><string>Microsoft Basic Data</string>
                    <key>DeviceIdentifier</key><string>disk6s2</string>
                    <key>Size</key><integer>63963136</integer>
                    <key>VolumeName</key><string>LUMIX</string>
                </dict>
            </array>
            <key>Size</key><integer>67108864</integer>
        </dict>
        <dict>
            <key>Content</key><string></string>
            <key>DeviceIdentifier</key><string>disk7</string>
            <key>OSInternal</key><false/>
            <key>Size</key><integer>33554432</integer>
            <key>VolumeName</key><string>SUPERFLOPPY</string>
        </dict>
    </array>
</dict>
</plist>
"#;

    #[test]
    fn candidates_are_the_unmounted_volumes_of_a_listing() {
        let listing: super::Listing = plist::from_bytes(DISKUTIL_LISTING.as_bytes())
            .expect("diskutil listing fixture does not parse");

        let identifiers: Vec<&str> = super::unmounted_candidates(&listing)
            .iter()
            .map(|disk| disk.device_identifier.as_str())
            .collect();

        assert_eq!(identifiers, ["disk5s1", "disk6s2", "disk7"]);
    }

    #[test]
    fn volume_missing_from_a_scan_is_carried_over_up_to_the_limit() {
        let known = [device("EOS_DIGITAL")];
        let mut misses = HashMap::new();

        for miss in 1..=super::MISSED_SCAN_LIMIT {
            let carried = super::carried_over(&known, &[], &mut misses, path_survives);
            assert_eq!(carried.len(), 1, "dropped on missed scan {miss}");
        }

        let carried = super::carried_over(&known, &[], &mut misses, path_survives);
        assert!(carried.is_empty());
    }

    #[test]
    fn volume_back_in_a_scan_starts_its_missed_scan_count_over() {
        let known = [device("EOS_DIGITAL")];
        let mut misses = HashMap::new();

        for _ in 1..=super::MISSED_SCAN_LIMIT {
            super::carried_over(&known, &[], &mut misses, path_survives);
        }
        super::carried_over(&known, &known, &mut misses, path_survives);

        for miss in 1..=super::MISSED_SCAN_LIMIT {
            let carried = super::carried_over(&known, &[], &mut misses, path_survives);
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

    #[test]
    fn unmounted_disk_is_carried_over_while_its_device_node_survives() {
        let known = [unmounted("NIKON_D850")];
        let device_node = Path::new("/dev/disk-NIKON_D850");
        let mut misses = HashMap::new();

        let attached = super::carried_over(&known, &[], &mut misses, |path| path == device_node);
        assert_eq!(attached.len(), 1);

        let detached = super::carried_over(&known, &[], &mut misses, |_| false);
        assert!(detached.is_empty());
    }
}
