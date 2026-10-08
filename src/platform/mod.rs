//! OS-specific filesystem metadata and file operations behind a small common
//! API. Nothing else in the program calls the OS directly for filesystem
//! work; the per-OS halves live in `unix.rs` and `windows.rs`.
//!
//! Per OS:
//! * Linux: statx relative to the directory fd (birth time), virtual
//!   filesystem detection, freedesktop.org trash, xdg-open.
//! * macOS: fstatat relative to the directory fd, Finder trash, `open -R`.
//! * FreeBSD, NetBSD: fstatat relative to the directory fd; no trash or
//!   reveal (permanent delete only, offered on headless systems).
//! * Windows: std directory listing and metadata. Allocated size is unknown
//!   and there is no inode identity, so symlinks are never followed and hard
//!   links are not deduplicated. Recycle Bin and Explorer reveal; no
//!   permanent delete.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

/// File mode bits as stored in snapshots, identical on every OS so that
/// snapshots are portable.
///
/// The low 9 bits are the rwxrwxrwx permissions. Bits 31 down to 19 are
/// flags, rendered by [`mode::string`] with one letter each:
///
/// | bit | letter | meaning |
/// |-----|--------|---------|
/// | 31 | d | directory |
/// | 30 | a | append-only |
/// | 29 | l | exclusive use |
/// | 28 | T | temporary |
/// | 27 | L | symbolic link |
/// | 26 | D | device |
/// | 25 | p | named pipe (FIFO) |
/// | 24 | S | Unix domain socket |
/// | 23 | u | setuid |
/// | 22 | g | setgid |
/// | 21 | c | character device (with D) |
/// | 20 | t | sticky |
/// | 19 | ? | irregular (none of the above types) |
///
/// A regular file has none of the type bits (d L D p S c ?) set.
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

    /// Formats the mode as the set flag letters (in bit order, or "-" if
    /// none) followed by rwxrwxrwx, e.g. "drwxr-xr-x", "Lrwxrwxrwx".
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

/// Metadata of one directory entry.
#[derive(Debug, Default)]
pub struct Meta {
    pub name: Vec<u8>,
    pub mode: u32, // see `mode`
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
    pub has_ino: bool,          // dev/ino are meaningful
    pub err: Option<io::Error>, // stat failed; only name is valid
}

/// Metadata for path without following a final symlink.
pub fn lstat(path: &[u8]) -> io::Result<Meta> {
    stat_path(path, false)
}

/// Metadata for path, following symlinks.
pub fn stat(path: &[u8]) -> io::Result<Meta> {
    stat_path(path, true)
}

/// What the scan recorded for an item about to be trashed or deleted. The
/// operation refuses an item whose owner or file type no longer matches.
#[derive(Clone, Copy, Debug)]
pub struct Expect {
    pub uid: u32,
    pub mode: u32, // only the type bits are compared
}

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

