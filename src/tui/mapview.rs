//! The treemap view: layout with level-of-detail grouping, nested previews
//! inside directories, painting and spatial navigation.

use super::canvas::{rgb, Canvas, Style, BOLD, BOX_ROUND, FAINT};
use super::data::Kids;
use super::model::{group_id, is_group, Cmd, Hit, HitAct, Model};
use super::render::kind_label;
use super::{san, trunc, tw};
use crate::inventory::{NodeId, Tree, NO_NODE};
use crate::textutil;
use crate::treemap::{self, Block, Direction, Item, Rect};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// The treemap geometry for one (zoom, rect, data version). It is recomputed
/// from scratch whenever any of those change — after a resize the layout
/// algorithm runs again for the new rectangle rather than scaling the old
/// one.
pub(crate) struct TmLayout {
    pub zoom: NodeId,
    pub rect: Rect,
    pub ver: u64,
    pub blocks: Vec<Block>,  // top level: navigable
    pub nested: Vec<Nested>, // previews inside directories: drawn only
    pub groups: HashMap<i64, Group>,
    pub visible: HashSet<i64>,
    pub inner: HashSet<i64>, // top-level blocks with nested previews
}

#[derive(Clone, Copy)]
pub(crate) struct Nested {
    pub id: i64,
    pub r: Rect,
    pub depth: i32,
    pub alt: bool,
    pub inner: bool, // has nested children drawn inside it
}

/// An LOD group: the children of parent from index `from` on.
#[derive(Clone)]
pub(crate) struct Group {
    pub parent: NodeId,
    kids: Rc<Kids>,
    from: usize,
    pub size: i64,
}

impl Group {
    pub fn ids(&self) -> &[NodeId] {
        &self.kids.ids[self.from..]
    }
}

// LOD parameters.
pub(crate) const TOP_MIN_AREA: i32 = 12; // cells: room for a 4×3 framed block
const TOP_MAX_ITEMS: usize = 400;
const NEST_MIN_AREA: i32 = 3;
const NEST_MAX_ITEMS: usize = 120;
const MAX_NEST_DEPTH: i32 = 8; // block-size thresholds stop recursion long before this
const NEST_MIN_W: i32 = 4;
const NEST_MIN_H: i32 = 2;
const NEST_RECURSE_W: i32 = 14;
const NEST_RECURSE_H: i32 = 5;

impl Model {
    pub fn layout_treemap(&mut self, tr: &Tree, r: Rect) -> Rc<TmLayout> {
        if let Some(l) = &self.tm {
            if l.zoom == self.zoom && l.rect == r && l.ver == self.data_ver {
                return l.clone();
            }
        }
        let mut l = TmLayout {
            zoom: self.zoom,
            rect: r,
            ver: self.data_ver,
            blocks: Vec::new(),
            nested: Vec::new(),
            groups: HashMap::new(),
            visible: HashSet::new(),
            inner: HashSet::new(),
        };
        l.blocks = self.layout_level(tr, &mut l, self.zoom, r, TOP_MIN_AREA, TOP_MAX_ITEMS);
        for i in 0..l.blocks.len() {
            let b = l.blocks[i];
            l.visible.insert(b.id);
            if b.id < 0 || b.rect.w < 3 || b.rect.h < 2 {
                continue;
            }
            if tr.node(b.id as NodeId).is_dir() {
                let inn = b.rect.inset(1);
                if inn.w >= NEST_MIN_W && inn.h >= NEST_MIN_H {
                    let before = l.nested.len();
                    let end = self.chain_end(tr, b.id as NodeId);
                    self.layout_nested(tr, &mut l, end, inn, 2);
                    if l.nested.len() > before {
                        l.inner.insert(b.id);
                    }
                }
            }
        }
        let l = Rc::new(l);
        self.tm = Some(l.clone());
        l
    }

