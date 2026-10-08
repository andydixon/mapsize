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

fn local_tm(unix_nanos: i64) -> crate::platform::LocalTime {
    crate::platform::local_time(unix_nanos.div_euclid(1_000_000_000))
}

/// Formats a timestamp as a local date ("2006-01-02").
pub(crate) fn fmt_date(unix_nanos: i64) -> String {
    let t = local_tm(unix_nanos);
    format!("{:04}-{:02}-{:02}", t.year, t.month, t.day)
}

/// Formats a timestamp as local date and time ("2006-01-02 15:04:05").
pub(crate) fn fmt_time(unix_nanos: i64) -> String {
    let t = local_tm(unix_nanos);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.year, t.month, t.day, t.hour, t.min, t.sec
    )
}

/// The current local time as "20060102-1504".
pub(crate) fn fmt_stamp_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let t = local_tm(now.as_nanos() as i64);
    format!(
        "{:04}{:02}{:02}-{:02}{:02}",
        t.year, t.month, t.day, t.hour, t.min
    )
}
