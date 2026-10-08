//! The bounded concurrent filesystem scanner.
//!
//! Architecture (see ARCHITECTURE.md): a single controller thread owns the
//! write side of the Tree and a FIFO of pending directories. A fixed pool of
//! workers performs directory I/O and sends result chunks back. The
//! controller's select loop offers jobs and accepts results at the same
//! time, so bounded channels can never deadlock: workers never enqueue work
//! themselves.

use crate::cancel::Cancel;
use crate::inventory::{self as inv, Delta, ErrorRecord, Kind, Node, NodeId, Tree};
use crate::platform::{self, Meta};
use crossbeam_channel::{Receiver, Select, Sender};
use std::collections::HashSet;
use std::io;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

/// Symlink traversal.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FollowMode {
    #[default]
    None,
    SameFs,
    All,
}

impl FollowMode {
    pub fn name(self) -> &'static str {
        ["none", "same-filesystem", "all"][self as usize]
    }
}

/// Parses a --follow-symlinks value.
pub fn parse_follow(s: &str) -> Result<FollowMode, String> {
    match s {
        "none" | "" => Ok(FollowMode::None),
        "same-filesystem" | "same-fs" => Ok(FollowMode::SameFs),
        "all" => Ok(FollowMode::All),
        _ => Err(format!(
            "invalid --follow-symlinks {s:?} (want none, same-filesystem or all)"
        )),
    }
}

/// Resolves a concurrency mode name or number to a worker count.
pub fn workers(mode: &str) -> Result<usize, String> {
    let cpu = std::thread::available_parallelism().map_or(1, |n| n.get());
    match mode {
        "conservative" => Ok(2),
        "balanced" | "" => Ok((2 * cpu).clamp(2, 8)),
        "aggressive" => Ok((8 * cpu).clamp(4, 64)),
        _ => match mode.trim().parse::<usize>() {
            Ok(n) if (1..=1024).contains(&n) => Ok(n),
            _ => Err(format!(
                "invalid workers {mode:?} (conservative, balanced, aggressive or 1-1024)"
            )),
        },
    }
}

/// Configures a scan. Zero tuning knobs mean default.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub workers: usize,
    pub follow: FollowMode,
    pub one_file_system: bool,
    pub excludes: Vec<String>,

    pub chunk_size: usize,  // entries per result chunk
    pub job_buffer: usize,  // capacity of the job channel
    pub result_buf: usize,  // capacity of the result channel
    pub apply_batch: usize, // max results applied per write-lock hold
}

impl Options {
    fn defaults(&mut self) {
        if self.workers == 0 {
            self.workers = workers("balanced").unwrap();
        }
        if self.chunk_size == 0 {
            self.chunk_size = 1024;
        }
        if self.job_buffer == 0 {
            self.job_buffer = self.workers * 2;
        }
        if self.result_buf == 0 {
            self.result_buf = self.workers * 4;
        }
        if self.apply_batch == 0 {
            self.apply_batch = 64;
        }
    }
}

/// A cheap, lock-free view of scan activity.
#[derive(Clone, Copy, Debug, Default)]
pub struct Progress {
    pub entries: i64,
    pub pending: i64,  // directories discovered but not yet dispatched
    pub inflight: i64, // directories being read
    pub workers: usize,
}

#[derive(Default)]
struct Counters {
    entries: AtomicI64,
    pending: AtomicI64,
    inflight: AtomicI64,
}

/// One running scan. `tree` is safe to read under its read lock while the
/// scan runs.
pub struct Scanner {
    pub tree: Arc<RwLock<Tree>>,
    counters: Arc<Counters>,
    workers: usize,
    done: Receiver<()>,
}

impl Scanner {
    /// Ready (disconnected) once the scan has finished or been cancelled and
    /// drained; for use in select.
    pub fn done(&self) -> &Receiver<()> {
        &self.done
    }

    pub fn is_done(&self) -> bool {
        matches!(
            self.done.try_recv(),
            Err(crossbeam_channel::TryRecvError::Disconnected)
        )
    }

    /// Blocks until the scan is done.
    pub fn wait(&self) {
        let _ = self.done.recv();
    }

