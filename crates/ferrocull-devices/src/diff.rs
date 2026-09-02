//! The per-device transition shared by the macOS and Windows watchers.
//!
//! Windows drives it from a whole-set rescan, macOS from `DiskArbitration`
//! callbacks. Devices are keyed by device path, so a card swapped under a
//! reused volume name, two cards carrying the factory label `EOS_DIGITAL` for
//! instance, arrives as a removal and an insertion rather than as one device
//! changing identity.

use crate::{DeviceEvent, Filesystem, StorageDevice};

/// The event implied by a device moving from `previous` to `current`, or `None`
/// when nothing the consumer cares about changed.
///
/// A device with no `previous` is new and reports an insertion. One that has a
/// `previous` reports a mount or an unmount when its mount point appears or
/// disappears, and a mount at the new path when it moves, as it does when a
/// card remounts beside the leftover directory of an abnormal unmount. Media
/// the OS cannot read carries no mount point either, so reformatting a card in
/// the same slot reports a mount.
pub(crate) fn change(
    previous: Option<&StorageDevice>,
    current: &StorageDevice,
) -> Option<DeviceEvent> {
    let Some(previous) = previous else {
        return Some(DeviceEvent::Inserted(current.clone()));
    };

    match (&previous.filesystem, &current.filesystem) {
        (Filesystem::Mounted(_), Filesystem::Unmounted | Filesystem::Unreadable) => {
            Some(DeviceEvent::Unmounted {
                device_path: current.device_path.clone(),
            })
        }
        (Filesystem::Unmounted | Filesystem::Unreadable, Filesystem::Mounted(mount_point)) => {
            Some(DeviceEvent::Mounted {
                device_path: current.device_path.clone(),
                mount_point: mount_point.clone(),
                total_bytes: current.total_bytes,
                used_bytes: current.used_bytes,
            })
        }
        (Filesystem::Mounted(previous_mount_point), Filesystem::Mounted(mount_point))
            if previous_mount_point != mount_point =>
        {
            Some(DeviceEvent::Mounted {
                device_path: current.device_path.clone(),
                mount_point: mount_point.clone(),
                total_bytes: current.total_bytes,
                used_bytes: current.used_bytes,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::{DeviceEvent, Filesystem, StorageDevice};

    fn device(name: &str) -> StorageDevice {
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
            ..device(name)
        }
    }

    #[test]
    fn device_with_no_previous_state_is_a_change_to_inserted() {
        let current = device("EOS_DIGITAL");

        let event = super::change(None, &current).expect("new device reported no change");

        assert!(matches!(event, DeviceEvent::Inserted(device) if device.name == "EOS_DIGITAL"));
    }

    #[test]
    fn device_gaining_a_mount_point_is_a_change_to_mounted() {
        let previous = unmounted("EOS_DIGITAL");
        let current = device("EOS_DIGITAL");

        let event = super::change(Some(&previous), &current).expect("mount reported no change");

        assert!(matches!(
            event,
            DeviceEvent::Mounted { mount_point, .. }
                if mount_point == Path::new("/Volumes/EOS_DIGITAL")
        ));
    }

    #[test]
    fn device_losing_its_mount_point_is_a_change_to_unmounted() {
        let previous = device("EOS_DIGITAL");
        let current = unmounted("EOS_DIGITAL");

        let event = super::change(Some(&previous), &current).expect("unmount reported no change");

        assert!(matches!(
            event,
            DeviceEvent::Unmounted { device_path }
                if device_path == Path::new("/dev/disk-EOS_DIGITAL")
        ));
    }

    #[test]
    fn unchanged_device_is_no_change() {
        let previous = device("EOS_DIGITAL");
        let current = device("EOS_DIGITAL");

        assert!(super::change(Some(&previous), &current).is_none());
    }
}
