//! Device and volume discovery, with a per-platform backend behind one API.
//!
//! Enumerates removable volumes and cameras, watches for them appearing and
//! disappearing, and scans them for media files.

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod diff;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
pub mod scanner;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod statvfs;
#[cfg(target_os = "windows")]
mod win32;
#[cfg(target_os = "windows")]
mod windows;

use std::{
    io,
    path::{Path, PathBuf},
};

use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, thiserror::Error)]
pub enum ScanError {
    #[error("device enumeration I/O failure: {0}")]
    Io(String),

    #[error("device backend error: {0}")]
    Backend(String),
}

impl From<io::Error> for ScanError {
    fn from(e: io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum WatchError {
    #[error("device watch I/O failure: {0}")]
    Io(String),

    #[error("device watch backend error: {0}")]
    Backend(String),
}

impl From<io::Error> for WatchError {
    fn from(e: io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Where a storage device's filesystem stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filesystem {
    /// Mounted at this path.
    Mounted(PathBuf),
    /// Attached but not mounted, and mountable on request.
    Unmounted,
    /// Media whose filesystem the OS cannot read, so it never carries a mount
    /// point and mounting it cannot succeed.
    Unreadable,
}

#[derive(Debug, Clone)]
pub struct StorageDevice {
    pub name: String,
    pub filesystem: Filesystem,
    pub device_path: PathBuf,
    pub total_bytes: Option<u64>,
    pub used_bytes: Option<u64>,
    #[cfg(target_os = "linux")]
    pub object_path: String,
}

impl StorageDevice {
    /// Path the filesystem is mounted at, or `None` when it is not mounted.
    #[must_use]
    pub fn mount_point(&self) -> Option<&Path> {
        match &self.filesystem {
            Filesystem::Mounted(mount_point) => Some(mount_point),
            Filesystem::Unmounted | Filesystem::Unreadable => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Camera {
    pub name: String,
    pub port: String,
}

#[derive(Debug, Clone)]
pub enum Source {
    Storage(StorageDevice),
    Camera(Camera),
    /// User-added directory (not a detected storage device).
    Directory(PathBuf),
}

impl Source {
    /// Filesystem-ish path used to identify and scan this source.
    /// For storage, the mount point if mounted (else the device path);
    /// for cameras, the gphoto2 port string; for directories, the directory itself.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Storage(s) => s.mount_point().unwrap_or(&s.device_path),
            Self::Camera(c) => Path::new(&c.port),
            Self::Directory(p) => p,
        }
    }
}

#[derive(Debug, Clone)]
pub enum DeviceEvent {
    /// Block device appeared (USB plugged in). May already be mounted on
    /// platforms that auto-mount (macOS, Windows).
    Inserted(StorageDevice),
    /// Filesystem mounted — mount point and disk space now available.
    Mounted {
        device_path: PathBuf,
        mount_point: PathBuf,
        total_bytes: Option<u64>,
        used_bytes: Option<u64>,
    },
    /// Filesystem unmounted — device still physically present.
    Unmounted { device_path: PathBuf },
    /// Block device physically removed.
    Removed { device_path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum UnmountError {
    #[error("path is not a mount point")]
    NotMounted,
    #[error("device is busy")]
    Busy,
    #[error("permission denied")]
    PermissionDenied,
    #[error("{0}")]
    Failed(String),
}

#[derive(Debug, thiserror::Error)]
pub enum MountError {
    #[error("device is already mounted")]
    AlreadyMounted,
    #[error("permission denied")]
    PermissionDenied,
    #[error("{0}")]
    Failed(String),
}

#[derive(Clone, Default, serde::Serialize)]
#[cfg_attr(target_os = "linux", derive(zbus::zvariant::Type))]
#[cfg_attr(target_os = "linux", zvariant(signature = "a{sv}"))]
pub struct MountOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fstype: Option<FsType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<String>,
}

#[derive(Clone, Default, serde::Serialize)]
#[cfg_attr(target_os = "linux", derive(zbus::zvariant::Type))]
#[cfg_attr(target_os = "linux", zvariant(signature = "a{sv}"))]
pub struct UnmountOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
}

#[derive(Clone, serde::Serialize)]
pub enum FsType {
    #[serde(rename = "vfat")]
    Vfat,
    #[serde(rename = "exfat")]
    Exfat,
    #[serde(rename = "ntfs")]
    Ntfs,
    #[serde(rename = "ext4")]
    Ext4,
}

pub enum MountFlag {
    ReadOnly,
    Sync,
    NoExec,
    Flush,
}

impl MountFlag {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ReadOnly => "ro",
            Self::Sync => "sync",
            Self::NoExec => "noexec",
            Self::Flush => "flush",
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::scan_cameras;
#[cfg(target_os = "macos")]
pub use macos::scan_cameras;
pub use scanner::{ScannedFile, scan_camera_folders, scan_directory};
#[cfg(target_os = "windows")]
pub use windows::scan_cameras;

/// Runs a blocking platform call on a dedicated thread. A panic in `f` is a bug
/// in the platform backend, so the join failure is a broken invariant, not a
/// runtime error.
#[cfg(not(target_os = "linux"))]
async fn run_blocking<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .expect("device backend task panicked")
}

/// Enumerates currently attached removable storage devices, one per drive.
#[cfg(target_os = "linux")]
pub async fn scan_storage() -> Result<Vec<StorageDevice>, ScanError> {
    linux::scan_storage().await
}

#[cfg(target_os = "macos")]
pub async fn scan_storage() -> Result<Vec<StorageDevice>, ScanError> {
    run_blocking(macos::scan_storage).await
}

#[cfg(target_os = "windows")]
pub async fn scan_storage() -> Result<Vec<StorageDevice>, ScanError> {
    run_blocking(windows::scan_storage).await
}

/// Mounts `device`'s filesystem, returning the resulting mount point.
#[cfg(target_os = "linux")]
pub async fn mount(device: &StorageDevice, options: &MountOptions) -> Result<PathBuf, MountError> {
    linux::mount(device, options).await
}

#[cfg(target_os = "macos")]
pub async fn mount(device: &StorageDevice, options: &MountOptions) -> Result<PathBuf, MountError> {
    let device = device.clone();
    let options = options.clone();
    run_blocking(move || macos::mount(&device, &options)).await
}

#[cfg(target_os = "windows")]
pub async fn mount(device: &StorageDevice, options: &MountOptions) -> Result<PathBuf, MountError> {
    let device = device.clone();
    let options = options.clone();
    run_blocking(move || windows::mount(&device, &options)).await
}

/// Unmounts `device`'s filesystem.
#[cfg(target_os = "linux")]
pub async fn unmount(device: &StorageDevice, options: &UnmountOptions) -> Result<(), UnmountError> {
    linux::unmount(device, options).await
}

#[cfg(target_os = "macos")]
pub async fn unmount(device: &StorageDevice, options: &UnmountOptions) -> Result<(), UnmountError> {
    let device = device.clone();
    let options = options.clone();
    run_blocking(move || macos::unmount(&device, &options)).await
}

#[cfg(target_os = "windows")]
pub async fn unmount(device: &StorageDevice, options: &UnmountOptions) -> Result<(), UnmountError> {
    let device = device.clone();
    let options = options.clone();
    run_blocking(move || windows::unmount(&device, &options)).await
}

/// Streams [`DeviceEvent`]s into `tx`. The returned future stays pending for the
/// watcher's lifetime, so every platform drives it as one long-lived task.
/// Dropping the future stops the watcher.
#[cfg(target_os = "linux")]
pub async fn watch(tx: UnboundedSender<DeviceEvent>) -> Result<(), WatchError> {
    linux::watch(tx).await
}

#[cfg(target_os = "macos")]
pub async fn watch(tx: UnboundedSender<DeviceEvent>) -> Result<(), WatchError> {
    macos::watch(tx).await
}

#[cfg(target_os = "windows")]
pub async fn watch(tx: UnboundedSender<DeviceEvent>) -> Result<(), WatchError> {
    windows::watch(tx).await;
    Ok(())
}
