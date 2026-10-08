use super::eval::{Expr, NumOp};
use super::lexer::{lex, TokKind, Token};
use crate::inventory::{self as inv, Flags};
use regex::Regex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A filterable attribute.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Name,
    Path,
    Ext,
    Type,
    Category,
    Size,
    Allocated,
    Age,
    Modified,
    Owner,
    Group,
    Files,
    Flag,
}

fn field_by_name(s: &str) -> Option<Field> {
    use Field::*;
    Some(match s {
        "name" => Name,
        "path" => Path,
        "ext" | "extension" => Ext,
        "type" | "kind" => Type,
        "category" | "cat" => Category,
        "size" => Size,
        "allocated" | "alloc" => Allocated,
        "age" => Age,
        "modified" | "mtime" => Modified,
        "owner" | "user" => Owner,
        "group" => Group,
        "files" => Files,
        "flag" | "is" => Flag,
        _ => return None,
    })
}

/// Field names for help text.
pub const FIELDS: &str = "name path ext type category size allocated age modified owner group files flag";

/// A compiled filter.
pub struct Query {
    src: String,
    pub(super) root: Expr,
    pub uses_path: bool,
}

impl std::fmt::Display for Query {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.src)
    }
}

struct Parser {
    toks: Vec<Token>,
    i: usize,
    uses_path: bool,
    now: SystemTime,
}

/// Compiles a query. now anchors relative ages (pass SystemTime::now()).
pub fn parse(s: &str, now: SystemTime) -> Result<Query, String> {
    let toks = lex(s)?;
    let mut p = Parser { toks, i: 0, uses_path: false, now };
    if p.peek().kind == TokKind::Eof {
        return Err("empty query".into());
    }
    let root = p.or()?;
    let t = p.peek();
    if t.kind != TokKind::Eof {
        return Err(format!("unexpected {:?} at {}", t.val, t.pos + 1));
    }
    Ok(Query { src: s.to_string(), root, uses_path: p.uses_path })
}

fn is_kw(t: &Token, kw: &str) -> bool {
    t.kind == TokKind::Word && t.val.eq_ignore_ascii_case(kw)
}

