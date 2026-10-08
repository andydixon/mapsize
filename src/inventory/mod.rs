//! The in-memory filesystem model: an append-only arena of nodes addressed
//! by stable NodeIds, with incrementally maintained directory aggregates.
//!
//! Concurrency: a Tree has exactly one writer (the scan controller or a
//! snapshot loader), shared as `Arc<RwLock<Tree>>`. Nodes are never removed,
//! so a NodeId stays valid for the Tree's lifetime.

mod category;
mod query;
mod stats;

pub use category::*;
pub use query::*;
pub use stats::*;

use crate::platform;
use std::collections::HashMap;
use std::path::PathBuf;

/// Addresses a node within one Tree. The root is always 0.
pub type NodeId = u32;

/// The nil NodeId.
pub const NO_NODE: NodeId = u32::MAX;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Kind {
    #[default]
    File,
    Dir,
    Symlink,
    Other, // devices, sockets, fifos, reparse points
}

impl Kind {
    pub fn from_u8(v: u8) -> Kind {
        [Kind::File, Kind::Dir, Kind::Symlink, Kind::Other][(v & 3) as usize]
    }
    pub fn name(self) -> &'static str {
        ["File", "Directory", "Symlink", "Special"][self as usize]
    }
}

/// Per-node facts (bit set).
pub type Flags = u16;
pub const FLAG_HARDLINK_DUP: Flags = 1 << 0; // additional link to an inode counted elsewhere
pub const FLAG_MOUNT_POINT: Flags = 1 << 1; // directory on a different device than its parent
pub const FLAG_VIRTUAL_FS: Flags = 1 << 2; // virtual filesystem, not descended
pub const FLAG_SKIPPED_FS: Flags = 1 << 3; // other filesystem, not descended (--one-file-system)
pub const FLAG_SPARSE: Flags = 1 << 4; // allocated < logical (sparse or compressed)
pub const FLAG_ERROR: Flags = 1 << 5; // stat or read failed
pub const FLAG_INCOMPLETE: Flags = 1 << 6; // directory contents not (fully) read
pub const FLAG_SCANNED: Flags = 1 << 7; // directory fully read
pub const FLAG_LOOP: Flags = 1 << 8; // directory already visited via another path
pub const FLAG_FOLLOWED: Flags = 1 << 9; // reached through a followed symlink
pub const FLAG_ALLOC_UNKNOWN: Flags = 1 << 10; // platform cannot report allocated size
pub const FLAG_BROKEN_LINK: Flags = 1 << 11; // symlink target does not exist
pub const FLAG_HARDLINKED: Flags = 1 << 12; // regular file with link count > 1
pub const FLAG_DELETED: Flags = 1 << 13; // moved to trash from the UI

/// One filesystem object. Sizes are own sizes; tot_* fields are aggregates
/// including the node itself and (for directories) everything beneath it.
#[derive(Clone, Debug)]
pub struct Node {
    pub parent: NodeId,
    pub first_child: NodeId,
    pub next_sibling: NodeId,
    dir: u32, // index into dir side table (directories only)

    /// Single path component (raw bytes); the root holds the full root path.
    pub name: Box<[u8]>,

    pub size: i64,     // own logical bytes
    pub alloc: i64,    // own allocated bytes
    pub tot_size: i64, // aggregates (hard-link duplicates contribute 0)
    pub tot_alloc: i64,
    pub mtime: i64, // unix nanoseconds

    pub files: u32, // recursive counts beneath (dirs excludes self)
    pub dirs: u32,
    pub errors: u32, // recursive error count beneath and including self
    pub mode: u32,   // portable mode bits (see platform::mode)
    pub uid: u32,
    pub gid: u32,
    pub nlink: u32,

    pub ext: u16,
    pub flags: Flags,
    pub kind: Kind,
    pub cat: Category,
}

impl Node {
    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }
    pub fn has(&self, f: Flags) -> bool {
        self.flags & f != 0
    }
    /// The name as (lossy) UTF-8, unsanitised.
    pub fn name_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.name)
    }
}

/// An aggregate change applied to a node and all its ancestors.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Delta {
    pub size: i64,
    pub alloc: i64,
    pub files: i64,
    pub dirs: i64,
    pub errors: i64,
    pub cat: [i64; NUM_CATEGORIES],
}

