//! Volume-set diffing shared by the macOS and Windows watchers.
//!
//! Both backends rescan the whole set of removable devices, mounted or not, and
//! report what changed since the previous set. Devices are keyed by device path,
//! so a card swapped under a reused volume name, two cards carrying the factory
//! label `EOS_DIGITAL` for instance, arrives as a removal and an insertion
//! rather than as one device changing identity. Windows derives the device path
//! from the drive letter, so a card swapped into the same letter keeps the key
//! and reports nothing.

use std::{collections::HashMap, path::Path};

use crate::{DeviceEvent, Filesystem, StorageDevice};

fn by_device_path(devices: &[StorageDevice]) -> HashMap<&Path, &StorageDevice> {
    devices
        .iter()
        .map(|device| (device.device_path.as_path(), device))
        .collect()
}

/// Events implied by moving from the `known` device set to `current`.
///
/// Removals read their device path from `known`, which is the only place it
/// survives once the device is gone. A device present in both sets reports a
/// mount or an unmount when its mount point appears or disappears, and a mount
/// at the new path when it moves, as it does when a card remounts beside the
/// leftover directory of an abnormal unmount. Media the OS cannot read carries
/// no mount point either, so reformatting a card in the same slot reports a
/// mount.
pub(crate) fn events(known: &[StorageDevice], current: &[StorageDevice]) -> Vec<DeviceEvent> {
    let known_by_path = by_device_path(known);
    let current_by_path = by_device_path(current);

    let appeared = current.iter().filter_map(|device| {
        let Some(previous) = known_by_path.get(device.device_path.as_path()) else {
            return Some(DeviceEvent::Inserted(device.clone()));
        };

        match (&previous.filesystem, &device.filesystem) {
            (Filesystem::Mounted(_), Filesystem::Unmounted | Filesystem::Unreadable) => {
                Some(DeviceEvent::Unmounted {
                    device_path: device.device_path.clone(),
                })
            }
            (Filesystem::Unmounted | Filesystem::Unreadable, Filesystem::Mounted(mount_point)) => {
                Some(DeviceEvent::Mounted {
                    device_path: device.device_path.clone(),
                    mount_point: mount_point.clone(),
                    total_bytes: device.total_bytes,
                    used_bytes: device.used_bytes,
                })
            }
            (Filesystem::Mounted(previous_mount_point), Filesystem::Mounted(mount_point))
                if previous_mount_point != mount_point =>
            {
                Some(DeviceEvent::Mounted {
                    device_path: device.device_path.clone(),
                    mount_point: mount_point.clone(),
                    total_bytes: device.total_bytes,
                    used_bytes: device.used_bytes,
                })
            }
            _ => None,
        }
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

    fn device_on(name: &str, device_path: &str) -> StorageDevice {
        StorageDevice {
            device_path: PathBuf::from(device_path),
            ..device(name)
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
        let events = super::events(&[], &[device("EOS_DIGITAL")]);

        assert_eq!(inserted_names(&events), vec!["EOS_DIGITAL".to_owned()]);
        assert_eq!(removed_paths(&events), Vec::<PathBuf>::new());
    }

    #[test]
    fn vanished_volume_is_reported_as_removed() {
        let events = super::events(&[device("EOS_DIGITAL")], &[]);

        assert_eq!(
            removed_paths(&events),
            vec![PathBuf::from("/dev/disk-EOS_DIGITAL")]
        );
        assert_eq!(inserted_names(&events), Vec::<String>::new());
    }

    #[test]
    fn unchanged_volume_set_emits_nothing() {
        let events = super::events(&[device("EOS_DIGITAL")], &[device("EOS_DIGITAL")]);

        assert!(events.is_empty());
    }

    #[test]
    fn swapped_card_is_reported_as_both_removed_and_inserted() {
        let events = super::events(&[device("EOS_DIGITAL")], &[device("NIKON D850")]);

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
        let events = super::events(&[device("EOS_DIGITAL")], &[unmounted("EOS_DIGITAL")]);

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            DeviceEvent::Unmounted { device_path }
                if device_path == Path::new("/dev/disk-EOS_DIGITAL")
        ));
    }

    #[test]
    fn volume_gaining_a_mount_point_is_reported_as_mounted() {
        let events = super::events(&[unmounted("EOS_DIGITAL")], &[device("EOS_DIGITAL")]);

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
        let events = super::events(&[unreadable("EOS_DIGITAL")], &[device("EOS_DIGITAL")]);

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
