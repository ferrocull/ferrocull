//! Win32 volume enumeration: the crate's FFI layer, and the only module in the
//! workspace that contains `unsafe`.
//!
//! Drives are enumerated by letter rather than by volume GUID, because the
//! mount manager answers for a letter without reading the media behind it: a
//! card whose filesystem Windows cannot read still has a letter, and so still
//! reaches the scan.

// The Win32 volume functions this enumeration needs have no safe binding in the
// `windows` crate: `GetLogicalDrives`, `GetDriveTypeW`, `GetVolumeInformationW`
// and `GetDiskFreeSpaceExW` are all `unsafe fn`.
#![expect(
    unsafe_code,
    reason = "Win32 volume enumeration has no safe binding in the `windows` crate"
)]

use windows::{
    Win32::{
        Foundation::{ERROR_NO_MEDIA_IN_DRIVE, ERROR_NOT_READY, MAX_PATH},
        Storage::FileSystem::{
            GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
        },
        System::WindowsProgramming::DRIVE_REMOVABLE,
    },
    core::{HRESULT, PCWSTR},
};

use crate::ScanError;

/// Drive letters Windows can assign, `A:` through `Z:`.
const LETTER_COUNT: u8 = 26;

/// Windows error codes for a removable drive whose slot holds no media. Readers
/// disagree on which one they report, so both stand for an empty slot.
const EMPTY_SLOT_ERRORS: [HRESULT; 2] = [
    HRESULT::from_win32(ERROR_NOT_READY.0),
    HRESULT::from_win32(ERROR_NO_MEDIA_IN_DRIVE.0),
];

/// What reading a drive root found.
pub(crate) enum Volume {
    /// A filesystem Windows can read. `label` is empty when the volume is
    /// unlabelled.
    Readable {
        label: String,
        total_bytes: u64,
        used_bytes: u64,
    },
    /// Media whose filesystem Windows does not recognize.
    Unreadable,
    /// No media in the drive.
    Empty,
}

/// Drive letters, uppercased, of every removable drive Windows has a letter
/// assigned for, whether or not its filesystem can be read.
pub(crate) fn removable_letters() -> Result<Vec<char>, ScanError> {
    // SAFETY: the call takes no arguments and reads only system state, so it
    // carries no invariant for the caller to uphold.
    let mask = unsafe { GetLogicalDrives() };

    // The system drive always carries a letter, so a mask with no bits set is
    // the enumeration itself having failed rather than a machine with no
    // drives. An empty removable subset, in contrast, is the normal case.
    if mask == 0 {
        return Err(ScanError::Backend(String::from(
            "drive letter enumeration returned no letters at all",
        )));
    }

    Ok((0..LETTER_COUNT)
        .filter(|bit| mask & (1u32 << bit) != 0)
        .map(|bit| char::from(b'A' + bit))
        .filter(|letter| is_removable(*letter))
        .collect())
}

/// What the drive at `letter` holds.
pub(crate) fn read_volume(letter: char) -> Volume {
    let root = root_path(letter);
    let mut name = [0u16; MAX_PATH as usize + 1];

    // SAFETY: `root` is NUL-terminated and outlives the call, which only reads
    // through the pointer. The binding derives the volume name length from the
    // `name` slice, so the buffer cannot be overrun.
    let volume_information = unsafe {
        GetVolumeInformationW(
            PCWSTR::from_raw(root.as_ptr()),
            Some(&mut name),
            None,
            None,
            None,
            None,
        )
    };

    match volume_information {
        Ok(()) => {
            let (total_bytes, free_bytes) = disk_space(&root);
            Volume::Readable {
                label: label(&name),
                total_bytes,
                used_bytes: total_bytes.saturating_sub(free_bytes),
            }
        }
        Err(error) if EMPTY_SLOT_ERRORS.contains(&error.code()) => Volume::Empty,
        Err(_) => Volume::Unreadable,
    }
}

/// Whether Windows classifies the drive at `letter` as removable.
///
/// `GetDriveTypeW` answers from the mount manager without touching the media,
/// so it does not stall on an empty reader.
fn is_removable(letter: char) -> bool {
    let root = root_path(letter);

    // SAFETY: `root` is NUL-terminated and outlives the call, which only reads
    // through the pointer.
    let drive_type = unsafe { GetDriveTypeW(PCWSTR::from_raw(root.as_ptr())) };

    drive_type == DRIVE_REMOVABLE
}

/// Total and free bytes of the volume rooted at `root`, both zero when the
/// query fails.
fn disk_space(root: &[u16]) -> (u64, u64) {
    let mut total_bytes = 0u64;
    let mut free_bytes = 0u64;

    // SAFETY: `root` is NUL-terminated and outlives the call, as do the two
    // locals the out-pointers address; the call only reads through the first
    // and writes through the others.
    let query = unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR::from_raw(root.as_ptr()),
            None,
            Some(&raw mut total_bytes),
            Some(&raw mut free_bytes),
        )
    };

    match query {
        Ok(()) => (total_bytes, free_bytes),
        Err(_) => (0, 0),
    }
}

/// NUL-terminated wide `X:\` root path for `letter`, the form both
/// `GetDriveTypeW` and `GetVolumeInformationW` require.
fn root_path(letter: char) -> [u16; 4] {
    let ascii = u8::try_from(letter).expect("drive letter outside ASCII");

    [u16::from(ascii), u16::from(b':'), u16::from(b'\\'), 0]
}

/// Volume label held in a filled volume name buffer, up to its NUL terminator.
fn label(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .expect("volume name buffer without a NUL terminator");

    String::from_utf16_lossy(&buffer[..end])
}
