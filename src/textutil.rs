//! Display helpers: sanitising untrusted strings for the terminal,
//! width-aware truncation, and human-readable units.

use std::fmt::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Makes an untrusted string (a filename, snapshot data) safe to write to a
/// terminal. Control characters (C0, DEL, C1) and bidi overrides become
/// visible escapes; invalid UTF-8 becomes U+FFFD. Keep the original bytes
/// for filesystem operations.
pub fn sanitize_bytes(s: &[u8]) -> String {
    if s.iter().all(|&c| (0x20..0x7f).contains(&c)) {
        // SAFETY-free: printable ASCII is valid UTF-8.
        return String::from_utf8(s.to_vec()).unwrap();
    }
    let mut b = String::with_capacity(s.len() + 8);
    for chunk in s.utf8_chunks() {
        for r in chunk.valid().chars() {
            let c = r as u32;
            if c < 0x20 || c == 0x7f || (0x80..=0x9f).contains(&c) {
                let _ = write!(b, "\\x{c:02x}");
            } else if is_bidi_control(c) {
                let _ = write!(b, "\\u{c:04x}");
            } else {
                b.push(r);
            }
        }
        for _ in chunk.invalid() {
            b.push('\u{FFFD}');
        }
    }
    b
}

/// sanitize_bytes for strings.
pub fn sanitize(s: &str) -> String {
    sanitize_bytes(s.as_bytes())
}

fn is_bidi_control(r: u32) -> bool {
    (0x202a..=0x202e).contains(&r)
        || (0x2066..=0x2069).contains(&r)
        || r == 0x200e
        || r == 0x200f
        || r == 0x061c
}

/// Calls f for each grapheme cluster of s with its display width. Clusters
/// whose width terminals disagree on (per-char wcwidth versus grapheme
/// width, e.g. emoji ZWJ sequences) are replaced by U+FFFD with width 1, and
/// zero-width clusters are dropped, so measuring and drawing always agree
/// with the terminal. f returns false to stop.
pub fn clusters(s: &str, mut f: impl FnMut(&str, usize) -> bool) {
    for c in s.graphemes(true) {
        let mut chars = c.chars();
        let first = chars.next().unwrap();
        let w = if chars.next().is_none() {
            first.width().unwrap_or(0)
        } else {
            c.width()
        };
        if w == 0 {
            continue;
        }
        let ok = if c.len() > 1 && c.chars().map(|r| r.width().unwrap_or(0)).sum::<usize>() != w {
            f("\u{FFFD}", 1)
        } else {
            f(c, w)
        };
        if !ok {
            return;
        }
    }
}

/// Display width of s in terminal cells.
pub fn width(s: &str) -> usize {
    if s.bytes().all(|c| (0x20..0x7f).contains(&c)) {
        return s.len();
    }
    let mut n = 0;
    clusters(s, |_, w| {
        n += w;
        true
    });
    n
}

/// Clips s to at most w cells, appending "…" if anything was cut.
pub fn truncate(s: &str, w: usize) -> String {
    if w == 0 {
        return String::new();
    }
    if width(s) <= w {
        return s.to_string();
    }
    clip(s, w - 1) + "…"
}

/// Clips from the left, prefixing "…" (for paths and breadcrumbs where the
/// tail is the informative part).
pub fn truncate_left(s: &str, w: usize) -> String {
    if w == 0 {
        return String::new();
    }
    let total = width(s);
    if total <= w {
        return s.to_string();
    }
    let mut skip = (total - (w - 1)) as isize;
    let mut b = String::from("…");
    clusters(s, |c, cw| {
        if skip > 0 {
            skip -= cw as isize;
            if skip < 0 {
                // A wide cluster overshot by one cell; pad to keep width exact.
                b.push_str(&" ".repeat((-skip) as usize));
            }
            return true;
        }
        b.push_str(c);
        true
    });
    b
}

/// The longest prefix of s that fits in w cells.
fn clip(s: &str, w: usize) -> String {
    let mut b = String::new();
    let mut used = 0;
    clusters(s, |c, cw| {
        if used + cw > w {
            return false;
        }
        used += cw;
        b.push_str(c);
        true
    });
    b
}

/// Pads s with spaces to exactly w cells (truncating if longer).
pub fn pad_right(s: &str, w: usize) -> String {
    let mut s = truncate(s, w);
    let sw = width(&s);
    if sw < w {
        s.push_str(&" ".repeat(w - sw));
    }
    s
}

/// Right-aligns s in w cells.
pub fn pad_left(s: &str, w: usize) -> String {
    let s = truncate(s, w);
    let sw = width(&s);
    if sw < w {
        return " ".repeat(w - sw) + &s;
    }
    s
}

static SI: AtomicBool = AtomicBool::new(false);

/// Selects decimal units (kB, MB, …) instead of IEC (KiB, MiB, …). Set once
/// at start-up.
pub fn set_si(v: bool) {
    SI.store(v, Ordering::Relaxed);
}

/// Whether decimal units are selected.
pub fn si() -> bool {
    SI.load(Ordering::Relaxed)
}

const IEC_UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
const SI_UNITS: [&str; 7] = ["B", "kB", "MB", "GB", "TB", "PB", "EB"];

fn scaled(n: i64) -> (f64, &'static str) {
    let (base, units) = if si() {
        (1000.0, &SI_UNITS)
    } else {
        (1024.0, &IEC_UNITS)
    };
    let neg = n < 0;
    let mut v = (n as f64).abs();
    let mut i = 0;
    while v >= base && i < units.len() - 1 {
        v /= base;
        i += 1;
    }
    (if neg { -v } else { v }, units[i])
}

