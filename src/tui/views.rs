//! The Tab-cycled main views and their dispatch.

use super::actions::DupView;
use super::canvas::{Canvas, Color, Style, BOLD};
use super::infoview::InfoView;
use super::mapview::{map_key, map_paint, map_wheel};
use super::modals::new_info_modal;
use super::model::{Cmd, Model};
use super::table::{col, Table, TableRow};
use super::{fmt_date, san};
use crate::inventory::{
    ExtStat, Kind, NodeId, SizeMode, TopK, Tree, FLAG_DELETED, FLAG_HARDLINKED, FLAG_HARDLINK_DUP,
    FLAG_SPARSE, NO_NODE,
};
use crate::snapshot::{Change, Status};
use crate::textutil;
use crate::treemap::Rect;
use std::collections::HashMap;
use std::rc::Rc;

/// One of the Tab-cycled main views. Taken is a placeholder while a view
/// is borrowed out of the model to run one of its methods.
pub(crate) enum View {
    Map,
    List(ListView),
    Ext(ExtView),
    Top(TopView),
    Info(InfoView),
    Changes(ChangesView),
    Dups(DupView),
    Taken,
}

impl View {
    pub fn name(&self) -> &'static str {
        match self {
            View::Map => "Map",
            View::List(_) => "List",
            View::Ext(_) => "Types",
            View::Top(_) => "Top",
            View::Info(_) => "Scan",
            View::Changes(_) => "Changes",
            View::Dups(_) => "Dups",
            View::Taken => "",
        }
    }

    pub fn table_mut(&mut self) -> Option<&mut Table> {
        match self {
            View::List(v) => Some(&mut v.tb),
            View::Ext(v) => Some(&mut v.tb),
            View::Top(v) => Some(&mut v.tb),
            View::Info(v) => Some(&mut v.tb),
            View::Changes(v) => Some(&mut v.tb),
            View::Dups(v) => Some(&mut v.tb),
            View::Map | View::Taken => None,
        }
    }
}

impl Model {
    /// Runs f on the current view, temporarily moved out of the model.
    fn with_view<R>(&mut self, f: impl FnOnce(&mut View, &mut Model) -> R) -> R {
        let i = self.view;
        let mut v = std::mem::replace(&mut self.views[i], View::Taken);
        let r = f(&mut v, self);
        self.views[i] = v;
        r
    }

    pub fn paint_view(&mut self, tr: &Tree, cv: &mut Canvas, r: Rect) {
        self.with_view(|v, m| match v {
            View::Map => map_paint(m, tr, cv, r),
            View::List(v) => v.paint(m, tr, cv, r),
            View::Ext(v) => v.paint(m, tr, cv, r),
            View::Top(v) => v.paint(m, tr, cv, r),
            View::Info(v) => v.paint(m, tr, cv, r),
            View::Changes(v) => v.paint(m, tr, cv, r),
            View::Dups(v) => v.paint(m, tr, cv, r),
            View::Taken => {}
        })
    }

    pub fn view_key(&mut self, tr: &Tree, k: &str) -> (bool, Cmd) {
        self.with_view(|v, m| match v {
            View::Map => map_key(m, k),
            View::List(v) => v.key(m, tr, k),
            View::Ext(v) => v.key(m, tr, k),
            View::Top(v) => v.key(m, tr, k),
            View::Info(v) => v.key(m, tr, k),
            View::Changes(v) => v.key(m, tr, k),
            View::Dups(v) => v.key(m, tr, k),
            View::Taken => (false, Cmd::None),
        })
    }

    pub fn view_wheel(&mut self, tr: &Tree, up: bool) -> Cmd {
        self.with_view(|v, m| {
            match v {
                View::Map => return map_wheel(m, tr, up),
                View::List(v) => v.wheel(m, tr, up),
                View::Ext(v) => {
                    let n = v.stats(m, tr).len();
                    v.tb.scroll(up, n)
                }
                View::Top(v) => v.tb.scroll(up, v.ids.len()),
                View::Info(v) => v.tb.scroll(up, tr.stats.errors.len()),
                View::Changes(v) => v.tb.scroll(up, v.rows.as_ref().map_or(0, |r| r.len())),
                View::Dups(v) => v.tb.scroll(up, v.rows.len()),
                View::Taken => {}
            }
            Cmd::None
        })
    }

    pub fn set_view_by_name(&mut self, n: &str) {
        for i in 0..self.views.len() {
            if self.views[i].name() == n {
                self.set_view(i);
            }
        }
    }
}

