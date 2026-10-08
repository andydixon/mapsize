//! Finds files with identical content in stages:
//!
//!  1. group by exact size (inventory only, no I/O)
//!  2. hash three 16 KiB samples (start, middle, end) of each candidate
//!  3. full SHA-256 of files whose samples still collide
//!
//! Only stage 3 establishes identity. Files are never declared duplicates by
//! name or size alone. Reading files may update their access times.

use crate::cancel::Cancel;
use crate::inventory::{Kind, NodeId, Tree, FLAG_HARDLINK_DUP};
use crate::platform;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Mutex, RwLock};

/// A set of files verified to have identical content.
#[derive(Clone, Debug)]
pub struct Group {
    pub size: i64,
    pub hash: [u8; 32],
    pub files: Vec<NodeId>,
}

impl Group {
    /// The bytes reclaimable by keeping one copy.
    pub fn wasted(&self) -> i64 {
        self.size * (self.files.len() as i64 - 1)
    }
}

/// Reports activity.
#[derive(Clone, Default, Debug)]
pub struct Progress {
    pub stage: String,
    pub candidates: i64, // files sharing a size with another file
    pub bytes_to_hash: i64,
    pub bytes_hashed: i64,
    pub skipped: i64, // unreadable or changed during hashing
    pub verified_groups: i64,
    pub candidate_groups: i64,
}

/// Tunes the search.
#[derive(Clone, Copy, Default)]
pub struct Options {
    pub min_size: i64,  // ignore files smaller than this (default 1 byte)
    pub workers: usize, // concurrent readers (default 4)
}

#[derive(Clone)]
struct Cand {
    id: NodeId,
    path: PathBuf,
    size: i64,
}

const SAMPLE_SIZE: i64 = 16 << 10;

type HashFn = fn(&Finder, &Cand) -> Option<[u8; 32]>;

/// Runs a search and exposes live progress.
#[derive(Default)]
pub struct Finder {
    hashed: AtomicI64,
    to_hash: AtomicI64,
    skipped: AtomicI64,
    p: Mutex<Progress>,
}

impl Finder {
    /// A copy of the current progress.
    pub fn progress(&self) -> Progress {
        let mut p = self.p.lock().unwrap().clone();
        p.bytes_hashed = self.hashed.load(Ordering::Relaxed);
        p.bytes_to_hash = self.to_hash.load(Ordering::Relaxed);
        p.skipped = self.skipped.load(Ordering::Relaxed);
        p
    }

    fn stage(&self, s: &str, f: impl FnOnce(&mut Progress)) {
        let mut p = self.p.lock().unwrap();
        p.stage = s.to_string();
        f(&mut p);
    }

    /// Searches the tree, taking its own read lock only while collecting
    /// candidates. None if cancelled.
    pub fn find(&self, cancel: &Cancel, tree: &RwLock<Tree>, mut o: Options) -> Option<Vec<Group>> {
        o.min_size = o.min_size.max(1);
        if o.workers < 1 {
            o.workers = 4;
        }
        self.stage("grouping by size", |_| {});
        let mut groups: Vec<Vec<Cand>> = Vec::new();
        let mut ncand = 0i64;
        {
            let t = tree.read().unwrap();
            let mut by_size: HashMap<i64, Vec<Cand>> = HashMap::new();
            t.walk(t.root(), |id, n| {
                if n.kind == Kind::File && !n.has(FLAG_HARDLINK_DUP) && n.size >= o.min_size {
                    by_size.entry(n.size).or_default().push(Cand {
                        id,
                        path: PathBuf::new(),
                        size: n.size,
                    });
                }
                true
            });
            for mut g in by_size.into_values() {
                if g.len() > 1 {
                    for c in &mut g {
                        c.path = t.path(c.id);
                    }
                    ncand += g.len() as i64;
                    groups.push(g);
                }
            }
        }
        self.stage("sampling", |p| {
            p.candidates = ncand;
            p.candidate_groups = groups.len() as i64;
        });

        // Stage 2: sample hashes.
        let mut sampled: Vec<Vec<Cand>> = Vec::new();
        for g in groups {
            if cancel.is_cancelled() {
                return None;
            }
            if g[0].size <= 3 * SAMPLE_SIZE {
                sampled.push(g); // a sample would be the whole file
                continue;
            }
            sampled.extend(
                self.split(cancel, &g, o.workers, sample_hash)
                    .into_values()
                    .filter(|v| v.len() > 1),
            );
        }

        // Stage 3: full hashes.
        self.to_hash.store(
            sampled.iter().map(|g| g[0].size * g.len() as i64).sum(),
            Ordering::Relaxed,
        );
        self.stage("hashing", |_| {});
        let mut out = Vec::new();
        for g in &sampled {
            if cancel.is_cancelled() {
                return None;
            }
            for (hash, v) in self.split(cancel, g, o.workers, full_hash) {
                if v.len() > 1 {
                    let mut files: Vec<NodeId> = v.iter().map(|c| c.id).collect();
                    files.sort_unstable();
                    out.push(Group {
                        size: v[0].size,
                        hash,
                        files,
                    });
                }
            }
        }
        if cancel.is_cancelled() {
            return None;
        }
        out.sort_by(|a, b| {
            b.wasted()
                .cmp(&a.wasted())
                .then(a.files[0].cmp(&b.files[0]))
        });
        self.stage("done", |p| p.verified_groups = out.len() as i64);
        Some(out)
    }

