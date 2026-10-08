//! Unix: directory reads and stat calls relative to the directory fd, owner
//! names, local time, and the trash/delete/reveal operations.

use super::{base_name, check_abs, mode, other, Expect, Meta};
use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// Converts a Unix st_mode to the portable layout in `mode`.
#[allow(clippy::unnecessary_cast)] // mode_t is u16 on some systems
pub fn mode_from_unix(m: u32) -> u32 {
    let mut out = m & 0o777;
    let fmt = m & libc::S_IFMT as u32;
    out |= if fmt == libc::S_IFDIR as u32 {
        mode::DIR
    } else if fmt == libc::S_IFLNK as u32 {
        mode::SYMLINK
    } else if fmt == libc::S_IFIFO as u32 {
        mode::NAMED_PIPE
    } else if fmt == libc::S_IFSOCK as u32 {
        mode::SOCKET
    } else if fmt == libc::S_IFCHR as u32 {
        mode::DEVICE | mode::CHAR_DEVICE
    } else if fmt == libc::S_IFBLK as u32 {
        mode::DEVICE
    } else {
        0
    };
    if m & libc::S_ISUID as u32 != 0 {
        out |= mode::SETUID;
    }
    if m & libc::S_ISGID as u32 != 0 {
        out |= mode::SETGID;
    }
    if m & libc::S_ISVTX as u32 != 0 {
        out |= mode::STICKY;
    }
    out
}

fn cstr(b: &[u8]) -> io::Result<CString> {
    CString::new(b).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))
}

