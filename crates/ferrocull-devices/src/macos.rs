use std::{
    ffi::{CStr, OsStr, c_void},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::Command,
    ptr::{NonNull, from_ref},
    thread::{self, JoinHandle},
};

use nix::sys::statvfs;
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFRunLoop, CFString, CFType, CFURL, kCFRunLoopDefaultMode,
};
use objc2_disk_arbitration::{
    DADisk, DARegisterDiskAppearedCallback, DARegisterDiskDisappearedCallback, DASession,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    Camera, DeviceEvent, MountError, MountOptions, ScanError, StorageDevice, UnmountError,
    UnmountOptions, WatchError,
};

fn disk_space(mount_point: &Path) -> Option<(u64, u64)> {
    let stat = statvfs::statvfs(mount_point).ok()?;
    let block_size = stat.fragment_size();
    let total = u64::from(stat.blocks()) * block_size;
    let available = u64::from(stat.blocks_available()) * block_size;
    let used = total.saturating_sub(available);

    Some((total, used))
}

const DA_VOLUME_NAME: &str = "DAVolumeName";
const DA_VOLUME_PATH: &str = "DAVolumePath";
const DA_MEDIA_REMOVABLE: &str = "DAMediaRemovable";
const DA_MEDIA_EJECTABLE: &str = "DAMediaEjectable";

/// # Safety
///
/// `DADiskGetBSDName` is FFI. The returned pointer is valid for the disk's lifetime.
unsafe fn bsd_name_path(disk: &DADisk) -> Option<PathBuf> {
    let ptr = unsafe { disk.bsd_name() };
    if ptr.is_null() {
        return None;
    }
    let name = unsafe { CStr::from_ptr(ptr) };
    Some(Path::new("/dev").join(OsStr::from_bytes(name.to_bytes())))
}

fn disk_to_storage_device(disk: &DADisk) -> Option<StorageDevice> {
    // SAFETY: FFI call; disk reference validity is guaranteed by the borrow
    let desc = unsafe { disk.description()? };

    let removable = dict_bool(&desc, DA_MEDIA_REMOVABLE);
    let ejectable = dict_bool(&desc, DA_MEDIA_EJECTABLE);
    if !removable && !ejectable {
        return None;
    }

    let mount_point = url_path(&desc, DA_VOLUME_PATH)?;

    if mount_point.starts_with("/System") || mount_point.as_os_str() == "/" {
        return None;
    }

    let name = dict_string(&desc, DA_VOLUME_NAME)
        .or_else(|| {
            mount_point
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "Unnamed".to_owned());

    let device_path = unsafe { bsd_name_path(disk) }.unwrap_or_else(|| mount_point.clone());

    let (total_bytes, used_bytes) = disk_space(&mount_point).unzip();

    Some(StorageDevice {
        name,
        mount_point: Some(mount_point),
        device_path,
        total_bytes,
        used_bytes,
    })
}

fn dict_value<'a>(dict: &'a CFDictionary, key: &str) -> Option<&'a CFType> {
    let key = CFString::from_str(key);
    let key_ptr: *const c_void = from_ref::<CFString>(key.as_ref()).cast();
    // SAFETY: key_ptr is a valid CFString pointer; value() returns NULL or a valid CF object
    let val_ptr = unsafe { dict.value(key_ptr) };
    if val_ptr.is_null() {
        return None;
    }
    // SAFETY: Non-null pointer from CFDictionaryGetValue is valid for the dict's lifetime
    Some(unsafe { &*(val_ptr.cast::<CFType>()) })
}

fn dict_string(dict: &CFDictionary, key: &str) -> Option<String> {
    let value = dict_value(dict, key)?;
    let s = value.downcast_ref::<CFString>()?;
    Some(s.to_string())
}

fn dict_bool(dict: &CFDictionary, key: &str) -> bool {
    dict_value(dict, key)
        .and_then(|v| v.downcast_ref::<CFBoolean>())
        .is_some_and(CFBoolean::value)
}

