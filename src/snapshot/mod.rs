//! The versioned, checksummed binary inventory format and snapshot
//! comparison.
//!
//! Layout (all integers little-endian / varint):
//!
//! ```text
//! magic      8 bytes  brand::SNAPSHOT_MAGIC
//! version    uint16   currently 1
//! flags      uint16   bit 0: body is gzip-compressed (always set by save)
//! body       gzip stream of:
//!   meta     uvarint length + JSON (root, timestamps, counters, options)
//!   exts     uvarint count, then count × (uvarint length + bytes)
//!   nodes    uvarint count, then per node:
//!              uvarint parentDelta (id-parent; 0 only for the root)
//!              byte kind, byte category, uvarint flags
//!              uvarint nameLen + name
//!              varint size, varint alloc, varint mtime
//!              uvarint mode, uid, gid, nlink, ext
//!   errors   uvarint count, then per record: uvarint node, byte kind,
//!            uvarint len + name, uvarint len + message
//! sha256     32 bytes over the uncompressed body
//! ```
//!
//! A uvarint is unsigned LEB128: 7 bits per byte, least significant group
//! first, the high bit set on every byte but the last; at most 10 bytes for
//! 64 bits. A varint (signed) is zig-zag encoded first, (v << 1) ^ (v >> 63),
//! so small magnitudes of either sign stay short: 0, -1, 1, -2 → 0, 1, 2, 3.
//!
//! In the meta JSON, timestamps are RFC 3339 strings with nanoseconds
//! (trailing zeros trimmed) and the writer's UTC offset, or "Z" for UTC,
//! e.g. "2026-10-08T10:40:00.123456789+01:00". An unset time is
//! "0001-01-01T00:00:00Z"; null is read as unset.
//!
//! Compatibility: snapshots written by mapsize 1.x load unchanged, and the
//! mode bits (see `platform::mode`) mean the same on every OS.
//!
//! Nodes are written in ID order; a parent always precedes its children, so
//! a reader can rebuild the tree in one pass and malformed input cannot
//! create cycles. Aggregates are recomputed on load, never trusted.

mod compare;

pub use compare::*;

use crate::brand;
use crate::inventory::{
    Category, ErrKind, ErrorRecord, Kind, NodeId, Tree, MAX_ERROR_RECORDS, NUM_CATEGORIES,
};
use crate::platform::{civil_from_days, days_from_civil};
use crossbeam_channel::Receiver;
use flate2::{bufread::GzDecoder, write::GzEncoder, Compression};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The current format version.
pub const VERSION: u16 = 1;

// Limits applied when reading untrusted snapshots.
pub const MAX_NODES: u64 = (1 << 31) - 1;
pub const MAX_DEPTH: u16 = 4096; // far beyond any real path; bounds recursion in exporters
pub const MAX_SIZE: i64 = 1 << 60; // per-node size cap (1 EiB); keeps aggregates from overflowing
/// Caps the decompressed body for load. load_file uses a limit proportional
/// to the file size instead (see body_limit).
pub const DEFAULT_MAX_BODY: i64 = 4 << 30;
pub const MAX_NAME_LEN: u64 = 1 << 16;
pub const MAX_META_LEN: u64 = 16 << 20;
pub const MAX_EXT_LEN: u64 = 64;
pub const MAX_MSG_LEN: u64 = 4096;
const MAX_EXTS: u64 = (1 << 16) - 1;
const FLAG_GZIPPED: u16 = 1;

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Meta {
    root: String,
    created: JsonTime,
    start: JsonTime,
    end: JsonTime,
    complete: bool,
    cancelled: bool,
    files: i64,
    dirs: i64,
    symlinks: i64,
    others: i64,
    err_counts: Option<Vec<i64>>,
    excluded: i64,
    skipped_mounts: i64,
    virtual_skipped: i64,
    loops_skipped: i64,
    broken_links: i64,
    hardlink_dups: i64,
    unscanned: i64,
    excludes: Option<Vec<String>>,
    one_file_system: bool,
    follow: String,
    workers: i64,
    host: String,
    generator: String,
}

/// Reports whether path starts with the snapshot magic.
pub fn is_snapshot(path: &Path) -> bool {
    let mut b = [0u8; 8];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut b))
        .is_ok()
        && &b == brand::SNAPSHOT_MAGIC
}

/// Accumulates the body; flushes it through the hash and gzip in chunks.
struct Writer<W: Write> {
    gz: GzEncoder<W>,
    h: Sha256,
    buf: Vec<u8>,
}

impl<W: Write> Writer<W> {
    fn bytes(&mut self, b: &[u8]) -> io::Result<()> {
        self.buf.extend_from_slice(b);
        if self.buf.len() >= 256 << 10 {
            self.flush()?;
        }
        Ok(())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.h.update(&self.buf);
        self.gz.write_all(&self.buf)?;
        self.buf.clear();
        Ok(())
    }
    fn uvarint(&mut self, mut v: u64) -> io::Result<()> {
        let (mut b, mut i) = ([0u8; 10], 0);
        while v >= 0x80 {
            b[i] = v as u8 | 0x80;
            v >>= 7;
            i += 1;
        }
        b[i] = v as u8;
        self.bytes(&b[..=i])
    }
    fn varint(&mut self, v: i64) -> io::Result<()> {
        self.uvarint(((v << 1) ^ (v >> 63)) as u64) // zig-zag
    }
    fn str(&mut self, s: &[u8]) -> io::Result<()> {
        self.uvarint(s.len() as u64)?;
        self.bytes(s)
    }
}

