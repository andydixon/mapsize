//! Size lookups, the cached sorted child lists, selection upkeep and
//! colouring.

use super::canvas::Color;
use super::model::{group_id, group_parent, is_group, Model, NO_SEL};
use crate::inventory::{NodeId, Tree, FLAG_DELETED, NO_NODE};
use crate::snapshot::Status;
use std::collections::HashSet;
use std::rc::Rc;

/// A cached, size-sorted child list (zero-size entries removed).
#[derive(Default)]
pub(crate) struct Kids {
    pub ids: Vec<NodeId>,
    pub sizes: Vec<i64>,
    pub total: i64,
}

impl Model {
    /// The size that drives layout: the filtered size when a filter is
    /// active, otherwise the aggregate in the current size mode.
    pub fn size_of(&self, tr: &Tree, id: NodeId) -> i64 {
        if let Some(r) = &self.search.result {
            return r.size(id);
        }
        let n = tr.node(id);
        if n.has(FLAG_DELETED) {
            return 0;
        }
        n.total(self.size_mode)
    }

    /// The sorted children of id, cached until data changes.
    pub fn kids_of(&mut self, tr: &Tree, id: NodeId) -> Rc<Kids> {
        if self.kids_ver != self.data_ver {
            self.kids_cache.clear();
            self.kids_ver = self.data_ver;
        }
        if let Some(k) = self.kids_cache.get(&id) {
            return k.clone();
        }
        let mut v: Vec<(i64, NodeId)> =
            tr.children(id).map(|(c, _)| (self.size_of(tr, c), c)).filter(|&(s, _)| s > 0).collect();
        v.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let k = Rc::new(Kids {
            total: v.iter().map(|e| e.0).sum(),
            sizes: v.iter().map(|e| e.0).collect(),
            ids: v.iter().map(|e| e.1).collect(),
        });
        self.kids_cache.insert(id, k.clone());
        k
    }

    /// The selected inventory node (NO_NODE for groups).
    pub fn selected_node(&self, tr: &Tree) -> NodeId {
        if self.sel == NO_SEL || is_group(self.sel) || self.sel >= tr.len() as i64 {
            return NO_NODE;
        }
        self.sel as NodeId
    }

    /// Keeps the selection meaningful after any change: if the selected node
    /// is no longer a direct, visible child of the zoom root it falls back to
    /// the visible ancestor, then the group containing it, then the largest
    /// child.
    pub fn ensure_selection(&mut self, tr: &Tree) {
        let k = self.kids_of(tr, self.zoom);
        if k.ids.is_empty() {
            self.sel = NO_SEL;
            return;
        }
        if !self.user_sel {
            // Until the user picks something, follow the largest block (it
            // changes while a scan is running).
            self.sel = k.ids[0] as i64;
            return;
        }
        if is_group(self.sel) {
            if group_parent(self.sel) == self.zoom {
                return;
            }
            self.sel = NO_SEL;
        }
        let id = self.selected_node(tr);
        if id != NO_NODE {
            // Walk up to the direct child of the zoom root.
            let mut cur = id;
            while cur != NO_NODE {
                if tr.node(cur).parent == self.zoom {
                    if self.size_of(tr, cur) > 0 {
                        self.sel = cur as i64;
                        return;
                    }
                    break;
                }
                cur = tr.node(cur).parent;
            }
        }
        self.sel = k.ids[0] as i64;
    }

    /// Maps the selection onto the treemap blocks: a node collapsed into the
    /// LOD group selects the group.
    pub fn visible_sel(&self, blocks: &HashSet<i64>) -> i64 {
        if blocks.contains(&self.sel) {
            return self.sel;
        }
        if !is_group(self.sel) && blocks.contains(&group_id(self.zoom)) {
            return group_id(self.zoom);
        }
        self.sel
    }

    /// Makes dir the treemap root.
    pub fn zoom_to(&mut self, tr: &Tree, dir: NodeId) {
        if dir == self.zoom {
            return;
        }
        let prev = self.zoom;
        self.zoom = dir;
        // Coming back out, keep the directory we left selected.
        self.sel = if tr.is_ancestor(dir, prev) { prev as i64 } else { NO_SEL };
        self.user_sel = self.sel != NO_SEL;
        self.zoomed = true;
        self.dirty = true;
    }

    pub fn set_view(&mut self, i: usize) {
        if i < self.views.len() {
            self.view = i;
            self.dirty = true;
        }
    }

    /// The base colour for a block.
    pub fn color_of(&self, tr: &Tree, id: i64) -> Color {
        let t = &self.theme;
        if is_group(id) {
            return t.group;
        }
        let nid = id as NodeId;
        let n = tr.node(nid);
        if let Some(d) = &self.opts.diff {
            let total = n.total(self.size_mode);
            return match d.status_of(tr, nid, self.size_mode) {
                Status::Added => t.diff_new,
                Status::Grew => {
                    let old = d.old_size(nid, self.size_mode).unwrap_or(0);
                    t.diff_same.mix(t.diff_grow, ratio(total - old, old))
                }
                Status::Shrank => {
                    let old = d.old_size(nid, self.size_mode).unwrap_or(0);
                    t.diff_same.mix(t.diff_shrink, ratio(old - total, old))
                }
                _ => t.diff_same,
            };
        }
        if !n.is_dir() {
            if let Some(c) = t.ext.as_ref().and_then(|e| e.get(tr.ext_name(n))) {
                return *c;
            }
            return t.cat[n.cat as usize];
        }
        let (mut best, mut best_v) = (None, 0);
        for (i, &v) in tr.cat_sizes(nid).into_iter().flatten().enumerate() {
            if v > best_v {
                (best, best_v) = (Some(i), v);
            }
        }
        best.map_or(t.empty_dir, |b| t.cat[b])
    }
}

/// Maps a relative change to 0.35..1 for colour intensity.
fn ratio(delta: i64, base: i64) -> f64 {
    if base <= 0 {
        return 1.0;
    }
    let r = delta as f64 / base as f64;
    (0.35 + r * 0.65).min(1.0)
}