    /// Lays out the children of parent in r with level-of-detail grouping:
    /// children too small to be useful collapse into one group block.
    fn layout_level(
        &mut self,
        tr: &Tree,
        l: &mut TmLayout,
        parent: NodeId,
        r: Rect,
        min_area: i32,
        max_items: usize,
    ) -> Vec<Block> {
        let k = self.kids_of(tr, parent);
        if k.ids.is_empty() || r.empty() {
            return Vec::new();
        }
        let sizes: Vec<f64> = k.sizes.iter().map(|&s| s as f64).collect();
        let vis = treemap::visible(&sizes, k.total as f64, r.area(), min_area, max_items);
        let mut items: Vec<Item> = (0..vis)
            .map(|i| Item {
                id: k.ids[i] as i64,
                size: sizes[i],
            })
            .collect();
        if vis < k.ids.len() {
            let size = k.sizes[vis..].iter().sum();
            let gid = group_id(parent);
            l.groups.insert(
                gid,
                Group {
                    parent,
                    kids: k.clone(),
                    from: vis,
                    size,
                },
            );
            items.push(Item {
                id: gid,
                size: size as f64,
            });
        }
        treemap::squarify(&items, r)
    }

    fn layout_nested(&mut self, tr: &Tree, l: &mut TmLayout, parent: NodeId, r: Rect, depth: i32) {
        // Deeper levels need bigger blocks to stay legible rather than noisy.
        let blocks = self.layout_level(
            tr,
            l,
            parent,
            r,
            NEST_MIN_AREA * (depth - 1),
            NEST_MAX_ITEMS,
        );
        for (i, b) in blocks.iter().enumerate() {
            let idx = l.nested.len();
            l.nested.push(Nested {
                id: b.id,
                r: b.rect,
                depth,
                alt: i % 2 == 1,
                inner: false,
            });
            let br = b.rect;
            if depth < MAX_NEST_DEPTH
                && b.id >= 0
                && br.w >= NEST_RECURSE_W
                && br.h >= NEST_RECURSE_H
                && tr.node(b.id as NodeId).is_dir()
            {
                l.nested[idx].inner = true;
                let end = self.chain_end(tr, b.id as NodeId);
                self.layout_nested(
                    tr,
                    l,
                    end,
                    Rect {
                        x: br.x,
                        y: br.y + 1,
                        w: br.w - 1,
                        h: br.h - 2,
                    },
                    depth + 1,
                );
            }
        }
    }

    /// Follows directories whose only non-empty child is a directory
    /// (mnt → raid → media), so nesting spends its space on the level that
    /// actually branches instead of on full-size wrappers.
    pub fn chain_end(&mut self, tr: &Tree, mut id: NodeId) -> NodeId {
        loop {
            let k = self.kids_of(tr, id);
            if k.ids.len() != 1 || !tr.node(k.ids[0]).is_dir() {
                return id;
            }
            id = k.ids[0];
        }
    }

    /// The LOD group for a group ID in the current layout.
    pub fn group_info(&self, id: i64) -> Option<Group> {
        self.tm.as_ref()?.groups.get(&id).cloned()
    }

    /// A sanitised display name for a block.
    pub fn block_name(&mut self, tr: &Tree, id: i64) -> String {
        if is_group(id) {
            return match self.group_info(id) {
                Some(g) => textutil::count(g.ids().len() as i64) + " smaller items",
                None => "smaller items".into(),
            };
        }
        let mut nid = id as NodeId;
        let n = tr.node(nid);
        let mut name = san(&n.name);
        if n.is_dir() && nid != 0 {
            name.push('/');
            let end = self.chain_end(tr, nid);
            while nid != end {
                nid = self.kids_of(tr, nid).ids[0];
                name += &san(&tr.node(nid).name);
                name.push('/');
            }
        }
        name
    }

    fn block_size(&self, tr: &Tree, id: i64) -> i64 {
        if is_group(id) {
            return self.group_info(id).map_or(0, |g| g.size);
        }
        self.size_of(tr, id as NodeId)
    }

    /// The size of the folder a block sits in.
    fn parent_size(&self, tr: &Tree, id: i64) -> i64 {
        if is_group(id) {
            return self
                .group_info(id)
                .map_or(0, |g| self.size_of(tr, g.parent));
        }
        let p = tr.node(id as NodeId).parent;
        if p != NO_NODE {
            return self.size_of(tr, p);
        }
        0
    }

