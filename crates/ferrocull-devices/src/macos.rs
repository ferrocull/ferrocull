//! macOS device detection through the `DiskArbitration` framework.
//!
//! `DiskArbitration` answers for every medium `diskarbitrationd` knows about, so
//! a card is listed whether it is mounted, waiting to be mounted, or carrying a
//! filesystem macOS cannot read. Cameras are detected only when they mount as
//! mass storage.
//!
//! TODO: Implement `ImageCaptureCore` integration for PTP cameras.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};

use objc2_disk_arbitration::{
    kDAReturnBusy, kDAReturnNotMounted, kDAReturnNotPermitted, kDAReturnNotPrivileged,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    Camera, DeviceEvent, Filesystem, MountError, MountOptions, ScanError, StorageDevice,
    UnmountError, UnmountOptions, WatchError, diff, diskarb,
};

/// Directory holding one node per BSD disk and partition.
const DEV_DIR: &str = "/dev";

/// `DiskArbitration`'s status for a request against a disk something else is
/// holding, and the one it reports for a mount of a disk already mounted.
const BUSY: i32 = kDAReturnBusy;

/// `DiskArbitration`'s statuses for a request the caller may not make.
const NOT_PERMITTED: i32 = kDAReturnNotPermitted;
const NOT_PRIVILEGED: i32 = kDAReturnNotPrivileged;

/// `DiskArbitration`'s status for an unmount of a disk that is not mounted.
const NOT_MOUNTED: i32 = kDAReturnNotMounted;

/// `DiskArbitration` reports a GPT partition's type as its GUID, and every
/// volume of an APFS container carries this one.
const APFS_VOLUME_TYPE: &str = "41504653-0000-11AA-AA11-00306543ECAC";

/// Every removable card `DiskArbitration` describes, mounted or not.
///
/// The disks to describe come from the `/dev` nodes rather than from `IOKit`:
/// every medium `DiskArbitration` answers for publishes one, and reading a
/// directory needs no second framework.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the blocking pool through run_blocking, so std::fs cannot stall the runtime"
)]
pub(crate) fn scan_storage() -> Result<Vec<StorageDevice>, ScanError> {
    let entries = std::fs::read_dir(DEV_DIR)?;

    let bsd_names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| is_disk_node(name))
        .collect();

    let descriptions =
        diskarb::describe(&bsd_names).map_err(|error| ScanError::Backend(error.to_string()))?;

    Ok(descriptions.iter().filter_map(device).collect())
}

