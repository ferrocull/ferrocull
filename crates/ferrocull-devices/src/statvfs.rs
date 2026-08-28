use std::path::Path;

/// Total and used bytes for the filesystem mounted at `mount_point`, or `None`
/// if `statvfs` fails (e.g. the mount vanished mid-scan).
#[expect(
    clippy::useless_conversion,
    reason = "libc statvfs field widths are platform-dependent: the u64::from conversions are no-ops on 64-bit targets but real widening on 32-bit ones"
)]
pub(crate) fn disk_space(mount_point: &Path) -> Option<(u64, u64)> {
    let stat = nix::sys::statvfs::statvfs(mount_point).ok()?;
    let block_size = u64::from(stat.fragment_size());
    let total = u64::from(stat.blocks()) * block_size;
    let available = u64::from(stat.blocks_available()) * block_size;
    let used = total.saturating_sub(available);
    Some((total, used))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn root_filesystem_reports_capacity() {
        let (total, used) =
            super::disk_space(Path::new("/")).expect("root filesystem disk space unavailable");
        assert!(total > 0, "root filesystem should report a nonzero size");
        assert!(used <= total, "used bytes cannot exceed total bytes");
    }

    #[test]
    fn nonexistent_path_returns_none() {
        assert!(super::disk_space(Path::new("/no/such/mount/point/ferrocull")).is_none());
    }
}
