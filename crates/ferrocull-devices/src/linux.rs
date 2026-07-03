use std::{
    collections::HashMap,
    ffi::OsStr,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use futures_util::StreamExt;
use gphoto2::Context;
use tokio::sync::{
    OnceCell,
    mpsc::{UnboundedSender, error::SendError},
};
use zbus::{
    Connection, MatchRule, MessageStream,
    fdo::ObjectManagerProxy,
    zvariant::{ObjectPath, OwnedObjectPath, OwnedValue},
};

use crate::{
    Camera, DeviceEvent, MountError, MountOptions, ScanError, StorageDevice, UnmountError,
    UnmountOptions, WatchError,
};

static CONNECTION: OnceCell<Connection> = OnceCell::const_new();

async fn connection() -> zbus::Result<&'static Connection> {
    CONNECTION.get_or_try_init(Connection::system).await
}

#[zbus::proxy(
    interface = "org.freedesktop.UDisks2.Filesystem",
    default_service = "org.freedesktop.UDisks2"
)]
trait Filesystem {
    fn mount(&self, options: &MountOptions) -> zbus::Result<String>;
    fn unmount(&self, options: &UnmountOptions) -> zbus::Result<()>;
    #[zbus(property)]
    fn mount_points(&self) -> zbus::Result<Vec<Vec<u8>>>;
}

#[zbus::proxy(
    interface = "org.freedesktop.UDisks2.Block",
    default_service = "org.freedesktop.UDisks2"
)]
trait Block {
    #[zbus(property)]
    fn preferred_device(&self) -> zbus::Result<Vec<u8>>;
    #[zbus(property)]
    fn drive(&self) -> zbus::Result<OwnedObjectPath>;
    #[zbus(property)]
    fn id_label(&self) -> zbus::Result<String>;
}

