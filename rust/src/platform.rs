//! OS-specific filesystem metadata and file operations (Unix; Linux first).
//! Nothing else in the program calls libc for filesystem work.

use std::collections::HashMap;
use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// File mode bits in Go's io/fs.FileMode layout. Kept so snapshots stay
/// byte-compatible with the Go implementation.
pub mod mode {
    pub const DIR: u32 = 1 << 31;
    pub const APPEND: u32 = 1 << 30;
    pub const EXCLUSIVE: u32 = 1 << 29;
    pub const TEMPORARY: u32 = 1 << 28;
    pub const SYMLINK: u32 = 1 << 27;
    pub const DEVICE: u32 = 1 << 26;
    pub const NAMED_PIPE: u32 = 1 << 25;
    pub const SOCKET: u32 = 1 << 24;
    pub const SETUID: u32 = 1 << 23;
    pub const SETGID: u32 = 1 << 22;
    pub const CHAR_DEVICE: u32 = 1 << 21;
    pub const STICKY: u32 = 1 << 20;
    pub const IRREGULAR: u32 = 1 << 19;
    pub const TYPE: u32 = DIR | SYMLINK | NAMED_PIPE | SOCKET | DEVICE | CHAR_DEVICE | IRREGULAR;
    pub const PERM: u32 = 0o777;

    pub fn is_dir(m: u32) -> bool {
        m & DIR != 0
    }
    pub fn is_regular(m: u32) -> bool {
        m & TYPE == 0
    }
    pub fn is_symlink(m: u32) -> bool {
        m & SYMLINK != 0
    }

    /// Formats like Go's FileMode.String, e.g. "drwxr-xr-x".
    pub fn string(m: u32) -> String {
        const STR: &[u8] = b"dalTLDpSugct?";
        let mut out = String::with_capacity(12);
        for (i, &c) in STR.iter().enumerate() {
            if m & (1 << (31 - i)) != 0 {
                out.push(c as char);
            }
        }
        if out.is_empty() {
            out.push('-');
        }
        for (i, c) in "rwxrwxrwx".chars().enumerate() {
            out.push(if m & (1 << (8 - i)) != 0 { c } else { '-' });
        }
        out
    }
}

/// Converts a Unix st_mode to the Go FileMode layout.
pub fn mode_from_unix(m: u32) -> u32 {
    let mut out = m & 0o777;
    out |= match m & libc::S_IFMT {
        libc::S_IFDIR => mode::DIR,
        libc::S_IFLNK => mode::SYMLINK,
        libc::S_IFIFO => mode::NAMED_PIPE,
        libc::S_IFSOCK => mode::SOCKET,
        libc::S_IFCHR => mode::DEVICE | mode::CHAR_DEVICE,
        libc::S_IFBLK => mode::DEVICE,
        _ => 0,
    };
    if m & libc::S_ISUID != 0 {
        out |= mode::SETUID;
    }
    if m & libc::S_ISGID != 0 {
        out |= mode::SETGID;
    }
    if m & libc::S_ISVTX != 0 {
        out |= mode::STICKY;
    }
    out
}