    /// Hashes every candidate with a bounded pool of reader threads and
    /// buckets the successes by hash. Stops starting new reads once cancelled.
    fn split(
        &self,
        cancel: &Cancel,
        g: &[Cand],
        workers: usize,
        f: HashFn,
    ) -> HashMap<[u8; 32], Vec<Cand>> {
        let next = AtomicUsize::new(0);
        let results = Mutex::new(Vec::with_capacity(g.len()));
        std::thread::scope(|s| {
            for _ in 0..workers.min(g.len()) {
                s.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= g.len() || cancel.is_cancelled() {
                        break;
                    }
                    match f(self, &g[i]) {
                        Some(sum) => results.lock().unwrap().push((i, sum)),
                        None => {
                            self.skipped.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                });
            }
        });
        let mut res = results.into_inner().unwrap();
        res.sort_unstable_by_key(|r| r.0);
        let mut by: HashMap<[u8; 32], Vec<Cand>> = HashMap::new();
        for (i, sum) in res {
            by.entry(sum).or_default().push(g[i].clone());
        }
        by
    }
}

/// Opens a candidate (never following symlinks or blocking on FIFOs) and
/// checks it is still a regular file of the inventoried size.
fn open(c: &Cand) -> Option<File> {
    let fh = platform::open_content(&c.path).ok()?;
    let md = fh.metadata().ok()?;
    (md.is_file() && md.len() as i64 == c.size).then_some(fh)
}

fn sample_hash(_: &Finder, c: &Cand) -> Option<[u8; 32]> {
    let fh = open(c)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; SAMPLE_SIZE as usize];
    for off in [0, c.size / 2 - SAMPLE_SIZE / 2, c.size - SAMPLE_SIZE] {
        match platform::read_exact_at(&fh, &mut buf, off as u64) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {} // shrank; stage 3 rejects it
            Err(_) => return None,
        }
        h.update(&buf);
    }
    Some(h.finalize().into())
}

fn full_hash(f: &Finder, c: &Cand) -> Option<[u8; 32]> {
    let mut fh = open(c)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 32 << 10];
    let mut n = 0i64;
    loop {
        match fh.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => {
                h.update(&buf[..k]);
                n += k as i64;
                f.hashed.fetch_add(k as i64, Ordering::Relaxed);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    (n == c.size).then(|| h.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{self, tests::TempDir};
    use std::path::Path;
    use std::sync::Arc;

    fn scan(root: &Path, workers: usize) -> Arc<RwLock<Tree>> {
        let s = scan::start(
            root.as_os_str().as_encoded_bytes(),
            scan::Options {
                workers,
                ..Default::default()
            },
            Cancel::new(),
        )
        .unwrap();
        s.wait();
        s.tree.clone()
    }

    #[test]
    fn find() {
        let root = TempDir::new("dup");
        let big: Vec<u8> = (0..200_000usize).map(|i| (i * 7) as u8).collect();
        let w = |name: &str, b: &[u8]| std::fs::write(root.0.join(name), b).unwrap();
        w("a.bin", &big);
        w("b.bin", &big);
        // Same size, same samples, different middle-of-nowhere byte: must not match.
        let mut c = big.clone();
        c[20_000] ^= 1;
        w("c.bin", &c);
        w("small1", b"hello");
        w("small2", b"hello");
        w("small3", b"world"); // same size, different content

        let tree = scan(&root.0, 2);
        let f = Finder::default();
        let groups = f.find(&Cancel::new(), &tree, Options::default()).unwrap();
        assert_eq!(groups.len(), 2);
        let t = tree.read().unwrap();
        let names = |g: &Group| {
            let mut v: Vec<String> = g
                .files
                .iter()
                .map(|&id| t.node(id).name_str().into_owned())
                .collect();
            v.sort();
            v
        };
        assert_eq!(names(&groups[0]), ["a.bin", "b.bin"], "big group");
        assert_eq!(groups[0].size, 200_000);
        assert_eq!(names(&groups[1]), ["small1", "small2"], "small group");
        let p = f.progress();
        assert!(
            p.candidates == 6 && p.verified_groups == 2,
            "progress {p:?}"
        );
    }

    /// A candidate replaced by a FIFO (or a symlink to one) after the scan
    /// must be skipped, not block the search forever in open(2).
    #[cfg(unix)]
    #[test]
    fn find_skips_swapped_fifo() {
        use std::os::unix::ffi::OsStrExt;
        use std::time::Duration;
        let root = TempDir::new("dupfifo");
        let other = TempDir::new("dupfifo2");
        for n in ["a", "b", "c"] {
            std::fs::write(root.0.join(n), b"same").unwrap();
        }
        let tree = scan(&root.0, 1);
        let mkfifo = |p: &Path| {
            let c = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap();
            unsafe { libc::mkfifo(c.as_ptr(), 0o644) == 0 }
        };
        std::fs::remove_file(root.0.join("b")).unwrap();
        if !mkfifo(&root.0.join("b")) {
            eprintln!("mkfifo unsupported; skipping");
            return;
        }
        let fifo2 = other.0.join("p");
        assert!(mkfifo(&fifo2));
        std::fs::remove_file(root.0.join("c")).unwrap();
        std::os::unix::fs::symlink(&fifo2, root.0.join("c")).unwrap();

        let f = Arc::new(Finder::default());
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (f2, t2) = (f.clone(), tree.clone());
        std::thread::spawn(move || {
            tx.send(f2.find(&Cancel::new(), &t2, Options::default()).is_some())
        });
        assert!(rx
            .recv_timeout(Duration::from_secs(5))
            .expect("duplicate search blocked on a FIFO"));
        assert_eq!(f.progress().skipped, 2);
    }
}