#[zbus::proxy(
    interface = "org.freedesktop.UDisks2.Drive",
    default_service = "org.freedesktop.UDisks2"
)]
trait Drive {
    #[zbus(property)]
    fn removable(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn connection_bus(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn model(&self) -> zbus::Result<String>;
}

const UDISKS2_SERVICE: &str = "org.freedesktop.UDisks2";
const BLOCK_IFACE: &str = "org.freedesktop.UDisks2.Block";
const FS_IFACE: &str = "org.freedesktop.UDisks2.Filesystem";
const DRIVE_IFACE: &str = "org.freedesktop.UDisks2.Drive";

fn bytes_to_path(raw: &[u8]) -> PathBuf {
    // D-Bus byte arrays are null-terminated
    let trimmed = raw.strip_suffix(&[0]).unwrap_or(raw);
    PathBuf::from(OsStr::from_bytes(trimmed))
}

fn device_name(label: Option<&str>, drive_model: Option<&str>, device_path: &Path) -> String {
    label.or(drive_model).map_or_else(
        || {
            device_path.file_name().map_or_else(
                || "Unknown".to_owned(),
                |s| s.to_string_lossy().into_owned(),
            )
        },
        str::to_owned,
    )
}

//
// Each struct is parsed once from a HashMap<String, OwnedValue> removed
// from the ManagedObjects response. Fields are plain Option<T> — no
// hidden mutation, no string keys at access sites.

type Props = HashMap<String, OwnedValue>;

fn remove_as<T: TryFrom<OwnedValue>>(props: &mut Props, key: &str) -> Option<T> {
    T::try_from(props.remove(key)?).ok()
}

fn remove_nonempty_string(props: &mut Props, key: &str) -> Option<String> {
    let s: String = remove_as(props, key)?;
    if s.is_empty() { None } else { Some(s) }
}

struct BlockProps {
    preferred_device: Option<PathBuf>,
    drive: Option<OwnedObjectPath>,
    id_label: Option<String>,
}

impl From<Props> for BlockProps {
    fn from(mut p: Props) -> Self {
        Self {
            preferred_device: remove_as::<Vec<u8>>(&mut p, "PreferredDevice")
                .map(|raw| bytes_to_path(&raw)),
            drive: remove_as(&mut p, "Drive"),
            id_label: remove_nonempty_string(&mut p, "IdLabel"),
        }
    }
}

struct FilesystemProps {
    mount_point: Option<PathBuf>,
}

impl From<Props> for FilesystemProps {
    fn from(mut p: Props) -> Self {
        let mount_point = remove_as::<Vec<Vec<u8>>>(&mut p, "MountPoints")
            .and_then(|all| all.first().map(|mp| bytes_to_path(mp)));
        Self { mount_point }
    }
}

struct DriveProps {
    removable: bool,
    is_usb: bool,
    /// Whether media is actually present. `false` for an empty card-reader slot,
    /// which exposes a 0-byte block device we must not surface as a source.
    media_available: bool,
    model: Option<String>,
}

impl TryFrom<Props> for DriveProps {
    type Error = ScanError;

    fn try_from(mut p: Props) -> Result<Self, Self::Error> {
        let removable = remove_as::<bool>(&mut p, "Removable").ok_or_else(|| {
            ScanError::Backend("UDisks2 Drive missing 'Removable' property".to_owned())
        })?;
        let is_usb = remove_as::<String>(&mut p, "ConnectionBus")
            .is_some_and(|bus| bus.eq_ignore_ascii_case("usb"));
        let media_available = remove_as::<bool>(&mut p, "MediaAvailable").ok_or_else(|| {
            ScanError::Backend("UDisks2 Drive missing 'MediaAvailable' property".to_owned())
        })?;
        let model = remove_nonempty_string(&mut p, "Model");
        Ok(Self {
            removable,
            is_usb,
            media_available,
            model,
        })
    }
}

impl From<zbus::Error> for ScanError {
    fn from(e: zbus::Error) -> Self {
        Self::Backend(e.to_string())
    }
}

impl From<zbus::fdo::Error> for ScanError {
    fn from(e: zbus::fdo::Error) -> Self {
        Self::Backend(e.to_string())
    }
}

pub(crate) async fn scan_storage() -> Result<Vec<StorageDevice>, ScanError> {
    let conn = connection().await?;
    let om = ObjectManagerProxy::builder(conn)
        .destination(UDISKS2_SERVICE)?
        .path("/org/freedesktop/UDisks2")?
        .build()
        .await?;

    let objects = om.get_managed_objects().await?;

    let (drive_entries, block_entries): (Vec<_>, Vec<_>) = objects
        .into_iter()
        .partition(|(_, ifaces)| ifaces.contains_key(DRIVE_IFACE));

    let drives: HashMap<OwnedObjectPath, DriveProps> = drive_entries
        .into_iter()
        .map(|(path, mut ifaces)| {
            let raw = ifaces
                .remove(DRIVE_IFACE)
                .expect("partitioned by DRIVE_IFACE presence");
            DriveProps::try_from(raw).map(|props| (path, props))
        })
        .collect::<Result<HashMap<_, _>, ScanError>>()?;

    // A physical card exposes several UDisks2 Block objects — the whole-disk
    // node plus one per partition — all referencing the same Drive. Collapse
    // them to one StorageDevice per Drive, keeping the most useful block: a
    // mounted filesystem beats an unmounted one beats the bare disk. The
    // survivor carries the mount point, capacity, and the object path used for
    // mount/unmount.
    let mut by_drive: HashMap<OwnedObjectPath, (u8, StorageDevice)> = HashMap::new();
    for (path, mut interfaces) in block_entries {
        let Some(block_raw) = interfaces.remove(BLOCK_IFACE) else {
            continue;
        };
        let block = BlockProps::from(block_raw);
        let fs = interfaces.remove(FS_IFACE).map(FilesystemProps::from);
        let has_fs = fs.is_some();
        let mount_point = fs.and_then(|fs| fs.mount_point);

        let Some(drive_path) = block.drive else {
            continue;
        };
        let Some(drive) = drives.get(&drive_path) else {
            continue;
        };
        if !drive.removable && !drive.is_usb {
            continue;
        }
        if !drive.media_available {
            continue; // empty card-reader slot — no media inserted
        }
        let Some(device_path) = block.preferred_device else {
            continue;
        };

        let name = device_name(
            block.id_label.as_deref(),
            drive.model.as_deref(),
            &device_path,
        );
        let (total_bytes, used_bytes) = mount_point
            .as_deref()
            .and_then(crate::statvfs::disk_space)
            .unzip();
        let priority = block_priority(mount_point.is_some(), has_fs);
        let candidate = StorageDevice {
            name,
            mount_point,
            device_path,
            total_bytes,
            used_bytes,
            object_path: path.to_string(),
        };

        // Higher priority wins; ties break on the lower device path so the
        // first partition (sdc1) is preferred over later ones deterministically.
        let better = match by_drive.get(&drive_path) {
            Some((best, existing)) => {
                priority > *best
                    || (priority == *best && candidate.device_path < existing.device_path)
            }
            None => true,
        };
        if better {
            by_drive.insert(drive_path, (priority, candidate));
        }
    }

    Ok(by_drive.into_values().map(|(_, device)| device).collect())
}

/// Ranks a block as a candidate to represent its drive: a mounted filesystem
/// outranks an unmounted one, which outranks a bare block (whole-disk node or
/// unformatted partition).
fn block_priority(mounted: bool, has_filesystem: bool) -> u8 {
    match (mounted, has_filesystem) {
        (true, _) => 2,
        (false, true) => 1,
        (false, false) => 0,
    }
}

async fn filesystem_proxy(object_path: &str) -> Result<FilesystemProxy<'static>, zbus::Error> {
    let conn = connection().await?;
    // Own the path so the proxy isn't tied to the borrowed `object_path`.
    let path = OwnedObjectPath::try_from(object_path)?;
    FilesystemProxy::builder(conn)
        .destination(UDISKS2_SERVICE)?
        .path(ObjectPath::from(path))?
        .build()
        .await
}

pub(crate) async fn mount(
    device: &StorageDevice,
    options: &MountOptions,
) -> Result<PathBuf, MountError> {
    let proxy = filesystem_proxy(&device.object_path)
        .await
        .map_err(|e| MountError::Failed(e.to_string()))?;

    proxy
        .mount(options)
        .await
        .map(PathBuf::from)
        .map_err(|e| classify_mount_error(&e))
}

fn classify_mount_error(e: &zbus::Error) -> MountError {
    if let zbus::Error::MethodError(name, _, _) = e {
        let name = name.as_str();
        if name.ends_with(".AlreadyMounted") {
            return MountError::AlreadyMounted;
        }
        if is_not_authorized(name) {
            return MountError::PermissionDenied;
        }
    }
    MountError::Failed(e.to_string())
}

pub(crate) async fn unmount(
    device: &StorageDevice,
    options: &UnmountOptions,
) -> Result<(), UnmountError> {
    let proxy = filesystem_proxy(&device.object_path)
        .await
        .map_err(|e| UnmountError::Failed(e.to_string()))?;

    proxy
        .unmount(options)
        .await
        .map_err(|e| classify_unmount_error(&e))
}

fn classify_unmount_error(e: &zbus::Error) -> UnmountError {
    if let zbus::Error::MethodError(name, _, _) = e {
        let name = name.as_str();
        if name.ends_with(".NotMounted") {
            return UnmountError::NotMounted;
        }
        if name.ends_with(".DeviceBusy") {
            return UnmountError::Busy;
        }
        if is_not_authorized(name) {
            return UnmountError::PermissionDenied;
        }
    }
    UnmountError::Failed(e.to_string())
}

fn is_not_authorized(error_name: &str) -> bool {
    error_name.ends_with(".NotAuthorized")
        || error_name.ends_with(".NotAuthorizedCanObtain")
        || error_name.ends_with(".NotAuthorizedDismissed")
}

#[must_use]
pub fn scan_cameras() -> Vec<Camera> {
    let cameras = match Context::new().and_then(|ctx| ctx.list_cameras().wait()) {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!("failed to scan cameras: {e}");
            return Vec::new();
        }
    };