/// Runs a helper program with an argument vector (never a shell), detached
/// from the terminal so it cannot disturb the TUI.
#[allow(dead_code)] // unused where reveal is unsupported
fn start_detached(name: &str, args: &[&OsStr]) -> io::Result<()> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(name)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Reads exactly buf.len() bytes at offset off, without relying on (or, on
/// Unix, moving) the file cursor.
pub fn read_exact_at(f: &File, buf: &mut [u8], off: u64) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        f.read_exact_at(buf, off)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let (mut buf, mut off) = (buf, off);
        while !buf.is_empty() {
            match f.seek_read(buf, off) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => {
                    buf = &mut buf[n..];
                    off += n as u64;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

// ---- Paths ----------------------------------------------------------------

/// The path separator: '/' on Unix, '\\' on Windows.
pub const SEP: u8 = if cfg!(windows) { b'\\' } else { b'/' };

/// Whether b separates path components ('/' everywhere, also '\\' on
/// Windows).
pub fn is_sep(b: u8) -> bool {
    b == b'/' || cfg!(windows) && b == b'\\'
}

/// The raw bytes of an OS string. On Windows this is UTF-8 (WTF-8 for
/// unpaired surrogates).
pub fn os_bytes(s: &OsStr) -> &[u8] {
    s.as_encoded_bytes()
}

/// The path named by raw bytes. On Windows the bytes are decoded as UTF-8,
/// replacing invalid sequences.
pub fn path_from_bytes(b: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(OsStr::from_bytes(b))
    }
    #[cfg(windows)]
    {
        PathBuf::from(String::from_utf8_lossy(b).into_owned())
    }
}

/// Joins a directory and an entry name without cleaning.
pub fn join_path(dir: &[u8], name: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(dir.len() + name.len() + 1);
    p.extend_from_slice(dir);
    if !dir.last().is_some_and(|&b| is_sep(b)) {
        p.push(SEP);
    }
    p.extend_from_slice(name);
    p
}

fn base_name(path: &[u8]) -> &[u8] {
    let mut end = path.len();
    while end > 1 && is_sep(path[end - 1]) {
        end -= 1;
    }
    let p = &path[..end];
    match p.iter().rposition(|&b| is_sep(b)) {
        Some(i) if p.len() > 1 => &p[i + 1..],
        _ => p,
    }
}

/// Lexically cleans a Unix path: collapses repeated separators, removes "."
/// and resolves ".." against the preceding component.
#[cfg(unix)]
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
    #[cfg(unix)]
    {
        if p.first() == Some(&b'/') {
            return Ok(clean_path(p));
        }
        let cwd = std::env::current_dir()?;
        Ok(clean_path(&join_path(os_bytes(cwd.as_os_str()), p)))
    }
    #[cfg(windows)]
    {
        // GetFullPathNameW resolves "." and "..", the drive-relative forms
        // and '/'; collecting the components drops a trailing separator.
        let a: PathBuf = std::path::absolute(path_from_bytes(p))?
            .components()
            .collect();
        Ok(os_bytes(a.as_os_str()).to_vec())
    }
}

// ---- Owner names ----------------------------------------------------------

type NameCache = (HashMap<u32, String>, HashMap<u32, String>);
static NAMES: Mutex<Option<NameCache>> = Mutex::new(None);

fn lookup_name(group: bool, id: u32) -> String {
    let mut g = NAMES.lock().unwrap();
    let (users, groups) = g.get_or_insert_with(Default::default);
    let cache = if group { groups } else { users };
    if let Some(n) = cache.get(&id) {
        return n.clone();
    }
    let name = lookup_os_name(group, id).unwrap_or_else(|| id.to_string());
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

// ---- Local time -----------------------------------------------------------

/// A broken-down local time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i64,
    pub month: i64, // 1-12
    pub day: i64,   // 1-31
    pub hour: i64,
    pub min: i64,
    pub sec: i64,
    pub off: i32, // seconds east of UTC
}

/// Converts unix seconds to local time in the system time zone.
pub fn local_time(secs: i64) -> LocalTime {
    let off = local_offset(secs);
    let s = secs + off as i64;
    let (year, month, day) = civil_from_days(s.div_euclid(86400));
    let rem = s.rem_euclid(86400);
    LocalTime {
        year,
        month,
        day,
        hour: rem / 3600,
        min: rem / 60 % 60,
        sec: rem % 60,
        off,
    }
}

/// Unix seconds of a local wall-clock time in the system time zone. A time
/// skipped or repeated by a DST change resolves to one of its neighbours.
pub fn local_to_unix(y: i64, mo: i64, d: i64, h: i64, mi: i64) -> i64 {
    let wall = days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60;
    let off = local_offset(wall) as i64;
    let t = wall - off;
    let off2 = local_offset(t) as i64;
    if off2 == off {
        t
    } else {
        wall - off2
    }
}

/// Year, month (1-12) and day of days since 1970-01-01 in the proleptic
/// Gregorian calendar (Howard Hinnant's civil_from_days).
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

// ---- Glob -----------------------------------------------------------------

// On Windows '\\' is the separator and cannot escape.
const ESCAPE: bool = cfg!(unix);

/// Shell-style glob match: `*` matches any run of non-separator bytes, `?`
/// one non-separator character, `[...]` a character class (`[^...]`
/// negates, `a-z` ranges), and on Unix `\\` escapes the next character. The
/// whole name must match. A malformed pattern matches nothing.
pub fn glob_match(pattern: &[u8], name: &[u8]) -> bool {
    // Fast path for the common "*.ext" shape: * cannot cross a separator.
    if let Some(suffix) = pattern.strip_prefix(b"*") {
        if !suffix.iter().any(|b| b"*?[\\".contains(b)) {
            return name.ends_with(suffix) && !name[..name.len() - suffix.len()].contains(&SEP);
        }
    }
    glob(pattern, name).unwrap_or(false)
}

struct BadPattern;

fn decode_rune(s: &[u8]) -> (u32, usize) {
    match s.utf8_chunks().next() {
        Some(c) if !c.valid().is_empty() => {
            let ch = c.valid().chars().next().unwrap();
            (ch as u32, ch.len_utf8())
        }
        _ => (0xFFFD, 1),
    }
}

fn glob(mut pattern: &[u8], mut name: &[u8]) -> Result<bool, BadPattern> {
    'pattern: while !pattern.is_empty() {
        let (star, chunk, rest) = scan_chunk(pattern);
        pattern = rest;
        if star && chunk.is_empty() {
            // Trailing * matches the rest of the string unless it has a separator.
            return Ok(!name.contains(&SEP));
        }
        let r = match_chunk(chunk, name);
        if let Ok(Some(t)) = r {
            // The last chunk must exhaust the name.
            if t.is_empty() || !pattern.is_empty() {
                name = t;
                continue;
            }
        }
        r?;
        if star {
            // Look for a match skipping i+1 bytes; cannot skip a separator.
            let mut i = 0;
            while i < name.len() && name[i] != SEP {
                if let Some(t) = match_chunk(chunk, &name[i + 1..])? {
                    if !pattern.is_empty() || t.is_empty() {
                        name = t;
                        continue 'pattern;
                    }
                }
                i += 1;
            }
        }
        // Before failing, check the rest of the pattern is well-formed.
        while !pattern.is_empty() {
            let (_, chunk, rest) = scan_chunk(pattern);
            pattern = rest;
            match_chunk(chunk, b"")?;
        }
        return Ok(false);
    }
    Ok(name.is_empty())
}

