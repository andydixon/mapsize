//! Windows: std directory listing and metadata, Recycle Bin and Explorer.
//!
//! Allocated size is reported as unknown (it would cost an extra handle open
//! per file), and there is no inode identity, owner or link count: symlinks
//! are never followed and hard links are not deduplicated.

use super::{base_name, check_abs, mode, os_bytes, other, path_from_bytes, Expect, Meta};
use std::fs::{File, Metadata};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn nanos(t: io::Result<SystemTime>) -> i64 {
    match t {
        Ok(t) => match t.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_nanos() as i64,
            Err(e) => -(e.duration().as_nanos() as i64),
        },
        Err(_) => 0,
    }
}

/// Permissions follow the read-only attribute (0444 or 0666, plus 0111 for
/// directories). Symlinks and junctions both report as symlinks.
fn mode_of(md: &Metadata) -> u32 {
    let ft = md.file_type();
    let mut m = if md.permissions().readonly() {
        0o444
    } else {
        0o666
    };
    if ft.is_symlink() {
        m |= mode::SYMLINK;
    } else if ft.is_dir() {
        m |= mode::DIR | 0o111;
    }
    m
}

fn from_metadata(md: &Metadata) -> Meta {
    let size = md.len() as i64;
    Meta {
        mode: mode_of(md),
        size,
        alloc: size,
        mtime: nanos(md.modified()),
        btime: nanos(md.created()),
        nlink: 1,
        ..Default::default()
    }
}

/// Streams the entries of a directory in chunks of up to `chunk`. Entry
/// metadata comes from the directory listing itself and does not follow
/// symlinks. Returning an error from f stops the read and returns it.
pub fn read_dir<E: From<io::Error>>(
    path: &[u8],
    chunk: usize,
    mut f: impl FnMut(Vec<Meta>) -> Result<(), E>,
) -> Result<(), E> {
    let mut out = Vec::with_capacity(chunk);
    for de in std::fs::read_dir(path_from_bytes(path))? {
        let de = de?;
        let mut m = de
            .metadata()
            .map(|md| from_metadata(&md))
            .unwrap_or_else(|e| Meta {
                err: Some(e),
                ..Default::default()
            });
        m.name = os_bytes(&de.file_name()).to_vec();
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
    let p = path_from_bytes(path);
    let md = if follow {
        std::fs::metadata(&p)
    } else {
        std::fs::symlink_metadata(&p)
    }?;
    let mut m = from_metadata(&md);
    m.name = base_name(path).to_vec();
    Ok(m)
}

/// Windows has no numeric uid/gid; names fall back to the number.
pub(super) fn lookup_os_name(_: bool, _: u32) -> Option<String> {
    None
}

/// The system time zone's offset from UTC (seconds east) at unix time secs.
pub(super) fn local_offset(secs: i64) -> i32 {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::Time::SystemTimeToTzSpecificLocalTime;
    let (y, mo, d) = super::civil_from_days(secs.div_euclid(86400));
    if !(1601..=30827).contains(&y) {
        return 0;
    }
    let rem = secs.rem_euclid(86400);
    let utc = SYSTEMTIME {
        wYear: y as u16,
        wMonth: mo as u16,
        wDayOfWeek: 0,
        wDay: d as u16,
        wHour: (rem / 3600) as u16,
        wMinute: (rem / 60 % 60) as u16,
        wSecond: (rem % 60) as u16,
        wMilliseconds: 0,
    };
    let mut local: SYSTEMTIME = unsafe { std::mem::zeroed() };
    if unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) } == 0 {
        return 0;
    }
    let l = super::days_from_civil(local.wYear as i64, local.wMonth as i64, local.wDay as i64)
        * 86400
        + local.wHour as i64 * 3600
        + local.wMinute as i64 * 60
        + local.wSecond as i64;
    (l - secs) as i32
}

/// The host name, or "" if unknown.
pub fn hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

/// Whether path is the root of a pseudo filesystem. Windows has none.
pub fn is_virtual_fs(_: &[u8]) -> bool {
    false
}

/// Opens a file whose content is about to be read; callers check the type.
pub fn open_content(path: &Path) -> io::Result<File> {
    File::open(path)
}

/// Creates a new file (it inherits the directory's access control list).
pub fn create_private(path: &Path) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// The user-facing name of the trash.
pub const TRASH_NAME: &str = "Recycle Bin";

/// Always false: Windows always has Explorer and the Recycle Bin.
pub fn headless() -> bool {
    false
}

/// Selects the item in Explorer.
pub fn reveal(path: &Path) -> io::Result<()> {
    check_abs(path)?;
    super::start_detached("explorer.exe", &["/select,".as_ref(), path.as_os_str()])
}

/// Moves path to the Recycle Bin (SHFileOperationW with FOF_ALLOWUNDO). It
/// never deletes permanently. want is not checked: the scan records no owner
/// or inode identity on Windows.
pub fn trash(path: &Path, _: Expect) -> io::Result<()> {
    use windows_sys::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NO_UI, FO_DELETE, SHFILEOPSTRUCTW,
    };
    check_abs(path)?;
    // A list of paths, each NUL-terminated, ending with an extra NUL.
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    if from.contains(&0) {
        return Err(other("path contains NUL"));
    }
    from.extend([0, 0]);
    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    op.wFunc = FO_DELETE;
    op.pFrom = from.as_ptr();
    op.fFlags = (FOF_ALLOWUNDO | FOF_NO_UI) as u16;
    let r = unsafe { SHFileOperationW(&mut op) };
    if r != 0 {
        return Err(other(format!("SHFileOperation failed: code {r}")));
    }
    if op.fAnyOperationsAborted != 0 {
        return Err(other("operation aborted"));
    }
    Ok(())
}

/// Permanent delete is not offered on Windows; items go to the Recycle Bin.
pub fn delete(_: &Path, _: Expect) -> io::Result<()> {
    Err(other("permanent delete not supported on Windows"))
}