fn is_value(t: &Token) -> bool {
    t.kind == TokKind::Word || t.kind == TokKind::Str
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.toks[self.i]
    }

    fn next(&mut self) -> Token {
        let t = self.toks[self.i].clone();
        if t.kind != TokKind::Eof {
            self.i += 1;
        }
        t
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut l = self.and()?;
        while is_kw(self.peek(), "OR") {
            self.next();
            let r = self.and()?;
            l = Expr::Or(Box::new(l), Box::new(r));
        }
        Ok(l)
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut l = self.unary()?;
        loop {
            let t = self.peek();
            if is_kw(t, "AND") {
                self.next();
            } else if matches!(t.kind, TokKind::Eof | TokKind::RParen) || is_kw(t, "OR") {
                return Ok(l);
            }
            let r = self.unary()?;
            l = Expr::And(Box::new(l), Box::new(r));
        }
    }

    fn unary(&mut self) -> Result<Expr, String> {
        let t = self.peek().clone();
        if is_kw(&t, "NOT") {
            self.next();
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        if t.kind == TokKind::LParen {
            self.next();
            let n = self.or()?;
            if self.next().kind != TokKind::RParen {
                return Err(format!("missing ) for ( at {}", t.pos + 1));
            }
            return Ok(n);
        }
        if is_value(&t) {
            self.next();
            if let Some(f) = field_by_name(&t.val.to_lowercase()) {
                if t.kind == TokKind::Word && self.is_op(self.peek()) {
                    let op = self.next();
                    return self.cmp(f, op);
                }
            }
            return Ok(name_match(&t.val));
        }
        Err(format!("unexpected {:?} at {}", t.val, t.pos + 1))
    }

    fn is_op(&self, t: &Token) -> bool {
        t.kind == TokKind::Op || is_kw(t, "contains") || is_kw(t, "matches") || is_kw(t, "in")
    }

    fn values(&mut self) -> Result<Vec<Token>, String> {
        if self.peek().kind != TokKind::LParen {
            let t = self.next();
            if !is_value(&t) {
                return Err(format!("expected value at {}", t.pos + 1));
            }
            return Ok(vec![t]);
        }
        self.next();
        let mut out = Vec::new();
        loop {
            let t = self.next();
            if !is_value(&t) {
                return Err(format!("expected value at {}", t.pos + 1));
            }
            out.push(t);
            let n = self.next();
            match n.kind {
                TokKind::Comma => {}
                TokKind::RParen => return Ok(out),
                _ => return Err(format!("expected , or ) at {}", n.pos + 1)),
            }
        }
    }

    fn cmp(&mut self, f: Field, op_tok: Token) -> Result<Expr, String> {
        let mut op = op_tok.val.to_lowercase();
        if op == "~" {
            op = "matches".into();
        }
        let vals = self.values()?;
        if op != "in" && vals.len() != 1 {
            return Err(format!("operator {op} takes one value"));
        }
        if f == Field::Path {
            self.uses_path = true;
        }
        if matches!(f, Field::Size | Field::Allocated | Field::Age | Field::Modified | Field::Files) {
            let nop = match op.as_str() {
                "=" | "in" => NumOp::Eq,
                "!=" => NumOp::Ne,
                ">" => NumOp::Gt,
                ">=" => NumOp::Ge,
                "<" => NumOp::Lt,
                "<=" => NumOp::Le,
                _ => return Err(format!("{op} cannot be used with numeric field")),
            };
            let nums = vals.iter().map(|v| self.number(f, &v.val)).collect::<Result<Vec<_>, _>>()?;
            return Ok(Expr::Num(f, nop, nums));
        }
        // String-like fields.
        match op.as_str() {
            "contains" => Ok(Expr::Contains(f, vals[0].val.to_lowercase())),
            "matches" => match Regex::new(&format!("(?i){}", vals[0].val)) {
                Ok(re) => Ok(Expr::Re(f, re)),
                Err(e) => Err(format!("bad regular expression: {e}")),
            },
            "=" | "!=" | "in" => {
                let mut set = Vec::new();
                for v in &vals {
                    let mut s = v.val.to_lowercase();
                    match f {
                        Field::Ext => s = s.strip_prefix('.').unwrap_or(&s).to_string(),
                        Field::Category => match inv::parse_category(&s) {
                            Some(c) => s = c.name().to_lowercase(),
                            None => return Err(format!("unknown category {:?}", v.val)),
                        },
                        Field::Type => s = norm_type(&s)?.to_string(),
                        Field::Flag if flag_bit(&s).is_none() => {
                            return Err(format!(
                                "unknown flag {:?} (sparse, hardlink, error, mount, incomplete, followed, broken)",
                                v.val
                            ))
                        }
                        _ => {}
                    }
                    set.push(s);
                }
                let n = if f == Field::Flag {
                    Expr::Flag(set.iter().filter_map(|s| flag_bit(s)).fold(0, |a, b| a | b))
                } else if set.len() == 1 && has_glob(&set[0]) {
                    if !glob_ok(set[0].as_bytes()) {
                        return Err(format!("bad pattern {:?}", vals[0].val));
                    }
                    Expr::Glob(f, set.pop().unwrap())
                } else {
                    Expr::Eq(f, set)
                };
                Ok(if op == "!=" { Expr::Not(Box::new(n)) } else { n })
            }
            _ => Err(format!("operator {op} not valid for {}", op_tok.val)),
        }
    }

    /// Parses sizes ("1.5GiB", "500MB", "10k"), ages ("365d", "2w") and dates
    /// ("2024-01-31"). Ages and dates are converted to modification times
    /// (unix nanoseconds) relative to self.now.
    fn number(&self, f: Field, s: &str) -> Result<i64, String> {
        match f {
            Field::Size | Field::Allocated => parse_size(s),
            Field::Files => s.parse::<i64>().map_err(|e| format!("bad number {s:?}: {e}")),
            Field::Age => parse_age(s).map(nanos),
            Field::Modified => {
                if let Some(t) = parse_date(s) {
                    return Ok(t);
                }
                if let Ok(d) = parse_age(s) {
                    let now = self.now.duration_since(UNIX_EPOCH).map(nanos).unwrap_or(0);
                    return Ok(now.saturating_sub(nanos(d)));
                }
                Err(format!("bad date {s:?} (use YYYY-MM-DD or an age like 30d)"))
            }
            _ => Err("not numeric".into()),
        }
    }
}

