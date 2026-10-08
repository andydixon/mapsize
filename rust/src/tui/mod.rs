//! The interactive terminal interface. All drawing goes to a Canvas; the
//! event loop (run.rs) only transports the finished frame to the terminal.

mod actions;
pub mod bench;
mod canvas;
mod color256;
mod data;
mod infoview;
mod keys;
mod mapview;
mod modal;
mod modals;
mod model;
mod mouse;
mod render;
mod run;
mod search;
mod side;
mod table;
mod theme;
mod views;

#[cfg(test)]
mod tests;

pub use model::{Options, StartFn};
pub use run::run;

use crate::textutil;

// Width-aware text helpers taking the canvas's signed cell counts.

pub(crate) fn tw(s: &str) -> i32 {
    textutil::width(s) as i32
}

pub(crate) fn trunc(s: &str, w: i32) -> String {
    textutil::truncate(s, w.max(0) as usize)
}

pub(crate) fn trunc_left(s: &str, w: i32) -> String {
    textutil::truncate_left(s, w.max(0) as usize)
}

pub(crate) fn pad_left(s: &str, w: i32) -> String {
    textutil::pad_left(s, w.max(0) as usize)
}

pub(crate) fn pad_right(s: &str, w: i32) -> String {
    textutil::pad_right(s, w.max(0) as usize)
}

pub(crate) fn san(b: &[u8]) -> String {
    textutil::sanitize_bytes(b)
}

/// Whether decimal (SI) units are selected. textutil keeps the switch
/// private, so it is inferred from the formatting.
pub(crate) fn si_units() -> bool {
    textutil::size(1000).ends_with("kB")
}

fn local_tm(unix_nanos: i64) -> libc::tm {
    let secs = unix_nanos.div_euclid(1_000_000_000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    tm
}

/// Formats a timestamp as a local date ("2006-01-02").
pub(crate) fn fmt_date(unix_nanos: i64) -> String {
    let t = local_tm(unix_nanos);
    format!("{:04}-{:02}-{:02}", t.tm_year + 1900, t.tm_mon + 1, t.tm_mday)
}

/// Formats a timestamp as local date and time ("2006-01-02 15:04:05").
pub(crate) fn fmt_time(unix_nanos: i64) -> String {
    let t = local_tm(unix_nanos);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.tm_year + 1900, t.tm_mon + 1, t.tm_mday, t.tm_hour, t.tm_min, t.tm_sec)
}

/// The current local time as "20060102-1504".
pub(crate) fn fmt_stamp_now() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let t = local_tm(now.as_nanos() as i64);
    format!("{:04}{:02}{:02}-{:02}{:02}", t.tm_year + 1900, t.tm_mon + 1, t.tm_mday, t.tm_hour, t.tm_min)
}
