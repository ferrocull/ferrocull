//! `DiskArbitration` bindings: the macOS FFI layer, and the only macOS module in
//! the workspace that contains `unsafe`.
//!
//! `DiskArbitration` answers for every medium `diskarbitrationd` knows about,
//! mounted or not, and reports attachments, detachments and description changes
//! as they happen, so the backend above needs neither a filesystem watch nor a
//! subprocess.

#![expect(
    unsafe_code,
    reason = "the DiskArbitration framework has no safe binding"
)]

use std::{
    ffi::{CStr, CString, c_char, c_void},
    path::PathBuf,
    ptr::NonNull,
    sync::mpsc,
};

use dispatch2::{DispatchQueue, DispatchRetained};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CFURL,
};
use objc2_disk_arbitration::{
    DADisk, DADissenter, DARegisterDiskAppearedCallback, DARegisterDiskDescriptionChangedCallback,
    DARegisterDiskDisappearedCallback, DASession, DAUnregisterCallback,
    kDADiskDescriptionMediaBSDNameKey, kDADiskDescriptionMediaContentKey,
    kDADiskDescriptionMediaEjectableKey, kDADiskDescriptionMediaLeafKey,
    kDADiskDescriptionMediaRemovableKey, kDADiskDescriptionMediaSizeKey,
    kDADiskDescriptionVolumeMountableKey, kDADiskDescriptionVolumeNameKey,
    kDADiskDescriptionVolumePathKey, kDADiskDescriptionWatchVolumePath, kDADiskMountOptionDefault,
    kDADiskUnmountOptionDefault, kDADiskUnmountOptionForce,
};

/// Label of the serial queue `DiskArbitration` delivers callbacks on.
const QUEUE_LABEL: &str = "io.github.ferrocull.diskarb";

/// A snapshot of one disk's `DiskArbitration` description. Every field is the
/// value DA reported for the matching `kDADiskDescription*` key.
#[derive(Debug)]
pub(crate) struct Description {
    pub(crate) bsd_name: String,
    pub(crate) volume_name: Option<String>,
    pub(crate) volume_path: Option<PathBuf>,
    pub(crate) volume_mountable: Option<bool>,
    pub(crate) media_removable: Option<bool>,
    pub(crate) media_ejectable: Option<bool>,
    pub(crate) media_leaf: Option<bool>,
    pub(crate) media_content: Option<String>,
    pub(crate) media_size: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("cannot open a DiskArbitration session")]
    NoSession,
    #[error("DiskArbitration completed the request on {0} but described nothing")]
    Undescribed(String),
    #[error("DiskArbitration refused with status {0}")]
    Dissented(i32),
}

/// What a running [`Watcher`] reports.
pub(crate) enum Event {
    /// A disk was attached, or was already attached when the watch started.
    Appeared(Description),
    /// A watched key of a disk's description changed.
    Changed(Description),
    /// A disk was detached.
    Disappeared { bsd_name: String },
}

/// The boxed closure a running watch dispatches its events to.
type Handler = Box<dyn Fn(Event) + Send>;

/// What a mount or an unmount request reports: the description of the disk once
/// the request landed, or the status `DiskArbitration` dissented it with.
type Completion = Result<Option<Description>, i32>;

/// Descriptions of the named BSD disks, skipping any DA cannot describe.
///
/// The session stays unscheduled: `DADiskCopyDescription` contacts
/// `diskarbitrationd` synchronously and answers on the calling thread, so it
/// needs neither a run loop nor a dispatch queue.
pub(crate) fn describe(bsd_names: &[String]) -> Result<Vec<Description>, Error> {
    // SAFETY: the call takes only an allocator, and `None` asks for the
    // default one.
    let session = unsafe { DASession::new(None) }.ok_or(Error::NoSession)?;

    Ok(bsd_names
        .iter()
        .filter_map(|bsd_name| description(&disk(&session, bsd_name)))
        .collect())
}

/// Mounts the named disk and returns its description once the mount lands.
///
/// `DiskArbitration` picks the mount point and the filesystem driver itself.
pub(crate) fn mount(bsd_name: &str) -> Result<Description, Error> {
    let issue = |disk: &DADisk, context: *mut c_void| {
        // SAFETY: `completed` has the signature DADiskMountCallback requires,
        // reads its context back as the `mpsc::Sender<Completion>` `request`
        // hands it, and that sender outlives the request.
        unsafe {
            disk.mount(None, kDADiskMountOptionDefault, Some(completed), context);
        }
    };

    match request(bsd_name, issue)? {
        Ok(Some(description)) => Ok(description),
        Ok(None) => Err(Error::Undescribed(bsd_name.to_owned())),
        Err(status) => Err(Error::Dissented(status)),
    }
}