fn cvt(r: libc::c_int) -> io::Result<libc::c_int> {
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

fn open_dir(path: &[u8]) -> io::Result<OwnedFd> {
    let c = cstr(path)?;
    let fd = cvt(unsafe {
        libc::open(
            c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    })?;
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[cfg(target_os = "linux")]
static NO_STATX: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// statx (one call, includes birth time), falling back to fstatat on kernels
/// before 4.11. Called through syscall(2) because not every libc (musl)
/// provides a wrapper.
#[cfg(target_os = "linux")]
fn stat_at(dirfd: RawFd, name: &CStr, flags: libc::c_int) -> io::Result<Meta> {
    use std::sync::atomic::Ordering;
    if !NO_STATX.load(Ordering::Relaxed) {
        let mut st: libc::statx = unsafe { std::mem::zeroed() };
        let mask = libc::STATX_BASIC_STATS | libc::STATX_BTIME;
        let r = unsafe {
            libc::syscall(
                libc::SYS_statx,
                dirfd,
                name.as_ptr(),
                flags | libc::AT_STATX_DONT_SYNC,
                mask,
                &mut st as *mut libc::statx,
            )
        };
        if r == 0 {
            let mut m = Meta {
                mode: mode_from_unix(st.stx_mode as u32),
                size: st.stx_size as i64,
                alloc: st.stx_blocks as i64 * 512,
                alloc_known: true,
                mtime: st.stx_mtime.tv_sec * 1_000_000_000 + st.stx_mtime.tv_nsec as i64,
                uid: st.stx_uid,
                gid: st.stx_gid,
                nlink: st.stx_nlink,
                dev: libc::makedev(st.stx_dev_major, st.stx_dev_minor),
                ino: st.stx_ino,
                has_ino: true,
                ..Default::default()
            };
            if st.stx_mask & libc::STATX_BTIME != 0 {
                m.btime = st.stx_btime.tv_sec * 1_000_000_000 + st.stx_btime.tv_nsec as i64;
            }
            return Ok(m);
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::ENOSYS) {
            return Err(err);
        }
        NO_STATX.store(true, Ordering::Relaxed);
    }
    fstat_at(dirfd, name, flags)
}

#[cfg(not(target_os = "linux"))]
fn stat_at(dirfd: RawFd, name: &CStr, flags: libc::c_int) -> io::Result<Meta> {
    fstat_at(dirfd, name, flags)
}

fn fstat_at(dirfd: RawFd, name: &CStr, flags: libc::c_int) -> io::Result<Meta> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    cvt(unsafe { libc::fstatat(dirfd, name.as_ptr(), &mut st, flags) })?;
    Ok(from_stat(&st))
}

#[allow(clippy::unnecessary_cast)]
fn from_stat(st: &libc::stat) -> Meta {
    #[cfg(target_os = "netbsd")]
    let mtime_nsec = st.st_mtimensec;
    #[cfg(not(target_os = "netbsd"))]
    let mtime_nsec = st.st_mtime_nsec;
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    let btime = st.st_birthtime as i64 * 1_000_000_000 + st.st_birthtime_nsec as i64;
    #[cfg(target_os = "netbsd")]
    let btime = st.st_birthtime as i64 * 1_000_000_000 + st.st_birthtimensec as i64;
    #[cfg(not(any(target_os = "macos", target_os = "freebsd", target_os = "netbsd")))]
    let btime = 0;
    Meta {
        mode: mode_from_unix(st.st_mode as u32),
        size: st.st_size as i64,
        alloc: st.st_blocks as i64 * 512,
        alloc_known: true,
        mtime: st.st_mtime as i64 * 1_000_000_000 + mtime_nsec as i64,
        btime,
        uid: st.st_uid,
        gid: st.st_gid,
        nlink: st.st_nlink as u32,
        dev: st.st_dev as u64,
        ino: st.st_ino as u64,
        has_ino: true,
        ..Default::default()
    }
}

/// An open directory stream (readdir over the fd we also stat relative to).
struct DirStream {
    dir: *mut libc::DIR,
}

impl Drop for DirStream {
    fn drop(&mut self) {
        unsafe { libc::closedir(self.dir) };
    }
}

fn errno_location() -> *mut libc::c_int {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    return unsafe { libc::__errno_location() };
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    return unsafe { libc::__error() };
    #[cfg(any(target_os = "netbsd", target_os = "openbsd"))]
    return unsafe { libc::__errno() };
}

/// Streams the entries of a directory in chunks of up to `chunk`, stat-ing
/// each entry relative to the directory fd (no repeated path resolution).
/// Returning an error from f stops the read and returns it.
pub fn read_dir<E: From<io::Error>>(
    path: &[u8],
    chunk: usize,
    mut f: impl FnMut(Vec<Meta>) -> Result<(), E>,
) -> Result<(), E> {
    let fd = open_dir(path)?;
    let raw = fd.as_raw_fd();
    let dir = unsafe { libc::fdopendir(raw) };
    if dir.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    std::mem::forget(fd); // owned by the DIR stream now
    let ds = DirStream { dir };
    let mut out = Vec::with_capacity(chunk);
    loop {
        // readdir signals an error only through errno.
        unsafe { *errno_location() = 0 };
        let ent = unsafe { libc::readdir(ds.dir) };
        if ent.is_null() {
            let e = io::Error::last_os_error();
            if e.raw_os_error().unwrap_or(0) != 0 {
                return Err(e.into());
            }
            break;
        }
        let name = unsafe { CStr::from_ptr((*ent).d_name.as_ptr()) };
        let nb = name.to_bytes();
        if nb == b"." || nb == b".." {
            continue;
        }
        let mut m = stat_at(raw, name, libc::AT_SYMLINK_NOFOLLOW).unwrap_or_else(|e| Meta {
            err: Some(e),
            ..Default::default()
        });
        m.name = nb.to_vec();
        out.push(m);
        if out.len() >= chunk {
            f(std::mem::replace(&mut out, Vec::with_capacity(chunk)))?;
        }
    }
    if !out.is_empty() {
        f(out)?;
    }
    Ok(())
}

pub(super) fn stat_path(path: &[u8], follow: bool) -> io::Result<Meta> {
    let c = cstr(path)?;
    let flags = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
    let mut m = stat_at(libc::AT_FDCWD, &c, flags)?;
    m.name = base_name(path).to_vec();
    Ok(m)
}

pub(super) fn lookup_os_name(group: bool, id: u32) -> Option<String> {
    let mut buf = vec![0u8; 16384];
    unsafe {
        if group {
            let mut gr: libc::group = std::mem::zeroed();
            let mut res = std::ptr::null_mut();
            libc::getgrgid_r(id, &mut gr, buf.as_mut_ptr().cast(), buf.len(), &mut res);
            (!res.is_null()).then(|| CStr::from_ptr(gr.gr_name).to_string_lossy().into_owned())
        } else {
            let mut pw: libc::passwd = std::mem::zeroed();
            let mut res = std::ptr::null_mut();
            libc::getpwuid_r(id, &mut pw, buf.as_mut_ptr().cast(), buf.len(), &mut res);
            (!res.is_null()).then(|| CStr::from_ptr(pw.pw_name).to_string_lossy().into_owned())
        }
    }
}

/// The system time zone's offset from UTC (seconds east) at unix time secs.
#[allow(deprecated)] // libc::time_t on 32-bit musl
#[allow(clippy::unnecessary_cast)]
pub(super) fn local_offset(secs: i64) -> i32 {
    let t = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return 0;
    }
    tm.tm_gmtoff as i32
}

/// The host name, or "" if unknown.
pub fn hostname() -> String {
    let mut b = [0u8; 256];
    if unsafe { libc::gethostname(b.as_mut_ptr() as *mut libc::c_char, b.len()) } != 0 {
        return String::new();
    }
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..n]).into_owned()
}

