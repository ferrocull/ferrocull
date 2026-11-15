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
    Connection, MatchRule, MessageStream, OwnedObjectPath, OwnedVariant,
    fdo::{ObjectManagerProxy, PropertiesChanged},
    message::Message,
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
    fn mount(&self, options: &MountOptions) -> Result<String, Udisks2Error>;
    fn unmount(&self, options: &UnmountOptions) -> Result<(), Udisks2Error>;
}

/// The `org.freedesktop.UDisks2.Error` replies the backend tells apart. Any
/// other failure, on the bus or from `UDisks2`, is carried by `ZBus`.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.UDisks2.Error")]
enum Udisks2Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    AlreadyMounted(String),
    NotMounted(String),
    DeviceBusy(String),
    NotAuthorized(String),
    NotAuthorizedCanObtain(String),
    NotAuthorizedDismissed(String),
}

impl From<Udisks2Error> for MountError {
    fn from(error: Udisks2Error) -> Self {
        match error {
            Udisks2Error::AlreadyMounted(_) => Self::AlreadyMounted,
            Udisks2Error::NotAuthorized(_)
            | Udisks2Error::NotAuthorizedCanObtain(_)
            | Udisks2Error::NotAuthorizedDismissed(_) => Self::PermissionDenied,
            Udisks2Error::ZBus(error) => Self::Failed(error.to_string()),
            Udisks2Error::NotMounted(_) | Udisks2Error::DeviceBusy(_) => {
                Self::Failed(error.to_string())
            }
        }
    }
}

impl From<Udisks2Error> for UnmountError {
    fn from(error: Udisks2Error) -> Self {
        match error {
            Udisks2Error::NotMounted(_) => Self::NotMounted,
            Udisks2Error::DeviceBusy(_) => Self::Busy,
            Udisks2Error::NotAuthorized(_)
            | Udisks2Error::NotAuthorizedCanObtain(_)
            | Udisks2Error::NotAuthorizedDismissed(_) => Self::PermissionDenied,
            Udisks2Error::ZBus(error) => Self::Failed(error.to_string()),
            Udisks2Error::AlreadyMounted(_) => Self::Failed(error.to_string()),
        }
    }
}

const UDISKS2_SERVICE: &str = "org.freedesktop.UDisks2";
const UDISKS2_PATH: &str = "/org/freedesktop/UDisks2";
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

// Each struct is parsed once from a HashMap<String, OwnedVariant> removed
// from the ManagedObjects response. Fields are plain Option<T>, with no
// hidden mutation and no string keys at access sites.

type Props = HashMap<String, OwnedVariant>;

fn remove_as<T: TryFrom<OwnedVariant>>(props: &mut Props, key: &str) -> Option<T> {
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

async fn object_manager(conn: &Connection) -> zbus::Result<ObjectManagerProxy<'_>> {
    ObjectManagerProxy::builder(conn)
        .destination(UDISKS2_SERVICE)
        .path(UDISKS2_PATH)
        .build()
        .await
}

pub(crate) async fn scan_storage() -> Result<Vec<StorageDevice>, ScanError> {
    let conn = connection().await?;
    let objects = object_manager(conn).await?.get_managed_objects().await?;

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
            filesystem: mount_point
                .map_or(crate::Filesystem::Unmounted, crate::Filesystem::Mounted),
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

async fn filesystem_proxy(object_path: &str) -> zbus::Result<FilesystemProxy<'static>> {
    FilesystemProxy::builder(connection().await?)
        .path(object_path.to_owned())
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
        .map_err(MountError::from)
}