fn num(v: f64, unit: &str) -> String {
    let a = v.abs();
    if unit == "B" {
        format!("{v:.0}")
    } else if a < 10.0 {
        format!("{v:.2}")
    } else if a < 100.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.0}")
    }
}

/// Formats a byte count, e.g. "86.4 GiB".
pub fn size(n: i64) -> String {
    let (v, u) = scaled(n);
    num(v, u) + " " + u
}

/// Formats a byte count in at most ~7 cells, e.g. "86GiB".
pub fn size_compact(n: i64) -> String {
    let (v, u) = scaled(n);
    if u != "B" && v.abs() < 10.0 {
        format!("{v:.1}{u}")
    } else {
        format!("{v:.0}{u}")
    }
}

/// Formats a delta with an explicit sign.
pub fn signed_size(n: i64) -> String {
    match n {
        n if n > 0 => "+".to_string() + &size(n),
        n if n < 0 => "-".to_string() + &size(n.saturating_neg()),
        _ => "±0 B".to_string(),
    }
}

/// Formats an integer with thousands separators.
pub fn count(n: i64) -> String {
    let s = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Formats an elapsed time as mm:ss or h:mm:ss.
pub fn duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// Formats part/whole as a percentage.
pub fn percent(part: i64, whole: i64) -> String {
    if whole <= 0 {
        return "–".to_string();
    }
    let p = part as f64 * 100.0 / whole as f64;
    if p < 10.0 {
        format!("{p:.1}%")
    } else {
        format!("{p:.0}%")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_cases() {
        let cases: [(&[u8], &str); 8] = [
            (b"plain.txt", "plain.txt"),
            (b"evil\x1b[2Jname", r"evil\x1b[2Jname"),
            (b"bell\x07", r"bell\x07"),
            ("c1\u{009b}x".as_bytes(), r"c1\x9bx"),
            (b"bad\xffutf8", "bad\u{FFFD}utf8"),
            ("rtl\u{202e}gnp.exe".as_bytes(), "rtl\\u202egnp.exe"),
            ("日本語".as_bytes(), "日本語"),
            (b"tab\tnew\nline\rdel\x7f", r"tab\x09new\x0aline\x0ddel\x7f"),
        ];
        for (input, want) in cases {
            let got = sanitize_bytes(input);
            assert_eq!(got, want);
            assert!(!got.contains(['\x1b', '\x07', '\u{009b}']));
        }
    }

    #[test]
    fn truncation() {
        assert_eq!(truncate("hello world", 5), "hell…");
        assert!(width(&truncate("日本語テキスト", 5)) <= 5);
        assert_eq!(truncate("abc", 3), "abc");
        assert_eq!(truncate("abc", 0), "");
        let got = truncate_left("/a/very/long/path", 8);
        assert!(width(&got) == 8 && got.ends_with("path"), "{got}");
        assert_eq!(width(&truncate_left("ab日本語", 4)), 4);
    }

    #[test]
    fn sizes() {
        for (n, want) in [
            (0, "0 B"),
            (1023, "1023 B"),
            (1024, "1.00 KiB"),
            (1536, "1.50 KiB"),
            (181 << 30, "181 GiB"),
            (1 << 62, "4.00 EiB"),
        ] {
            assert_eq!(size(n), want);
        }
        assert_eq!(count(2184392), "2,184,392");
        assert_eq!(signed_size(-2048), "-2.00 KiB");
    }

    #[test]
    fn clusters_agree_with_width() {
        for s in ["abc", "日本", "é", "👨‍👩‍👧", "🎉x", "\u{200b}"] {
            let mut n = 0;
            clusters(s, |_, w| {
                n += w;
                true
            });
            assert_eq!(n, width(s), "{s:?}");
        }
    }
}

/// Formats unix nanoseconds as an RFC 3339 UTC timestamp, e.g.
/// "2026-10-08T09:30:00Z".
pub fn rfc3339_utc(unix_nanos: i64) -> String {
    let secs = unix_nanos.div_euclid(1_000_000_000);
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Civil-from-days (Howard Hinnant).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// Formats a duration like Go's time.Duration.String after rounding to
/// milliseconds, e.g. "1.234s", "250ms", "2m3.5s".
pub fn go_duration(d: Duration) -> String {
    let ms = (d.as_nanos() + 500_000) / 1_000_000;
    if ms == 0 {
        return "0s".into();
    }
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let (h, m, s) = (ms / 3_600_000, ms / 60_000 % 60, ms % 60_000);
    let mut secs = format!("{}.{:03}", s / 1000, s % 1000);
    while secs.ends_with('0') {
        secs.pop();
    }
    if secs.ends_with('.') {
        secs.pop();
    }
    match (h, m) {
        (0, 0) => format!("{secs}s"),
        (0, m) => format!("{m}m{secs}s"),
        (h, m) => format!("{h}h{m}m{secs}s"),
    }
}

#[cfg(test)]
mod time_tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(
            rfc3339_utc(1_791_456_000 * 1_000_000_000),
            "2026-10-08T10:40:00Z"
        );
        assert_eq!(go_duration(Duration::from_micros(1_234_400)), "1.234s");
        assert_eq!(go_duration(Duration::from_millis(250)), "250ms");
        assert_eq!(go_duration(Duration::from_millis(123_500)), "2m3.5s");
    }
}