/// Writes t to out. The caller holds t's read lock.
pub fn save(out: &mut dyn Write, t: &Tree) -> io::Result<()> {
    let mut bw = BufWriter::with_capacity(1 << 20, out);
    bw.write_all(brand::SNAPSHOT_MAGIC)?;
    bw.write_all(&VERSION.to_le_bytes())?;
    bw.write_all(&FLAG_GZIPPED.to_le_bytes())?;
    let mut w = Writer {
        gz: GzEncoder::new(bw, Compression::fast()),
        h: Sha256::new(),
        buf: Vec::with_capacity(256 << 10),
    };

    let st = &t.stats;
    let m = Meta {
        root: String::from_utf8_lossy(&st.root).into_owned(),
        created: JsonTime::from_system(SystemTime::now()),
        start: JsonTime::from_system(st.start).local(),
        end: st
            .end
            .map(|e| JsonTime::from_system(e).local())
            .unwrap_or_default(),
        complete: st.complete,
        cancelled: st.cancelled,
        files: st.files,
        dirs: st.dirs,
        symlinks: st.symlinks,
        others: st.others,
        err_counts: Some(st.err_counts.to_vec()),
        excluded: st.excluded,
        skipped_mounts: st.skipped_mounts,
        virtual_skipped: st.virtual_skipped,
        loops_skipped: st.loops_skipped,
        broken_links: st.broken_links,
        hardlink_dups: st.hardlink_dups,
        unscanned: st.unscanned,
        excludes: Some(st.excludes.clone()),
        one_file_system: st.one_file_system,
        follow: st.follow.clone(),
        workers: st.workers as i64,
        host: crate::platform::hostname(),
        generator: format!("{} {}", brand::NAME, brand::VERSION),
    };
    let mb = serde_json::to_vec(&m)?;
    w.uvarint(mb.len() as u64)?;
    w.bytes(&mb)?;

    w.uvarint(t.ext_count() as u64)?;
    for i in 0..t.ext_count() {
        w.str(t.ext_by_index(i).as_bytes())?;
    }
    w.uvarint(t.len() as u64)?;
    for id in 0..t.len() as NodeId {
        let n = t.node(id);
        w.uvarint(if id == 0 { 0 } else { (id - n.parent) as u64 })?;
        w.bytes(&[n.kind as u8, n.cat as u8])?;
        w.uvarint(n.flags as u64)?;
        w.str(&n.name)?;
        w.varint(n.size)?;
        w.varint(n.alloc)?;
        w.varint(n.mtime)?;
        for v in [n.mode, n.uid, n.gid, n.nlink, n.ext as u32] {
            w.uvarint(v as u64)?;
        }
    }
    w.uvarint(st.errors.len() as u64)?;
    for e in &st.errors {
        w.uvarint(e.node as u64)?;
        w.bytes(&[e.kind as u8])?;
        w.str(&e.name)?;
        w.str(e.msg.as_bytes())?;
    }
    w.flush()?;
    let mut bw = w.gz.finish()?;
    bw.write_all(&w.h.finalize())?;
    bw.flush()
}

/// Writes a snapshot atomically (temp file + rename).
pub fn save_file(path: &Path, t: &Tree) -> io::Result<()> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let (tmp, mut f) = create_temp(dir)?;
    let res = save(&mut f, t).and_then(|_| {
        drop(f);
        fs::rename(&tmp, path)
    });
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

