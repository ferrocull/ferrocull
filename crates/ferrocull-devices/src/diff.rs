//! Volume-set diffing shared by the macOS and Windows watchers.
//!
//! Windows rescans the whole set of removable devices, mounted or not, and
//! reports what changed since the previous set; macOS keys the same per-device
//! primitive off `DiskArbitration` callbacks. Devices are keyed by device path,
//! so a card swapped under a reused volume name, two cards carrying the factory
//! label `EOS_DIGITAL` for instance, arrives as a removal and an insertion
//! rather than as one device changing identity. Windows derives the device path
//! from the drive letter, so a card swapped into the same letter keeps the key
//! and reports nothing.

#[cfg(target_os = "windows")]
use std::{collections::HashMap, path::Path};

use crate::{DeviceEvent, Filesystem, StorageDevice};

#[cfg(target_os = "windows")]
fn by_device_path(devices: &[StorageDevice]) -> HashMap<&Path, &StorageDevice> {
    devices
        .iter()
        .map(|device| (device.device_path.as_path(), device))
        .collect()
}

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

/// Events implied by moving from the `known` device set to `current`.
///
/// Removals read their device path from `known`, which is the only place it
/// survives once the device is gone.
#[cfg(target_os = "windows")]
pub(crate) fn events(known: &[StorageDevice], current: &[StorageDevice]) -> Vec<DeviceEvent> {
    let known_by_path = by_device_path(known);
    let current_by_path = by_device_path(current);

    let appeared = current.iter().filter_map(|device| {
        change(
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

    /// The whole-set diff, which only the polling Windows watcher drives.
    #[cfg(target_os = "windows")]
    mod sets {
        use std::path::{Path, PathBuf};

        use super::{device, unmounted};
        use crate::{DeviceEvent, Filesystem, StorageDevice, diff};

        fn device_on(name: &str, device_path: &str) -> StorageDevice {
            StorageDevice {
                device_path: PathBuf::from(device_path),
                ..device(name)
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
        fn appearing_volume_is_reported_as_inserted() {
            let events = diff::events(&[], &[device("EOS_DIGITAL")]);

            assert_eq!(inserted_names(&events), vec!["EOS_DIGITAL".to_owned()]);
            assert_eq!(removed_paths(&events), Vec::<PathBuf>::new());
        }

        #[test]
        fn vanished_volume_is_reported_as_removed() {
            let events = diff::events(&[device("EOS_DIGITAL")], &[]);

            assert_eq!(
                removed_paths(&events),
                vec![PathBuf::from("/dev/disk-EOS_DIGITAL")]
            );
            assert_eq!(inserted_names(&events), Vec::<String>::new());
        }

        #[test]
        fn unchanged_volume_set_emits_nothing() {
            let events = diff::events(&[device("EOS_DIGITAL")], &[device("EOS_DIGITAL")]);

            assert!(events.is_empty());
        }

        #[test]
        fn swapped_card_is_reported_as_both_removed_and_inserted() {
            let events = diff::events(&[device("EOS_DIGITAL")], &[device("NIKON D850")]);

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

            let events = diff::events(&known, &current);

            assert_eq!(events.len(), 2);
            assert_eq!(removed_paths(&events), vec![PathBuf::from("/dev/disk2s1")]);
            assert_eq!(inserted_names(&events), vec!["EOS_DIGITAL".to_owned()]);
        }

        #[test]
        fn volume_losing_its_mount_point_is_reported_as_unmounted() {
            let events = diff::events(&[device("EOS_DIGITAL")], &[unmounted("EOS_DIGITAL")]);

            assert_eq!(events.len(), 1);
            assert!(matches!(
                &events[0],
                DeviceEvent::Unmounted { device_path }
                    if device_path == Path::new("/dev/disk-EOS_DIGITAL")
            ));
        }

        #[test]
        fn volume_gaining_a_mount_point_is_reported_as_mounted() {
            let events = diff::events(&[unmounted("EOS_DIGITAL")], &[device("EOS_DIGITAL")]);

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
            let known = [device("EOS_DIGITAL")];
            let current = [StorageDevice {
                filesystem: Filesystem::Mounted(PathBuf::from("/Volumes/EOS_DIGITAL 1")),
                ..device("EOS_DIGITAL")
            }];

            let events = diff::events(&known, &current);

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
            let events = diff::events(&[unreadable("EOS_DIGITAL")], &[device("EOS_DIGITAL")]);

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
            let events = diff::events(&[], &[unmounted("NIKON_D850")]);

            assert_eq!(inserted_names(&events), vec!["NIKON_D850".to_owned()]);
        }
    }
}