    fn paint_top(
        &mut self,
        tr: &Tree,
        cv: &mut Canvas,
        b: Block,
        sel: bool,
        hov: bool,
        total: i64,
    ) {
        let t = self.theme.clone();
        let base = self.color_of(tr, b.id);
        let fill = base.mix(t.bg, 0.62);
        let fill_ch = if !is_group(b.id) {
            " "
        } else if t.ascii {
            "."
        } else {
            "░"
        };
        let name = self.block_name(tr, b.id);
        let size = self.block_size(tr, b.id);
        let r = b.rect;
        if r.w < 3 || r.h < 2 {
            // Too small for a frame: a solid tile.
            let mut st = Style::new(base, base.mix(t.bg, 0.3));
            let mut ch = fill_ch;
            if sel {
                st = Style::new(t.text_on(t.sel), t.sel).with(BOLD | self.mono_rev());
                ch = if t.ascii { "#" } else { "◆" };
            } else if t.mono {
                ch = "▒";
            }
            cv.fill(r, " ", st);
            cv.text(r.x + (r.w - 1) / 2, r.y + (r.h - 1) / 2, ch, 1, st);
            return;
        }
        cv.fill(r, fill_ch, Style::new(base.mix(t.bg, 0.35), fill));
        let mut frame = Style::new(base.mix(t.bg, 0.15), fill);
        let mut bx = t.box_chars();
        let mut title = Style::new(base.mix(rgb(255, 255, 255), 0.35), fill).with(BOLD);
        if sel {
            bx = t.sel_box();
            frame = Style::new(t.sel, fill).with(BOLD);
            title = Style::new(t.text_on(t.sel), t.sel).with(BOLD | self.mono_rev());
        } else if hov {
            frame.fg = base.mix(rgb(255, 255, 255), 0.5);
        }
        if t.mono && !sel {
            title.attr = BOLD;
        }
        cv.draw_box(r, bx, frame);
        cv.text(
            r.x + 1,
            r.y,
            &trunc(&format!(" {name} "), r.w - 2),
            r.w - 2,
            title,
        );

        let mut label = textutil::size(size);
        let pct = textutil::percent(size, total);
        if r.w - 2 >= tw(&label) + tw(&pct) + 5 {
            label += &format!(" · {pct}");
        } else if r.w - 2 < tw(&label) + 2 {
            label = textutil::size_compact(size);
        }
        let label = format!(" {label} ");
        if r.h >= 3 && tw(&label) <= r.w - 2 {
            cv.text_right(
                r.x + 1,
                r.y + r.h - 1,
                r.w - 2,
                &label,
                Style {
                    fg: frame.fg,
                    bg: fill,
                    attr: frame.attr,
                },
            );
        }
        // Interior labels for blocks without nested previews.
        let inn = r.inset(1);
        if inn.empty() || self.has_nested(b.id) {
            return;
        }
        let mut lines = Vec::new();
        if !is_group(b.id) {
            let nd = tr.node(b.id as NodeId);
            if !nd.is_dir() {
                lines.push(kind_label(nd));
            } else if nd.files > 0 {
                lines.push(textutil::count(nd.files as i64) + " files");
            }
        }
        if r.h < 4 || inn.w < 6 {
            return;
        }
        let y0 = inn.y + (inn.h - lines.len() as i32) / 2;
        for (i, s) in lines.iter().enumerate() {
            cv.text_centered(
                inn.x,
                y0 + i as i32,
                inn.w,
                s,
                Style::new(base.mix(t.fg, 0.4), fill),
            );
        }
    }

    fn has_nested(&self, id: i64) -> bool {
        self.tm.as_ref().is_some_and(|l| l.inner.contains(&id))
    }