/// Creates a new private (0600 on Unix) file named .mapsize-<random>.tmp in
/// dir, retrying on name collisions.
fn create_temp(dir: &Path) -> io::Result<(PathBuf, File)> {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos()
        ^ std::process::id();
    for i in 0..10000u32 {
        let p = dir.join(format!(
            ".mapsize-{}.tmp",
            seed.wrapping_add(i.wrapping_mul(7919))
        ));
        match crate::platform::create_private(&p) {
            Ok(f) => return Ok((p, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "snapshot: cannot create temporary file",
    ))
}

/// The decompressed body, fed in chunks by the decompressing thread.
struct Body {
    rx: Receiver<Result<Vec<u8>, String>>,
    buf: Vec<u8>,
    pos: usize,
    err: Option<String>,
}

const EOF: &str = "EOF";
const UNEXPECTED_EOF: &str = "unexpected EOF";

impl Body {
    fn new(rx: Receiver<Result<Vec<u8>, String>>) -> Body {
        Body {
            rx,
            buf: Vec::new(),
            pos: 0,
            err: None,
        }
    }

    /// Ensures unread bytes are buffered; false at end of stream.
    fn fill(&mut self) -> Result<bool, String> {
        while self.pos == self.buf.len() {
            if let Some(e) = &self.err {
                return Err(e.clone());
            }
            match self.rx.recv() {
                Ok(Ok(b)) => (self.buf, self.pos) = (b, 0),
                Ok(Err(e)) => self.err = Some(e),
                Err(_) => return Ok(false),
            }
        }
        Ok(true)
    }

    /// The next byte; Err("EOF") at end of stream.
    #[inline]
    fn byte(&mut self) -> Result<u8, String> {
        if self.pos == self.buf.len() && !self.fill()? {
            return Err(EOF.into());
        }
        self.pos += 1;
        Ok(self.buf[self.pos - 1])
    }

    /// Fills out completely: "EOF" if the stream ends before the first byte,
    /// "unexpected EOF" if it ends part way.
    fn read_full(&mut self, out: &mut [u8]) -> Result<(), String> {
        let mut done = 0;
        while done < out.len() {
            if !self.fill()? {
                return Err(if done == 0 { EOF } else { UNEXPECTED_EOF }.into());
            }
            let k = (out.len() - done).min(self.buf.len() - self.pos);
            out[done..done + k].copy_from_slice(&self.buf[self.pos..self.pos + k]);
            self.pos += k;
            done += k;
        }
        Ok(())
    }

    fn byte1(&mut self) -> Result<u8, String> {
        self.byte().map_err(|e| format!("snapshot: truncated: {e}"))
    }

    fn uvarint(&mut self, max: u64, what: &str) -> Result<u64, String> {
        let v = read_uvarint(self).map_err(|e| format!("snapshot: bad {what}: {e}"))?;
        if v > max {
            return Err(format!("snapshot: {what} {v} exceeds limit {max}"));
        }
        Ok(v)
    }

    fn varint(&mut self, what: &str) -> Result<i64, String> {
        let ux = read_uvarint(self).map_err(|e| format!("snapshot: bad {what}: {e}"))?;
        let v = (ux >> 1) as i64; // undo zig-zag
        Ok(if ux & 1 != 0 { !v } else { v })
    }

    fn bytes_n(&mut self, n: u64) -> Result<Vec<u8>, String> {
        let mut b = vec![0u8; n as usize];
        self.read_full(&mut b)
            .map_err(|e| format!("snapshot: truncated: {e}"))?;
        Ok(b)
    }

    fn str(&mut self, max: u64, what: &str) -> Result<Vec<u8>, String> {
        let n = self.uvarint(max, &format!("{what} length"))?;
        self.bytes_n(n)
    }
}

/// Reads a uvarint (LEB128) from the body; more than 64 bits is an error.
fn read_uvarint(b: &mut Body) -> Result<u64, String> {
    let (mut x, mut s) = (0u64, 0u32);
    for i in 0..10 {
        let c = match b.byte() {
            Ok(c) => c,
            Err(e) if i > 0 && e == EOF => return Err(UNEXPECTED_EOF.into()),
            Err(e) => return Err(e),
        };
        if c < 0x80 {
            if i == 9 && c > 1 {
                break;
            }
            return Ok(x | (c as u64) << s);
        }
        x |= ((c & 0x7f) as u64) << s;
        s += 7;
    }
    Err("varint overflows 64 bits".into())
}

const BOMB: &str = "snapshot: decompressed size exceeds limit (possible decompression bomb)";

/// Allows decompressed bodies up to 64× the compressed file size (real
/// snapshots compress ~6×), with a 64 MiB floor for tiny files.
fn body_limit(file_size: i64) -> i64 {
    (64 << 20).max(file_size.saturating_mul(64))
}

/// Caps the node count for a file of file_size bytes. Real snapshots spend
/// ~15 compressed bytes per node; a crafted one describes a node in a
/// fraction of a byte, so without this a 1 MB file can demand gigabytes of
/// memory while staying within body_limit.
fn node_limit(file_size: i64) -> u64 {
    (1u64 << 20).max(file_size.max(0) as u64)
}

/// Reads and validates a snapshot, allowing up to DEFAULT_MAX_BODY bytes of
/// decompressed data.
pub fn load(r: impl Read) -> Result<Tree, crate::Error> {
    load_limit(r, DEFAULT_MAX_BODY)
}

/// load with an explicit decompressed-size limit.
pub fn load_limit(r: impl Read, max_body: i64) -> Result<Tree, crate::Error> {
    load_inner(r, max_body, MAX_NODES).map_err(Into::into)
}

/// Opens and loads a snapshot file.
pub fn load_file(path: &Path) -> Result<Tree, crate::Error> {
    let f = File::open(path)?;
    let (limit, nodes) = match f.metadata() {
        Ok(m) => (body_limit(m.len() as i64), node_limit(m.len() as i64)),
        Err(_) => (DEFAULT_MAX_BODY, MAX_NODES),
    };
    load_inner(f, limit, nodes).map_err(Into::into)
}

fn load_inner(input: impl Read, max_body: i64, max_nodes: u64) -> Result<Tree, String> {
    let mut br = BufReader::with_capacity(1 << 20, input);
    let mut hdr = [0u8; 12];
    br.read_exact(&mut hdr)
        .map_err(|_| "snapshot: file too short".to_string())?;
    if &hdr[..8] != brand::SNAPSHOT_MAGIC {
        return Err(format!("snapshot: not a {} snapshot", brand::NAME));
    }
    let v = u16::from_le_bytes([hdr[8], hdr[9]]);
    if v != VERSION {
        return Err(format!(
            "snapshot: unsupported version {v} (this build reads {VERSION})"
        ));
    }
    if u16::from_le_bytes([hdr[10], hdr[11]]) & FLAG_GZIPPED == 0 {
        return Err("snapshot: uncompressed bodies are not supported".into());
    }
    // Decompression and hashing run on this thread while a scoped thread
    // parses, overlapping the two. The parser only sees end of stream once
    // the channel's sender is dropped, i.e. after decompression finished.
    let mut h = Sha256::new();
    let res = {
        let mut gz = GzDecoder::new(&mut br); // single member, like gz.Multistream(false)
        std::thread::scope(|s| {
            let (tx, rx) = crossbeam_channel::bounded(4);
            let parser = s.spawn(move || parse(&mut Body::new(rx), max_nodes));
            let mut left = max_body;
            loop {
                if left <= 0 {
                    let _ = tx.send(Err(BOMB.to_string()));
                    break;
                }
                let mut buf = vec![0u8; (256 << 10).min(left as usize)];
                match gz.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.truncate(n);
                        left -= n as i64;
                        h.update(&buf);
                        if tx.send(Ok(buf)).is_err() {
                            break; // parser bailed out
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => {
                        let _ = tx.send(Err(format!("gzip: {e}")));
                        break;
                    }
                }
            }
            drop(tx);
            parser
                .join()
                .unwrap_or_else(|p| std::panic::resume_unwind(p))
        })
    };
    let (mut t, m) = res?;
    // The checksum follows the gzip stream.
    let mut sum = [0u8; 32];
    br.read_exact(&mut sum)
        .map_err(|_| "snapshot: missing checksum".to_string())?;
    if sum[..] != h.finalize()[..] {
        return Err("snapshot: checksum mismatch (file corrupt)".into());
    }

    t.recompute();
    let st = &mut t.stats;
    st.start = m.start.to_system();
    st.end = (!m.end.is_zero()).then(|| m.end.to_system());
    st.complete = true;
    st.cancelled = m.cancelled;
    (st.files, st.dirs, st.symlinks, st.others) = (m.files, m.dirs, m.symlinks, m.others);
    for (d, s) in st
        .err_counts
        .iter_mut()
        .zip(m.err_counts.unwrap_or_default())
    {
        *d = s;
    }
    (st.excluded, st.skipped_mounts, st.virtual_skipped) =
        (m.excluded, m.skipped_mounts, m.virtual_skipped);
    (
        st.loops_skipped,
        st.broken_links,
        st.hardlink_dups,
        st.unscanned,
    ) = (
        m.loops_skipped,
        m.broken_links,
        m.hardlink_dups,
        m.unscanned,
    );
    st.excludes = m.excludes.unwrap_or_default();
    st.one_file_system = m.one_file_system;
    st.follow = m.follow;
    st.workers = m.workers.max(0) as usize;
    st.from_snapshot = format!(
        "{} ({}, {})",
        m.created.local_minutes(),
        m.host,
        m.generator
    );
    Ok(t)
}

/// Parses the decompressed body up to (and including) its end.
fn parse(r: &mut Body, max_nodes: u64) -> Result<(Tree, Meta), String> {
    let n = r.uvarint(MAX_META_LEN, "metadata length")?;
    let mb = r.bytes_n(n)?;
    let m: Meta = serde_json::from_slice(&mb).map_err(|e| format!("snapshot: metadata: {e}"))?;
    let mut t = Tree::new(m.root.as_bytes(), Kind::Dir);

    let n_ext = r.uvarint(MAX_EXTS, "extension count")?;
    let mut exts = Vec::with_capacity(n_ext.min(1024) as usize);
    for _ in 0..n_ext {
        exts.push(String::from_utf8_lossy(&r.str(MAX_EXT_LEN, "extension")?).into_owned());
    }
    let n_nodes = r.uvarint(max_nodes, "node count")?;
    if n_nodes == 0 {
        return Err("snapshot: no root node".into());
    }
    let mut depth: Vec<u16> = vec![0]; // per node, for the depth limit; parents precede children
    for i in 0..n_nodes {
        let pd = r.uvarint(MAX_NODES, "parent")?;
        let kind = r.byte1()?;
        let cat = r.byte1()?;
        let flags = r.uvarint((1 << 16) - 1, "flags")? as u16;
        let name = r.str(MAX_NAME_LEN, "name")?;
        let (size, alloc, mtime) = (r.varint("size")?, r.varint("alloc")?, r.varint("mtime")?);
        let mode = r.uvarint(u32::MAX as u64, "mode")? as u32;
        let uid = r.uvarint(u32::MAX as u64, "uid")? as u32;
        let gid = r.uvarint(u32::MAX as u64, "gid")? as u32;
        let nlink = r.uvarint(u32::MAX as u64, "nlink")? as u32;
        let ext = r.uvarint(n_ext, "ext")?;
        if kind > Kind::Other as u8 || cat as usize >= NUM_CATEGORIES {
            return Err(format!("snapshot: node {i}: invalid kind/category"));
        }
        let kind = Kind::from_u8(kind);
        if size < 0 || alloc < 0 || size > MAX_SIZE || alloc > MAX_SIZE {
            return Err(format!("snapshot: node {i}: size out of range"));
        }
        let id = if i == 0 {
            if pd != 0 || kind != Kind::Dir && n_nodes > 1 {
                return Err("snapshot: invalid root".into());
            }
            t.node_mut(0).kind = kind;
            0
        } else {
            if pd == 0 || pd > i {
                return Err(format!("snapshot: node {i}: invalid parent reference"));
            }
            let parent = (i - pd) as NodeId;
            if t.node(parent).kind != Kind::Dir {
                return Err(format!("snapshot: node {i}: parent is not a directory"));
            }
            if !valid_name(&name) {
                return Err(format!(
                    "snapshot: node {i}: invalid name {:?}",
                    String::from_utf8_lossy(&name)
                ));
            }
            let d = depth[parent as usize] + 1;
            if d > MAX_DEPTH {
                return Err(format!(
                    "snapshot: node {i}: deeper than {MAX_DEPTH} levels"
                ));
            }
            depth.push(d);
            t.add(parent, &name, kind)
        };
        let n = t.node_mut(id);
        n.cat = Category::from_u8(cat);
        n.flags = flags;
        (n.size, n.alloc, n.mtime) = (size, alloc, mtime);
        (n.mode, n.uid, n.gid, n.nlink) = (mode, uid, gid, nlink);
        if kind != Kind::Dir && (ext as usize) < exts.len() {
            let e = t.intern_ext(&exts[ext as usize]);
            t.node_mut(id).ext = e;
        }
    }
    let n_err = r.uvarint(MAX_ERROR_RECORDS as u64, "error count")?;
    for _ in 0..n_err {
        let node = r.uvarint(n_nodes - 1, "error node")? as NodeId;
        let kind = ErrKind::from_u8(r.byte1()?); // out-of-range kinds become Other
        let name = r.str(MAX_NAME_LEN, "error name")?;
        let msg = String::from_utf8_lossy(&r.str(MAX_MSG_LEN, "error message")?).into_owned();
        t.stats.errors.push(ErrorRecord {
            node,
            kind,
            name,
            msg,
        });
    }
    // The body must end here.
    if r.byte() != Err(EOF.into()) {
        return Err("snapshot: trailing data in body".into());
    }
    Ok((t, m))
}

fn valid_name(s: &[u8]) -> bool {
    !s.is_empty() && s != b"." && s != b".." && !s.iter().any(|&c| c == b'/' || c == 0)
}

/// A timestamp as the meta JSON carries it (see the module docs): unix
/// seconds plus nanoseconds, and the UTC offset it is written with. The
/// default is the unset time, 0001-01-01T00:00:00Z.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct JsonTime {
    secs: i64,
    nanos: u32,
    off: i32, // seconds east of UTC; presentation only
}

const ZERO_SECS: i64 = -62_135_596_800;

impl Default for JsonTime {
    fn default() -> Self {
        JsonTime {
            secs: ZERO_SECS,
            nanos: 0,
            off: 0,
        }
    }
}

impl JsonTime {
    fn from_system(t: SystemTime) -> JsonTime {
        match t.duration_since(UNIX_EPOCH) {
            Ok(d) => JsonTime {
                secs: d.as_secs() as i64,
                nanos: d.subsec_nanos(),
                off: 0,
            },
            Err(e) => {
                let d = e.duration();
                let (s, n) = (d.as_secs() as i64, d.subsec_nanos());
                let (secs, nanos) = if n == 0 {
                    (-s, 0)
                } else {
                    (-s - 1, 1_000_000_000 - n)
                };
                JsonTime {
                    secs,
                    nanos,
                    off: 0,
                }
            }
        }
    }

    /// The same instant, presented in the local time zone.
    fn local(self) -> JsonTime {
        let off = crate::platform::local_time(self.secs).off;
        JsonTime { off, ..self }
    }

    fn is_zero(self) -> bool {
        (self.secs, self.nanos) == (ZERO_SECS, 0)
    }

    fn to_system(self) -> SystemTime {
        if self.secs >= 0 {
            UNIX_EPOCH + Duration::new(self.secs as u64, self.nanos)
        } else {
            UNIX_EPOCH - Duration::from_secs(self.secs.unsigned_abs())
                + Duration::from_nanos(self.nanos as u64)
        }
    }

    /// RFC 3339 with nanoseconds (trailing zeros trimmed) and "Z" for UTC.
    fn format(self) -> String {
        let secs = self.secs + self.off as i64;
        let (y, mo, d) = civil_from_days(secs.div_euclid(86400));
        let rem = secs.rem_euclid(86400);
        let mut s = format!(
            "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}",
            rem / 3600,
            rem / 60 % 60,
            rem % 60
        );
        if self.nanos != 0 {
            s += format!(".{:09}", self.nanos).trim_end_matches('0');
        }
        if self.off == 0 {
            return s + "Z";
        }
        let a = self.off.unsigned_abs() / 60;
        format!(
            "{s}{}{:02}:{:02}",
            if self.off < 0 { '-' } else { '+' },
            a / 60,
            a % 60
        )
    }

    /// Parses RFC 3339 ("2026-10-08T10:40:00.123456789+01:00").
    fn parse(s: &str) -> Option<JsonTime> {
        let b = s.as_bytes();
        let num = |r: std::ops::Range<usize>| -> Option<i64> {
            let p = b.get(r)?;
            p.iter()
                .all(u8::is_ascii_digit)
                .then(|| p.iter().fold(0i64, |a, &c| a * 10 + (c - b'0') as i64))
        };
        if b.len() < 20
            || b[4] != b'-'
            || b[7] != b'-'
            || b[10] != b'T'
            || b[13] != b':'
            || b[16] != b':'
        {
            return None;
        }
        let (y, mo, d, h, mi, se) = (
            num(0..4)?,
            num(5..7)?,
            num(8..10)?,
            num(11..13)?,
            num(14..16)?,
            num(17..19)?,
        );
        if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 59 {
            return None;
        }
        let mut i = 19;
        let mut nanos = 0u32;
        if b[i] == b'.' {
            let start = i + 1;
            i = start;
            while i < b.len() && b[i].is_ascii_digit() {
                if i - start < 9 {
                    nanos = nanos * 10 + (b[i] - b'0') as u32;
                }
                i += 1;
            }
            if i == start {
                return None;
            }
            for _ in (i - start)..9 {
                nanos *= 10;
            }
        }
        let off = match b.get(i)? {
            b'Z' if b.len() == i + 1 => 0,
            &c @ (b'+' | b'-') if b.len() == i + 6 && b[i + 3] == b':' => {
                let (oh, om) = (num(i + 1..i + 3)?, num(i + 4..i + 6)?);
                if oh > 23 || om > 59 {
                    return None;
                }
                (oh * 3600 + om * 60) * if c == b'-' { -1 } else { 1 }
            }
            _ => return None,
        };
        let secs = days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + se - off;
        Some(JsonTime {
            secs,
            nanos,
            off: off as i32,
        })
    }

    /// "2006-01-02 15:04" in local time.
    fn local_minutes(self) -> String {
        let t = crate::platform::local_time(self.secs);
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            t.year, t.month, t.day, t.hour, t.min
        )
    }
}

