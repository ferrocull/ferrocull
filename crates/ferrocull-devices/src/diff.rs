//! Volume-set diffing shared by the macOS and Windows watchers.
//!
//! Both backends rescan the whole set of mounted removable volumes and report
//! what changed since the previous set. The swap arm, one mount point held by a
//! different device in each set, only fires on macOS: Windows derives the device
//! path from the drive letter, so a card swapped into the same letter keeps the
//! same device path.

use std::{collections::HashMap, path::Path};

use crate::{DeviceEvent, StorageDevice};

/// Mount point of a scanned volume.
pub(crate) fn mount_point(device: &StorageDevice) -> &Path {
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
pub(crate) fn events(known: &[StorageDevice], current: &[StorageDevice]) -> Vec<DeviceEvent> {
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
    fn card_swapped_under_a_reused_volume_name_is_removed_then_inserted() {
        let known = [device_on("EOS_DIGITAL", "/dev/disk2s1")];
        let current = [device_on("EOS_DIGITAL", "/dev/disk4s1")];

        let events = super::events(&known, &current);

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