    /// Live counters without locking the tree.
    pub fn progress(&self) -> Progress {
        Progress {
            entries: self.counters.entries.load(Ordering::Relaxed),
            pending: self.counters.pending.load(Ordering::Relaxed),
            inflight: self.counters.inflight.load(Ordering::Relaxed),
            workers: self.workers,
        }
    }
}

struct Job {
    id: NodeId,
    path: Vec<u8>,
    dev: u64,
    // hints derived once per directory
    in_cache: bool,
    in_log: bool,
}

enum ReadErr {
    Io(io::Error),
    Cancelled,
}

impl From<io::Error> for ReadErr {
    fn from(e: io::Error) -> Self {
        ReadErr::Io(e)
    }
}

struct ScanResult {
    job: Arc<Job>,
    entries: Vec<Meta>,
    is_final: bool,
    err: Option<ReadErr>,
}

type InodeKey = (u64, u64);

fn kind_of(m: &Meta) -> Kind {
    if platform::mode::is_dir(m.mode) {
        Kind::Dir
    } else if platform::mode::is_regular(m.mode) {
        Kind::File
    } else if platform::mode::is_symlink(m.mode) {
        Kind::Symlink
    } else {
        Kind::Other
    }
}

fn set_meta(n: &mut Node, m: &Meta) {
    n.size = m.size;
    n.alloc = m.alloc;
    n.mtime = m.mtime;
    n.mode = m.mode;
    n.uid = m.uid;
    n.gid = m.gid;
    n.nlink = m.nlink;
    if !m.alloc_known {
        n.flags |= inv::FLAG_ALLOC_UNKNOWN;
    }
}

/// Runs f, aborting the process if it panics: a scanner bug must never
/// leave the controller waiting forever. The panic hook (which the TUI uses
/// to restore the terminal) runs first.
fn guard(f: impl FnOnce()) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
        std::process::abort();
    }
}

/// Stats root, creates the tree and begins scanning in the background.
pub fn start(root: &[u8], mut opts: Options, cancel: Cancel) -> io::Result<Scanner> {
    opts.defaults();
    // Path-style exclusions are matched against absolute paths.
    for p in &mut opts.excludes {
        if p.contains(['/', '\\']) {
            if let Ok(a) = platform::abs_path(p.as_bytes()) {
                *p = String::from_utf8_lossy(&a).into_owned();
            }
        }
    }
    let abs = platform::abs_path(root)?;
    let m = platform::stat(&abs)?;
    let kind = kind_of(&m);
    let mut t = Tree::new(&abs, kind);
    t.stats.workers = opts.workers;
    t.stats.excludes = opts.excludes.clone();
    t.stats.one_file_system = opts.one_file_system;
    t.stats.follow = opts.follow.name().to_string();
    let r = t.node_mut(0);
    set_meta(r, &m);
    r.tot_size = m.size;
    r.tot_alloc = m.alloc;
    let (done_tx, done) = crossbeam_channel::bounded::<()>(0);
    let counters = Arc::new(Counters::default());
    if kind != Kind::Dir {
        t.stats.files = 1;
        t.stats.complete = true;
        t.stats.end = Some(SystemTime::now());
        drop(done_tx);
        return Ok(Scanner {
            tree: Arc::new(RwLock::new(t)),
            counters,
            workers: opts.workers,
            done,
        });
    }
    t.stats.dirs = 1;
    // Loop detection needs inode identity; without it symlinks are not
    // followed at all.
    if opts.follow != FollowMode::None && !m.has_ino {
        opts.follow = FollowMode::None;
        t.stats.follow = "none (no inode identity on this platform)".into();
    }
    let visited = (opts.follow != FollowMode::None).then(|| HashSet::from([(m.dev, m.ino)]));
    let path_excludes = opts
        .excludes
        .iter()
        .map(|p| (p.contains(['/', '\\']), p.as_bytes().to_vec()))
        .collect();
    let tree = Arc::new(RwLock::new(t));
    let workers = opts.workers;
    counters.pending.store(1, Ordering::Relaxed);
    let c = Controller {
        tree: tree.clone(),
        counters: counters.clone(),
        excludes: path_excludes,
        opts,
        pending: vec![(0, m.dev)],
        head: 0,
        links: HashSet::new(),
        visited,
    };
    std::thread::Builder::new()
        .name("scan-controller".into())
        .spawn(move || {
            guard(|| c.run(cancel));
            drop(done_tx);
        })?;
    Ok(Scanner {
        tree,
        counters,
        workers,
        done,
    })
}