impl Serialize for JsonTime {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.format())
    }
}

impl<'de> Deserialize<'de> for JsonTime {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // null leaves the time unset.
        match Option::<String>::deserialize(d)? {
            None => Ok(JsonTime::default()),
            Some(s) => JsonTime::parse(&s)
                .ok_or_else(|| serde::de::Error::custom(format!("parsing time {s:?}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{SizeMode, FLAG_SPARSE};
    use std::collections::HashMap;

    fn sample() -> Tree {
        let mut t = Tree::new(b"/srv", Kind::Dir);
        let a = t.add(0, b"a", Kind::Dir);
        let b = t.add(0, b"b", Kind::Dir);
        let f1 = t.add(a, b"x.iso", Kind::File);
        (t.node_mut(f1).size, t.node_mut(f1).alloc) = (1000, 1024);
        let f2 = t.add(b, b"y.log", Kind::File);
        (t.node_mut(f2).size, t.node_mut(f2).alloc) = (50, 4096);
        t.node_mut(f2).flags = FLAG_SPARSE;
        t.recompute();
        (t.stats.files, t.stats.dirs) = (2, 3);
        t.stats.add_error(ErrorRecord {
            node: b,
            kind: ErrKind::Permission,
            msg: "denied".into(),
            ..Default::default()
        });
        t
    }

    fn saved(t: &Tree) -> Vec<u8> {
        let mut buf = Vec::new();
        save(&mut buf, t).unwrap();
        buf
    }

    #[test]
    fn round_trip() {
        let src = sample();
        let got = load(&saved(&src)[..]).unwrap();
        assert_eq!(got.len(), src.len());
        for i in 0..src.len() as NodeId {
            let (a, b) = (src.node(i), got.node(i));
            assert!(
                a.name == b.name
                    && a.tot_size == b.tot_size
                    && a.tot_alloc == b.tot_alloc
                    && a.flags == b.flags
                    && src.path(i) == got.path(i)
                    && src.ext_name(a) == got.ext_name(b),
                "node {i} differs: {a:?} vs {b:?}"
            );
        }
        assert_eq!(got.stats.err_counts[ErrKind::Permission as usize], 1);
        assert_eq!(got.stats.errors.len(), 1);
        assert_eq!(got.stats.start, src.stats.start);
        assert_eq!(got.stats.end, None);
    }

    #[test]
    fn corruption_detected() {
        let data = saved(&sample());
        let mut bad = data.clone();
        *bad.last_mut().unwrap() ^= 0xff; // flip a byte in the checksum trailer
        assert!(load(&bad[..]).is_err(), "checksum corruption not detected");
        // Every truncation must fail cleanly.
        for i in (0..data.len() - 1).step_by(7) {
            assert!(load(&data[..i]).is_err(), "truncation at {i} accepted");
        }
    }

    #[test]
    fn compare_trees() {
        let old = sample();
        let mut nw = Tree::new(b"/srv", Kind::Dir);
        let a = nw.add(0, b"a", Kind::Dir);
        let f1 = nw.add(a, b"x.iso", Kind::File);
        (nw.node_mut(f1).size, nw.node_mut(f1).alloc) = (5000, 5120);
        let c = nw.add(0, b"c", Kind::Dir);
        let f3 = nw.add(c, b"z", Kind::File);
        (nw.node_mut(f3).size, nw.node_mut(f3).alloc) = (10, 4096);
        nw.recompute();

        let d = compare(old, &nw);
        let al = SizeMode::Allocated;
        assert_eq!(d.status_of(&nw, f1, al), Status::Grew);
        assert_eq!(d.delta(&nw, f1, al), 4096);
        assert_eq!(d.status_of(&nw, c, al), Status::Added);
        assert_eq!(d.removed.len(), 1);
        assert_eq!(&d.old.node(d.removed[0]).name[..], b"b");
        let ch = d.changes(&nw, 10, al);
        let paths: HashMap<&str, Status> = ch.iter().map(|c| (c.path.as_str(), c.status)).collect();
        assert_eq!(paths.get("/srv/a/x.iso"), Some(&Status::Grew), "{ch:?}");
        assert_eq!(paths.get("/srv/b"), Some(&Status::Removed), "{ch:?}");
        assert_eq!(paths.get("/srv/c"), Some(&Status::Added), "{ch:?}");
        assert!(
            !paths.contains_key("/srv/a"),
            "/srv/a should be explained by its child"
        );
        assert!(
            !paths.contains_key("/srv/c/z"),
            "contents of new dirs should not be listed separately"
        );
    }

    fn body(b: &[u8]) -> Body {
        let (tx, rx) = crossbeam_channel::unbounded();
        tx.send(Ok(b.to_vec())).unwrap();
        Body::new(rx)
    }

    fn put_uvarint(v: u64) -> Vec<u8> {
        let mut w = Writer {
            gz: GzEncoder::new(Vec::new(), Compression::fast()),
            h: Sha256::new(),
            buf: Vec::new(),
        };
        w.uvarint(v).unwrap();
        w.buf
    }

    #[test]
    fn varint_encoding() {
        for v in [0, 1, 127, 128, 300, 1 << 35, u64::MAX] {
            assert_eq!(read_uvarint(&mut body(&put_uvarint(v))), Ok(v));
        }
        assert_eq!(put_uvarint(300), [0xac, 0x02]);
        assert!(
            read_uvarint(&mut body(&[0xff; 11])).is_err(),
            "overflow not detected"
        );
        // Zig-zag.
        for (v, enc) in [(0i64, 0u64), (-1, 1), (1, 2), (-2, 3), (i64::MIN, u64::MAX)] {
            assert_eq!(((v << 1) ^ (v >> 63)) as u64, enc);
            assert_eq!(body(&put_uvarint(enc)).varint("v"), Ok(v));
        }
    }

    #[test]
    fn json_time() {
        let t = JsonTime::parse("2026-10-08T10:40:00.123456789+01:00").unwrap();
        assert_eq!((t.secs, t.nanos, t.off), (1_791_452_400, 123_456_789, 3600));
        assert_eq!(t.format(), "2026-10-08T10:40:00.123456789+01:00");
        assert_eq!(
            JsonTime { off: 0, ..t }.format(),
            "2026-10-08T09:40:00.123456789Z"
        );
        assert_eq!(
            JsonTime {
                off: -5 * 3600 - 1800,
                ..t
            }
            .format(),
            "2026-10-08T04:10:00.123456789-05:30"
        );
        assert_eq!(
            JsonTime::parse("2026-10-08T09:40:00.1Z").unwrap().format(),
            "2026-10-08T09:40:00.1Z"
        );
        assert_eq!(
            JsonTime::parse("0001-01-01T00:00:00Z"),
            Some(JsonTime::default())
        );
        assert_eq!(JsonTime::default().format(), "0001-01-01T00:00:00Z");
        assert_eq!(
            JsonTime::from_system(JsonTime::default().to_system()),
            JsonTime::default()
        );
        for bad in [
            "",
            "2026-10-08",
            "2026-13-08T00:00:00Z",
            "2026-10-08T00:00:00",
            "2026-10-08T00:00:00.Z",
        ] {
            assert_eq!(JsonTime::parse(bad), None, "{bad}");
        }
    }

    /// A snapshot written by mapsize 1.2.0 of a tiny tree
    /// (tree/{a/x.iso 10000 B, a/b/note.txt, c/y.log 3000 B, c/link -> ../a}).
    #[test]
    fn loads_1_2_snapshot() {
        let t = load_file(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/snapshot/testdata/v1.2.0.msz"
        )))
        .unwrap();
        let r = t.node(0);
        assert_eq!((r.tot_size, r.tot_alloc), (13310, 20480));
        assert_eq!((t.stats.files, t.stats.dirs, t.stats.symlinks), (3, 4, 1));
        assert_eq!(t.len(), 8);
        assert!(t.stats.root.ends_with(b"/tree"));
        assert!(t.stats.end.is_some() && t.stats.complete);
        assert!(
            t.stats.from_snapshot.contains("mapsize"),
            "{}",
            t.stats.from_snapshot
        );
        let iso = (0..t.len() as NodeId)
            .find(|&i| &t.node(i).name[..] == b"x.iso")
            .unwrap();
        assert_eq!(t.node(iso).size, 10000);
        assert_eq!(t.ext_name(t.node(iso)), "iso");
        assert!(t.path_string(iso).ends_with("/tree/a/x.iso"));
        // And it survives a save/load round trip unchanged.
        let again = load(&saved(&t)[..]).unwrap();
        assert_eq!(
            (again.node(0).tot_size, again.len(), again.stats.start),
            (13310, 8, t.stats.start)
        );
    }

    // Hostile input.

    /// Builds a well-formed (correctly checksummed) but hostile snapshot.
    fn craft(n: usize, meta: &str, rec: impl Fn(usize, &mut Writer<Vec<u8>>)) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(brand::SNAPSHOT_MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&FLAG_GZIPPED.to_le_bytes());
        let mut w = Writer {
            gz: GzEncoder::new(out, Compression::best()),
            h: Sha256::new(),
            buf: Vec::new(),
        };
        w.uvarint(meta.len() as u64).unwrap();
        w.bytes(meta.as_bytes()).unwrap();
        w.uvarint(1).unwrap();
        w.str(b"").unwrap();
        w.uvarint(n as u64).unwrap();
        for i in 0..n {
            rec(i, &mut w);
        }
        w.uvarint(0).unwrap();
        w.flush().unwrap();
        let mut out = w.gz.finish().unwrap();
        let sum = w.h.finalize();
        out.extend_from_slice(&sum);
        out
    }

    fn node(w: &mut Writer<Vec<u8>>, pd: u64, kind: Kind, name: &str, size: i64) {
        w.uvarint(pd).unwrap();
        w.bytes(&[kind as u8, 0]).unwrap();
        w.uvarint(0).unwrap();
        w.str(name.as_bytes()).unwrap();
        w.varint(size).unwrap();
        w.varint(size).unwrap();
        w.varint(0).unwrap();
        for _ in 0..5 {
            w.uvarint(0).unwrap();
        }
    }

    fn chain(n: usize) -> Vec<u8> {
        craft(n, r#"{"root":"/r"}"#, |i, w| {
            if i == 0 {
                node(w, 0, Kind::Dir, "", 0)
            } else {
                node(w, 1, Kind::Dir, "a", 0)
            }
        })
    }