// ---- Directory list -------------------------------------------------------

#[derive(Default)]
pub(crate) struct ListView {
    tb: Table,
    last_sel: i64,
}

impl ListView {
    /// Reconciles the table cursor with the shared selection: if the
    /// selection changed elsewhere (treemap, jump) the cursor follows it;
    /// otherwise the cursor (moved by keys or clicks) drives the selection.
    fn sync(&mut self, m: &mut Model, tr: &Tree) {
        let k = m.kids_of(tr, m.zoom);
        if m.sel != self.last_sel {
            for (i, &id) in k.ids.iter().enumerate() {
                if id as i64 == m.sel {
                    self.tb.cursor = i as i32;
                }
            }
        } else if self.tb.at() < k.ids.len() {
            m.sel = k.ids[self.tb.at()] as i64;
        }
        self.last_sel = m.sel;
    }

    fn paint(&mut self, m: &mut Model, tr: &Tree, cv: &mut Canvas, r: Rect) {
        let t = m.theme.clone();
        let k = m.kids_of(tr, m.zoom);
        self.sync(m, tr);
        let total = m.size_of(tr, m.zoom);
        let mut rows: Vec<TableRow> = Vec::with_capacity(k.ids.len());
        for (i, &id) in k.ids.iter().enumerate() {
            let n = tr.node(id);
            let mut name = san(&n.name);
            let mut files = String::new();
            if n.is_dir() {
                name.push('/');
                files = textutil::count(n.files as i64);
            }
            let modified = if n.mtime > 0 {
                fmt_date(n.mtime)
            } else {
                String::new()
            };
            let c = m.color_of(tr, id as i64);
            let mut name_st = Style::new(c.mix(t.fg, 0.45), t.bg);
            if n.is_dir() {
                name_st.attr = BOLD;
            }
            let frac = if k.sizes[0] > 0 {
                k.sizes[i] as f64 / k.sizes[0] as f64
            } else {
                0.0
            };
            let muted = Some(Style::new(t.muted, t.bg));
            let mut row = TableRow {
                cells: vec![
                    textutil::size(k.sizes[i]),
                    textutil::percent(k.sizes[i], total),
                    String::new(),
                    files,
                    modified,
                    name,
                ],
                styles: vec![None, muted, None, muted, muted, Some(name_st)],
                bar: frac,
                bar_col: c,
            };
            if let Some(d) = &m.opts.diff {
                row.cells[4] = textutil::signed_size(d.delta(tr, id, m.size_mode));
            }
            rows.push(row);
        }
        let mut cols = vec![
            col("SIZE", 10, true, false),
            col("%", 5, true, false),
            col("", 12, false, false),
            col("FILES", 10, true, false),
            col("MODIFIED", 10, false, false),
            col("NAME", 0, false, false),
        ];
        if m.opts.diff.is_some() {
            cols[4] = col("CHANGE", 12, true, false);
        }
        if r.w < 70 {
            cols = vec![
                col("SIZE", 10, true, false),
                col("%", 5, true, false),
                col("NAME", 0, false, false),
            ];
            for row in rows.iter_mut() {
                row.cells = vec![
                    std::mem::take(&mut row.cells[0]),
                    std::mem::take(&mut row.cells[1]),
                    std::mem::take(&mut row.cells[5]),
                ];
                row.styles = vec![None, row.styles[1], row.styles[5]];
            }
        }
        self.tb.paint(m, cv, r, &cols, &rows, "");
        self.sync(m, tr);
    }

    fn key(&mut self, m: &mut Model, tr: &Tree, k: &str) -> (bool, Cmd) {
        self.sync(m, tr);
        let n = m.kids_of(tr, m.zoom).ids.len();
        let handled = self.tb.key(k, n);
        self.sync(m, tr);
        (handled, Cmd::None)
    }

    fn wheel(&mut self, m: &mut Model, tr: &Tree, up: bool) {
        self.sync(m, tr);
        let n = m.kids_of(tr, m.zoom).ids.len();
        self.tb.scroll(up, n);
        self.sync(m, tr);
    }
}

// ---- Extension statistics ------------------------------------------------

#[derive(Default)]
pub(crate) struct ExtView {
    tb: Table,
    cache: Option<Rc<Vec<ExtStat>>>,
    cache_key: (NodeId, u64),
}

