use super::NodeId;
use std::io;
use std::time::{Duration, SystemTime};

/// Classifies scan errors for reporting.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum ErrKind {
    Permission,
    Vanished,
    Io,
    Loop,
    #[default]
    Other,
}

pub const NUM_ERR_KINDS: usize = 5;

impl ErrKind {
    pub fn from_u8(v: u8) -> ErrKind {
        [ErrKind::Permission, ErrKind::Vanished, ErrKind::Io, ErrKind::Loop, ErrKind::Other]
            .get(v as usize)
            .copied()
            .unwrap_or(ErrKind::Other)
    }
    pub fn name(self) -> &'static str {
        ["Permission denied", "Vanished during scan", "I/O error", "Symlink loop", "Other error"][self as usize]
    }
}

/// Maps an OS error to an ErrKind.
pub fn classify(err: &io::Error) -> ErrKind {
    match err.raw_os_error() {
        Some(libc::EACCES) | Some(libc::EPERM) => ErrKind::Permission,
        Some(libc::ENOENT) => ErrKind::Vanished,
        Some(libc::ELOOP) => ErrKind::Loop,
        Some(libc::EIO) => ErrKind::Io,
        _ => match err.kind() {
            io::ErrorKind::PermissionDenied => ErrKind::Permission,
            io::ErrorKind::NotFound => ErrKind::Vanished,
            _ => ErrKind::Other,
        },
    }
}

/// One recorded scan error.
#[derive(Clone, Debug, Default)]
pub struct ErrorRecord {
    pub node: NodeId,  // node the error is attached to (the directory for listing failures)
    pub name: Vec<u8>, // entry name when no node was created (e.g. vanished)
    pub kind: ErrKind,
    pub msg: String,
}

/// Caps the detailed error list; counts are always exact.
pub const MAX_ERROR_RECORDS: usize = 100_000;

/// Describes a scan as a whole.
#[derive(Clone, Debug)]
pub struct Stats {
    pub root: Vec<u8>,
    pub start: SystemTime,
    pub end: Option<SystemTime>,
    pub complete: bool, // scan finished (possibly with errors)
    pub cancelled: bool,

    pub files: i64,
    pub dirs: i64,
    pub symlinks: i64,
    pub others: i64,

    pub err_counts: [i64; NUM_ERR_KINDS],
    pub errors: Vec<ErrorRecord>,
    pub excluded: i64,
    pub skipped_mounts: i64,
    pub virtual_skipped: i64,
    pub loops_skipped: i64,
    pub broken_links: i64,
    pub hardlink_dups: i64,
    pub unscanned: i64, // directories never read because the scan was cancelled

    pub excludes: Vec<String>,
    pub one_file_system: bool,
    pub follow: String,
    pub workers: usize,
    pub from_snapshot: String,
}

impl Default for Stats {
    fn default() -> Self {
        Stats {
            root: Vec::new(), start: SystemTime::now(), end: None, complete: false, cancelled: false,
            files: 0, dirs: 0, symlinks: 0, others: 0, err_counts: [0; NUM_ERR_KINDS], errors: Vec::new(),
            excluded: 0, skipped_mounts: 0, virtual_skipped: 0, loops_skipped: 0, broken_links: 0,
            hardlink_dups: 0, unscanned: 0, excludes: Vec::new(), one_file_system: false,
            follow: String::new(), workers: 0, from_snapshot: String::new(),
        }
    }
}

impl Stats {
    pub fn total_errors(&self) -> i64 {
        self.err_counts.iter().sum()
    }

    /// Records an error (the detailed list is capped).
    pub fn add_error(&mut self, r: ErrorRecord) {
        self.err_counts[r.kind as usize] += 1;
        if self.errors.len() < MAX_ERROR_RECORDS {
            self.errors.push(r);
        }
    }

    /// The scan duration so far (or total once finished).
    pub fn elapsed(&self) -> Duration {
        let end = self.end.unwrap_or_else(SystemTime::now);
        end.duration_since(self.start).unwrap_or_default()
    }

    /// Whether totals may under-count real usage.
    pub fn incomplete(&self) -> bool {
        !self.complete || self.cancelled || self.total_errors() > 0 || self.unscanned > 0
    }
}