/// Unmounts the named disk.
pub(crate) fn unmount(bsd_name: &str, force: bool) -> Result<(), Error> {
    let options = if force {
        kDADiskUnmountOptionForce
    } else {
        kDADiskUnmountOptionDefault
    };

    let issue = |disk: &DADisk, context: *mut c_void| {
        // SAFETY: `completed` has the signature DADiskUnmountCallback requires,
        // reads its context back as the `mpsc::Sender<Completion>` `request`
        // hands it, and that sender outlives the request.
        unsafe { disk.unmount(options, Some(completed), context) };
    };

    // An unmount leaves nothing to describe, so only the status matters.
    match request(bsd_name, issue)? {
        Ok(_) => Ok(()),
        Err(status) => Err(Error::Dissented(status)),
    }
}

/// A live `DiskArbitration` watch. Dropping it stops the callbacks.
pub(crate) struct Watcher {
    session: CFRetained<DASession>,
    queue: DispatchRetained<DispatchQueue>,
    context: *mut c_void,
}

// SAFETY: DiskArbitration has no thread affinity, and callback delivery is
// routed by the dispatch queue the session is scheduled on rather than by the
// thread that created the session. The handle is moved into the watching task
// rather than shared, so no two threads ever touch it at once.
unsafe impl Send for Watcher {}

impl Watcher {
    /// Starts a watch that calls `handler` on the `DiskArbitration` queue for
    /// every disk that appears, changes, or disappears. Registration replays
    /// every attached disk as an [`Event::Appeared`], so the caller needs no
    /// baseline scan.
    pub(crate) fn start(handler: impl Fn(Event) + Send + 'static) -> Result<Self, Error> {
        // SAFETY: the call takes only an allocator, and `None` asks for the
        // default one.
        let session = unsafe { DASession::new(None) }.ok_or(Error::NoSession)?;
        let queue = DispatchQueue::new(QUEUE_LABEL, None);
        let handler: Handler = Box::new(handler);
        let context = Box::into_raw(Box::new(handler)).cast::<c_void>();

        // SAFETY: `disk_appeared` has the signature DADiskAppearedCallback
        // requires, and `context` is the handler box it reads back, which
        // `Drop` frees only after unregistering.
        unsafe { DARegisterDiskAppearedCallback(&session, None, Some(disk_appeared), context) };

        // SAFETY: `disk_disappeared` has the signature
        // DADiskDisappearedCallback requires, and `context` is the handler box
        // it reads back, which `Drop` frees only after unregistering.
        unsafe {
            DARegisterDiskDisappearedCallback(&session, None, Some(disk_disappeared), context);
        }

        // SAFETY: the volume path array DiskArbitration exports is the key set
        // the callback watches, `disk_changed` has the signature
        // DADiskDescriptionChangedCallback requires, and `context` is the
        // handler box it reads back, which `Drop` frees only after
        // unregistering.
        unsafe {
            DARegisterDiskDescriptionChangedCallback(
                &session,
                None,
                Some(watched_keys()),
                Some(disk_changed),
                context,
            );
        }

        // Scheduling comes last so no callback can fire before every
        // registration is in place.
        //
        // SAFETY: `queue` is a serial queue this watch owns and keeps alive for
        // as long as the session is scheduled on it.
        unsafe { session.set_dispatch_queue(Some(&queue)) };

        Ok(Self {
            session,
            queue,
            context,
        })
    }