fn pick(m: &Model, s: &ExtStat) -> i64 {
    if m.size_mode == SizeMode::Logical {
        s.size
    } else {
        s.alloc
    }
}

impl ExtView {
    fn stats(&mut self, m: &Model, tr: &Tree) -> Rc<Vec<ExtStat>> {
        let k = (m.zoom, m.data_ver * 2 + m.size_mode as u64);
        if self.cache.is_none() || self.cache_key != k {
            let mut v = tr.ext_stats(m.zoom);
            v.sort_by(|a, b| pick(m, b).cmp(&pick(m, a)).then_with(|| a.ext.cmp(&b.ext)));
            self.cache = Some(Rc::new(v));
            self.cache_key = k;
        }
        self.cache.clone().unwrap()
    }

    fn paint(&mut self, m: &mut Model, tr: &Tree, cv: &mut Canvas, r: Rect) {
        let t = m.theme.clone();
        let st = self.stats(m, tr);
        let total: i64 = st.iter().map(|s| pick(m, s)).sum();
        let v0 = st.first().map_or(0, |s| pick(m, s));
        let rows: Vec<TableRow> = st
            .iter()
            .map(|s| {
                let ext = if s.ext.is_empty() {
                    "(none)".to_string()
                } else {
                    format!(".{}", s.ext)
                };
                let c = t
                    .ext
                    .as_ref()
                    .and_then(|e| e.get(&s.ext))
                    .copied()
                    .unwrap_or(t.cat[s.cat as usize]);
                let frac = if v0 > 0 {
                    pick(m, s) as f64 / v0 as f64
                } else {
                    0.0
                };
                TableRow {
                    cells: vec![
                        textutil::sanitize(&ext),
                        s.cat.name().into(),
                        textutil::count(s.count),
                        textutil::size(pick(m, s)),
                        textutil::percent(pick(m, s), total),
                        String::new(),
                    ],
                    styles: vec![
                        Some(Style::new(c, t.bg).with(BOLD)),
                        Some(Style::new(t.muted, t.bg)),
                        None,
                        None,
                        Some(Style::new(t.muted, t.bg)),
                        None,
                    ],
                    bar: frac,
                    bar_col: c,
                }
            })
            .collect();
        let cols = [
            col("EXTENSION", 14, false, false),
            col("CATEGORY", 12, false, false),
            col("FILES", 11, true, false),
            col("SIZE", 10, true, false),
            col("%", 5, true, false),
            col("", 0, false, false),
        ];
        let title = format!(
            "File types under {} — Enter filters the map by extension",
            san(&tr.node(m.zoom).name)
        );
        self.tb.paint(m, cv, r, &cols, &rows, &title);
    }

    fn key(&mut self, m: &mut Model, tr: &Tree, k: &str) -> (bool, Cmd) {
        let st = self.stats(m, tr);
        if self.tb.key(k, st.len()) {
            return (true, Cmd::None);
        }
        if (k == "enter" || k == "space") && self.tb.at() < st.len() {
            let e = &st[self.tb.at()].ext;
            let q = if e.is_empty() {
                "type = file AND ext = \"\"".to_string()
            } else {
                format!("ext = \"{e}\"")
            };
            m.set_view_by_name("Map");
            return (true, m.set_filter(&q));
        }
        (false, Cmd::None)
    }
}

// ---- Top-N investigation views --------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum TopMode {
    #[default]
    Files,
    Dirs,
    Old,
    New,
    Count,
    Sparse,
    Hardlinks,
}

const TOP_MODES: [TopMode; 7] = [
    TopMode::Files,
    TopMode::Dirs,
    TopMode::Old,
    TopMode::New,
    TopMode::Count,
    TopMode::Sparse,
    TopMode::Hardlinks,
];

const TOP_NAMES: [&str; 7] = [
    "Largest files",
    "Largest directories (own files)",
    "Oldest large files",
    "Newest large files",
    "Most files",
    "Sparse files",
    "Hard-linked files",
];

const TOP_LIMIT: usize = 500;

#[derive(Default)]
pub(crate) struct TopView {
    tb: Table,
    mode: TopMode,
    ids: Vec<NodeId>,
    vals: Vec<i64>,
    computed: bool,
    cache_key: (NodeId, u64, u8),
}