    cameras
        .map(|desc| Camera {
            name: desc.model,
            port: desc.port,
        })
        .collect()
}

struct KnownDevice {
    device_path: PathBuf,
    mounted: bool,
}

/// If `block` belongs to a removable/USB drive, insert it into `known` and
/// return the corresponding `StorageDevice` so the caller can emit `Inserted`.
fn register_block(
    block: BlockProps,
    obj_path: &OwnedObjectPath,
    drives: &HashMap<OwnedObjectPath, DriveProps>,
    known: &mut HashMap<OwnedObjectPath, KnownDevice>,
) -> Option<StorageDevice> {
    let drive = block
        .drive
        .as_ref()
        .and_then(|d| drives.get(d))
        .filter(|d| (d.removable || d.is_usb) && d.media_available)?;
    let device_path = block.preferred_device?;
    let name = device_name(
        block.id_label.as_deref(),
        drive.model.as_deref(),
        &device_path,
    );
    known.insert(
        obj_path.clone(),
        KnownDevice {
            device_path: device_path.clone(),
            mounted: false,
        },
    );
    Some(StorageDevice {
        name,
        mount_point: None,
        device_path,
        total_bytes: None,
        used_bytes: None,
        object_path: obj_path.to_string(),
    })
}

fn emit_mounted(
    known: &mut HashMap<OwnedObjectPath, KnownDevice>,
    obj_path: &OwnedObjectPath,
    mount_point: PathBuf,
    tx: &UnboundedSender<DeviceEvent>,
) -> Result<(), Box<SendError<DeviceEvent>>> {
    let Some(kd) = known.get_mut(obj_path).filter(|kd| !kd.mounted) else {
        return Ok(());
    };
    kd.mounted = true;
    let (total_bytes, used_bytes) = crate::statvfs::disk_space(&mount_point).unzip();
    tx.send(DeviceEvent::Mounted {
        device_path: kd.device_path.clone(),
        mount_point,
        total_bytes,
        used_bytes,
    })
    .map_err(Box::new)
}