const CHUNK_BITS: u32 = 16;
const CHUNK_SIZE: usize = 1 << CHUNK_BITS;

/// A growable array that never moves existing elements, so growth never
/// doubles peak memory.
#[derive(Clone)]
struct Chunked<T> {
    c: Vec<Vec<T>>,
    n: usize,
}

impl<T> Chunked<T> {
    fn new() -> Self {
        Chunked {
            c: Vec::new(),
            n: 0,
        }
    }
    fn add(&mut self, v: T) -> u32 {
        if self.n & (CHUNK_SIZE - 1) == 0 && self.n >> CHUNK_BITS == self.c.len() {
            self.c.push(Vec::with_capacity(CHUNK_SIZE));
        }
        self.c[self.n >> CHUNK_BITS].push(v);
        self.n += 1;
        (self.n - 1) as u32
    }
    #[inline]
    fn at(&self, i: u32) -> &T {
        &self.c[(i >> CHUNK_BITS) as usize][i as usize & (CHUNK_SIZE - 1)]
    }
    #[inline]
    fn at_mut(&mut self, i: u32) -> &mut T {
        &mut self.c[(i >> CHUNK_BITS) as usize][i as usize & (CHUNK_SIZE - 1)]
    }
}

/// The inventory for one scan root.
#[derive(Clone)]
pub struct Tree {
    nodes: Chunked<Node>,
    dirs: Chunked<[i64; NUM_CATEGORIES]>,
    exts: Vec<String>,
    ext_idx: HashMap<String, u16>,
    pub stats: Stats,
}

impl Tree {
    /// Creates a tree whose root has the given full path and kind.
    pub fn new(root_path: &[u8], kind: Kind) -> Tree {
        let mut t = Tree {
            nodes: Chunked::new(),
            dirs: Chunked::new(),
            exts: vec![String::new()],
            ext_idx: HashMap::from([(String::new(), 0)]),
            stats: Stats::default(),
        };
        t.stats.root = root_path.to_vec();
        t.stats.start = std::time::SystemTime::now();
        t.add(NO_NODE, root_path, kind);
        t
    }

