// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The mount table: which file system a library folder is on, and whether inotify
//! sees the changes made to it.
//!
//! The table is read from `/proc/self/mountinfo` (see `proc_pid_mountinfo(5)`), which
//! names the file-system type of every mount, so no system call is needed.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

/// The mount table of this process.
pub(crate) const MOUNTINFO: &str = "/proc/self/mountinfo";

/// The kernel's limit on inotify watches per user.
const MAX_USER_WATCHES: &str = "/proc/sys/fs/inotify/max_user_watches";

/// One line of the mount table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mount {
    /// Where it is mounted.
    pub(crate) point: PathBuf,
    /// The file-system type, such as `ext4`, `nfs4` or `fuse.portal`.
    pub(crate) fs_type: String,
}

/// Reads the mount table at `path`.
pub(crate) fn read_mounts(path: &Path) -> io::Result<Vec<Mount>> {
    fs::read(path).map(|text| parse_mountinfo(&text))
}

/// The mounts listed in `text`, in the format of `/proc/self/mountinfo`. Lines that
/// do not have that format are left out.
pub(crate) fn parse_mountinfo(text: &[u8]) -> Vec<Mount> {
    text.split(|&b| b == b'\n')
        .filter_map(|line| {
            let fields: Vec<&[u8]> = line
                .split(|&b| b == b' ')
                .filter(|f| !f.is_empty())
                .collect();
            // ID, parent ID, major:minor, root, mount point, options, optional
            // fields up to "-", then the type, the source and the super options.
            let point = fields.get(4)?;
            let separator = fields.iter().skip(6).position(|f| *f == b"-")? + 6;
            let fs_type = fields.get(separator + 1)?;
            Some(Mount {
                point: PathBuf::from(OsString::from_vec(unescape(point))),
                fs_type: String::from_utf8_lossy(fs_type).into_owned(),
            })
        })
        .collect()
}

/// Undoes the octal escapes (`\040` for a space) of a mount-table field.
fn unescape(field: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(field.len());
    let mut i = 0;
    while i < field.len() {
        if field[i] == b'\\'
            && let Some(digits) = field.get(i + 1..i + 4)
            && digits.iter().all(|d| (b'0'..=b'7').contains(d))
        {
            let value = digits
                .iter()
                .fold(0u32, |value, d| value * 8 + u32::from(d - b'0'));
            if let Ok(byte) = u8::try_from(value) {
                out.push(byte);
                i += 4;
                continue;
            }
        }
        out.push(field[i]);
        i += 1;
    }
    out
}

/// The mount that holds `path`: the one with the longest mount point above it, and
/// of several at the same point the last, which is on top.
pub(crate) fn mount_of<'a>(mounts: &'a [Mount], path: &Path) -> Option<&'a Mount> {
    let mut best: Option<&Mount> = None;
    for mount in mounts {
        if path.starts_with(&mount.point)
            && best.is_none_or(|b| mount.point.as_os_str().len() >= b.point.as_os_str().len())
        {
            best = Some(mount);
        }
    }
    best
}

/// Whether inotify misses changes on a file system of type `fs_type`, so that a
/// library folder on it is rescanned periodically instead.
///
/// - Network file systems: changes made by other machines never reach this kernel.
/// - FUSE file systems (`fuse`, `fuse.*`), among them the document portal of
///   Flatpak (`fuse.portal`), `sshfs` and GVfs: their contents come from a program
///   that need not tell the kernel about changes. `fuseblk` (NTFS or exFAT on a local
///   disk through FUSE) is the exception: every change to it goes through this
///   kernel.
/// - Shared folders of virtual machines, changed from the host.
pub(crate) fn needs_polling(fs_type: &str) -> bool {
    const POLLED: &[&str] = &[
        "nfs",
        "nfs4",
        "cifs",
        "smb3",
        "smbfs",
        "ncpfs",
        "afs",
        "coda",
        "9p",
        "ceph",
        "glusterfs",
        "lustre",
        "gfs2",
        "ocfs2",
        "davfs",
        "fuse",
        "virtiofs",
        "vboxsf",
        "vmhgfs",
    ];
    POLLED.contains(&fs_type) || (fs_type.starts_with("fuse.") && fs_type != "fuseblk")
}

/// The kernel's limit on inotify watches per user, if it can be read.
pub(crate) fn max_user_watches() -> Option<u64> {
    fs::read_to_string(MAX_USER_WATCHES)
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &[u8] = b"\
22 1 0:21 / /proc rw,nosuid,nodev,noexec,relatime shared:5 - proc proc rw
1 0 259:2 /root / rw,relatime shared:1 - btrfs /dev/nvme0n1p2 rw,ssd
40 1 259:2 /home /home rw,relatime shared:30 - btrfs /dev/nvme0n1p2 rw,ssd
70 40 0:55 / /home/u/Music\\040Share rw,relatime shared:40 - nfs4 nas:/music rw,vers=4.2
71 1 0:56 / /run/user/1000/doc rw,nosuid,nodev,relatime shared:41 - fuse.portal portal rw
72 1 8:17 / /media/usb rw,relatime shared:42 master:3 - vfat /dev/sdb1 rw
73 1 8:18 / /media/usb rw,relatime shared:43 - exfat /dev/sdc1 rw
garbage line
";

    #[test]
    fn the_table_is_parsed() {
        let mounts = parse_mountinfo(TABLE);
        assert_eq!(mounts.len(), 7);
        assert_eq!(mounts[3].point, Path::new("/home/u/Music Share"));
        assert_eq!(mounts[3].fs_type, "nfs4");
        assert_eq!(mounts[5].fs_type, "vfat");
    }

    #[test]
    fn the_longest_and_latest_mount_holds_a_path() {
        let mounts = parse_mountinfo(TABLE);
        let fs_of = |path: &str| mount_of(&mounts, Path::new(path)).map(|m| m.fs_type.as_str());
        assert_eq!(fs_of("/home/u/Music Share/Album"), Some("nfs4"));
        assert_eq!(fs_of("/home/u/Music Shared"), Some("btrfs"));
        assert_eq!(fs_of("/run/user/1000/doc/abc/Music"), Some("fuse.portal"));
        assert_eq!(fs_of("/media/usb/Music"), Some("exfat"));
        assert_eq!(fs_of("/srv"), Some("btrfs"));
        assert_eq!(fs_of("relative"), None);
    }

    #[test]
    fn network_fuse_and_portal_file_systems_are_polled() {
        for polled in [
            "nfs4",
            "cifs",
            "smb3",
            "fuse.portal",
            "fuse.sshfs",
            "fuse",
            "9p",
        ] {
            assert!(needs_polling(polled), "{polled}");
        }
        for watched in ["ext4", "btrfs", "xfs", "vfat", "exfat", "fuseblk", "tmpfs"] {
            assert!(!needs_polling(watched), "{watched}");
        }
    }

    #[test]
    fn escapes_are_undone() {
        assert_eq!(unescape(b"a\\040b\\134c\\011"), b"a b\\c\t");
        assert_eq!(unescape(b"bad\\04"), b"bad\\04");
        assert_eq!(unescape(b"\\999"), b"\\999");
    }
}