fn emit_unmounted(
    known: &mut HashMap<OwnedObjectPath, KnownDevice>,
    obj_path: &OwnedObjectPath,
    tx: &UnboundedSender<DeviceEvent>,
) -> Result<(), Box<SendError<DeviceEvent>>> {
    let Some(kd) = known.get_mut(obj_path).filter(|kd| kd.mounted) else {
        return Ok(());
    };
    kd.mounted = false;
    tx.send(DeviceEvent::Unmounted {
        device_path: kd.device_path.clone(),
    })
    .map_err(Box::new)
}

/// Streams device events from `UDisks2`.
///
/// Emits one `Inserted`/`Removed` per `UDisks2` Block — the whole-disk node plus
/// one per partition — so a multi-partition card produces several events. The UI
/// consumes these only as change triggers: each event drives a full
/// [`scan_storage`] rescan, which collapses a card to one `StorageDevice` per
/// Drive, so the per-Block granularity here is harmless.
pub(crate) async fn watch(tx: UnboundedSender<DeviceEvent>) -> Result<(), WatchError> {
    watch_inner(tx)
        .await
        .map_err(|e| WatchError::Backend(e.to_string()))
}

async fn watch_inner(tx: UnboundedSender<DeviceEvent>) -> zbus::Result<()> {
    let conn = connection().await?;
    let om = ObjectManagerProxy::builder(conn)
        .destination(UDISKS2_SERVICE)?
        .path("/org/freedesktop/UDisks2")?
        .build()
        .await?;

    let mut added_stream = om.receive_interfaces_added().await?;
    let mut removed_stream = om.receive_interfaces_removed().await?;

    // Subscribe to PropertiesChanged *before* fetching managed objects so no
    // mount/unmount signals are lost during the snapshot.
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(UDISKS2_SERVICE)?
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .path_namespace("/org/freedesktop/UDisks2/block_devices")?
        .build();
    let mut props_stream = MessageStream::for_match_rule(rule, conn, None).await?;

    // Seed drives and known devices from current state so events for
    // already-present devices aren't silently dropped.
    let mut drives: HashMap<OwnedObjectPath, DriveProps> = HashMap::new();
    let mut known: HashMap<OwnedObjectPath, KnownDevice> = HashMap::new();

    let objects = om.get_managed_objects().await?;
    let (drive_entries, block_entries): (Vec<_>, Vec<_>) = objects
        .into_iter()
        .partition(|(_, ifaces)| ifaces.contains_key(DRIVE_IFACE));

    for (path, mut ifaces) in drive_entries {
        if let Some(raw) = ifaces.remove(DRIVE_IFACE) {
            match DriveProps::try_from(raw) {
                Ok(props) => {
                    drives.insert(path, props);
                }
                Err(e) => {
                    tracing::error!(?path, %e, "skipping malformed UDisks2 drive");
                }
            }
        }
    }
    for (path, mut ifaces) in block_entries {
        if let Some(raw) = ifaces.remove(BLOCK_IFACE) {
            register_block(BlockProps::from(raw), &path, &drives, &mut known);
        }
        let mounted = ifaces
            .remove(FS_IFACE)
            .map(FilesystemProps::from)
            .and_then(|fs| fs.mount_point)
            .is_some();
        if mounted && let Some(kd) = known.get_mut(&path) {
            kd.mounted = true;
        }
    }

    loop {
        tokio::select! {
            Some(signal) = added_stream.next() => {
                let Ok((obj_path, mut interfaces)): Result<(OwnedObjectPath, HashMap<String, Props>), _> =
                    signal.message().body().deserialize_unchecked()
                else {
                    continue;
                };

                if let Some(drive_raw) = interfaces.remove(DRIVE_IFACE) {
                    match DriveProps::try_from(drive_raw) {
                        Ok(props) => {
                            drives.insert(obj_path.clone(), props);
                        }
                        Err(e) => {
                            tracing::error!(?obj_path, %e, "skipping malformed UDisks2 drive");
                        }
                    }
                }

                if let Some(block_raw) = interfaces.remove(BLOCK_IFACE) {
                    // Skip if already seeded from the snapshot — re-registering
                    // would reset the mounted flag.
                    if !known.contains_key(&obj_path)
                        && let Some(storage) = register_block(
                            BlockProps::from(block_raw),
                            &obj_path,
                            &drives,
                            &mut known,
                        )
                        && tx.send(DeviceEvent::Inserted(storage)).is_err()
                    {
                        return Ok(());
                    }
                }

                if let Some(mount_point) = interfaces
                    .remove(FS_IFACE)
                    .map(FilesystemProps::from)
                    .and_then(|fs| fs.mount_point)
                    && emit_mounted(&mut known, &obj_path, mount_point, &tx).is_err()
                {
                    return Ok(());
                }
            }
            Some(signal) = removed_stream.next() => {
                let Ok((obj_path, removed_ifaces)): Result<(OwnedObjectPath, Vec<String>), _> =
                    signal.message().body().deserialize_unchecked()
                else {
                    continue;
                };

                let has = |target: &str| removed_ifaces.iter().any(|s| s == target);

                if has(DRIVE_IFACE) {
                    drives.remove(&obj_path);
                }

                // Drop from `known` only on Block removal. Keeping the entry
                // on FS-only removal lets us re-detect the device if a new
                // filesystem is created (e.g. reformat).
                if has(BLOCK_IFACE) {
                    if let Some(kd) = known.remove(&obj_path) {
                        if kd.mounted {
                            let ev = DeviceEvent::Unmounted { device_path: kd.device_path.clone() };
                            if tx.send(ev).is_err() {
                                return Ok(());
                            }
                        }
                        let ev = DeviceEvent::Removed { device_path: kd.device_path };
                        if tx.send(ev).is_err() {
                            return Ok(());
                        }
                    }
                } else if has(FS_IFACE)
                    && emit_unmounted(&mut known, &obj_path, &tx).is_err()
                {
                    return Ok(());
                }
            }
            Some(result) = props_stream.next() => {
                let msg = match result {
                    Ok(msg) => msg,
                    Err(e) => {
                        tracing::debug!("PropertiesChanged stream error: {e}");
                        continue;
                    }
                };
                let Ok((interface_name, mut changed_props, _invalidated)):
                    Result<(String, Props, Vec<String>), _> =
                    msg.body().deserialize_unchecked()
                else {
                    continue;
                };
                if interface_name != FS_IFACE {
                    continue;
                }

                let Some(obj_path) = msg.header().path().map(|p| OwnedObjectPath::from(p.clone()))
                else {
                    continue;
                };

                let Some(mount_points) =
                    remove_as::<Vec<Vec<u8>>>(&mut changed_props, "MountPoints")
                else {
                    continue;
                };
                let send_result = match mount_points.first() {
                    Some(mp) => emit_mounted(&mut known, &obj_path, bytes_to_path(mp), &tx),
                    None => emit_unmounted(&mut known, &obj_path, &tx),
                };
                if send_result.is_err() {
                    return Ok(());
                }
            }
            else => {
                tracing::warn!("all UDisks2 streams ended, stopping watch");
                return Ok(());
            }
        }
    }
}