fn nanos(d: Duration) -> i64 {
    d.as_nanos().min(i64::MAX as u128) as i64
}

fn has_glob(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

pub(super) fn name_match(v: &str) -> Expr {
    let lv = v.to_lowercase();
    if has_glob(v) {
        // Go's filepath.Match never matches a malformed pattern.
        return if glob_ok(lv.as_bytes()) { Expr::Glob(Field::Name, lv) } else { Expr::False };
    }
    Expr::Contains(Field::Name, lv)
}

/// Reports whether pattern is well formed by Go's filepath.Match rules
/// (fnmatch, which does the matching, accepts anything).
fn glob_ok(p: &[u8]) -> bool {
    // One possibly escaped character of a range; None if malformed.
    let esc = |mut i: usize| -> Option<usize> {
        match p.get(i)? {
            b'-' | b']' => return None,
            b'\\' => {
                i += 1;
                p.get(i)?;
            }
            _ => {}
        }
        Some(i + 1)
    };
    let mut i = 0;
    while i < p.len() {
        match p[i] {
            b'\\' => {
                if i + 1 >= p.len() {
                    return false;
                }
                i += 2;
            }
            b'[' => {
                i += 1;
                if p.get(i) == Some(&b'^') {
                    i += 1;
                }
                let mut n = 0;
                loop {
                    if p.get(i) == Some(&b']') && n > 0 {
                        i += 1;
                        break;
                    }
                    let Some(j) = esc(i) else { return false };
                    i = j;
                    if p.get(i) == Some(&b'-') {
                        let Some(j) = esc(i + 1) else { return false };
                        i = j;
                    }
                    n += 1;
                }
            }
            _ => i += 1,
        }
    }
    true
}

fn norm_type(s: &str) -> Result<&'static str, String> {
    match s {
        "f" | "file" => Ok("file"),
        "d" | "dir" | "directory" => Ok("directory"),
        "l" | "link" | "symlink" => Ok("symlink"),
        "other" | "special" => Ok("special"),
        _ => Err(format!("unknown type {s:?} (file, dir, symlink, other)")),
    }
}

fn flag_bit(s: &str) -> Option<Flags> {
    Some(match s {
        "sparse" => inv::FLAG_SPARSE,
        "hardlink" | "hardlinked" => inv::FLAG_HARDLINKED,
        "error" => inv::FLAG_ERROR,
        "mount" => inv::FLAG_MOUNT_POINT,
        "incomplete" => inv::FLAG_INCOMPLETE,
        "followed" => inv::FLAG_FOLLOWED,
        "broken" => inv::FLAG_BROKEN_LINK,
        _ => return None,
    })
}

/// Splits a leading run of digits and dots from s.
fn split_num(s: &str) -> (&str, &str) {
    let i = s.bytes().position(|c| !(c.is_ascii_digit() || c == b'.')).unwrap_or(s.len());
    s.split_at(i)
}

/// Parses a size with optional unit. kB/MB/GB/TB are SI (powers of 1000);
/// KiB/MiB/GiB/TiB are IEC (1024). Bare K/M/G/T follow the display setting
/// (textutil::set_si).
pub fn parse_size(s: &str) -> Result<i64, String> {
    let (num, unit_raw) = split_num(s);
    let Ok(v) = num.parse::<f64>() else { return Err(format!("bad size {s:?}")) };
    // ponytail: textutil exposes no SI getter; its formatting reveals the setting.
    let bare: f64 = if crate::textutil::size(1000).ends_with("kB") { 1000.0 } else { 1024.0 };
    let m = match unit_raw.trim().to_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" => bare,
        "m" => bare.powi(2),
        "g" => bare.powi(3),
        "t" => bare.powi(4),
        "p" => bare.powi(5),
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        "pb" => 1e15,
        "kib" => (1u64 << 10) as f64,
        "mib" => (1u64 << 20) as f64,
        "gib" => (1u64 << 30) as f64,
        "tib" => (1u64 << 40) as f64,
        "pib" => (1u64 << 50) as f64,
        _ => return Err(format!("unknown size unit {unit_raw:?}")),
    };
    let r = v * m;
    if r > 9e18 {
        return Err(format!("size {s:?} too large"));
    }
    Ok(r as i64)
}