fn scan_chunk(mut pattern: &[u8]) -> (bool, &[u8], &[u8]) {
    let mut star = false;
    while pattern.first() == Some(&b'*') {
        pattern = &pattern[1..];
        star = true;
    }
    let mut in_range = false;
    let mut i = 0;
    while i < pattern.len() {
        match pattern[i] {
            b'\\' if ESCAPE && i + 1 < pattern.len() => i += 1,
            b'[' => in_range = true,
            b']' => in_range = false,
            b'*' if !in_range => break,
            _ => {}
        }
        i += 1;
    }
    (star, &pattern[..i], &pattern[i..])
}

/// Matches one chunk against the start of s, returning the rest of s.
fn match_chunk<'a>(mut chunk: &[u8], mut s: &'a [u8]) -> Result<Option<&'a [u8]>, BadPattern> {
    // After a failure, keep parsing chunk to validate it, but stop reading s.
    let mut failed = false;
    while !chunk.is_empty() {
        if !failed && s.is_empty() {
            failed = true;
        }
        match chunk[0] {
            b'[' => {
                let mut r = 0;
                if !failed {
                    let (c, n) = decode_rune(s);
                    r = c;
                    s = &s[n..];
                }
                chunk = &chunk[1..];
                let negated = chunk.first() == Some(&b'^');
                if negated {
                    chunk = &chunk[1..];
                }
                let (mut matched, mut nrange) = (false, 0);
                loop {
                    if chunk.first() == Some(&b']') && nrange > 0 {
                        chunk = &chunk[1..];
                        break;
                    }
                    let (lo, rest) = get_esc(chunk)?;
                    chunk = rest;
                    let mut hi = lo;
                    if chunk[0] == b'-' {
                        (hi, chunk) = get_esc(&chunk[1..])?;
                    }
                    if lo <= r && r <= hi {
                        matched = true;
                    }
                    nrange += 1;
                }
                if matched == negated {
                    failed = true;
                }
            }
            b'?' => {
                if !failed {
                    if s[0] == SEP {
                        failed = true;
                    }
                    s = &s[decode_rune(s).1..];
                }
                chunk = &chunk[1..];
            }
            c => {
                let c = if ESCAPE && c == b'\\' {
                    chunk = &chunk[1..];
                    *chunk.first().ok_or(BadPattern)?
                } else {
                    c
                };
                if !failed {
                    if c != s[0] {
                        failed = true;
                    }
                    s = &s[1..];
                }
                chunk = &chunk[1..];
            }
        }
    }
    Ok((!failed).then_some(s))
}