/// Bounds how long the controller holds the write lock, so the UI's read
/// lock is never starved for more than a few milliseconds.
const MAX_ENTRIES_PER_LOCK: usize = 8192;

struct Controller {
    tree: Arc<RwLock<Tree>>,
    counters: Arc<Counters>,
    opts: Options,
    excludes: Vec<(bool, Vec<u8>)>, // (is path pattern, pattern)
    pending: Vec<(NodeId, u64)>,    // FIFO of (dir, dev); head advances, compacted occasionally
    head: usize,
    links: HashSet<InodeKey>,
    visited: Option<HashSet<InodeKey>>, // only when following symlinks
}

fn worker(cancel: &Cancel, jobs: Receiver<Arc<Job>>, results: Sender<ScanResult>, chunk: usize) {
    for j in jobs {
        let err = if cancel.is_cancelled() {
            Some(ReadErr::Cancelled)
        } else {
            platform::read_dir(&j.path, chunk, |ms| {
                if cancel.is_cancelled() {
                    return Err(ReadErr::Cancelled);
                }
                results
                    .send(ScanResult {
                        job: j.clone(),
                        entries: ms,
                        is_final: false,
                        err: None,
                    })
                    .map_err(|_| ReadErr::Cancelled)
            })
            .err()
        };
        let _ = results.send(ScanResult {
            job: j,
            entries: Vec::new(),
            is_final: true,
            err,
        });
    }
}

impl Controller {
    fn run(mut self, cancel: Cancel) {
        let (jobs_tx, jobs_rx) = crossbeam_channel::bounded::<Arc<Job>>(self.opts.job_buffer);
        let (res_tx, res_rx) = crossbeam_channel::bounded::<ScanResult>(self.opts.result_buf);
        let mut handles = Vec::with_capacity(self.opts.workers);
        for i in 0..self.opts.workers {
            let (jobs, results, cancel, chunk) = (
                jobs_rx.clone(),
                res_tx.clone(),
                cancel.clone(),
                self.opts.chunk_size,
            );
            handles.push(
                std::thread::Builder::new()
                    .name(format!("scan-worker-{i}"))
                    .spawn(move || guard(|| worker(&cancel, jobs, results, chunk)))
                    .expect("spawn scan worker"),
            );
        }
        drop((jobs_rx, res_tx));

        let mut inflight = 0i64;
        let mut cancelled = false;
        let mut next: Option<Arc<Job>> = None;
        let mut batch: Vec<ScanResult> = Vec::with_capacity(self.opts.apply_batch);
        loop {
            if next.is_none() && !cancelled && self.head < self.pending.len() {
                next = Some(self.make_job(self.pending[self.head]));
            }
            if inflight == 0 && next.is_none() {
                break;
            }
            let mut sel = Select::new();
            let send_i = next.as_ref().map(|_| sel.send(&jobs_tx));
            let recv_i = sel.recv(&res_rx);
            let cancel_i = (!cancelled).then(|| sel.recv(cancel.done()));
            let op = sel.select();
            let i = op.index();
            if Some(i) == send_i {
                op.send(&jobs_tx, next.take().unwrap())
                    .expect("workers alive");
                self.head += 1;
                if self.head > 4096 && self.head * 2 > self.pending.len() {
                    self.pending.drain(..self.head);
                    self.head = 0;
                }
                inflight += 1;
            } else if i == recv_i {
                let r = op.recv(&res_rx).expect("workers alive");
                let mut n = r.entries.len();
                batch.push(r);
                while batch.len() < self.opts.apply_batch && n < MAX_ENTRIES_PER_LOCK {
                    match res_rx.try_recv() {
                        Ok(r) => {
                            n += r.entries.len();
                            batch.push(r);
                        }
                        Err(_) => break,
                    }
                }
                let tree = self.tree.clone();
                let mut t = tree.write().unwrap();
                for r in batch.drain(..) {
                    if self.apply(&mut t, r) {
                        inflight -= 1;
                    }
                }
            } else {
                debug_assert_eq!(Some(i), cancel_i);
                let _ = op.recv(cancel.done());
                cancelled = true;
                next = None;
            }
            self.counters.inflight.store(inflight, Ordering::Relaxed);
            self.counters
                .pending
                .store((self.pending.len() - self.head) as i64, Ordering::Relaxed);
        }
        drop(jobs_tx);
        for h in handles {
            let _ = h.join();
        }

        let mut t = self.tree.write().unwrap();
        if cancelled {
            t.stats.cancelled = true;
            t.stats.unscanned += (self.pending.len() - self.head) as i64;
            for &(id, _) in &self.pending[self.head..] {
                t.node_mut(id).flags |= inv::FLAG_INCOMPLETE;
            }
        }
        t.stats.complete = true;
        t.stats.end = Some(SystemTime::now());
        drop(t);
        self.counters.pending.store(0, Ordering::Relaxed);
        self.counters.inflight.store(0, Ordering::Relaxed);
    }