/// Linux pseudo-filesystems whose contents are not disk usage.
#[cfg(target_os = "linux")]
const VIRTUAL_FS: [i64; 19] = [
    0x9fa0,     // proc
    0x62656572, // sysfs
    0x1cd1,     // devpts
    0x27e0eb,   // cgroup
    0x63677270, // cgroup2
    0x64626720, // debugfs
    0x74726163, // tracefs
    0x73636673, // securityfs
    0x6165676c, // pstore
    0xcafe4a11, // bpf
    0x62656570, // configfs
    0x65735543, // fusectl
    0x19800202, // mqueue
    0xde5e81e4, // efivarfs
    0x42494e4d, // binfmt_misc
    0xf97cff8c, // selinuxfs
    0x958458f6, // hugetlbfs
    0x6e736673, // nsfs
    0x50495045, // pipefs
];

/// Whether path is the root of a pseudo filesystem (proc, sysfs, cgroup, …)
/// that should not be scanned as disk usage. Only Linux has these.
#[cfg(target_os = "linux")]
#[allow(clippy::unnecessary_cast)]
pub fn is_virtual_fs(path: &[u8]) -> bool {
    let Ok(c) = cstr(path) else { return false };
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return false;
    }
    // f_type is 32-bit on some architectures: compare as unsigned 32-bit.
    VIRTUAL_FS.contains(&(st.f_type as u32 as i64))
}

/// Whether path is the root of a pseudo filesystem. Only Linux has these.
#[cfg(not(target_os = "linux"))]
pub fn is_virtual_fs(_: &[u8]) -> bool {
    false
}

/// Opens a file whose content is about to be read. It does not follow a
/// final symlink, and O_NONBLOCK keeps a FIFO swapped in since the scan from
/// blocking open(2) forever; callers still check the file type.
pub fn open_content(path: &Path) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

/// Creates a new file readable and writable only by the owner.
pub fn create_private(path: &Path) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

const CHANGED: &str = "item changed since the scan (owner or type differs); rescan first";

fn split_parent(path: &Path) -> io::Result<(&Path, &OsStr)> {
    match (path.parent(), path.file_name()) {
        (Some(d), Some(b)) => Ok((d, b)),
        _ => Err(other("refusing to operate on a filesystem root")),
    }
}

fn check_expect(dirfd: RawFd, base: &CStr, want: Expect) -> io::Result<libc::stat> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    cvt(unsafe { libc::fstatat(dirfd, base.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) })?;
    #[allow(clippy::unnecessary_cast)]
    if st.st_uid != want.uid
        || mode_from_unix(st.st_mode as u32) & mode::TYPE != want.mode & mode::TYPE
    {
        return Err(other(CHANGED));
    }
    Ok(st)
}

/// Permanently removes path (recursively for a directory) if it still
/// matches want. The check and the removal both go through descriptors
/// opened from the parent directory without following symlinks, so a
/// symlink swapped in since the scan cannot redirect the removal outside it.
pub fn delete(path: &Path, want: Expect) -> io::Result<()> {
    check_abs(path)?;
    let (dir, base) = split_parent(path)?;
    let dfd = open_dir(dir.as_os_str().as_bytes())?;
    let cbase = cstr(base.as_bytes())?;
    let st = check_expect(dfd.as_raw_fd(), &cbase, want)?;
    remove_at(
        dfd.as_raw_fd(),
        &cbase,
        st.st_mode & libc::S_IFMT == libc::S_IFDIR,
    )
}