/// Metadata of one directory entry.
#[derive(Debug, Default)]
pub struct Meta {
    pub name: Vec<u8>,
    pub mode: u32, // Go FileMode layout
    pub size: i64,
    pub alloc: i64,
    pub alloc_known: bool,
    pub mtime: i64, // unix nanoseconds
    pub btime: i64, // birth time, 0 if unknown
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,
    pub dev: u64,
    pub ino: u64,
    pub has_ino: bool,
    pub err: Option<io::Error>, // stat failed; only name is valid
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
    let fd = cvt(unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) })?;
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[cfg(target_os = "linux")]
static NO_STATX: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "linux")]
fn stat_at(dirfd: RawFd, name: &CStr, flags: libc::c_int) -> io::Result<Meta> {
    if !NO_STATX.load(Ordering::Relaxed) {
        let mut st: libc::statx = unsafe { std::mem::zeroed() };
        let mask = libc::STATX_BASIC_STATS | libc::STATX_BTIME;
        let r = unsafe { libc::statx(dirfd, name.as_ptr(), flags | libc::AT_STATX_DONT_SYNC, mask, &mut st) };
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
    Meta {
        mode: mode_from_unix(st.st_mode as u32),
        size: st.st_size as i64,
        alloc: st.st_blocks as i64 * 512,
        alloc_known: true,
        mtime: st.st_mtime as i64 * 1_000_000_000 + st.st_mtime_nsec as i64,
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

/// Streams the entries of a directory in chunks of up to `chunk`, stat-ing
/// each entry relative to the directory fd (no repeated path resolution).
/// Returning an error from f stops the read and returns it.
pub fn read_dir<E: From<io::Error>>(path: &[u8], chunk: usize, mut f: impl FnMut(Vec<Meta>) -> Result<(), E>) -> Result<(), E> {
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
        unsafe { *libc::__errno_location() = 0 };
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
        let mut m = stat_at(raw, name, libc::AT_SYMLINK_NOFOLLOW).unwrap_or_else(|e| Meta { err: Some(e), ..Default::default() });
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

fn stat_path(path: &[u8], follow: bool) -> io::Result<Meta> {
    let c = cstr(path)?;
    let flags = if follow { 0 } else { libc::AT_SYMLINK_NOFOLLOW };
    let mut m = stat_at(libc::AT_FDCWD, &c, flags)?;
    m.name = base_name(path).to_vec();
    Ok(m)
}

/// Metadata for path without following a final symlink.
pub fn lstat(path: &[u8]) -> io::Result<Meta> {
    stat_path(path, false)
}

/// Metadata for path, following symlinks.
pub fn stat(path: &[u8]) -> io::Result<Meta> {
    stat_path(path, true)
}

/// Joins a directory and an entry name without cleaning.
pub fn join_path(dir: &[u8], name: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(dir.len() + name.len() + 1);
    p.extend_from_slice(dir);
    if dir.last() != Some(&b'/') {
        p.push(b'/');
    }
    p.extend_from_slice(name);
    p
}

fn base_name(path: &[u8]) -> &[u8] {
    let mut end = path.len();
    while end > 1 && path[end - 1] == b'/' {
        end -= 1;
    }
    let p = &path[..end];
    match p.iter().rposition(|&b| b == b'/') {
        Some(i) if p.len() > 1 => &p[i + 1..],
        _ => p,
    }
}

static NAMES: Mutex<Option<(HashMap<u32, String>, HashMap<u32, String>)>> = Mutex::new(None);

fn lookup_name(group: bool, id: u32) -> String {
    let mut g = NAMES.lock().unwrap();
    let (users, groups) = g.get_or_insert_with(Default::default);
    let cache = if group { groups } else { users };
    if let Some(n) = cache.get(&id) {
        return n.clone();
    }
    let mut buf = vec![0u8; 16384];
    let name = unsafe {
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
    .unwrap_or_else(|| id.to_string());
    cache.insert(id, name.clone());
    name
}

/// Resolves a uid to a name (cached; falls back to the number).
pub fn user_name(uid: u32) -> String {
    lookup_name(false, uid)
}

/// Resolves a gid to a name (cached; falls back to the number).
pub fn group_name(gid: u32) -> String {
    lookup_name(true, gid)
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
/// that should not be scanned as disk usage.
#[cfg(target_os = "linux")]
pub fn is_virtual_fs(path: &[u8]) -> bool {
    let Ok(c) = cstr(path) else { return false };
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return false;
    }
    VIRTUAL_FS.contains(&(st.f_type as i64))
}

#[cfg(not(target_os = "linux"))]
pub fn is_virtual_fs(_: &[u8]) -> bool {
    false
}

/// What the scan recorded for an item about to be trashed or deleted. The
/// operation refuses an item whose owner or file type no longer matches.
#[derive(Clone, Copy, Debug)]
pub struct Expect {
    pub uid: u32,
    pub mode: u32, // only the type bits are compared
}

const CHANGED: &str = "item changed since the scan (owner or type differs); rescan first";

fn other(msg: impl Into<String>) -> io::Error {
    io::Error::other(msg.into())
}

/// Guards helper invocations: an absolute path can never be parsed as an
/// option by the helper program.
fn check_abs(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(other("refusing relative path"));
    }
    std::fs::symlink_metadata(path).map(|_| ())
}

/// Opens a file whose content is about to be read. It does not follow a
/// final symlink, and O_NONBLOCK keeps a FIFO swapped in since the scan from
/// blocking open(2) forever; callers still check the file type.
pub fn open_content(path: &Path) -> io::Result<File> {
    std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)
}

/// Runs a helper program with an argument vector (never a shell), detached
/// from the terminal so it cannot disturb the TUI.
fn start_detached(name: &str, arg: &Path) -> io::Result<()> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(name).arg(arg).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// The user-facing name of the trash.
pub const TRASH_NAME: &str = "trash";

/// Opens the item's directory in the desktop file manager.
pub fn reveal(path: &Path) -> io::Result<()> {
    check_abs(path)?;
    let md = std::fs::symlink_metadata(path)?;
    let target = if md.is_dir() { path } else { path.parent().unwrap_or(path) };
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    start_detached(opener, target)
}

/// Whether there is no graphical session (no X or Wayland display), so there
/// is no file manager to open and no desktop trash anyone will empty.
pub fn headless() -> bool {
    if cfg!(target_os = "macos") {
        return false;
    }
    std::env::var_os("DISPLAY").is_none_or(|v| v.is_empty()) && std::env::var_os("WAYLAND_DISPLAY").is_none_or(|v| v.is_empty())
}

fn split_parent(path: &Path) -> io::Result<(&Path, &OsStr)> {
    match (path.parent(), path.file_name()) {
        (Some(d), Some(b)) => Ok((d, b)),
        _ => Err(other("refusing to operate on a filesystem root")),
    }
}

fn check_expect(dirfd: RawFd, base: &CStr, want: Expect) -> io::Result<libc::stat> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    cvt(unsafe { libc::fstatat(dirfd, base.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) })?;
    if st.st_uid != want.uid || mode_from_unix(st.st_mode as u32) & mode::TYPE != want.mode & mode::TYPE {
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
    remove_at(dfd.as_raw_fd(), &cbase, st.st_mode & libc::S_IFMT == libc::S_IFDIR)
}

fn remove_at(dirfd: RawFd, name: &CStr, is_dir: bool) -> io::Result<()> {
    if !is_dir {
        return cvt(unsafe { libc::unlinkat(dirfd, name.as_ptr(), 0) }).map(|_| ());
    }
    let fd = cvt(unsafe {
        libc::openat(dirfd, name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
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
                libc::DT_UNKNOWN => fstat_at(sub.as_raw_fd(), n, libc::AT_SYMLINK_NOFOLLOW).is_ok_and(|m| mode::is_dir(m.mode)),
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
fn rename_checked(path: &Path, dst: &Path, want: Expect) -> io::Result<()> {
    let (dir, base) = split_parent(path)?;
    let dfd = open_dir(dir.as_os_str().as_bytes())?;
    let cbase = cstr(base.as_bytes())?;
    check_expect(dfd.as_raw_fd(), &cbase, want)?;
    let cdst = cstr(dst.as_os_str().as_bytes())?;
    cvt(unsafe { libc::renameat(dfd.as_raw_fd(), cbase.as_ptr(), libc::AT_FDCWD, cdst.as_ptr()) }).map(|_| ())
}

/// Creates dir (mode 0700) if missing; an existing entry must be a real
/// directory (not a symlink) owned by us. With strict (trash on a shared
/// filesystem) it must also be inaccessible to others, as the freedesktop.org
/// spec requires.
fn ensure_private_dir(dir: &Path, strict: bool) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
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

/// The top directory of the filesystem containing path.
fn mount_top(path: &Path, dev: u64) -> PathBuf {
    let mut cur = path.parent().unwrap_or(path).to_path_buf();
    loop {
        let Some(parent) = cur.parent() else { return cur };
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
    let mut t: libc::time_t = unsafe { libc::time(std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&mut t, &mut tm) };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// Moves path to the freedesktop.org trash: the home trash when on the same
/// filesystem, otherwise $topdir/.Trash-$uid on the item's filesystem. It
/// never copies across filesystems and never deletes.
pub fn trash(path: &Path, want: Expect) -> io::Result<()> {
    check_abs(path)?;
    let dev = std::fs::symlink_metadata(path)?.dev();
    let data_home = match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => {
            let home = std::env::var_os("HOME").filter(|v| !v.is_empty()).ok_or_else(|| other("$HOME is not defined"))?;
            PathBuf::from(home).join(".local/share")
        }
    };
    let home = data_home.join("Trash");
    let (trash, info_path): (PathBuf, Vec<u8>) = match std::fs::metadata(&data_home) {
        Ok(m) if m.dev() == dev => (home.clone(), path.as_os_str().as_bytes().to_vec()),
        _ => {
            let top = mount_top(path, dev);
            let rel = path.strip_prefix(&top).map_err(|e| other(e.to_string()))?;
            (top.join(format!(".Trash-{}", unsafe { libc::getuid() })), rel.as_os_str().as_bytes().to_vec())
        }
    };
    let shared = trash != home;
    if !shared {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&trash)?;
    } else if let Err(e) = ensure_private_dir(&trash, true) {
        // On a shared filesystem another user could plant a symlink or a
        // world-writable directory here; refuse rather than follow it.
        return Err(other(format!("trash directory {} is unsafe: {e}", trash.display())));
    }
    let (files, info) = (trash.join("files"), trash.join("info"));
    for d in [&files, &info] {
        ensure_private_dir(d, shared).map_err(|e| other(format!("trash directory {} is unsafe: {e}", d.display())))?;
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
        let mut f = match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&ip) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        let w = write!(f, "[Trash Info]\nPath={}\nDeletionDate={}\n", percent_encode_path(&info_path), local_timestamp()).and_then(|_| f.sync_all());
        drop(f);
        if let Err(e) = w.and_then(|_| rename_checked(path, &dst, want)) {
            let _ = std::fs::remove_file(&ip);
            return Err(e);
        }
        return Ok(());
    }
    Err(other("trash: too many items with the same name"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_string_matches_go() {
        assert_eq!(mode::string(mode::DIR | 0o755), "drwxr-xr-x");
        assert_eq!(mode::string(0o644), "-rw-r--r--");
        assert_eq!(mode::string(mode::SYMLINK | 0o777), "Lrwxrwxrwx");
    }

    #[test]
    fn delete_refuses_changed_type() {
        let d = std::env::temp_dir().join(format!("mapsize-del-{}", std::process::id()));
        std::fs::create_dir_all(d.join("sub/inner")).unwrap();
        std::fs::write(d.join("sub/inner/f"), b"x").unwrap();
        let uid = unsafe { libc::getuid() };
        assert!(delete(&d.join("sub"), Expect { uid, mode: 0 }).is_err());
        delete(&d.join("sub"), Expect { uid, mode: mode::DIR }).unwrap();
        assert!(!d.join("sub").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }
}

/// Shell-style glob match with Go filepath.Match semantics: `*` and `?` do
/// not match '/'. Unlike Go, glibc treats a malformed bracket literally.
pub fn glob_match(pattern: &[u8], name: &[u8]) -> bool {
    let (Ok(p), Ok(n)) = (CString::new(pattern), CString::new(name)) else { return false };
    unsafe { libc::fnmatch(p.as_ptr(), n.as_ptr(), libc::FNM_PATHNAME) == 0 }
}

/// Lexically cleans a path like Go's filepath.Clean.
pub fn clean_path(p: &[u8]) -> Vec<u8> {
    if p.is_empty() {
        return b".".to_vec();
    }
    let rooted = p[0] == b'/';
    let mut parts: Vec<&[u8]> = Vec::new();
    for c in p.split(|&b| b == b'/') {
        match c {
            b"" | b"." => {}
            b".." => {
                if parts.last().is_some_and(|l| *l != b"..") {
                    parts.pop();
                } else if !rooted {
                    parts.push(c);
                }
            }
            _ => parts.push(c),
        }
    }
    let mut out = Vec::with_capacity(p.len());
    if rooted {
        out.push(b'/');
    }
    out.extend_from_slice(&parts.join(&b'/'));
    if out.is_empty() {
        out.push(b'.');
    }
    out
}

/// Makes a path absolute (relative to the working directory) and cleans it.
pub fn abs_path(p: &[u8]) -> io::Result<Vec<u8>> {
    if p.first() == Some(&b'/') {
        return Ok(clean_path(p));
    }
    let cwd = std::env::current_dir()?;
    Ok(clean_path(&join_path(cwd.as_os_str().as_bytes(), p)))
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn clean_and_glob() {
        assert_eq!(clean_path(b"/a/b/../c/./"), b"/a/c");
        assert_eq!(clean_path(b"/.."), b"/");
        assert_eq!(clean_path(b"a/../../b"), b"../b");
        assert!(glob_match(b"*.log", b"x.log"));
        assert!(!glob_match(b"*.log", b"d/x.log"));
        assert!(glob_match(b"/tmp/*/x", b"/tmp/a/x"));
    }
}