    fn make_job(&self, (id, dev): (NodeId, u64)) -> Arc<Job> {
        let path = self.tree.read().unwrap().path_bytes(id);
        let (in_cache, in_log) = inv::path_hints(&path);
        Arc::new(Job {
            id,
            path,
            dev,
            in_cache,
            in_log,
        })
    }

    /// Merges one result into the tree. Reports whether the result completed
    /// its directory. Called with the tree write-locked.
    fn apply(&mut self, t: &mut Tree, r: ScanResult) -> bool {
        let j = &*r.job;
        let mut d = Delta::default();
        self.counters
            .entries
            .fetch_add(r.entries.len() as i64, Ordering::Relaxed);
        for m in r.entries {
            self.add_entry(t, j, m, &mut d);
        }
        if r.is_final {
            let dn = t.node_mut(j.id);
            match r.err {
                None => dn.flags |= inv::FLAG_SCANNED,
                Some(ReadErr::Cancelled) => {
                    dn.flags |= inv::FLAG_INCOMPLETE;
                    t.stats.unscanned += 1;
                }
                Some(ReadErr::Io(e)) => {
                    dn.flags |= inv::FLAG_ERROR | inv::FLAG_INCOMPLETE;
                    t.stats.add_error(ErrorRecord {
                        node: j.id,
                        kind: inv::classify(&e),
                        msg: format!("open: {}", err_msg(&e)),
                        ..Default::default()
                    });
                    d.errors += 1;
                }
            }
        }
        if d != Delta::default() {
            t.propagate(j.id, &d);
        }
        r.is_final
    }

    fn excluded(&self, path: &[u8], name: &[u8]) -> bool {
        self.excludes.iter().any(|(is_path, p)| {
            if *is_path {
                path == p.as_slice()
                    || path.starts_with(p)
                        && path.get(p.len()).is_some_and(|&b| platform::is_sep(b))
                    || platform::glob_match(p, path)
            } else {
                platform::glob_match(p, name)
            }
        })
    }