impl TopView {
    fn compute(&mut self, m: &Model, tr: &Tree) {
        let k = (m.zoom, m.data_ver * 2 + m.size_mode as u64, self.mode as u8);
        if self.computed && self.cache_key == k {
            return;
        }
        (self.cache_key, self.computed) = (k, true);
        let is_file = |n: &crate::inventory::Node| {
            n.kind == Kind::File && n.flags & (FLAG_HARDLINK_DUP | FLAG_DELETED) == 0
        };
        let matches = |id: NodeId| m.search.result.as_ref().is_none_or(|r| r.size(id) > 0);
        let mut tk = TopK::new(TOP_LIMIT);
        let mut val: HashMap<NodeId, i64> = HashMap::new();
        const LARGE_FILE: i64 = 1 << 20;
        match self.mode {
            TopMode::Dirs => {
                let mut own: HashMap<NodeId, i64> = HashMap::new();
                tr.walk(m.zoom, |id, n| {
                    if is_file(n) && matches(id) {
                        *own.entry(n.parent).or_default() += n.own(m.size_mode);
                    }
                    true
                });
                for (id, s) in own {
                    tk.offer(id, s);
                    val.insert(id, s);
                }
            }
            TopMode::Count => tr.walk(m.zoom, |id, n| {
                if n.is_dir() && matches(id) {
                    let c = tr.child_count(id) as i64;
                    tk.offer(id, c);
                    val.insert(id, c);
                }
                true
            }),
            mode => tr.walk(m.zoom, |id, n| {
                if !is_file(n) || !matches(id) {
                    return true;
                }
                let sz = n.own(m.size_mode);
                match mode {
                    TopMode::Files => {
                        tk.offer(id, sz);
                        val.insert(id, sz);
                    }
                    TopMode::Old | TopMode::New => {
                        if sz >= LARGE_FILE && n.mtime > 0 {
                            tk.offer(
                                id,
                                if mode == TopMode::Old {
                                    -n.mtime
                                } else {
                                    n.mtime
                                },
                            );
                            val.insert(id, sz);
                        }
                    }
                    TopMode::Sparse => {
                        if n.has(FLAG_SPARSE) {
                            tk.offer(id, n.size - n.alloc);
                            val.insert(id, n.size - n.alloc);
                        }
                    }
                    TopMode::Hardlinks => {
                        if n.has(FLAG_HARDLINKED) {
                            tk.offer(id, n.size);
                            val.insert(id, n.size);
                        }
                    }
                    _ => {}
                }
                true
            }),
        }
        self.ids = tk.sorted();
        self.vals = self.ids.iter().map(|id| val[id]).collect();
    }

    fn paint(&mut self, m: &mut Model, tr: &Tree, cv: &mut Canvas, r: Rect) {
        let t = m.theme.clone();
        self.compute(m, tr);
        let first = self.vals.first().copied().unwrap_or(0);
        let mut rows = Vec::with_capacity(self.ids.len());
        for (i, &id) in self.ids.iter().enumerate() {
            let n = tr.node(id);
            let val = if self.mode == TopMode::Count {
                textutil::count(self.vals[i])
            } else {
                textutil::size(self.vals[i])
            };
            let modified = if n.mtime > 0 {
                fmt_date(n.mtime)
            } else {
                String::new()
            };
            let frac = if first > 0 && self.mode != TopMode::Old && self.mode != TopMode::New {
                self.vals[i] as f64 / first as f64
            } else {
                0.0
            };
            let c = m.color_of(tr, id as i64);
            rows.push(TableRow {
                cells: vec![val, modified, String::new(), san(&tr.path_bytes(id))],
                styles: vec![
                    None,
                    Some(Style::new(t.muted, t.bg)),
                    None,
                    Some(Style::new(c.mix(t.fg, 0.45), t.bg)),
                ],
                bar: frac,
                bar_col: c,
            });
        }
        let mut title = format!(
            "◂ {} ▸   (←/→ change list · Enter details · Space jump to it)",
            TOP_NAMES[self.mode as usize]
        );
        if m.search.result.is_some() {
            title += "  · filtered";
        }
        let val_title = match self.mode {
            TopMode::Count => "ENTRIES",
            TopMode::Sparse => "SAVED",
            _ => "SIZE",
        };
        let cols = [
            col(val_title, 10, true, false),
            col("MODIFIED", 10, false, false),
            col("", 10, false, false),
            col("PATH", 0, false, true),
        ];
        self.tb.paint(m, cv, r, &cols, &rows, &title);
    }

