use super::*;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Which size drives layout and sorting.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SizeMode {
    #[default]
    Allocated, // disk usage
    Logical, // apparent size
}

impl SizeMode {
    pub fn name(self) -> &'static str {
        match self {
            SizeMode::Logical => "logical",
            SizeMode::Allocated => "allocated",
        }
    }
}

impl Node {
    /// The aggregate size in the given mode.
    #[inline]
    pub fn total(&self, m: SizeMode) -> i64 {
        match m {
            SizeMode::Logical => self.tot_size,
            SizeMode::Allocated => self.tot_alloc,
        }
    }
    /// The node's own size in the given mode.
    #[inline]
    pub fn own(&self, m: SizeMode) -> i64 {
        match m {
            SizeMode::Logical => self.size,
            SizeMode::Allocated => self.alloc,
        }
    }
}

/// Keeps the k entries with the largest keys (ties: smaller id wins); O(n log k).
pub struct TopK {
    k: usize,
    // Min-heap on (key, Reverse(id)): the root is the weakest entry.
    h: BinaryHeap<Reverse<(i64, Reverse<NodeId>)>>,
}

impl TopK {
    pub fn new(k: usize) -> TopK {
        TopK {
            k,
            h: BinaryHeap::with_capacity(k.min(1 << 16)),
        }
    }

    pub fn offer(&mut self, id: NodeId, key: i64) {
        if self.k == 0 {
            return;
        }
        let e = Reverse((key, Reverse(id)));
        if self.h.len() < self.k {
            self.h.push(e);
        } else if let Some(mut top) = self.h.peek_mut() {
            if e < *top {
                *top = e;
            }
        }
    }

    /// The collected IDs, largest key first (ties by ID).
    pub fn sorted(self) -> Vec<NodeId> {
        // into_sorted_vec is ascending in Reverse order = descending key.
        self.h
            .into_sorted_vec()
            .into_iter()
            .map(|Reverse((_, Reverse(id)))| id)
            .collect()
    }
}

impl Tree {
    /// Visits id and all its descendants depth-first (iteratively). fn
    /// returns false to skip a node's children.
    pub fn walk(&self, id: NodeId, mut f: impl FnMut(NodeId, &Node) -> bool) {
        let mut stack = vec![id];
        while let Some(cur) = stack.pop() {
            let n = self.node(cur);
            if !f(cur, n) {
                continue;
            }
            stack.extend(self.children(cur).map(|(c, _)| c));
        }
    }

    /// The k nodes beneath (and including) root with the largest key,
    /// considering only nodes accepted by keep.
    pub fn top(
        &self,
        root: NodeId,
        k: usize,
        keep: impl Fn(&Node) -> bool,
        key: impl Fn(&Node) -> i64,
    ) -> Vec<NodeId> {
        let mut tk = TopK::new(k);
        self.walk(root, |id, n| {
            if keep(n) {
                tk.offer(id, key(n));
            }
            true
        });
        tk.sorted()
    }

    /// The children of id ordered by size descending (ties by ID).
    pub fn sorted_children(&self, id: NodeId, m: SizeMode) -> Vec<NodeId> {
        let mut out: Vec<(i64, NodeId)> = self.children(id).map(|(c, n)| (n.total(m), c)).collect();
        out.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        out.into_iter().map(|(_, c)| c).collect()
    }

    /// Aggregates file statistics by extension beneath root.
    pub fn ext_stats(&self, root: NodeId) -> Vec<ExtStat> {
        let mut stats = vec![ExtStat::default(); self.ext_count()];
        self.walk(root, |_, n| {
            if n.kind == Kind::File && n.flags & FLAG_HARDLINK_DUP == 0 {
                let s = &mut stats[n.ext as usize];
                s.count += 1;
                s.size += n.size;
                s.alloc += n.alloc;
            }
            true
        });
        stats
            .into_iter()
            .enumerate()
            .filter(|(_, s)| s.count > 0)
            .map(|(i, mut s)| {
                s.ext = self.exts[i].clone();
                s.cat = category_for(&s.ext, false, false);
                s
            })
            .collect()
    }
}

/// Summarises one extension.
#[derive(Clone, Default, Debug)]
pub struct ExtStat {
    pub ext: String,
    pub cat: Category,
    pub count: i64,
    pub size: i64, // logical
    pub alloc: i64,
}