    /// Ends one of the registrations [`Watcher::start`] made, naming it by the
    /// callback it registered.
    fn unregister(&self, callback: *const ()) {
        let callback = NonNull::new(callback.cast_mut().cast::<c_void>())
            .expect("null callback function pointer");

        // SAFETY: `callback` and `self.context` are the pair `start`
        // registered, so the unregistration names a registration that exists.
        unsafe { DAUnregisterCallback(&self.session, callback, self.context) };
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        // SAFETY: unscheduling the session stops DiskArbitration from queuing
        // any further callback.
        unsafe { self.session.set_dispatch_queue(None) };

        let appeared: unsafe extern "C-unwind" fn(NonNull<DADisk>, *mut c_void) = disk_appeared;
        let disappeared: unsafe extern "C-unwind" fn(NonNull<DADisk>, *mut c_void) =
            disk_disappeared;
        let changed: unsafe extern "C-unwind" fn(NonNull<DADisk>, NonNull<CFArray>, *mut c_void) =
            disk_changed;

        self.unregister(appeared as *const ());
        self.unregister(disappeared as *const ());
        self.unregister(changed as *const ());

        // A callback already dispatched runs to completion before this empty
        // block does, so once the barrier returns nothing is reading the
        // handler and freeing it is sound.
        self.queue.exec_sync(|| {});

        // SAFETY: the box was built by `start`, has not been freed since, and
        // the barrier above leaves no callback holding a reference to it.
        drop(unsafe { Box::from_raw(self.context.cast::<Handler>()) });
    }
}

/// The description keys a watch reports changes to: the volume path, which is
/// where a mount and an unmount show up.
fn watched_keys() -> &'static CFArray {
    // SAFETY: DiskArbitration initializes its exported key sets when the
    // framework loads, before any call into it can return.
    unsafe { kDADiskDescriptionWatchVolumePath }
}

/// Reports a disk `DiskArbitration` has attached, or replays one that was already
/// attached when the watch started.
///
/// # Safety
///
/// `context` must be the [`Handler`] pointer [`Watcher::start`] registered.
unsafe extern "C-unwind" fn disk_appeared(disk: NonNull<DADisk>, context: *mut c_void) {
    // SAFETY: DiskArbitration hands the callback a live disk for the duration
    // of the call.
    let disk = unsafe { disk.as_ref() };

    let Some(description) = description(disk) else {
        tracing::debug!("DiskArbitration described no disk for an appearance");
        return;
    };

    // SAFETY: `context` is the handler box the watch registered, which outlives
    // every callback it can deliver.
    let handler = unsafe { &*context.cast::<Handler>() };

    handler(Event::Appeared(description));
}

/// Reports a disk `DiskArbitration` has detached.
///
/// # Safety
///
/// `context` must be the [`Handler`] pointer [`Watcher::start`] registered.
unsafe extern "C-unwind" fn disk_disappeared(disk: NonNull<DADisk>, context: *mut c_void) {
    // SAFETY: DiskArbitration hands the callback a live disk for the duration
    // of the call.
    let disk = unsafe { disk.as_ref() };

    let Some(bsd_name) = bsd_name(disk) else {
        tracing::debug!("DiskArbitration named no disk for a disappearance");
        return;
    };

    // SAFETY: `context` is the handler box the watch registered, which outlives
    // every callback it can deliver.
    let handler = unsafe { &*context.cast::<Handler>() };

    handler(Event::Disappeared { bsd_name });
}

/// Reports a disk whose watched description keys changed.
///
/// # Safety
///
/// `context` must be the [`Handler`] pointer [`Watcher::start`] registered.
unsafe extern "C-unwind" fn disk_changed(
    disk: NonNull<DADisk>,
    _keys: NonNull<CFArray>,
    context: *mut c_void,
) {
    // SAFETY: DiskArbitration hands the callback a live disk for the duration
    // of the call.
    let disk = unsafe { disk.as_ref() };

    let Some(description) = description(disk) else {
        tracing::debug!("DiskArbitration described no disk for a description change");
        return;
    };

    // SAFETY: `context` is the handler box the watch registered, which outlives
    // every callback it can deliver.
    let handler = unsafe { &*context.cast::<Handler>() };

    handler(Event::Changed(description));
}

/// Answers a [`mount`] or an [`unmount`] request with the description of the
/// disk the request landed on, or with the status `DiskArbitration` dissented
/// the request with.
///
/// # Safety
///
/// `context` must address the [`mpsc::Sender`] the request was issued with.
unsafe extern "C-unwind" fn completed(
    disk: NonNull<DADisk>,
    dissenter: *const DADissenter,
    context: *mut c_void,
) {
    // SAFETY: `context` addresses the sender on the requesting thread's stack,
    // which stays there until this answer arrives.
    let sender = unsafe { &*context.cast::<mpsc::Sender<Completion>>() };

    let completion = if dissenter.is_null() {
        // SAFETY: DiskArbitration hands the callback a live disk for the
        // duration of the call.
        let disk = unsafe { disk.as_ref() };

        Ok(description(disk))
    } else {
        // SAFETY: a non-null dissenter is the one DiskArbitration hands the
        // callback, live for the duration of the call.
        Err(unsafe { status(dissenter) })
    };

    // A closed channel means the requesting thread is gone, leaving nothing to
    // answer.
    sender.send(completion).ok();
}