    fn add_entry(&mut self, t: &mut Tree, j: &Job, m: Meta, d: &mut Delta) {
        if let Some(e) = &m.err {
            let k = inv::classify(e);
            if k == inv::ErrKind::Vanished {
                t.stats.add_error(ErrorRecord {
                    node: j.id,
                    name: m.name,
                    kind: k,
                    msg: "vanished between listing and stat".into(),
                });
                d.errors += 1;
                return;
            }
            let id = t.add(j.id, &m.name, Kind::Other);
            let n = t.node_mut(id);
            n.flags |= inv::FLAG_ERROR;
            n.errors = 1;
            t.stats.add_error(ErrorRecord {
                node: id,
                kind: k,
                msg: format!("stat: {}", err_msg(e)),
                ..Default::default()
            });
            d.errors += 1;
            d.files += 1;
            t.stats.others += 1;
            return;
        }
        let mut path = Vec::new();
        if !self.excludes.is_empty() {
            path = platform::join_path(&j.path, &m.name);
            if self.excluded(&path, &m.name) {
                t.stats.excluded += 1;
                return;
            }
        }
        let mut kind = kind_of(&m);
        let mut target = None;
        let mut broken = false;
        if kind == Kind::Symlink {
            if path.is_empty() {
                path = platform::join_path(&j.path, &m.name);
            }
            match platform::stat(&path) {
                Err(_) => {
                    t.stats.broken_links += 1;
                    broken = true;
                }
                Ok(tm)
                    if platform::mode::is_dir(tm.mode)
                        && tm.has_ino
                        && (self.opts.follow == FollowMode::All
                            || self.opts.follow == FollowMode::SameFs && tm.dev == j.dev) =>
                {
                    target = Some(tm);
                    kind = Kind::Dir;
                }
                Ok(_) => {}
            }
        }

        let id = t.add(j.id, &m.name, kind);
        let n = t.node_mut(id);
        set_meta(n, &m);
        n.tot_size = n.size;
        n.tot_alloc = n.alloc;
        let ext = n.ext;
        let mut flags = if broken { inv::FLAG_BROKEN_LINK } else { 0 };
        let st = &mut t.stats;

        match kind {
            Kind::Dir => {
                st.dirs += 1;
                d.dirs += 1;
                d.size += m.size;
                d.alloc += m.alloc;
                let mut dev = m.dev;
                let mut key = (m.dev, m.ino);
                if let Some(tm) = &target {
                    flags |= inv::FLAG_FOLLOWED;
                    dev = tm.dev;
                    key = (tm.dev, tm.ino);
                }
                let descend = 'check: {
                    if let Some(v) = &mut self.visited {
                        if !v.insert(key) {
                            flags |= inv::FLAG_LOOP;
                            st.loops_skipped += 1;
                            break 'check false;
                        }
                    }
                    if m.has_ino && dev != j.dev {
                        flags |= inv::FLAG_MOUNT_POINT;
                        if self.opts.one_file_system {
                            flags |= inv::FLAG_SKIPPED_FS;
                            st.skipped_mounts += 1;
                            break 'check false;
                        }
                        if path.is_empty() {
                            path = platform::join_path(&j.path, &m.name);
                        }
                        if platform::is_virtual_fs(&path) {
                            flags |= inv::FLAG_VIRTUAL_FS;
                            st.virtual_skipped += 1;
                            break 'check false;
                        }
                    }
                    true
                };
                if descend {
                    self.pending.push((id, dev));
                }
            }
            Kind::File => {
                st.files += 1;
                d.files += 1;
                let dup = 'check: {
                    if m.nlink > 1 && m.has_ino {
                        flags |= inv::FLAG_HARDLINKED;
                        if !self.links.insert((m.dev, m.ino)) {
                            flags |= inv::FLAG_HARDLINK_DUP;
                            st.hardlink_dups += 1;
                            break 'check true;
                        }
                    }
                    false
                };
                if !dup {
                    if m.alloc_known && m.alloc < m.size && m.size - m.alloc >= 4096 {
                        flags |= inv::FLAG_SPARSE;
                    }
                    let cat = inv::category_for(t.ext_by_index(ext as usize), j.in_cache, j.in_log);
                    t.node_mut(id).cat = cat;
                    d.size += m.size;
                    d.alloc += m.alloc;
                    d.cat[cat as usize] += m.alloc;
                } else {
                    // Duplicates contribute zero to aggregates.
                    let n = t.node_mut(id);
                    n.tot_size = 0;
                    n.tot_alloc = 0;
                }
            }
            _ => {
                if kind == Kind::Symlink {
                    st.symlinks += 1;
                } else {
                    st.others += 1;
                }
                d.files += 1;
                d.size += m.size;
                d.alloc += m.alloc;
            }
        }
        t.node_mut(id).flags |= flags;
    }
}

