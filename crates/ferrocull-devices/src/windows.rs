//! Windows device detection using Win32 APIs.
//!
//! Uses `GetLogicalDrives` and `GetVolumeInformation` for removable storage detection.
//! Camera detection currently relies on cameras mounting as mass storage devices.
//!
//! TODO: Proper WPD (Windows Portable Devices) integration for PTP/MTP cameras.

use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    process::Command,
    thread::{self, JoinHandle},
    time::Duration,
};

use tokio::sync::mpsc::UnboundedSender;
use windows::{
    Win32::{
        Storage::FileSystem::{
            GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
        },
        System::WindowsProgramming::DRIVE_REMOVABLE,
    },
    core::PCWSTR,
};

use crate::{
    Camera, DeviceEvent, MountError, MountOptions, ScanError, StorageDevice, UnmountError,
    UnmountOptions, WatchError,
};

fn disk_space(mount_point: &Path) -> Option<(u64, u64)> {
    let root_wide = os_to_wide_null(mount_point.as_os_str());
    let mut total: u64 = 0;
    let mut free: u64 = 0;

    unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR(root_wide.as_ptr()),
            None,
            Some(&mut total),
            Some(&mut free),
        )
        .ok()?;
    }

    let used = total.saturating_sub(free);
    Some((total, used))
}

pub fn scan_storage() -> Result<Vec<StorageDevice>, ScanError> {
    let drive_mask = unsafe { GetLogicalDrives() };
    if drive_mask == 0 {
        return Err(ScanError::Backend(
            "GetLogicalDrives returned 0 (no drives accessible or Win32 error)".to_owned(),
        ));
    }

    let devices = (0u8..26)
        .filter(|i| drive_mask & (1 << i) != 0)
        .filter_map(|i| {
            let letter = (b'A' + i) as char;
            let root = format!("{letter}:\\");
            let root_wide = to_wide_null(&root);

            let drive_type = unsafe { GetDriveTypeW(PCWSTR(root_wide.as_ptr())) };
            if drive_type != DRIVE_REMOVABLE {
                return None;
            }

            let name = volume_name(&root_wide).unwrap_or_else(|| format!("Removable ({letter}:)"));
            let mount_point = PathBuf::from(&root);
            let device_path = PathBuf::from(format!("\\\\.\\{letter}:"));

            let (total_bytes, used_bytes) = disk_space(&mount_point).unzip();

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

/// TODO: Implement WPD (Windows Portable Devices) for PTP/MTP camera detection.
/// For now, cameras that mount as mass storage are detected by `scan_storage()`.
#[must_use]
pub const fn scan_cameras() -> Vec<Camera> {
    Vec::new()
}

/// TODO: Replace polling with `RegisterDeviceNotificationW` for event-driven detection.
/// TODO: Propagate per-poll scan failures (currently `scan_current_removable_drives`
/// silently degrades to empty on a `scan_storage` error, which can produce spurious
/// `Removed` events). Requires extending the channel payload to carry errors.
pub fn watch(tx: UnboundedSender<DeviceEvent>) -> Result<JoinHandle<()>, WatchError> {
    Ok(thread::spawn(move || {
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
    }))
}

fn scan_current_removable_drives() -> HashMap<char, StorageDevice> {
    scan_storage()
        .unwrap_or_default()
        .into_iter()
        .map(|d| {
            let letter = d
                .mount_point
                .as_ref()
                .expect("scan_storage sets mount_point")
                .to_string_lossy()
                .chars()
                .next()
                .expect("mount point has drive letter");
            (letter, d)
        })
        .collect()
}

fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn os_to_wide_null(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

fn volume_name(root_wide: &[u16]) -> Option<String> {
    let mut name_buf = [0u16; 256];

    unsafe {
        GetVolumeInformationW(
            PCWSTR(root_wide.as_ptr()),
            Some(&mut name_buf),
            None,
            None,
            None,
            None,
        )
        .ok()?;
    }

    let len = name_buf
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(name_buf.len());
    if len == 0 {
        return None;
    }

    Some(
        OsString::from_wide(&name_buf[..len])
            .to_string_lossy()
            .into_owned(),
    )
}

/// Windows auto-mounts devices -- manual mount is not supported.
pub fn mount(_device: &StorageDevice, _options: &MountOptions) -> Result<PathBuf, MountError> {
    Err(MountError::Failed(String::from(
        "Windows auto-mounts devices — manual mount not supported",
    )))
}

/// Unmount via `mountvol /P` (dismounts the volume and takes it offline).
pub fn unmount(device: &StorageDevice, _options: &UnmountOptions) -> Result<(), UnmountError> {
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