/// Parses durations like 90s, 30min, 12h, 7d, 2w, 6mo, 1y.
pub fn parse_age(s: &str) -> Result<Duration, String> {
    let (num, unit) = split_num(s);
    let Ok(v) = num.parse::<f64>() else { return Err(format!("bad age {s:?}")) };
    const DAY: f64 = 86400.0;
    let secs = match unit.to_lowercase().as_str() {
        "s" | "sec" => 1.0,
        "min" => 60.0,
        "h" => 3600.0,
        "d" | "" => DAY,
        "w" => 7.0 * DAY,
        "m" | "mo" => 30.0 * DAY,
        "y" => 365.0 * DAY,
        _ => return Err(format!("unknown age unit {unit:?} (s, min, h, d, w, mo, y)")),
    };
    // Saturates where Go's time.Duration would overflow.
    Ok(Duration::from_nanos((v * secs * 1e9) as u64))
}

/// Parses "2006-01-02", "2006-01-02T15:04", "2006-01-02 15:04" (local time)
/// or RFC 3339, as Go's time.ParseInLocation with time.Local would, into unix
/// nanoseconds.
fn parse_date(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let fixed = |i: usize, n: usize| -> Option<i64> {
        let d = b.get(i..i + n)?;
        d.iter().all(u8::is_ascii_digit).then(|| d.iter().fold(0, |a, &c| a * 10 + (c - b'0') as i64))
    };
    let (y, mo, d) = (fixed(0, 4)?, fixed(5, 2)?, fixed(8, 2)?);
    if b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') || !(1..=12).contains(&mo) || d < 1 || d > days_in(y, mo) {
        return None;
    }
    if b.len() == 10 {
        return local_nanos(y, mo, d, 0, 0);
    }
    let sep = *b.get(10)?;
    if sep != b'T' && sep != b' ' {
        return None;
    }
    // Go's "15" hour accepts one or two digits.
    let mut i = 11;
    let hl = if fixed(i, 2).is_some() { 2 } else { 1 };
    let h = fixed(i, hl)?;
    i += hl;
    if b.get(i) != Some(&b':') || h > 23 {
        return None;
    }
    let mi = fixed(i + 1, 2)?;
    i += 3;
    if mi > 59 {
        return None;
    }
    if i == b.len() {
        return local_nanos(y, mo, d, h, mi);
    }
    // RFC 3339: :05[.frac](Z|±07:00)
    if sep != b'T' || b.get(i) != Some(&b':') {
        return None;
    }
    let sec = fixed(i + 1, 2)?;
    i += 3;
    if sec > 59 {
        return None;
    }
    let mut ns = 0i64;
    if matches!(b.get(i), Some(b'.' | b',')) {
        let n = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if n == 0 {
            return None;
        }
        for k in 0..9 {
            ns = ns * 10 + b.get(i + 1 + k).filter(|_| k < n).map_or(0, |c| (c - b'0') as i64);
        }
        i += 1 + n;
    }
    let off = match b.get(i)? {
        b'Z' => {
            i += 1;
            0
        }
        &c @ (b'+' | b'-') => {
            let (oh, om) = (fixed(i + 1, 2)?, fixed(i + 4, 2)?);
            if b.get(i + 3) != Some(&b':') || oh > 23 || om > 59 {
                return None;
            }
            i += 6;
            (oh * 3600 + om * 60) * if c == b'-' { -1 } else { 1 }
        }
        _ => return None,
    };
    if i != b.len() {
        return None;
    }
    let secs = days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + sec - off;
    Some(secs * 1_000_000_000 + ns)
}

fn days_in(y: i64, m: i64) -> i64 {
    match m {
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn local_nanos(y: i64, mo: i64, d: i64, h: i64, mi: i64) -> Option<i64> {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = (y - 1900) as i32;
    tm.tm_mon = (mo - 1) as i32;
    tm.tm_mday = d as i32;
    tm.tm_hour = h as i32;
    tm.tm_min = mi as i32;
    tm.tm_isdst = -1;
    let t = unsafe { libc::mktime(&mut tm) };
    (t != -1).then(|| t as i64 * 1_000_000_000)
}