fn remove_at(dirfd: RawFd, name: &CStr, is_dir: bool) -> io::Result<()> {
    if !is_dir {
        return cvt(unsafe { libc::unlinkat(dirfd, name.as_ptr(), 0) }).map(|_| ());
    }
    let fd = cvt(unsafe {
        libc::openat(
            dirfd,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    })?;
    let sub = unsafe { OwnedFd::from_raw_fd(fd) };
    // Collect names first: removing while iterating a DIR stream is unspecified.
    let mut names = Vec::new();
    let dup = cvt(unsafe { libc::dup(sub.as_raw_fd()) })?;
    let dir = unsafe { libc::fdopendir(dup) };
    if dir.is_null() {
        unsafe { libc::close(dup) };
        return Err(io::Error::last_os_error());
    }
    let ds = DirStream { dir };
    loop {
        let ent = unsafe { libc::readdir(ds.dir) };
        if ent.is_null() {
            break;
        }
        let n = unsafe { CStr::from_ptr((*ent).d_name.as_ptr()) };
        if n.to_bytes() != b"." && n.to_bytes() != b".." {
            let t = unsafe { (*ent).d_type };
            let is_dir = match t {
                libc::DT_UNKNOWN => fstat_at(sub.as_raw_fd(), n, libc::AT_SYMLINK_NOFOLLOW)
                    .is_ok_and(|m| mode::is_dir(m.mode)),
                t => t == libc::DT_DIR,
            };
            names.push((n.to_owned(), is_dir));
        }
    }
    drop(ds);
    for (n, d) in names {
        remove_at(sub.as_raw_fd(), &n, d)?;
    }
    cvt(unsafe { libc::unlinkat(dirfd, name.as_ptr(), libc::AT_REMOVEDIR) }).map(|_| ())
}

/// Moves path to dst if it still matches want. The parent directory is
/// opened once and both the check and the rename are relative to that
/// descriptor, so a parent swapped for a symlink since the scan (or between
/// check and rename) cannot redirect the move to another user's file.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn rename_checked(path: &Path, dst: &Path, want: Expect) -> io::Result<()> {
    let (dir, base) = split_parent(path)?;
    let dfd = open_dir(dir.as_os_str().as_bytes())?;
    let cbase = cstr(base.as_bytes())?;
    check_expect(dfd.as_raw_fd(), &cbase, want)?;
    let cdst = cstr(dst.as_os_str().as_bytes())?;
    cvt(unsafe {
        libc::renameat(
            dfd.as_raw_fd(),
            cbase.as_ptr(),
            libc::AT_FDCWD,
            cdst.as_ptr(),
        )
    })
    .map(|_| ())
}

/// Creates dir (mode 0700) if missing; an existing entry must be a real
/// directory (not a symlink) owned by us. With strict (trash on a shared
/// filesystem) it must also be inaccessible to others, as the freedesktop.org
/// spec requires; home trash directories made by desktop environments are
/// sometimes 0755, which is harmless there.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn ensure_private_dir(dir: &Path, strict: bool) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    if let Err(e) = std::fs::DirBuilder::new().mode(0o700).create(dir) {
        if e.kind() != io::ErrorKind::AlreadyExists {
            return Err(e);
        }
    }
    let md = std::fs::symlink_metadata(dir)?;
    if !md.is_dir() {
        return Err(other("not a directory (symlink?)"));
    }
    if md.uid() != unsafe { libc::getuid() } {
        return Err(other("owned by another user"));
    }
    if strict && md.mode() & 0o077 != 0 {
        return Err(other("accessible to other users"));
    }
    Ok(())
}

/// Whether there is no graphical session (no X or Wayland display), so there
/// is no file manager to open and no desktop trash anyone will empty.
#[cfg(not(target_os = "macos"))]
pub fn headless() -> bool {
    std::env::var_os("DISPLAY").is_none_or(|v| v.is_empty())
        && std::env::var_os("WAYLAND_DISPLAY").is_none_or(|v| v.is_empty())
}