/// The status a non-null dissenter carries.
///
/// # Safety
///
/// `dissenter` must be the non-null dissenter of a completion callback.
unsafe fn status(dissenter: *const DADissenter) -> i32 {
    // SAFETY: DiskArbitration hands the callback a live dissenter for the
    // duration of the call.
    let dissenter = unsafe { &*dissenter };

    // SAFETY: `dissenter` is a live DADissenter, the only state the call reads.
    unsafe { dissenter.status() }
}

/// Issues a `DiskArbitration` request against `bsd_name` and blocks until its
/// completion callback answers.
///
/// `DiskArbitration` only delivers completions on a scheduled session, so the
/// request runs against a session carrying a serial queue of its own. `issue`
/// receives the disk and a context pointer addressing the sending half of the
/// answer channel, which lives on this thread's stack until the answer arrives.
/// Registering a callback against that pointer is itself an unsafe call, so the
/// obligation to read it back as an `mpsc::Sender<Completion>` is discharged
/// where `issue` is written.
fn request(bsd_name: &str, issue: impl FnOnce(&DADisk, *mut c_void)) -> Result<Completion, Error> {
    // SAFETY: the call takes only an allocator, and `None` asks for the
    // default one.
    let session = unsafe { DASession::new(None) }.ok_or(Error::NoSession)?;
    let disk = disk(&session, bsd_name);

    let queue = DispatchQueue::new(QUEUE_LABEL, None);
    // SAFETY: `queue` is a serial queue this call owns and keeps alive for as
    // long as the session is scheduled on it.
    unsafe { session.set_dispatch_queue(Some(&queue)) };

    let (sender, receiver) = mpsc::channel::<Completion>();
    issue(&disk, (&raw const sender).cast::<c_void>().cast_mut());

    let answer = receiver
        .recv()
        .expect("DiskArbitration answer channel closed");

    // SAFETY: unscheduling the session stops DiskArbitration from queuing any
    // further callback.
    unsafe { session.set_dispatch_queue(None) };

    // The completion callback runs to completion before this empty block does,
    // so once the barrier returns nothing is reading `sender` and dropping it
    // at the end of this call is sound.
    queue.exec_sync(|| {});

    Ok(answer)
}

/// The disk `DiskArbitration` knows under `bsd_name`.
///
/// `DADiskCreateFromBSDName` answers for any name; a name no medium carries
/// yields a disk that describes nothing.
fn disk(session: &DASession, bsd_name: &str) -> CFRetained<DADisk> {
    let name = CString::new(bsd_name).expect("BSD disk name with an interior NUL");
    let pointer: NonNull<c_char> =
        NonNull::new(name.as_ptr().cast_mut()).expect("null pointer from a CString");

    // SAFETY: `pointer` addresses `name`, a NUL-terminated C string that
    // outlives the call, which only reads through it.
    unsafe { DADisk::from_bsd_name(None, session, pointer) }
        .expect("DADiskCreateFromBSDName returned null")
}

/// The `DiskArbitration` description of `disk`, or `None` when DA describes it
/// without even a BSD name.
fn description(disk: &DADisk) -> Option<Description> {
    // SAFETY: `disk` is a live DADisk, the only state the call reads.
    let dictionary = unsafe { disk.description() }?;

    // SAFETY: DiskArbitration builds its descriptions with CFString keys, and
    // every value is a Core Foundation object.
    let dictionary = unsafe { dictionary.cast_unchecked::<CFString, CFType>() };

    let keys = Keys::new();

    Some(Description {
        bsd_name: string_value(dictionary, keys.bsd_name).or_else(|| bsd_name(disk))?,
        volume_name: string_value(dictionary, keys.volume_name),
        volume_path: path_value(dictionary, keys.volume_path),
        volume_mountable: bool_value(dictionary, keys.volume_mountable),
        media_removable: bool_value(dictionary, keys.media_removable),
        media_ejectable: bool_value(dictionary, keys.media_ejectable),
        media_leaf: bool_value(dictionary, keys.media_leaf),
        media_content: string_value(dictionary, keys.media_content),
        media_size: u64_value(dictionary, keys.media_size),
    })
}