    pub fn len(&self) -> usize {
        self.nodes.n
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.n == 0
    }
    #[inline]
    pub fn node(&self, id: NodeId) -> &Node {
        self.nodes.at(id)
    }
    #[inline]
    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        self.nodes.at_mut(id)
    }
    pub fn root(&self) -> NodeId {
        0
    }

    /// Appends a child node and links it under parent. It does not touch
    /// aggregates; use propagate.
    pub fn add(&mut self, parent: NodeId, name: &[u8], kind: Kind) -> NodeId {
        let mut n = Node {
            parent,
            first_child: NO_NODE,
            next_sibling: NO_NODE,
            dir: 0,
            name: name.into(),
            size: 0,
            alloc: 0,
            tot_size: 0,
            tot_alloc: 0,
            mtime: 0,
            files: 0,
            dirs: 0,
            errors: 0,
            mode: 0,
            uid: 0,
            gid: 0,
            nlink: 0,
            ext: 0,
            flags: 0,
            kind,
            cat: Category::Other,
        };
        if kind == Kind::Dir {
            n.dir = self.dirs.add([0; NUM_CATEGORIES]);
        } else {
            n.ext = self.intern_ext(&ext(name));
        }
        let id = self.nodes.add(n);
        if parent != NO_NODE {
            let first = self.node(parent).first_child;
            self.node_mut(id).next_sibling = first;
            self.node_mut(parent).first_child = id;
        }
        id
    }

    /// Interns an extension string; used by loaders rebuilding a tree.
    pub fn intern_ext(&mut self, e: &str) -> u16 {
        if let Some(&i) = self.ext_idx.get(e) {
            return i;
        }
        if self.exts.len() >= (1 << 16) - 1 {
            return 0;
        }
        self.exts.push(e.to_string());
        let i = (self.exts.len() - 1) as u16;
        self.ext_idx.insert(e.to_string(), i);
        i
    }

    /// The extension string for a node ("" if none).
    pub fn ext_name(&self, n: &Node) -> &str {
        &self.exts[n.ext as usize]
    }
    pub fn ext_count(&self) -> usize {
        self.exts.len()
    }
    pub fn ext_by_index(&self, i: usize) -> &str {
        &self.exts[i]
    }

    /// Per-category allocated bytes beneath a directory.
    pub fn cat_sizes(&self, id: NodeId) -> Option<&[i64; NUM_CATEGORIES]> {
        let n = self.node(id);
        (n.kind == Kind::Dir).then(|| self.dirs.at(n.dir))
    }

    /// Adds d to id and every ancestor. Cost is O(depth); callers batch many
    /// entries into one Delta.
    pub fn propagate(&mut self, mut id: NodeId, d: &Delta) {
        while id != NO_NODE {
            let n = self.nodes.at_mut(id);
            n.tot_size += d.size;
            n.tot_alloc += d.alloc;
            n.files = add_u32(n.files, d.files);
            n.dirs = add_u32(n.dirs, d.dirs);
            n.errors = add_u32(n.errors, d.errors);
            let (is_dir, dir, parent) = (n.kind == Kind::Dir, n.dir, n.parent);
            if is_dir {
                let cs = self.dirs.at_mut(dir);
                for (c, v) in cs.iter_mut().zip(d.cat) {
                    *c += v;
                }
            }
            id = parent;
        }
    }

    /// Iterates the children of id (in reverse insertion order).
    pub fn children(&self, id: NodeId) -> Children<'_> {
        Children {
            t: self,
            cur: self.node(id).first_child,
        }
    }

    pub fn child_count(&self, id: NodeId) -> usize {
        self.children(id).count()
    }

    /// Reconstructs the full path of id as raw bytes.
    pub fn path_bytes(&self, mut id: NodeId) -> Vec<u8> {
        let mut parts: Vec<&[u8]> = Vec::new();
        while id != NO_NODE {
            let n = self.node(id);
            parts.push(&n.name);
            id = n.parent;
        }
        let mut out = Vec::with_capacity(parts.iter().map(|p| p.len() + 1).sum());
        for p in parts.iter().rev() {
            if !out.is_empty() && !out.last().is_some_and(|&b| platform::is_sep(b)) {
                out.push(platform::SEP);
            }
            out.extend_from_slice(p);
        }
        out
    }

    /// Reconstructs the full path of id.
    pub fn path(&self, id: NodeId) -> PathBuf {
        platform::path_from_bytes(&self.path_bytes(id))
    }

    /// The full path as (lossy) UTF-8, unsanitised.
    pub fn path_string(&self, id: NodeId) -> String {
        String::from_utf8_lossy(&self.path_bytes(id)).into_owned()
    }

    /// The chain root..id inclusive.
    pub fn ancestors(&self, mut id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        while id != NO_NODE {
            out.push(id);
            id = self.node(id).parent;
        }
        out.reverse();
        out
    }

    /// Reports whether a is id or an ancestor of id.
    pub fn is_ancestor(&self, a: NodeId, mut id: NodeId) -> bool {
        while id != NO_NODE {
            if id == a {
                return true;
            }
            id = self.node(id).parent;
        }
        false
    }

    /// Number of ancestors of id.
    pub fn depth(&self, id: NodeId) -> usize {
        let mut d = 0;
        let mut id = self.node(id).parent;
        while id != NO_NODE {
            d += 1;
            id = self.node(id).parent;
        }
        d
    }

    /// Rebuilds all aggregates bottom-up. Relies on a parent's ID being
    /// smaller than its children's, which both the scanner and the snapshot
    /// loader guarantee. Used after loading a snapshot.
    pub fn recompute(&mut self) {
        for i in 0..self.len() as NodeId {
            let n = self.nodes.at_mut(i);
            n.tot_size = 0;
            n.tot_alloc = 0;
            n.files = 0;
            n.dirs = 0;
            n.errors = (n.flags & FLAG_ERROR != 0) as u32;
            if n.flags & FLAG_HARDLINK_DUP == 0 {
                n.tot_size = n.size;
                n.tot_alloc = n.alloc;
            }
            if n.kind == Kind::Dir {
                let d = n.dir;
                *self.dirs.at_mut(d) = [0; NUM_CATEGORIES];
            }
        }
        // Sums saturate: snapshot data is untrusted and must not wrap to
        // negative totals.
        for i in (1..self.len() as NodeId).rev() {
            let n = self.nodes.at(i).clone_shallow();
            let p = self.nodes.at_mut(n.parent);
            p.tot_size = sat_add(p.tot_size, n.tot_size);
            p.tot_alloc = sat_add(p.tot_alloc, n.tot_alloc);
            p.errors = add_u32(p.errors, n.errors as i64);
            let pdir = p.dir;
            if n.kind == Kind::Dir {
                p.files = add_u32(p.files, n.files as i64);
                p.dirs = add_u32(p.dirs, n.dirs as i64 + 1);
                let child = *self.dirs.at(n.dir);
                let cs = self.dirs.at_mut(pdir);
                for (c, v) in cs.iter_mut().zip(child) {
                    *c = sat_add(*c, v);
                }
            } else {
                p.files = add_u32(p.files, 1);
                if n.flags & FLAG_HARDLINK_DUP == 0 {
                    let cs = self.dirs.at_mut(pdir);
                    cs[n.cat as usize] = sat_add(cs[n.cat as usize], n.alloc);
                }
            }
        }
    }
}

