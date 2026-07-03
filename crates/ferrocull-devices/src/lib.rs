#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
pub mod scanner;
#[cfg(target_os = "windows")]
mod windows;

use std::{
    io,
    path::{Path, PathBuf},
};

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

#[derive(Debug, Clone)]
pub struct StorageDevice {
    pub name: String,
    pub mount_point: Option<PathBuf>,
    pub device_path: PathBuf,
    pub total_bytes: Option<u64>,
    pub used_bytes: Option<u64>,
    #[cfg(target_os = "linux")]
    pub object_path: String,
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
            Self::Storage(s) => s.mount_point.as_deref().unwrap_or(&s.device_path),
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

#[derive(Default, serde::Serialize)]
#[cfg_attr(target_os = "linux", derive(zbus::zvariant::Type))]
#[cfg_attr(target_os = "linux", zvariant(signature = "a{sv}"))]
pub struct MountOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fstype: Option<FsType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<String>,
}

#[derive(Default, serde::Serialize)]
#[cfg_attr(target_os = "linux", derive(zbus::zvariant::Type))]
#[cfg_attr(target_os = "linux", zvariant(signature = "a{sv}"))]
pub struct UnmountOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
}

#[derive(serde::Serialize)]
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
pub use linux::{mount, scan_cameras, scan_storage, unmount, watch};
#[cfg(target_os = "macos")]
pub use macos::{mount, scan_cameras, scan_storage, unmount, watch};
pub use scanner::{ScannedFile, scan_camera_folders, scan_directory};
#[cfg(target_os = "windows")]
pub use windows::{mount, scan_cameras, scan_storage, unmount, watch};