fn url_path(dict: &CFDictionary, key: &str) -> Option<PathBuf> {
    let value = dict_value(dict, key)?;
    let url = value.downcast_ref::<CFURL>()?;
    url.to_file_path()
}

pub fn scan_storage() -> Result<Vec<StorageDevice>, ScanError> {
    // Use /Volumes directory - simpler and more reliable than DASession for one-shot scan
    let volumes_dir = PathBuf::from("/Volumes");
    let entries = std::fs::read_dir(&volumes_dir)?;

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

/// Parsed subset of `diskutil info -plist` output.
/// Uses `RemovableMedia` + `Ejectable` to match `disk_to_storage_device`'s
/// `DAMediaRemovable || DAMediaEjectable` criteria.
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

struct WatchContext {
    tx: UnboundedSender<DeviceEvent>,
}

unsafe extern "C-unwind" fn disk_appeared_callback(disk: NonNull<DADisk>, context: *mut c_void) {
    let ctx = unsafe { &*(context.cast::<WatchContext>()) };
    let disk = unsafe { disk.as_ref() };

    if let Some(storage) = disk_to_storage_device(disk) {
        drop(ctx.tx.send(DeviceEvent::Inserted(storage)));
    }
}

unsafe extern "C-unwind" fn disk_disappeared_callback(disk: NonNull<DADisk>, context: *mut c_void) {
    let ctx = unsafe { &*(context.cast::<WatchContext>()) };
    let disk = unsafe { disk.as_ref() };

    let Some(device_path) = (unsafe { bsd_name_path(disk) }) else {
        return;
    };
    drop(ctx.tx.send(DeviceEvent::Removed { device_path }));
}

/// The thread runs a `CFRunLoop` until the process exits.
///
/// `DASession` is not `Send`, so it must be constructed inside the worker thread.
/// A one-shot channel relays the construction result back so the caller can fail
/// fast on `DiskArbitration` unavailability without losing the typed error.
pub fn watch(tx: UnboundedSender<DeviceEvent>) -> Result<JoinHandle<()>, WatchError> {
    let (init_tx, init_rx) = std::sync::mpsc::channel();

    let handle = thread::spawn(move || {
        // SAFETY: No special allocator needed, None uses the default
        let Some(session) = (unsafe { DASession::new(None) }) else {
            drop(init_tx.send(Err(WatchError::Backend(
                "DASession::new returned null (DiskArbitration unavailable)".to_owned(),
            ))));
            return;
        };
        drop(init_tx.send(Ok(())));

        let ctx = Box::into_raw(Box::new(WatchContext { tx }));

        unsafe {
            DARegisterDiskAppearedCallback(
                &session,
                None,
                Some(disk_appeared_callback),
                ctx.cast(),
            );
            DARegisterDiskDisappearedCallback(
                &session,
                None,
                Some(disk_disappeared_callback),
                ctx.cast(),
            );

            if let Some(run_loop) = CFRunLoop::current()
                && let Some(mode) = kCFRunLoopDefaultMode
            {
                session.schedule_with_run_loop(&run_loop, mode);
                CFRunLoop::run();
            }

            // Reclaim context whether the run loop ran or failed to start.
            drop(Box::from_raw(ctx));
        }
    });

    match init_rx.recv() {
        Ok(Ok(())) => Ok(handle),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(WatchError::Backend(
            "watch thread panicked before reporting init result".to_owned(),
        )),
    }
}

/// macOS auto-mounts devices -- manual mount is not supported.
pub fn mount(_device: &StorageDevice, _options: &MountOptions) -> Result<PathBuf, MountError> {
    Err(MountError::Failed(String::from(
        "macOS auto-mounts devices — manual mount not supported",
    )))
}

/// Unmount via `diskutil unmount [force]`.
pub fn unmount(device: &StorageDevice, options: &UnmountOptions) -> Result<(), UnmountError> {
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