/// Whether `name` is a BSD disk node: `disk` followed by the unit number and
/// one slice number per level of nesting, as `disk4s1` and `disk3s1s1` are.
/// The `rdisk` character nodes name the same media, so they are left out.
fn is_disk_node(name: &str) -> bool {
    name.strip_prefix("disk").is_some_and(|numbers| {
        numbers
            .split('s')
            .all(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
    })
}

/// The removable card `description` describes, or `None` when it is not one.
fn device(description: &diskarb::Description) -> Option<StorageDevice> {
    // A whole disk carrying a partition table is not a volume, while a card
    // formatted end to end is a leaf and belongs in the list.
    if description.media_leaf != Some(true) {
        return None;
    }

    // The IOKit properties `diskutil` reports as `RemovableMedia` and
    // `Ejectable`.
    if description.media_removable != Some(true) && description.media_ejectable != Some(true) {
        return None;
    }

    // The EFI System Partition of a GPT card. Filtering on the partition type
    // rather than on the volume name keeps a card the user labelled `EFI` in
    // the list.
    if description.media_content.as_deref() == Some("EFI") {
        return None;
    }

    // An APFS container holds Preboot, Recovery, Update and VM beside the
    // volume the user sees; macOS mounts that one and leaves the rest, so an
    // APFS volume earns a place in the list only once it carries a mount point.
    if description.media_content.as_deref() == Some(APFS_VOLUME_TYPE)
        && description.volume_path.is_none()
    {
        return None;
    }

    let filesystem = filesystem(description);
    let (total_bytes, used_bytes) = match &filesystem {
        Filesystem::Mounted(mount_point) => crate::statvfs::disk_space(mount_point).unzip(),
        Filesystem::Unmounted | Filesystem::Unreadable => (description.media_size, None),
    };

    Some(StorageDevice {
        name: description
            .volume_name
            .clone()
            .unwrap_or_else(|| unlabelled_name(&description.bsd_name)),
        filesystem,
        device_path: device_path(&description.bsd_name),
        total_bytes,
        used_bytes,
    })
}

/// Where the described media's filesystem stands. Media whose filesystem macOS
/// cannot read is not mountable and never carries a path, which is also how a
/// partition that holds no filesystem at all describes itself, so a reserved
/// partition of a foreign partition table reads as unreadable media.
fn filesystem(description: &diskarb::Description) -> Filesystem {
    match &description.volume_path {
        Some(mount_point) => Filesystem::Mounted(mount_point.clone()),
        None if description.volume_mountable == Some(true) => Filesystem::Unmounted,
        None => Filesystem::Unreadable,
    }
}

/// Display name for a volume that carries no label of its own.
fn unlabelled_name(bsd_name: &str) -> String {
    format!("Removable ({bsd_name})")
}

/// `/dev` node of the disk named `bsd_name`, the identity the watcher keys
/// devices by.
fn device_path(bsd_name: &str) -> PathBuf {
    PathBuf::from(format!("{DEV_DIR}/{bsd_name}"))
}

/// The BSD name of the disk at `device_path`, the last component of its `/dev`
/// node.
fn bsd_name(device_path: &Path) -> &str {
    device_path
        .file_name()
        .expect("device path without a file name")
        .to_str()
        .expect("BSD disk name outside UTF-8")
}

/// TODO: Implement `ImageCaptureCore` integration for PTP cameras.
/// For now, cameras that mount as mass storage are detected by `scan_storage()`.
#[must_use]
pub const fn scan_cameras() -> Vec<Camera> {
    Vec::new()
}

/// Reports removable cards appearing, mounting, unmounting and disappearing as
/// `DiskArbitration` publishes them. The future stays pending for as long as the
/// watch lasts, and dropping it tears the watch down.
///
/// Registration replays every attached disk, so the watch needs no baseline
/// scan. `DiskArbitration` publishes a mount the moment `diskarbitrationd` knows
/// where the volume landed, so a card that arrives before its mount point is
/// settled reaches the consumer as an insertion carrying no mount point
/// followed by a mount carrying one, with no timer in between.
pub(crate) async fn watch(tx: UnboundedSender<DeviceEvent>) -> Result<(), WatchError> {
    let known = Mutex::new(HashMap::new());
    let sender = tx.clone();

    let watcher = diskarb::Watcher::start(move |event| {
        if let Some(device_event) = reported(&known, event) {
            // A closed channel means the consumer is gone, leaving nothing to
            // notify; the watch itself ends with the task holding it.
            sender.send(device_event).ok();
        }
    })
    .map_err(|error| WatchError::Backend(error.to_string()))?;

    tx.closed().await;
    drop(watcher);

    Ok(())
}

/// The event `report` implies for the consumer, with `known` advanced to the
/// device set the report leaves behind.
///
/// A disk `DiskArbitration` reports that is no card, one of the internal disks
/// or an APFS snapshot, reads the same as one that detached: it leaves the
/// known set, and only a disk that was in it reports a removal. Describing the
/// disk reads its free space, so it happens before the map is locked.
fn reported(
    known: &Mutex<HashMap<String, StorageDevice>>,
    report: diskarb::Event,
) -> Option<DeviceEvent> {
    let (bsd_name, current) = match report {
        diskarb::Event::Appeared(description) | diskarb::Event::Changed(description) => {
            (description.bsd_name.clone(), device(&description))
        }
        diskarb::Event::Disappeared { bsd_name } => (bsd_name, None),
    };

    // Unwinding out of a DiskArbitration callback would cross the framework's C
    // frames, so a map poisoned by an earlier panic is taken as it stands.
    let mut known = known.lock().unwrap_or_else(PoisonError::into_inner);

    let Some(current) = current else {
        return known.remove(&bsd_name).map(removed);
    };

    let device_event = diff::change(known.get(&bsd_name), &current);
    known.insert(bsd_name, current);

    device_event
}

/// The removal of a device the watch was reporting.
fn removed(device: StorageDevice) -> DeviceEvent {
    DeviceEvent::Removed {
        device_path: device.device_path,
    }
}

/// Mounts through `DiskArbitration`, which picks the mount point and the
/// filesystem driver itself, so `options` carries nothing macOS can act on.
pub(crate) fn mount(
    device: &StorageDevice,
    _options: &MountOptions,
) -> Result<PathBuf, MountError> {
    let disk = bsd_name(&device.device_path);

    let description = diskarb::mount(disk).map_err(|error| match error {
        diskarb::Error::Dissented(status) => mount_error(status),
        diskarb::Error::NoSession
        | diskarb::Error::NoSuchDisk(_)
        | diskarb::Error::Undescribed(_) => MountError::Failed(error.to_string()),
    })?;

    description.volume_path.ok_or_else(|| {
        MountError::Failed(format!(
            "mounted {disk} but DiskArbitration reported no mount point"
        ))
    })
}

/// `MountError` for the status `DiskArbitration` dissented a mount with.
fn mount_error(status: i32) -> MountError {
    match status {
        BUSY => MountError::AlreadyMounted,
        NOT_PERMITTED | NOT_PRIVILEGED => MountError::PermissionDenied,
        _ => MountError::Failed(format!(
            "DiskArbitration refused the mount with status {status}"
        )),
    }
}

/// Unmounts through `DiskArbitration`, forcing the unmount past open files when
/// `options` asks for it.
pub(crate) fn unmount(
    device: &StorageDevice,
    options: &UnmountOptions,
) -> Result<(), UnmountError> {
    diskarb::unmount(
        bsd_name(&device.device_path),
        options.force.unwrap_or(false),
    )
    .map_err(|error| match error {
        diskarb::Error::Dissented(status) => unmount_error(status),
        diskarb::Error::NoSession
        | diskarb::Error::NoSuchDisk(_)
        | diskarb::Error::Undescribed(_) => UnmountError::Failed(error.to_string()),
    })
}

/// `UnmountError` for the status `DiskArbitration` dissented an unmount with.
fn unmount_error(status: i32) -> UnmountError {
    match status {
        BUSY => UnmountError::Busy,
        NOT_PERMITTED | NOT_PRIVILEGED => UnmountError::PermissionDenied,
        NOT_MOUNTED => UnmountError::NotMounted,
        _ => UnmountError::Failed(format!(
            "DiskArbitration refused the unmount with status {status}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
        sync::Mutex,
    };

    use objc2_disk_arbitration::{
        kDAReturnBusy, kDAReturnError, kDAReturnNotMounted, kDAReturnNotPermitted,
        kDAReturnNotPrivileged,
    };

    use crate::{DeviceEvent, Filesystem, MountError, UnmountError, diskarb};

    /// A mounted, removable, FAT-formatted card.
    fn description(bsd_name: &str) -> diskarb::Description {
        diskarb::Description {
            bsd_name: bsd_name.to_owned(),
            volume_name: Some(String::from("EOS_DIGITAL")),
            volume_path: Some(PathBuf::from("/Volumes/EOS_DIGITAL")),
            volume_mountable: Some(true),
            media_removable: Some(true),
            media_ejectable: Some(true),
            media_leaf: Some(true),
            media_content: Some(String::from("DOS_FAT_32")),
            media_size: Some(64_000_000_000),
        }
    }

    /// The same card attached without a mount point.
    fn unmounted(bsd_name: &str) -> diskarb::Description {
        diskarb::Description {
            volume_path: None,
            ..description(bsd_name)
        }
    }

    /// Media `DiskArbitration` reports as carrying no mountable filesystem.
    fn unreadable(bsd_name: &str) -> diskarb::Description {
        diskarb::Description {
            volume_name: None,
            volume_mountable: Some(false),
            media_content: None,
            ..unmounted(bsd_name)
        }
    }

    #[test]
    fn mounted_card_keeps_its_volume_name_mount_point_and_device_node() {
        let device = super::device(&description("disk4s1")).expect("mounted card dropped");

        assert_eq!(device.name, "EOS_DIGITAL");
        assert_eq!(
            device.filesystem,
            Filesystem::Mounted(PathBuf::from("/Volumes/EOS_DIGITAL"))
        );
        assert_eq!(device.device_path, PathBuf::from("/dev/disk4s1"));
    }

    #[test]
    fn unmounted_card_carries_its_media_size_as_its_total() {
        let device = super::device(&unmounted("disk4s1")).expect("unmounted card dropped");

        assert_eq!(device.filesystem, Filesystem::Unmounted);
        assert_eq!(device.total_bytes, Some(64_000_000_000));
        assert_eq!(device.used_bytes, None);
    }

    #[test]
    fn media_macos_cannot_read_is_surfaced_as_unreadable() {
        let device = super::device(&unreadable("disk4s1")).expect("unreadable media dropped");

        assert_eq!(device.filesystem, Filesystem::Unreadable);
        assert_eq!(device.device_path, PathBuf::from("/dev/disk4s1"));
    }

    #[test]
    fn card_without_a_volume_name_is_named_after_its_bsd_name() {
        let described = diskarb::Description {
            volume_name: None,
            ..description("disk4s1")
        };

        let device = super::device(&described).expect("unlabelled card dropped");

        assert_eq!(device.name, "Removable (disk4s1)");
    }

    #[test]
    fn internal_disk_yields_no_device() {
        let described = diskarb::Description {
            media_removable: Some(false),
            media_ejectable: Some(false),
            ..description("disk1s1")
        };

        assert!(super::device(&described).is_none());
    }

    #[test]
    fn partitioned_whole_disk_yields_no_device() {
        let described = diskarb::Description {
            media_leaf: Some(false),
            ..description("disk4")
        };

        assert!(super::device(&described).is_none());
    }

    #[test]
    fn efi_system_partition_yields_no_device() {
        let described = diskarb::Description {
            volume_name: Some(String::from("EFI")),
            media_content: Some(String::from("EFI")),
            ..description("disk4s1")
        };

        assert!(super::device(&described).is_none());
    }

    #[test]
    fn unmounted_apfs_volume_yields_no_device() {
        let described = diskarb::Description {
            volume_name: Some(String::from("Recovery")),
            media_content: Some(String::from(super::APFS_VOLUME_TYPE)),
            ..unmounted("disk4s3")
        };

        assert!(super::device(&described).is_none());
    }

    #[test]
    fn mounted_apfs_volume_is_kept() {
        let described = diskarb::Description {
            volume_name: Some(String::from("Backup")),
            media_content: Some(String::from(super::APFS_VOLUME_TYPE)),
            ..description("disk4s1")
        };

        let device = super::device(&described).expect("mounted APFS volume dropped");

        assert_eq!(device.name, "Backup");
        assert_eq!(
            device.filesystem,
            Filesystem::Mounted(PathBuf::from("/Volumes/EOS_DIGITAL"))
        );
    }

    #[test]
    fn card_labelled_efi_is_kept() {
        let described = diskarb::Description {
            volume_name: Some(String::from("EFI")),
            media_content: Some(String::from("Windows_FAT_32")),
            ..description("disk4s1")
        };

        let device = super::device(&described).expect("card labelled EFI dropped");

        assert_eq!(device.name, "EFI");
    }

    #[test]
    fn disk_nodes_are_the_buffered_ones_carrying_a_unit_and_its_slices() {
        assert!(super::is_disk_node("disk4"));
        assert!(super::is_disk_node("disk4s1"));
        assert!(super::is_disk_node("disk3s1s1"));

        assert!(!super::is_disk_node("rdisk4s1"));
        assert!(!super::is_disk_node("null"));
        assert!(!super::is_disk_node("disk"));
        assert!(!super::is_disk_node("disks1"));
    }

    #[test]
    fn appearing_card_is_reported_inserted_and_joins_the_known_set() {
        let known = Mutex::new(HashMap::new());

        let event = super::reported(&known, diskarb::Event::Appeared(unmounted("disk4s1")))
            .expect("appearing card reported nothing");

        assert!(matches!(event, DeviceEvent::Inserted(device) if device.name == "EOS_DIGITAL"));
        assert!(
            known
                .lock()
                .expect("known set poisoned")
                .contains_key("disk4s1")
        );
    }

    #[test]
    fn card_that_gains_a_mount_point_is_reported_mounted() {
        let known = Mutex::new(HashMap::new());
        super::reported(&known, diskarb::Event::Appeared(unmounted("disk4s1")));

        let event = super::reported(&known, diskarb::Event::Changed(description("disk4s1")))
            .expect("settling mount reported nothing");

        assert!(matches!(
            event,
            DeviceEvent::Mounted { mount_point, .. }
                if mount_point == Path::new("/Volumes/EOS_DIGITAL")
        ));
    }

    #[test]
    fn detached_card_is_reported_removed_and_leaves_the_known_set() {
        let known = Mutex::new(HashMap::new());
        super::reported(&known, diskarb::Event::Appeared(unmounted("disk4s1")));

        let event = super::reported(
            &known,
            diskarb::Event::Disappeared {
                bsd_name: String::from("disk4s1"),
            },
        )
        .expect("detached card reported nothing");

        assert!(matches!(
            event,
            DeviceEvent::Removed { device_path } if device_path == Path::new("/dev/disk4s1")
        ));
        assert!(known.lock().expect("known set poisoned").is_empty());
    }

    #[test]
    fn disk_the_watch_never_reported_disappears_silently() {
        let known = Mutex::new(HashMap::new());

        let event = super::reported(
            &known,
            diskarb::Event::Disappeared {
                bsd_name: String::from("disk0"),
            },
        );

        assert!(event.is_none());
    }

    #[test]
    fn dissented_mount_statuses_map_to_mount_errors() {
        assert!(matches!(
            super::mount_error(kDAReturnBusy),
            MountError::AlreadyMounted
        ));
        assert!(matches!(
            super::mount_error(kDAReturnNotPermitted),
            MountError::PermissionDenied
        ));
        assert!(matches!(
            super::mount_error(kDAReturnNotPrivileged),
            MountError::PermissionDenied
        ));
        assert!(matches!(
            super::mount_error(kDAReturnError),
            MountError::Failed(_)
        ));
    }

    #[test]
    fn dissented_unmount_statuses_map_to_unmount_errors() {
        assert!(matches!(
            super::unmount_error(kDAReturnBusy),
            UnmountError::Busy
        ));
        assert!(matches!(
            super::unmount_error(kDAReturnNotPermitted),
            UnmountError::PermissionDenied
        ));
        assert!(matches!(
            super::unmount_error(kDAReturnNotPrivileged),
            UnmountError::PermissionDenied
        ));
        assert!(matches!(
            super::unmount_error(kDAReturnNotMounted),
            UnmountError::NotMounted
        ));
        assert!(matches!(
            super::unmount_error(kDAReturnError),
            UnmountError::Failed(_)
        ));
    }
}