#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::MetadataExt;
    use std::path::PathBuf;

    /// The user-facing name of the trash.
    pub const TRASH_NAME: &str = "trash";

    /// Opens the item's directory in the desktop file manager.
    pub fn reveal(path: &Path) -> io::Result<()> {
        check_abs(path)?;
        let md = std::fs::symlink_metadata(path)?;
        let target = if md.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        crate::platform::start_detached("xdg-open", &[target.as_os_str()])
    }

    /// The top directory of the filesystem containing path.
    fn mount_top(path: &Path, dev: u64) -> PathBuf {
        let mut cur = path.parent().unwrap_or(path).to_path_buf();
        loop {
            let Some(parent) = cur.parent() else {
                return cur;
            };
            match std::fs::metadata(parent) {
                Ok(m) if m.dev() == dev => cur = parent.to_path_buf(),
                _ => return cur,
            }
        }
    }

    fn percent_encode_path(p: &[u8]) -> String {
        let mut s = String::with_capacity(p.len());
        for &b in p {
            if b.is_ascii_alphanumeric() || b"-._~/!$&'()*+,;=:@".contains(&b) {
                s.push(b as char);
            } else {
                s.push_str(&format!("%{b:02X}"));
            }
        }
        s
    }

    fn local_timestamp() -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let t = crate::platform::local_time(now.as_secs() as i64);
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            t.year, t.month, t.day, t.hour, t.min, t.sec
        )
    }

    /// Moves path to the freedesktop.org trash: the home trash when on the
    /// same filesystem, otherwise $topdir/.Trash-$uid on the item's
    /// filesystem. It never copies across filesystems and never deletes.
    pub fn trash(path: &Path, want: Expect) -> io::Result<()> {
        check_abs(path)?;
        let dev = std::fs::symlink_metadata(path)?.dev();
        let data_home = match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
            Some(d) => PathBuf::from(d),
            None => {
                let home = std::env::var_os("HOME")
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| other("$HOME is not defined"))?;
                PathBuf::from(home).join(".local/share")
            }
        };
        let home = data_home.join("Trash");
        let (trash, info_path): (PathBuf, Vec<u8>) = match std::fs::metadata(&data_home) {
            Ok(m) if m.dev() == dev => (home.clone(), path.as_os_str().as_bytes().to_vec()),
            _ => {
                let top = mount_top(path, dev);
                let rel = path.strip_prefix(&top).map_err(|e| other(e.to_string()))?;
                (
                    top.join(format!(".Trash-{}", unsafe { libc::getuid() })),
                    rel.as_os_str().as_bytes().to_vec(),
                )
            }
        };
        let shared = trash != home;
        if !shared {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&trash)?;
        } else if let Err(e) = ensure_private_dir(&trash, true) {
            // On a shared filesystem another user could plant a symlink or a
            // world-writable directory here; refuse rather than follow it.
            return Err(other(format!(
                "trash directory {} is unsafe: {e}",
                trash.display()
            )));
        }
        let (files, info) = (trash.join("files"), trash.join("info"));
        for d in [&files, &info] {
            ensure_private_dir(d, shared)
                .map_err(|e| other(format!("trash directory {} is unsafe: {e}", d.display())))?;
        }
        let base = path.file_name().ok_or_else(|| other("no file name"))?;
        for i in 1..10000 {
            let mut name = base.to_os_string();
            if i > 1 {
                name.push(format!(".{i}"));
            }
            let dst = files.join(&name);
            if std::fs::symlink_metadata(&dst).is_ok() {
                continue; // never overwrite an earlier trashed item (even without .trashinfo)
            }
            let mut info_name = name.clone();
            info_name.push(".trashinfo");
            let ip = info.join(info_name);
            let mut f = match create_private(&ip) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            };
            let w = write!(
                f,
                "[Trash Info]\nPath={}\nDeletionDate={}\n",
                percent_encode_path(&info_path),
                local_timestamp()
            )
            .and_then(|_| f.sync_all());
            drop(f);
            if let Err(e) = w.and_then(|_| rename_checked(path, &dst, want)) {
                let _ = std::fs::remove_file(&ip);
                return Err(e);
            }
            return Ok(());
        }
        Err(other("trash: too many items with the same name"))
    }
}