pub(crate) async fn unmount(
    device: &StorageDevice,
    options: &UnmountOptions,
) -> Result<(), UnmountError> {
    let proxy = filesystem_proxy(&device.object_path)
        .await
        .map_err(|e| UnmountError::Failed(e.to_string()))?;

    proxy.unmount(options).await.map_err(UnmountError::from)
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
        filesystem: crate::Filesystem::Unmounted,
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

/// Decodes the body of an `InterfacesAdded` signal into owned property maps.
fn added_interfaces(message: &Message) -> zbus::Result<(OwnedObjectPath, HashMap<String, Props>)> {
    message.body().deserialize()
}

/// The `MountPoints` a `PropertiesChanged` signal reports for a filesystem, or
/// `None` when the signal is about another interface or property.
fn changed_mount_points(message: Message) -> Option<Vec<Vec<u8>>> {
    let signal = PropertiesChanged::from_message(message)?;
    let args = signal.args().ok()?;
    if args.interface_name() != FS_IFACE {
        return None;
    }
    args.changed_properties()
        .get("MountPoints")?
        .try_clone()
        .ok()?
        .downcast()
        .ok()
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
    let om = object_manager(conn).await?;

    let mut added_stream = om.receive_interfaces_added().await?;
    let mut removed_stream = om.receive_interfaces_removed().await?;

    // Subscribe to PropertiesChanged *before* fetching managed objects so no
    // mount/unmount signals are lost during the snapshot.
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(UDISKS2_SERVICE)
        .interface("org.freedesktop.DBus.Properties")
        .member("PropertiesChanged")
        .path_namespace("/org/freedesktop/UDisks2/block_devices")
        .build()?;
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
                let Ok((obj_path, mut interfaces)) = added_interfaces(signal.message()) else {
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
                let Ok(args) = signal.args() else {
                    continue;
                };
                let obj_path = OwnedObjectPath::from(args.object_path().clone());
                let has = |target: &str| args.interfaces().iter().any(|name| name == target);

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
                let Some(obj_path) = msg.header().path().map(|p| OwnedObjectPath::from(p.clone()))
                else {
                    continue;
                };
                let Some(mount_points) = changed_mount_points(msg) else {
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use zbus::{
        OwnedObjectPath, OwnedVariant, Variant,
        message::Message,
        wire::{DynamicType, LE, serialized::Context, to_bytes},
    };

    use super::{BLOCK_IFACE, FS_IFACE, Udisks2Error, added_interfaces, changed_mount_points};
    use crate::{FsType, MountError, MountOptions, UnmountError, UnmountOptions};

    const BLOCK_PATH: &str = "/org/freedesktop/UDisks2/block_devices/sdc1";

    /// The error `UDisks2` would reply with to a `Filesystem` method call.
    fn error_reply(name: &str) -> Udisks2Error {
        let call = Message::method_call(BLOCK_PATH, "Mount")
            .sender(":1.1")
            .build(&())
            .unwrap();
        let reply = Message::error(&call.header(), name)
            .build(&"refused")
            .unwrap();
        Udisks2Error::from(zbus::Error::from(reply))
    }

    fn properties_changed(interface: &str, property: &str) -> Message {
        let changed =
            HashMap::from([(property, Variant::from(vec![b"/run/media/card\0".to_vec()]))]);
        Message::signal(
            BLOCK_PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
        )
        .build(&(interface, changed, Vec::<&str>::new()))
        .unwrap()
    }

    #[test]
    fn authorization_replies_are_permission_denied() {
        for name in [
            "NotAuthorized",
            "NotAuthorizedCanObtain",
            "NotAuthorizedDismissed",
        ] {
            let name = format!("org.freedesktop.UDisks2.Error.{name}");
            assert!(matches!(
                MountError::from(error_reply(&name)),
                MountError::PermissionDenied
            ));
            assert!(matches!(
                UnmountError::from(error_reply(&name)),
                UnmountError::PermissionDenied
            ));
        }
    }

    #[test]
    fn mount_state_replies_map_to_their_variants() {
        assert!(matches!(
            MountError::from(error_reply("org.freedesktop.UDisks2.Error.AlreadyMounted")),
            MountError::AlreadyMounted
        ));
        assert!(matches!(
            UnmountError::from(error_reply("org.freedesktop.UDisks2.Error.NotMounted")),
            UnmountError::NotMounted
        ));
        assert!(matches!(
            UnmountError::from(error_reply("org.freedesktop.UDisks2.Error.DeviceBusy")),
            UnmountError::Busy
        ));
    }

    #[test]
    fn other_replies_fail_with_the_error_name_and_detail() {
        for name in [
            "org.freedesktop.UDisks2.Error.Failed",
            "org.freedesktop.UDisks2.Error.DeviceBusy",
        ] {
            let MountError::Failed(message) = MountError::from(error_reply(name)) else {
                panic!("{name} did not map to MountError::Failed");
            };
            assert_eq!(message, format!("{name}: refused"));
        }
    }

    #[test]
    fn interfaces_added_body_decodes_to_owned_properties() {
        let block = HashMap::from([("IdLabel", Variant::from("EOS_DIGITAL"))]);
        let signal = Message::signal(
            "/org/freedesktop/UDisks2",
            "org.freedesktop.DBus.ObjectManager",
            "InterfacesAdded",
        )
        .build(&(
            OwnedObjectPath::try_from(BLOCK_PATH).unwrap(),
            HashMap::from([(BLOCK_IFACE, block)]),
        ))
        .unwrap();

        let (path, mut interfaces) = added_interfaces(&signal).unwrap();

        assert_eq!(path.as_str(), BLOCK_PATH);
        let mut decoded = interfaces.remove(BLOCK_IFACE).unwrap();
        let label = String::try_from(decoded.remove("IdLabel").unwrap()).unwrap();
        assert_eq!(label, "EOS_DIGITAL");
    }

    #[test]
    fn filesystem_mount_points_change_is_reported() {
        let mount_points = changed_mount_points(properties_changed(FS_IFACE, "MountPoints"));

        assert_eq!(mount_points, Some(vec![b"/run/media/card\0".to_vec()]));
    }

    #[test]
    fn unrelated_property_changes_are_ignored() {
        assert_eq!(
            changed_mount_points(properties_changed(BLOCK_IFACE, "MountPoints")),
            None
        );
        assert_eq!(
            changed_mount_points(properties_changed(FS_IFACE, "Size")),
            None
        );
    }

    fn encoded_options<T: serde::Serialize + DynamicType>(
        options: &T,
    ) -> HashMap<String, OwnedVariant> {
        let encoded = to_bytes(Context::new(LE, 0), options).unwrap();
        let (decoded, _) = encoded.deserialize().unwrap();
        decoded
    }

    #[test]
    fn mount_options_encode_as_string_variants() {
        let mut encoded = encoded_options(&MountOptions {
            fstype: Some(FsType::Exfat),
            options: Some("ro".to_owned()),
        });

        let fstype = String::try_from(encoded.remove("fstype").unwrap()).unwrap();
        let options = String::try_from(encoded.remove("options").unwrap()).unwrap();
        assert_eq!(fstype, "exfat");
        assert_eq!(options, "ro");
        assert!(encoded.is_empty());
    }

    #[test]
    fn unmount_force_encodes_as_a_boolean_variant() {
        let mut encoded = encoded_options(&UnmountOptions { force: Some(true) });

        assert!(bool::try_from(encoded.remove("force").unwrap()).unwrap());
        assert!(encoded.is_empty());
    }

    #[test]
    fn default_options_encode_as_an_empty_vardict() {
        assert!(encoded_options(&MountOptions::default()).is_empty());
        assert!(encoded_options(&UnmountOptions::default()).is_empty());
    }
}