/// The numeric fields recompute needs, without cloning the name.
struct Shallow {
    parent: NodeId,
    dir: u32,
    tot_size: i64,
    tot_alloc: i64,
    alloc: i64,
    files: u32,
    dirs: u32,
    errors: u32,
    flags: Flags,
    kind: Kind,
    cat: Category,
}

impl Node {
    fn clone_shallow(&self) -> Shallow {
        Shallow {
            parent: self.parent,
            dir: self.dir,
            tot_size: self.tot_size,
            tot_alloc: self.tot_alloc,
            alloc: self.alloc,
            files: self.files,
            dirs: self.dirs,
            errors: self.errors,
            flags: self.flags,
            kind: self.kind,
            cat: self.cat,
        }
    }
}

pub struct Children<'a> {
    t: &'a Tree,
    cur: NodeId,
}

impl<'a> Iterator for Children<'a> {
    type Item = (NodeId, &'a Node);
    fn next(&mut self) -> Option<Self::Item> {
        if self.cur == NO_NODE {
            return None;
        }
        let id = self.cur;
        let n = self.t.node(id);
        self.cur = n.next_sibling;
        Some((id, n))
    }
}

/// Adds non-negative i64s, saturating at the maximum.
pub fn sat_add(a: i64, b: i64) -> i64 {
    if b > 0 && a > i64::MAX - b {
        return i64::MAX;
    }
    a + b
}

fn add_u32(a: u32, d: i64) -> u32 {
    (a as i64 + d).clamp(0, u32::MAX as i64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topk_and_recompute() {
        let mut tr = Tree::new(b"/r", Kind::Dir);
        let d = tr.add(0, b"d", Kind::Dir);
        for i in 0..100 {
            let id = tr.add(d, b"f", Kind::File);
            let n = tr.node_mut(id);
            n.size = i;
            n.alloc = i;
        }
        tr.recompute();
        assert_eq!(tr.node(0).tot_size, 4950);
        assert_eq!((tr.node(0).files, tr.node(0).dirs), (100, 1));
        let top = tr.top(0, 3, |n| n.kind == Kind::File, |n| n.size);
        assert_eq!(top.len(), 3);
        assert_eq!(tr.node(top[0]).size, 99);
        assert_eq!(tr.node(top[2]).size, 97);
        assert_eq!(tr.path_string(top[0]).replace('\\', "/"), "/r/d/f");
    }

    #[test]
    fn ext_and_category() {
        for (input, want) in [
            ("a.QCOW2", "qcow2"),
            (".bashrc", ""),
            ("noext", ""),
            ("x.tar.gz", "gz"),
            ("trail.", ""),
        ] {
            assert_eq!(ext(input.as_bytes()), want, "{input}");
        }
        assert_eq!(category_for("qcow2", false, false), Category::DiskImage);
        assert_eq!(category_for("bin", true, false), Category::Executable);
        assert_eq!(category_for("", true, false), Category::Cache);
    }
}