#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::path::PathBuf;

    /// The user-facing name of the trash.
    pub const TRASH_NAME: &str = "Trash";

    /// Selects the item in Finder.
    pub fn reveal(path: &Path) -> io::Result<()> {
        check_abs(path)?;
        crate::platform::start_detached("open", &[OsStr::new("-R"), path.as_os_str()])
    }

    /// Always false: macOS has Finder and the Trash without an X display.
    pub fn headless() -> bool {
        false
    }

    /// "/Volumes/<name>" for a path on a mounted volume, else None.
    fn volume_of(path: &Path) -> Option<PathBuf> {
        let rest = path.strip_prefix("/Volumes/").ok()?;
        rest.components()
            .next()
            .map(|c| Path::new("/Volumes").join(c))
    }

    /// Moves path into ~/.Trash (same volume) or the volume's .Trashes/<uid>
    /// directory. It never copies and never deletes.
    pub fn trash(path: &Path, want: Expect) -> io::Result<()> {
        check_abs(path)?;
        let home = std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .ok_or_else(|| other("$HOME is not defined"))?;
        let mut dirs = vec![PathBuf::from(home).join(".Trash")];
        if let Some(vol) = volume_of(path) {
            dirs.push(
                vol.join(".Trashes")
                    .join(unsafe { libc::getuid() }.to_string()),
            );
        }
        let base = path.file_name().ok_or_else(|| other("no file name"))?;
        for (vi, dir) in dirs.iter().enumerate() {
            if dir
                .parent()
                .is_some_and(|p| std::fs::create_dir_all(p).is_err())
            {
                continue;
            }
            // A volume trash is on a shared filesystem: another user could
            // plant a symlink or an open directory there, so insist on a
            // private one.
            if ensure_private_dir(dir, vi > 0).is_err() {
                continue;
            }
            for i in 1..10000 {
                let mut name = base.to_os_string();
                if i > 1 {
                    name.push(format!(" {i}"));
                }
                let dst = dir.join(&name);
                if std::fs::symlink_metadata(&dst).is_ok() {
                    continue;
                }
                match rename_checked(path, &dst, want) {
                    Err(e) if e.raw_os_error() == Some(libc::EXDEV) => break, // try the next trash location
                    r => return r,
                }
            }
        }
        Err(other("trash: item is on a volume without a usable trash"))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub use bsd::*;

/// FreeBSD, NetBSD and other Unix systems: no desktop trash or file
/// manager integration. Permanent delete is still available.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod bsd {
    use super::*;

    /// The user-facing name of the trash.
    pub const TRASH_NAME: &str = "trash";

    /// Unsupported on this platform.
    pub fn reveal(_: &Path) -> io::Result<()> {
        Err(other("not supported on this platform"))
    }

    /// Unsupported on this platform.
    pub fn trash(_: &Path, _: Expect) -> io::Result<()> {
        Err(other("trash not supported on this platform"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uid() -> u32 {
        unsafe { libc::getuid() }
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("mapsize-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn delete_refuses_changed_type() {
        let d = tmp("del");
        std::fs::create_dir_all(d.join("sub/inner")).unwrap();
        std::fs::write(d.join("sub/inner/f"), b"x").unwrap();
        // A symlink inside must be removed, not followed.
        std::fs::write(d.join("keep"), b"k").unwrap();
        std::os::unix::fs::symlink(d.join("keep"), d.join("sub/link")).unwrap();
        assert!(delete(
            &d.join("sub"),
            Expect {
                uid: uid(),
                mode: 0
            }
        )
        .is_err());
        delete(
            &d.join("sub"),
            Expect {
                uid: uid(),
                mode: mode::DIR,
            },
        )
        .unwrap();
        assert!(!d.join("sub").exists());
        assert!(d.join("keep").exists());
        assert!(delete(Path::new("relative/path"), Expect { uid: 0, mode: 0 }).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    #[allow(clippy::unnecessary_cast)]
    fn mode_bits() {
        assert_eq!(
            mode_from_unix(libc::S_IFDIR as u32 | 0o755),
            mode::DIR | 0o755
        );
        assert_eq!(
            mode_from_unix(libc::S_IFCHR as u32 | 0o600),
            mode::DEVICE | mode::CHAR_DEVICE | 0o600
        );
        assert_eq!(
            mode_from_unix(libc::S_IFREG as u32 | libc::S_ISUID as u32 | 0o4755),
            mode::SETUID | 0o755
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ensure_private_dir_rejects_symlink_and_open_dir() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("priv");
        std::fs::create_dir(d.join("attacker")).unwrap();
        std::os::unix::fs::symlink(d.join("attacker"), d.join(".Trash-1000")).unwrap();
        assert!(ensure_private_dir(&d.join(".Trash-1000"), false).is_err());
        std::fs::create_dir(d.join("open")).unwrap();
        std::fs::set_permissions(d.join("open"), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(ensure_private_dir(&d.join("open"), true).is_err());
        ensure_private_dir(&d.join("fresh"), true).unwrap();
        std::fs::remove_dir_all(&d).unwrap();
    }
}
