use super::eval::Ctx;
use super::parser::Query;
use crate::cancel::Cancel;
use crate::inventory::{NodeId, SizeMode, Tree, FLAG_HARDLINK_DUP};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Per-node filtered sizes: the bytes of matching content beneath each node.
/// A matching directory contributes its whole subtree (once, even if
/// descendants match too). matched marks every node satisfying the query.
/// Nodes added after the result was computed (live scan) have no entry and
/// are treated as not matching until the filter is re-applied.
pub struct FilterResult {
    pub query: Arc<Query>,
    pub mode: SizeMode,
    pub sizes: Vec<i64>,    // per NodeId; len = tree size at evaluation time
    pub matched: Vec<bool>, // node itself matched
    pub count: i64,         // number of matching nodes
    pub total: i64,
    pub elapsed: Duration,
}

impl FilterResult {
    /// The filtered size of id (0 if out of range).
    pub fn size(&self, id: NodeId) -> i64 {
        self.sizes.get(id as usize).copied().unwrap_or(0)
    }

    /// Reports whether id itself matched.
    pub fn is_match(&self, id: NodeId) -> bool {
        self.matched.get(id as usize).copied().unwrap_or(false)
    }
}

/// Evaluates q over the whole tree. The caller holds the read lock. It relies
/// on parents having smaller IDs than their children. None if cancelled.
pub fn apply(cancel: &Cancel, t: &Tree, q: Arc<Query>, m: SizeMode) -> Option<FilterResult> {
    let start = Instant::now();
    let n = t.len();
    let mut sizes = vec![0i64; n];
    let mut matched = vec![false; n];
    let mut covered = vec![false; n];
    let mut count = 0;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as i64);
    let mut c = Ctx::new(t, 0, now);
    if q.uses_path {
        c.dir_paths = Some(vec![String::new(); n]);
    }
    for i in 0..n {
        if i & 0xffff == 0 && cancel.is_cancelled() {
            return None;
        }
        let id = i as NodeId;
        let nd = t.node(id);
        c.reset(id);
        if c.dir_paths.is_some() && nd.is_dir() {
            c.fill_path();
            let p = c.path.clone();
            c.dir_paths.as_mut().unwrap()[i] = p;
        }
        if i > 0 && q.matches(&mut c) {
            matched[i] = true;
            covered[i] = true;
            count += 1;
        }
        if i > 0 && covered[nd.parent as usize] {
            covered[i] = true;
        }
        if covered[i] && nd.flags & FLAG_HARDLINK_DUP == 0 {
            sizes[i] = nd.own(m);
        }
    }
    for i in (1..n).rev() {
        let p = t.node(i as NodeId).parent as usize;
        sizes[p] = sizes[p].saturating_add(sizes[i]);
    }
    let total = sizes.first().copied().unwrap_or(0);
    Some(FilterResult { query: q, mode: m, sizes, matched, count, total, elapsed: start.elapsed() })
}