/// The OS error text without Rust's " (os error N)" suffix, matching Go.
fn err_msg(e: &io::Error) -> String {
    let s = e.to_string();
    match s.rfind(" (os error ") {
        Some(i) => s[..i].to_lowercase(),
        None => s,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    pub struct TempDir(pub PathBuf);
    impl TempDir {
        pub fn new(tag: &str) -> TempDir {
            static N: AtomicI64 = AtomicI64::new(0);
            let p = std::env::temp_dir().join(format!(
                "mapsize-{tag}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Creates a fan-out tree: (files, dirs, bytes).
    pub fn mk_tree(dir: &Path, fanout: usize, depth: usize, files: usize) -> (i64, i64, i64) {
        fn rec(
            dir: &Path,
            d: usize,
            fanout: usize,
            depth: usize,
            files: usize,
            acc: &mut (i64, i64, i64),
        ) {
            for i in 0..files {
                let sz = (i + 1) * 100;
                std::fs::write(dir.join(format!("f{i}.txt")), vec![0u8; sz]).unwrap();
                acc.0 += 1;
                acc.2 += sz as i64;
            }
            if d == depth {
                return;
            }
            for i in 0..fanout {
                let sub = dir.join(format!("d{i}"));
                std::fs::create_dir(&sub).unwrap();
                acc.1 += 1;
                rec(&sub, d + 1, fanout, depth, files, acc);
            }
        }
        let mut acc = (0, 0, 0);
        rec(dir, 0, fanout, depth, files, &mut acc);
        acc
    }

    pub fn run_scan(root: &Path, o: Options) -> Tree {
        let s = start(root.as_os_str().as_encoded_bytes(), o, Cancel::new()).unwrap();
        s.done()
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect_err("scan did not finish (deadlock?)");
        let t = s.tree.read().unwrap().clone();
        t
    }

    fn totals(t: &Tree) -> Vec<[i64; 4]> {
        (0..t.len() as NodeId)
            .map(|i| t.node(i))
            .map(|n| [n.tot_size, n.tot_alloc, n.files as i64, n.dirs as i64])
            .collect()
    }

    #[test]
    fn counts_and_aggregates() {
        let root = TempDir::new("scan");
        let (nf, nd, bytes) = mk_tree(&root.0, 3, 3, 4);
        let mut t = run_scan(
            &root.0,
            Options {
                workers: 4,
                ..Default::default()
            },
        );
        let r = t.node(0);
        assert_eq!((r.files as i64, r.dirs as i64), (nf, nd));
        let dir_bytes: i64 = (0..t.len() as NodeId)
            .map(|i| t.node(i))
            .filter(|n| n.is_dir())
            .map(|n| n.size)
            .sum();
        assert_eq!(r.tot_size, bytes + dir_bytes);
        assert!(t.stats.complete && !t.stats.incomplete());
        for i in 0..t.len() as NodeId {
            let n = t.node(i);
            if n.is_dir() {
                let sum: i64 = n.size + t.children(i).map(|(_, c)| c.tot_size).sum::<i64>();
                assert_eq!(sum, n.tot_size, "{}", t.path_string(i));
            }
        }
        let before = totals(&t);
        t.recompute();
        assert_eq!(
            before,
            totals(&t),
            "incremental aggregates differ from recompute"
        );
    }

    #[test]
    fn backpressure_no_deadlock() {
        let root = TempDir::new("bp");
        let (nf, nd, _) = mk_tree(&root.0, 6, 4, 3);
        for w in [1, 2, 16] {
            let t = run_scan(
                &root.0,
                Options {
                    workers: w,
                    job_buffer: 1,
                    result_buf: 1,
                    chunk_size: 1,
                    apply_batch: 1,
                    ..Default::default()
                },
            );
            assert_eq!(
                (t.node(0).files as i64, t.node(0).dirs as i64),
                (nf, nd),
                "workers={w}"
            );
        }
    }

    #[test]
    fn cancel_stops() {
        let root = TempDir::new("cancel");
        mk_tree(&root.0, 8, 3, 2);
        let c = Cancel::new();
        let s = start(
            root.0.as_os_str().as_encoded_bytes(),
            Options {
                workers: 2,
                job_buffer: 1,
                result_buf: 1,
                chunk_size: 1,
                ..Default::default()
            },
            c.clone(),
        )
        .unwrap();
        c.cancel();
        s.done()
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect_err("cancel did not stop scan");
        let t = s.tree.read().unwrap();
        assert!(t.stats.cancelled && t.stats.incomplete());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_loops() {
        let root = TempDir::new("loop");
        let a = root.0.join("a");
        std::fs::create_dir_all(a.join("b")).unwrap();
        std::fs::write(a.join("b/x"), vec![0u8; 5000]).unwrap();
        std::os::unix::fs::symlink("..", a.join("b/up")).unwrap();
        std::os::unix::fs::symlink("../a", a.join("self")).unwrap();
        std::os::unix::fs::symlink("nowhere", root.0.join("broken")).unwrap();
        for f in [FollowMode::None, FollowMode::SameFs, FollowMode::All] {
            let t = run_scan(
                &root.0,
                Options {
                    workers: 4,
                    follow: f,
                    ..Default::default()
                },
            );
            let r = t.node(0);
            assert!(r.files > 0 && r.tot_size > 5000, "{f:?}");
            assert_eq!(t.stats.broken_links, 1);
            assert!(f == FollowMode::None || t.stats.loops_skipped > 0);
            let count = (0..t.len() as NodeId)
                .filter(|&i| &*t.node(i).name == b"x")
                .count();
            assert_eq!(count, 1, "{f:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn hardlinks_counted_once() {
        let root = TempDir::new("hl");
        let p = root.0.join("orig");
        std::fs::write(&p, vec![0u8; 100000]).unwrap();
        std::fs::create_dir(root.0.join("d")).unwrap();
        std::fs::hard_link(&p, root.0.join("d/link")).unwrap();
        let t = run_scan(
            &root.0,
            Options {
                workers: 2,
                ..Default::default()
            },
        );
        assert_eq!(t.stats.hardlink_dups, 1);
        assert!(t.node(0).tot_size < 200000);
    }

    #[cfg(unix)]
    #[test]
    fn permission_errors_surface() {
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let root = TempDir::new("perm");
        let locked = root.0.join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("f"), b"x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let t = run_scan(
            &root.0,
            Options {
                workers: 2,
                ..Default::default()
            },
        );
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(t.stats.err_counts[inv::ErrKind::Permission as usize], 1);
        assert!(t.stats.incomplete());
        assert_eq!(t.node(0).errors, 1);
    }

    #[test]
    fn excludes() {
        let root = TempDir::new("excl");
        std::fs::create_dir_all(root.0.join("keep")).unwrap();
        std::fs::create_dir_all(root.0.join("skip")).unwrap();
        std::fs::write(root.0.join("keep/a.tmp"), b"x").unwrap();
        std::fs::write(root.0.join("keep/b.txt"), b"x").unwrap();
        std::fs::write(root.0.join("skip/c.txt"), b"x").unwrap();
        let skip = root.0.join("skip").to_string_lossy().into_owned();
        let t = run_scan(
            &root.0,
            Options {
                workers: 2,
                excludes: vec!["*.tmp".into(), skip],
                ..Default::default()
            },
        );
        assert_eq!((t.stats.excluded, t.node(0).files), (2, 1));
    }

    #[cfg(unix)]
    #[test]
    fn hostile_names_preserved() {
        use std::os::unix::ffi::OsStrExt;
        let root = TempDir::new("names");
        let names: [&[u8]; 6] = [
            "日本語.txt".as_bytes(),
            "emoji-🎉.png".as_bytes(),
            b"esc\x1b[31mred",
            b"nl\nname",
            b"sp ace",
            b"bad\xffutf8",
        ];
        for n in names {
            std::fs::write(root.0.join(std::ffi::OsStr::from_bytes(n)), b"x").unwrap();
        }
        let t = run_scan(
            &root.0,
            Options {
                workers: 2,
                ..Default::default()
            },
        );
        let got: HashSet<&[u8]> = t.children(0).map(|(_, n)| &*n.name).collect();
        for n in names {
            assert!(got.contains(n), "missing {n:?}");
        }
    }

    #[test]
    fn single_file_root() {
        let root = TempDir::new("single");
        let p = root.0.join("file");
        std::fs::write(&p, vec![0u8; 1234]).unwrap();
        let t = run_scan(&p, Options::default());
        assert_eq!(t.node(0).tot_size, 1234);
        assert_eq!(t.node(0).kind, Kind::File);
    }
}