fn get_esc(mut chunk: &[u8]) -> Result<(u32, &[u8]), BadPattern> {
    if chunk.is_empty() || chunk[0] == b'-' || chunk[0] == b']' {
        return Err(BadPattern);
    }
    if ESCAPE && chunk[0] == b'\\' {
        chunk = &chunk[1..];
        if chunk.is_empty() {
            return Err(BadPattern);
        }
    }
    let (r, n) = decode_rune(chunk);
    if (r == 0xFFFD && n == 1) || chunk.len() == n {
        return Err(BadPattern);
    }
    Ok((r, &chunk[n..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_string() {
        assert_eq!(mode::string(mode::DIR | 0o755), "drwxr-xr-x");
        assert_eq!(mode::string(0o644), "-rw-r--r--");
        assert_eq!(mode::string(mode::SYMLINK | 0o777), "Lrwxrwxrwx");
    }

    #[test]
    fn civil_round_trip() {
        for days in [-719468, -1, 0, 59, 11016, 20000, 2932896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
    }

    #[test]
    fn local_time_round_trip() {
        let t = 1_780_000_000; // 2026-05-28
        let lt = local_time(t);
        assert_eq!(
            local_to_unix(lt.year, lt.month, lt.day, lt.hour, lt.min) + lt.sec,
            t
        );
    }

    #[test]
    fn join_and_base() {
        let s = SEP as char;
        assert_eq!(join_path(b"dir", b"x"), format!("dir{s}x").into_bytes());
        assert_eq!(
            join_path(format!("dir{s}").as_bytes(), b"x"),
            format!("dir{s}x").into_bytes()
        );
        assert_eq!(base_name(format!("a{s}b{s}").as_bytes()), b"b");
    }

    #[cfg(unix)]
    #[test]
    fn clean_and_glob() {
        assert_eq!(clean_path(b"/a/b/../c/./"), b"/a/c");
        assert_eq!(clean_path(b"/.."), b"/");
        assert_eq!(clean_path(b"a/../../b"), b"../b");
        assert!(glob_match(b"*.log", b"x.log"));
        assert!(!glob_match(b"*.log", b"d/x.log"));
        assert!(glob_match(b"/tmp/*/x", b"/tmp/a/x"));
        assert!(!glob_match(b"[", b"["));
        assert!(glob_match(b"a[^b-d]?", "ax\u{e9}".as_bytes()));
        assert!(!glob_match(b"a[^b-d]c", b"abc"));
        assert!(glob_match(b"\\*x", b"*x"));
        assert!(glob_match(b"*.dat", b"f1.dat"));
        assert!(!glob_match(b"*.dat", b"f1.dax"));
        assert!(glob_match(b"a*b*c", b"axxbyyc"));
        assert!(glob_match(b"*/x", b"a/x") && !glob_match(b"*/x", b"a/b/x"));
        assert!(glob_match(b"*", b"abc") && !glob_match(b"*", b"a/c"));
        assert!(!glob_match(b"*.dat", b"x.dat/y.da") && !glob_match(b"*.dat", b"d/x.dat"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_glob_and_abs() {
        assert!(glob_match(br"C:\tmp\*\x", br"C:\tmp\a\x"));
        assert!(!glob_match(b"*.log", br"d\x.log"));
        assert!(glob_match(b"*.log", b"d/x.log"));
        let a = abs_path(br"C:\a\b\..\c\").unwrap();
        assert_eq!(a, br"C:\a\c");
    }
}