/// The BSD name `disk` carries, or `None` when `DiskArbitration` reports none.
fn bsd_name(disk: &DADisk) -> Option<String> {
    // SAFETY: `disk` is a live DADisk, the only state the call reads.
    let name = unsafe { disk.bsd_name() };

    if name.is_null() {
        return None;
    }

    // SAFETY: a non-null BSD name is a NUL-terminated string owned by `disk`,
    // so it outlives the borrow taken here.
    let name = unsafe { CStr::from_ptr(name) };

    name.to_str().ok().map(ToOwned::to_owned)
}

/// The `kDADiskDescription*` keys a [`Description`] is read from.
struct Keys {
    bsd_name: &'static CFString,
    volume_name: &'static CFString,
    volume_path: &'static CFString,
    volume_mountable: &'static CFString,
    media_removable: &'static CFString,
    media_ejectable: &'static CFString,
    media_leaf: &'static CFString,
    media_content: &'static CFString,
    media_size: &'static CFString,
}

impl Keys {
    /// The keys as `DiskArbitration` exports them.
    fn new() -> Self {
        // SAFETY: DiskArbitration initializes its exported description keys
        // when the framework loads, before any call into it can return.
        unsafe {
            Self {
                bsd_name: kDADiskDescriptionMediaBSDNameKey,
                volume_name: kDADiskDescriptionVolumeNameKey,
                volume_path: kDADiskDescriptionVolumePathKey,
                volume_mountable: kDADiskDescriptionVolumeMountableKey,
                media_removable: kDADiskDescriptionMediaRemovableKey,
                media_ejectable: kDADiskDescriptionMediaEjectableKey,
                media_leaf: kDADiskDescriptionMediaLeafKey,
                media_content: kDADiskDescriptionMediaContentKey,
                media_size: kDADiskDescriptionMediaSizeKey,
            }
        }
    }
}

/// The string a description holds under `key`.
fn string_value(description: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<String> {
    let value = description.get(key)?;

    value.downcast_ref::<CFString>().map(ToString::to_string)
}

/// The boolean a description holds under `key`.
fn bool_value(description: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<bool> {
    let value = description.get(key)?;

    value.downcast_ref::<CFBoolean>().map(CFBoolean::as_bool)
}

/// The unsigned number a description holds under `key`.
fn u64_value(description: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<u64> {
    let value = description.get(key)?;

    value
        .downcast_ref::<CFNumber>()?
        .as_i64()
        .and_then(|size| u64::try_from(size).ok())
}

/// The POSIX path of the URL a description holds under `key`, which comes back
/// without a trailing separator.
fn path_value(description: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<PathBuf> {
    let value = description.get(key)?;

    value.downcast_ref::<CFURL>()?.to_file_path()
}

/// Smoke tests against the running framework. No fixture can stand in for
/// `diskarbitrationd`, so these check the bindings against the disks the
/// machine actually has.
#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    /// How long a watch may take to replay the disks already attached.
    const REPLAY_TIMEOUT: Duration = Duration::from_secs(5);

    /// The whole disk holding the boot media, which every Mac has attached.
    const BOOT_DISK: &str = "disk0";

    #[test]
    fn boot_disk_is_described_as_a_partitioned_whole_disk() {
        let descriptions = super::describe(&[String::from(BOOT_DISK)])
            .expect("`DiskArbitration` session unavailable");

        let description = descriptions.first().expect("boot disk not described");

        assert_eq!(description.bsd_name, BOOT_DISK);
        assert_eq!(description.media_leaf, Some(false));
    }

    #[test]
    fn starting_a_watch_replays_the_disks_already_attached() {
        let (tx, rx) = mpsc::channel();

        let watcher = super::Watcher::start(move |event| {
            if let super::Event::Appeared(description) = event {
                // The receiver is dropped once the test has its answer, and the
                // replay carries every attached disk.
                tx.send(description.bsd_name).ok();
            }
        })
        .expect("`DiskArbitration` session unavailable");

        let replayed = rx
            .recv_timeout(REPLAY_TIMEOUT)
            .expect("no disk replayed within the timeout");

        let descriptions = super::describe(std::slice::from_ref(&replayed))
            .expect("`DiskArbitration` session unavailable");
        let description = descriptions.first().expect("replayed disk not described");

        assert_eq!(description.bsd_name, replayed);

        drop(watcher);
    }
}