    fn paint_nested(&mut self, tr: &Tree, cv: &mut Canvas, nb: &Nested) {
        let t = self.theme.clone();
        let base = self.color_of(tr, nb.id);
        let mut f = (0.38 - 0.1 * (nb.depth - 2) as f64).max(0.1);
        if nb.alt {
            f += 0.07;
        }
        let bg = base.mix(t.bg, f);
        let shadow = bg.mix(rgb(0, 0, 0), 0.45);
        let mut st = Style::new(t.text_on(bg), bg);
        let mut ch = " ";
        if is_group(nb.id) {
            ch = if t.ascii { "." } else { "░" };
            st.fg = bg.mix(t.fg, 0.25);
        }
        let r = nb.r;
        cv.fill(r, ch, st);
        if t.mono {
            // No colour: draw a light outline so neighbours stay distinguishable.
            if r.w >= 2 && r.h >= 2 {
                cv.draw_box(r, BOX_ROUND, Style::default().with(FAINT));
            }
        } else {
            if r.w >= 4 {
                cv.fill(
                    Rect {
                        x: r.x + r.w - 1,
                        y: r.y,
                        w: 1,
                        h: r.h,
                    },
                    " ",
                    Style::new(Default::default(), shadow),
                );
            }
            if r.h >= 3 {
                cv.fill(
                    Rect {
                        x: r.x,
                        y: r.y + r.h - 1,
                        w: r.w,
                        h: 1,
                    },
                    " ",
                    Style::new(Default::default(), shadow),
                );
            }
        }
        let lw = if t.mono { r.w - 2 } else { r.w - 1 };
        if lw < 5 {
            return;
        }
        let lx = if t.mono { r.x + 1 } else { r.x };
        let ly = if t.mono && r.h >= 3 { r.y + 1 } else { r.y };
        let name = self.block_name(tr, nb.id);
        if tw(&name) > lw && lw < 8 {
            return; // a stub like "te…" is noise, not information
        }
        cv.text(
            lx,
            ly,
            &trunc(&name, lw),
            lw,
            Style::new(st.fg, bg).with(BOLD),
        );
        if !nb.inner && r.h >= 3 && lw >= 5 {
            let size = self.block_size(tr, nb.id);
            let mut label = textutil::size_compact(size);
            let pct = textutil::percent(size, self.parent_size(tr, nb.id));
            if tw(&label) + tw(&pct) + 3 <= lw {
                label += &format!(" · {pct}");
            }
            cv.text(
                lx,
                ly + 1,
                &trunc(&label, lw),
                lw,
                Style::new(bg.mix(st.fg, 0.7), bg),
            );
        }
    }
}

pub(crate) fn map_paint(m: &mut Model, tr: &Tree, cv: &mut Canvas, r: Rect) {
    let t = m.theme.clone();
    let l = m.layout_treemap(tr, r);
    if l.blocks.is_empty() {
        let z = tr.node(m.zoom);
        let msg = if m.search.query.is_some() {
            "No matches here — Esc clears the filter".to_string()
        } else if m.scanning {
            "Scanning…".to_string()
        } else if !z.is_dir() {
            format!(
                "{} — {}",
                san(&z.name),
                textutil::size(m.size_of(tr, m.zoom))
            )
        } else {
            "Empty directory".to_string()
        };
        cv.text_centered(r.x, r.y + r.h / 2, r.w, &msg, t.muted());
        return;
    }
    let sel = m.visible_sel(&l.visible);
    let total = m.size_of(tr, m.zoom);
    for b in &l.blocks {
        let hov = b.id == m.hover;
        m.paint_top(tr, cv, *b, b.id == sel, hov, total);
    }
    for nb in &l.nested {
        m.paint_nested(tr, cv, nb);
    }
    for b in &l.blocks {
        m.hits.push(Hit {
            r: b.rect,
            act: HitAct::Block(b.id),
        });
    }
}

pub(crate) fn map_key(m: &mut Model, k: &str) -> (bool, Cmd) {
    let d = match k {
        "left" | "h" => Direction::Left,
        "right" | "l" => Direction::Right,
        "up" | "k" => Direction::Up,
        "down" | "j" => Direction::Down,
        _ => return (false, Cmd::None),
    };
    if let Some(l) = m.tm.clone() {
        if let Some(id) = treemap::neighbour(&l.blocks, m.visible_sel(&l.visible), d) {
            m.sel = id;
        }
    }
    (true, Cmd::None)
}

pub(crate) fn map_wheel(m: &mut Model, tr: &Tree, up: bool) -> Cmd {
    if up {
        return m.zoom_into_selection(tr);
    }
    m.zoom_out(tr);
    Cmd::None
}