    fn key(&mut self, m: &mut Model, tr: &Tree, k: &str) -> (bool, Cmd) {
        self.compute(m, tr);
        let n = TOP_MODES.len();
        match k {
            "left" | "h" => {
                self.mode = TOP_MODES[(self.mode as usize + n - 1) % n];
                self.tb.cursor = 0;
                return (true, Cmd::None);
            }
            "right" | "l" => {
                self.mode = TOP_MODES[(self.mode as usize + 1) % n];
                self.tb.cursor = 0;
                return (true, Cmd::None);
            }
            _ => {}
        }
        if self.tb.key(k, self.ids.len()) {
            return (true, Cmd::None);
        }
        if self.tb.at() < self.ids.len() {
            let id = self.ids[self.tb.at()];
            match k {
                "enter" => {
                    let md = new_info_modal(m, tr, id);
                    m.open_modal(md);
                    return (true, Cmd::None);
                }
                "space" => {
                    m.jump_to(tr, id);
                    return (true, Cmd::None);
                }
                "c" | "o" | "d" => m.sel = id as i64,
                _ => {}
            }
        }
        (false, Cmd::None)
    }
}

// ---- Snapshot comparison -----------------------------------------------------

#[derive(Default)]
pub(crate) struct ChangesView {
    tb: Table,
    rows: Option<Vec<Change>>,
    cache_key: Option<SizeMode>,
}

impl ChangesView {
    fn paint(&mut self, m: &mut Model, tr: &Tree, cv: &mut Canvas, r: Rect) {
        let t = m.theme.clone();
        let Some(d) = &m.opts.diff else { return };
        if self.rows.is_none() || self.cache_key != Some(m.size_mode) {
            self.rows = Some(d.changes(tr, 1000, m.size_mode));
            self.cache_key = Some(m.size_mode);
        }
        let changes = self.rows.as_ref().unwrap();
        let max_abs = changes.iter().map(|c| c.delta.abs()).fold(1, i64::max);
        let rows: Vec<TableRow> = changes
            .iter()
            .map(|c| {
                let col = match c.status {
                    Status::Added => t.diff_new,
                    Status::Removed => t.muted,
                    Status::Grew => t.diff_grow,
                    Status::Shrank => t.diff_shrink,
                    Status::Unchanged => Color::default(),
                };
                TableRow {
                    cells: vec![
                        textutil::signed_size(c.delta),
                        c.status.name().into(),
                        textutil::size(c.old),
                        textutil::size(c.new),
                        String::new(),
                        textutil::sanitize(&c.path),
                    ],
                    styles: vec![
                        Some(Style::new(col, t.bg).with(BOLD)),
                        Some(Style::new(col, t.bg)),
                        Some(Style::new(t.muted, t.bg)),
                        None,
                        None,
                        None,
                    ],
                    bar: c.delta.abs() as f64 / max_abs as f64,
                    bar_col: col,
                }
            })
            .collect();
        let cols = [
            col("CHANGE", 13, true, false),
            col("STATUS", 9, false, false),
            col("OLD", 10, true, false),
            col("NEW", 10, true, false),
            col("", 10, false, false),
            col("PATH", 0, false, true),
        ];
        let (ro, rn) = (
            d.old.node(0).total(m.size_mode),
            tr.node(0).total(m.size_mode),
        );
        let title = format!(
            "Changes  {} → {}  ({})",
            textutil::size(ro),
            textutil::size(rn),
            textutil::signed_size(rn - ro)
        );
        self.tb.paint(m, cv, r, &cols, &rows, &title);
    }

    fn key(&mut self, m: &mut Model, tr: &Tree, k: &str) -> (bool, Cmd) {
        let n = self.rows.as_ref().map_or(0, |r| r.len());
        if self.tb.key(k, n) {
            return (true, Cmd::None);
        }
        if self.tb.at() < n {
            let c = &self.rows.as_ref().unwrap()[self.tb.at()];
            if c.node == NO_NODE {
                if k == "enter" || k == "space" {
                    let msg = format!(
                        "Removed since the old snapshot: {}",
                        textutil::sanitize(&c.path)
                    );
                    return (true, m.info(msg));
                }
                return (false, Cmd::None);
            }
            let id = c.node;
            match k {
                "enter" => {
                    let md = new_info_modal(m, tr, id);
                    m.open_modal(md);
                    return (true, Cmd::None);
                }
                "space" => {
                    m.jump_to(tr, id);
                    return (true, Cmd::None);
                }
                _ => {}
            }
        }
        (false, Cmd::None)
    }
}
