use crate::inventory::{sat_add, NodeId, SizeMode, Tree, NO_NODE};
use std::collections::HashMap;
use std::fmt;

/// Relates two inventories by path. It owns the old tree; methods that need
/// the new one take it as a parameter. The new tree must not be mutated
/// while a Diff is in use (snapshots and finished scans satisfy this).
pub struct Diff {
    pub old: Tree,
    /// Maps a new NodeId to the matching old NodeId, or NO_NODE if the entry
    /// is new.
    pub old_of: Vec<NodeId>,
    /// The topmost old nodes with no counterpart in the new tree.
    pub removed: Vec<NodeId>,
    /// Per new directory, the sizes of removed children ([allocated,
    /// logical]) so growth colouring can account for them.
    removed_under: HashMap<NodeId, [i64; 2]>,
}

/// Matches nodes by name, directory by directory, from the roots.
pub fn compare(old: Tree, new: &Tree) -> Diff {
    let mut old_of = vec![NO_NODE; new.len()];
    let mut removed = Vec::new();
    let mut removed_under: HashMap<NodeId, [i64; 2]> = HashMap::new();
    let mut stack = vec![(old.root(), new.root())];
    while let Some((o, n)) = stack.pop() {
        old_of[n as usize] = o;
        if !old.node(o).is_dir() || !new.node(n).is_dir() {
            continue;
        }
        let mut by_name: HashMap<&[u8], NodeId> =
            old.children(o).map(|(id, c)| (&c.name[..], id)).collect();
        for (id, c) in new.children(n) {
            if let Some(oid) = by_name.remove(&c.name[..]) {
                stack.push((oid, id));
            }
        }
        for oid in by_name.into_values() {
            removed.push(oid);
            let on = old.node(oid);
            let r = removed_under.entry(n).or_default();
            r[0] = sat_add(r[0], on.tot_alloc);
            r[1] = sat_add(r[1], on.tot_size);
        }
    }
    removed.sort_unstable();
    Diff {
        old,
        old_of,
        removed,
        removed_under,
    }
}

/// Classifies a node.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Status {
    Unchanged,
    Added,
    Removed,
    Grew,
    Shrank,
}

impl Status {
    pub fn name(self) -> &'static str {
        ["unchanged", "new", "removed", "increased", "decreased"][self as usize]
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One row in a change report.
#[derive(Clone, Debug)]
pub struct Change {
    pub path: String,
    pub status: Status,
    pub delta: i64,
    pub old: i64,
    pub new: i64,
    pub node: NodeId, // in the new tree (NO_NODE for removed)
}

impl Diff {
    /// The matching old size of a new node, if it existed.
    pub fn old_size(&self, id: NodeId, m: SizeMode) -> Option<i64> {
        let o = self.old_of[id as usize];
        (o != NO_NODE).then(|| self.old.node(o).total(m))
    }

    /// New minus old size for a new node (full size if new).
    pub fn delta(&self, new: &Tree, id: NodeId, m: SizeMode) -> i64 {
        new.node(id).total(m) - self.old_size(id, m).unwrap_or(0)
    }

    /// Classifies a new node.
    pub fn status_of(&self, new: &Tree, id: NodeId, m: SizeMode) -> Status {
        if self.old_of[id as usize] == NO_NODE {
            return Status::Added;
        }
        match self.delta(new, id, m) {
            d if d > 0 => Status::Grew,
            d if d < 0 => Status::Shrank,
            _ => Status::Unchanged,
        }
    }

    /// The k most significant changes (all if k is 0). A directory is listed
    /// only if its change is not almost entirely explained by a single
    /// child, so the report points at /var/lib/postgresql rather than /,
    /// /var and /var/lib.
    pub fn changes(&self, new: &Tree, k: usize, m: SizeMode) -> Vec<Change> {
        let mut out = Vec::new();
        for id in 0..new.len() as NodeId {
            let delta = self.delta(new, id, m);
            if delta == 0 {
                continue;
            }
            let n = new.node(id);
            if self.old_of[id as usize] != NO_NODE
                && n.is_dir()
                && self.explained_by_child(new, id, delta, m)
            {
                continue;
            }
            if n.parent != NO_NODE && self.old_of[n.parent as usize] == NO_NODE {
                continue; // inside a wholly new directory; the directory is listed
            }
            out.push(Change {
                path: new.path_string(id),
                status: self.status_of(new, id, m),
                delta,
                old: self.old_size(id, m).unwrap_or(0),
                new: n.total(m),
                node: id,
            });
        }
        for &oid in &self.removed {
            let o = self.old.node(oid).total(m);
            out.push(Change {
                path: self.old.path_string(oid),
                status: Status::Removed,
                delta: -o,
                old: o,
                new: 0,
                node: NO_NODE,
            });
        }
        out.sort_by(|a, b| {
            b.delta
                .unsigned_abs()
                .cmp(&a.delta.unsigned_abs())
                .then_with(|| a.path.cmp(&b.path))
        });
        if k > 0 {
            out.truncate(k);
        }
        out
    }

    // Products are widened to i128 so saturated (untrusted) totals cannot wrap.
    fn explained_by_child(&self, new: &Tree, id: NodeId, delta: i64, m: SizeMode) -> bool {
        let whole = delta.unsigned_abs() as i128 * 8;
        let by_child = new.children(id).any(|(c, _)| {
            let cd = self.delta(new, c, m);
            (cd > 0) == (delta > 0) && cd.unsigned_abs() as i128 * 10 >= whole
        });
        by_child
            || match self.removed_under.get(&id) {
                Some(r) => {
                    let v = if m == SizeMode::Logical { r[1] } else { r[0] };
                    delta < 0 && v as i128 * 10 >= whole
                }
                None => false,
            }
    }
}