    #[test]
    fn deep_chain_rejected() {
        load(&chain(MAX_DEPTH as usize + 1)[..]).expect("depth at the limit must load");
        let err = load(&chain(MAX_DEPTH as usize + 2)[..])
            .err()
            .expect("over-deep chain accepted");
        assert!(err.to_string().contains("deeper"), "{err}");
    }

    #[test]
    fn decompression_bomb_limited() {
        let flat = craft(100_000, r#"{"root":"/r"}"#, |i, w| {
            if i == 0 {
                node(w, 0, Kind::Dir, "", 0)
            } else {
                node(w, i as u64, Kind::File, "f", 0)
            }
        });
        let err = load_limit(&flat[..], 64 << 10)
            .err()
            .expect("bomb not limited");
        assert!(err.to_string().contains("limit"), "{err}");
        assert!(
            body_limit(flat.len() as i64) >= 64 << 20,
            "body_limit floor"
        );
    }

    #[test]
    fn huge_sizes_do_not_wrap() {
        let data = craft(3, r#"{"root":"/r"}"#, |i, w| {
            if i == 0 {
                node(w, 0, Kind::Dir, "", 0)
            } else {
                node(w, i as u64, Kind::File, "f", MAX_SIZE)
            }
        });
        let tr = load(&data[..]).unwrap();
        assert!(tr.node(0).tot_size >= 0, "aggregate wrapped negative");
        let over = craft(2, r#"{"root":"/r"}"#, |i, w| {
            if i == 0 {
                node(w, 0, Kind::Dir, "", 0)
            } else {
                node(w, 1, Kind::File, "f", MAX_SIZE + 1)
            }
        });
        assert!(load(&over[..]).is_err(), "size above MAX_SIZE accepted");
    }

    #[test]
    fn hostile_metadata_sanitised_in_summary() {
        let data = craft(
            1,
            r#"{"root":"/r\u001b[2J","excludes":["\u001b]0;PWNED\u0007"],"excluded":1}"#,
            |_, w| node(w, 0, Kind::Dir, "", 0),
        );
        let mut tr = load(&data[..]).unwrap();
        tr.stats.excluded = 1;
        let mut b = Vec::new();
        crate::export::summary(&mut b, &tr).unwrap();
        assert!(
            !b.iter().any(|&c| c == 0x1b || c == 0x07),
            "raw control characters in summary: {:?}",
            String::from_utf8_lossy(&b)
        );
    }

    /// Blocks of one directory plus K-1 files repeat byte for byte, so a
    /// small file describes millions of nodes; load_file must refuse before
    /// allocating.
    #[test]
    fn node_amplification_limited() {
        const K: usize = 1000;
        const N: usize = 1_200_001;
        let data = craft(N, r#"{"root":"/r"}"#, |i, w| match i {
            0 => node(w, 0, Kind::Dir, "", 0),
            1 => node(w, 1, Kind::Dir, "d", 0),
            _ if (i - 1) % K == 0 => node(w, K as u64, Kind::Dir, "d", 0),
            _ => node(w, ((i - 1) % K) as u64, Kind::File, "f", 0),
        });
        load(&data[..]).expect("crafted tree is otherwise valid");
        let p = std::env::temp_dir().join(format!("mapsize-amp-{}.msz", std::process::id()));
        fs::write(&p, &data).unwrap();
        let res = load_file(&p);
        let _ = fs::remove_file(&p);
        let err = res
            .err()
            .unwrap_or_else(|| panic!("{N} nodes from a {}-byte file accepted", data.len()));
        assert!(err.to_string().contains("node count"), "{err}");
    }

    #[test]
    fn save_file_is_atomic_and_detected() {
        let dir = std::env::temp_dir().join(format!("mapsize-save-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("s.msz");
        save_file(&p, &sample()).unwrap();
        assert!(is_snapshot(&p));
        assert_eq!(load_file(&p).unwrap().len(), 5);
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            1,
            "temp file left behind"
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
