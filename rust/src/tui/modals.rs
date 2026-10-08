//! The concrete modals: item information, LOD group, help, confirmation and
//! save snapshot.

use super::canvas::{rgb, Style, BOLD};
use super::keys::default_snapshot_name;
use super::mapview::Group;
use super::modal::{kv, styled, txt, wrap, Line, Modal, Span};
use super::model::{Cmd, Model};
use super::search::LineEditor;
use super::{fmt_time, pad_left, pad_right, san, si_units, trunc, tw};
use crate::export;
use crate::filter;
use crate::inventory::{NodeId, Tree, FLAG_ALLOC_UNKNOWN, FLAG_HARDLINK_DUP, FLAG_INCOMPLETE, FLAG_LOOP, FLAG_SCANNED, FLAG_SKIPPED_FS, FLAG_SPARSE, FLAG_VIRTUAL_FS, NO_NODE};
use crate::platform;
use crate::textutil;

// ---- Item information -------------------------------------------------------

struct InfoModal {
    id: NodeId,
    btime: i64, // live birth time (0 if unknown)
}

pub(crate) fn new_info_modal(m: &Model, tr: &Tree, id: NodeId) -> Box<dyn Modal> {
    let mut md = InfoModal { id, btime: 0 };
    if !m.snapshot {
        // Birth time is not kept in the inventory; read it live.
        if let Ok(meta) = platform::lstat(&tr.path_bytes(id)) {
            md.btime = meta.btime;
        }
    }
    Box::new(md)
}

impl Modal for InfoModal {
    fn width(&self) -> i32 {
        72
    }

    fn title(&self) -> String {
        "Item information".into()
    }

    fn footer(&self, m: &Model) -> String {
        let mut s = format!("Esc close  ·  c copy path  ·  o {}  ·  Space go to", reveal_verb());
        if !m.opts.read_only && !m.snapshot {
            s += &format!("  ·  d {}", delete_verb());
        }
        s
    }

    fn content(&self, m: &mut Model, tr: &Tree, w: i32) -> Vec<Line> {
        let t = m.theme.clone();
        let n = tr.node(self.id);
        let ks = Style::new(t.muted, Default::default());
        let vs = Style::new(t.fg, Default::default());
        let bold = vs.with(BOLD);
        let warn = Style::new(t.warn, Default::default()).with(BOLD);
        let mut out: Vec<Line> = Vec::new();
        let add = |out: &mut Vec<Line>, k: &str, v: String| out.push(kv(k, v, ks, vs));
        let add_s = |out: &mut Vec<Line>, k: &str, v: String, st: Style| out.push(kv(k, v, ks, st));

        let col = m.color_of(tr, self.id as i64);
        let title = Style::new(col.mix(rgb(255, 255, 255), 0.3), Default::default()).with(BOLD);
        for l in wrap(&san(&n.name), w) {
            out.push(styled(l, title));
        }
        out.push(txt(""));
        out.push(styled("Path", ks));
        for l in wrap(&san(&tr.path_bytes(self.id)), w) {
            out.push(styled(l, vs));
        }
        if san(&n.name).as_bytes() != &*n.name {
            out.push(styled("Name contains control or invalid characters (shown escaped)", warn));
        }
        out.push(txt(""));
        let mode = m.size_mode;
        if n.is_dir() {
            add_s(&mut out, "Total logical", format!("{}  ({} bytes)", textutil::size(n.tot_size), textutil::count(n.tot_size)), bold);
            add_s(&mut out, "Total allocated", format!("{}  ({} bytes)", textutil::size(n.tot_alloc), textutil::count(n.tot_alloc)), bold);
            add(&mut out, "Files", textutil::count(n.files as i64));
            add(&mut out, "Directories", textutil::count(n.dirs as i64));
            if n.parent != NO_NODE {
                add(&mut out, "Of parent", textutil::percent(n.total(mode), tr.node(n.parent).total(mode)));
            }
            add(&mut out, "Of scan", textutil::percent(n.total(mode), tr.node(0).total(mode)));
            let k = m.kids_of(tr, self.id);
            if let Some(&first) = k.ids.first() {
                add(&mut out, "Largest child", format!("{} ({})", san(&tr.node(first).name), textutil::size(k.sizes[0])));
            }
            if n.errors > 0 {
                add_s(&mut out, "Errors beneath", textutil::count(n.errors as i64), Style::new(t.err, Default::default()).with(BOLD));
            }
            let state = if n.has(FLAG_INCOMPLETE) {
                "incomplete (not fully read)"
            } else if !n.has(FLAG_SCANNED) && m.scanning {
                "scanning…"
            } else if n.has(FLAG_SKIPPED_FS | FLAG_VIRTUAL_FS | FLAG_LOOP) {
                "not descended"
            } else {
                "complete"
            };
            add(&mut out, "Scan state", state.into());
        } else {
            add_s(&mut out, "Logical size", format!("{}  ({} bytes)", textutil::size(n.size), textutil::count(n.size)), bold);
            let alloc = if n.has(FLAG_ALLOC_UNKNOWN) {
                "unknown on this platform".to_string()
            } else {
                format!("{}  ({} bytes)", textutil::size(n.alloc), textutil::count(n.alloc))
            };
            add_s(&mut out, "Allocated size", alloc, bold);
            if n.parent != NO_NODE {
                add(&mut out, "Of parent", textutil::percent(n.own(mode), tr.node(n.parent).total(mode)));
            }
        }
        if let Some(r) = &m.search.result {
            add(&mut out, "Matching filter", textutil::size(r.size(self.id)));
        }
        if let Some(d) = &m.opts.diff {
            match d.old_size(self.id, mode) {
                Some(old) => {
                    add(&mut out, "In old snapshot", textutil::size(old));
                    add_s(&mut out, "Change", textutil::signed_size(n.total(mode) - old), bold);
                }
                None => add_s(&mut out, "Change", "new since old snapshot".into(), Style::new(t.diff_new, Default::default()).with(BOLD)),
            }
        }
        out.push(txt(""));
        add(&mut out, "Type", n.kind.name().into());
        if !n.is_dir() {
            let e = tr.ext_name(n);
            if !e.is_empty() {
                add(&mut out, "Extension", textutil::sanitize(e));
            }
            add(&mut out, "Category", n.cat.name().into());
        }
        out.push(txt(""));
        if n.mtime > 0 {
            add(&mut out, "Modified", fmt_time(n.mtime));
        }
        if self.btime > 0 {
            add(&mut out, "Created", fmt_time(self.btime));
        }
        out.push(txt(""));
        add(&mut out, "Mode", platform::mode::string(n.mode));
        add(&mut out, "Owner", textutil::sanitize(&platform::user_name(n.uid)));
        add(&mut out, "Group", textutil::sanitize(&platform::group_name(n.gid)));
        if !n.is_dir() {
            add(&mut out, "Hard links", n.nlink.to_string());
            let sparse = if n.has(FLAG_SPARSE) {
                format!("Yes (or compressed) — {} not allocated", textutil::size(n.size - n.alloc))
            } else {
                "No".into()
            };
            add(&mut out, "Sparse", sparse);
        }
        let fl = export::flag_list(n.flags & !(FLAG_SCANNED | FLAG_SPARSE));
        if !fl.is_empty() {
            add_s(&mut out, "Flags", fl.join(", "), warn);
        }
        if n.has(FLAG_HARDLINK_DUP) {
            out.push(styled("Another path to this inode was counted; this one adds nothing to totals.", ks));
        }
        if m.snapshot {
            out.push(txt(""));
            out.push(styled("From snapshot — metadata as recorded, not live.", ks));
        }
        out
    }

    fn key(&mut self, m: &mut Model, tr: &Tree, k: &str) -> (bool, Cmd) {
        match k {
            "c" => (false, m.copy_path(&tr.path_bytes(self.id))),
            "o" => {
                m.sel = self.id as i64;
                // Headless, o navigates the map, which the modal would cover.
                let c = m.reveal(tr);
                (platform::headless(), c)
            }
            "enter" => (true, Cmd::None),
            "g" | "right" => {
                m.jump_to(tr, self.id);
                (true, Cmd::None)
            }
            "d" | "delete" => {
                m.sel = self.id as i64;
                (true, m.request_trash(tr))
            }
            _ => (false, Cmd::None),
        }
    }
}

// ---- LOD group ------------------------------------------------------------------

struct GroupModal {
    g: Group,
}

pub(crate) fn new_group_modal(g: Group) -> Box<dyn Modal> {
    Box::new(GroupModal { g })
}

impl Modal for GroupModal {
    fn width(&self) -> i32 {
        70
    }
    fn title(&self) -> String {
        "Smaller items (grouped)".into()
    }
    fn footer(&self, _: &Model) -> String {
        "Esc close  ·  Space shows them in the list view".into()
    }

    fn content(&self, m: &mut Model, tr: &Tree, w: i32) -> Vec<Line> {
        let t = m.theme.clone();
        let ids = self.g.ids();
        let mut out = vec![
            txt(format!("{} items too small to draw individually at this size,", textutil::count(ids.len() as i64))),
            txt(format!("totalling {}. Zoom in or enlarge the terminal to see them.", textutil::size(self.g.size))),
            txt(""),
        ];
        for (i, &id) in ids.iter().enumerate() {
            if i == 200 {
                out.push(styled(format!("… and {} more", textutil::count(ids.len() as i64 - 200)), Style::new(t.muted, Default::default())));
                break;
            }
            let n = tr.node(id);
            let mut name = san(&n.name);
            if n.is_dir() {
                name.push('/');
            }
            out.push(vec![
                Span { s: pad_left(&textutil::size(m.size_of(tr, id)), 11) + "  ", st: Some(Style::new(t.muted, Default::default())) },
                Span { s: trunc(&name, w - 13), st: Some(Style::new(m.color_of(tr, id as i64).mix(t.fg, 0.4), Default::default())) },
            ]);
        }
        out
    }

    fn key(&mut self, m: &mut Model, _: &Tree, k: &str) -> (bool, Cmd) {
        if k == "enter" {
            m.sel = self.g.ids()[0] as i64;
            m.set_view_by_name("List");
            return (true, Cmd::None);
        }
        (false, Cmd::None)
    }
}

// ---- Help ---------------------------------------------------------------------

struct HelpModal;

pub(crate) fn new_help_modal() -> Box<dyn Modal> {
    Box::new(HelpModal)
}

impl Modal for HelpModal {
    fn width(&self) -> i32 {
        78
    }
    fn title(&self) -> String {
        "Keyboard reference".into()
    }
    fn footer(&self, _: &Model) -> String {
        "Esc close  ·  ↑↓ scroll".into()
    }

    fn content(&self, m: &mut Model, _: &Tree, _: i32) -> Vec<Line> {
        let t = m.theme.clone();
        let h = Style::new(t.accent, Default::default()).with(BOLD);
        let k = Style::new(t.sel, Default::default()).with(BOLD);
        let d = Style::new(t.fg, Default::default());
        let muted = Style::new(t.muted, Default::default());
        let mut out: Vec<Line> = Vec::new();
        let sec = |out: &mut Vec<Line>, s: &str| {
            if !out.is_empty() {
                out.push(txt(""));
            }
            out.push(styled(s, h));
        };
        let row = |out: &mut Vec<Line>, keys: &str, desc: &str| {
            out.push(vec![Span { s: format!("  {}", pad_right(keys, 19)), st: Some(k) }, Span { s: desc.into(), st: Some(d) }]);
        };
        sec(&mut out, "NAVIGATION");
        row(&mut out, "↑ ↓ ← →  hjkl", "move spatially through the treemap");
        row(&mut out, "Enter", "inspect the selected item");
        row(&mut out, "Space  →", "zoom into the selected directory");
        row(&mut out, "Backspace  ←", "zoom out one level");
        row(&mut out, "Home", "back to the scan root");
        row(&mut out, "click / dbl-click", "select / zoom (mouse)");
        row(&mut out, "wheel", "zoom in/out (map), scroll (lists)");
        sec(&mut out, "SEARCH & FILTER");
        row(&mut out, "/", "search or filter; the map shows only matches");
        row(&mut out, "Esc", "clear the filter");
        out.push(styled("    ubuntu   *.iso   size > 5GB   ext IN (iso,qcow2)   age > 365d", muted));
        out.push(styled("    path contains cache AND NOT type = dir   owner = andy   flag = sparse", muted));
        let fields: Vec<&str> = filter::FIELDS.split_whitespace().collect();
        let split = 7.min(fields.len());
        out.push(styled(format!("    fields: {}", fields[..split].join(" ")), muted));
        out.push(styled(format!("            {}", fields[split..].join(" ")), muted));
        out.push(styled("    sizes: KiB/MiB/GiB (1024), kB/MB/GB (1000); ages: s min h d w mo y", muted));
        sec(&mut out, "VIEWS");
        row(&mut out, "Tab  Shift+Tab", "next / previous view");
        row(&mut out, "1 – 7", "jump to a view");
        row(&mut out, "x", "file types (extension statistics)");
        row(&mut out, "g", "top lists: largest, oldest, sparse, hard links …");
        row(&mut out, "e", "scan information and errors");
        row(&mut out, "D", "find duplicate files (reads file contents)");
        sec(&mut out, "APPLICATION");
        row(&mut out, "a", "toggle allocated / logical (apparent) size");
        row(&mut out, "c", "copy path to clipboard");
        row(&mut out, "o", reveal_verb());
        if !m.opts.read_only {
            row(&mut out, "d", &format!("{} (asks first)", delete_verb()));
        }
        row(&mut out, "s", "save snapshot");
        row(&mut out, "r", "rescan");
        row(&mut out, "T", "cycle colour theme");
        row(&mut out, "Ctrl+C", "cancel scan; press again to quit");
        row(&mut out, "q", "quit");
        out.push(txt(""));
        let units = if si_units() { "SI (kB = 1000 bytes)" } else { "IEC (KiB = 1024 bytes)" };
        out.push(styled(format!("Sizes: {} · units: {units}", m.size_mode.name()), muted));
        out
    }

    fn key(&mut self, _: &mut Model, _: &Tree, _: &str) -> (bool, Cmd) {
        (false, Cmd::None)
    }
}

// ---- Confirmation -------------------------------------------------------------

pub(crate) type OnYes = Box<dyn FnOnce(&mut Model) -> Cmd>;

pub(crate) struct ConfirmModal {
    pub ttl: String,
    pub body: Vec<String>,
    pub yes: String,
    pub on_yes: Option<OnYes>,
}

impl Modal for ConfirmModal {
    fn width(&self) -> i32 {
        self.body.iter().map(|b| tw(b)).fold(40, i32::max).min(90)
    }
    fn title(&self) -> String {
        self.ttl.clone()
    }
    fn footer(&self, _: &Model) -> String {
        format!("y {}  ·  n / Esc cancel", self.yes)
    }
    fn content(&self, _: &mut Model, _: &Tree, w: i32) -> Vec<Line> {
        self.body.iter().flat_map(|b| wrap(b, w)).map(txt).collect()
    }
    fn key(&mut self, m: &mut Model, _: &Tree, k: &str) -> (bool, Cmd) {
        match k {
            "y" | "Y" => (true, self.on_yes.take().map_or(Cmd::None, |f| f(m))),
            "n" | "N" | "enter" => (true, Cmd::None),
            _ => (false, Cmd::None),
        }
    }
}

// ---- Save snapshot ---------------------------------------------------------------

struct SaveModal {
    ed: LineEditor,
}

pub(crate) fn new_save_modal(tr: &Tree) -> Box<dyn Modal> {
    let mut md = SaveModal { ed: LineEditor::default() };
    md.ed.set(&default_snapshot_name(&tr.stats.root));
    Box::new(md)
}

impl Modal for SaveModal {
    fn editor(&mut self) -> Option<&mut LineEditor> {
        Some(&mut self.ed)
    }
    fn width(&self) -> i32 {
        70
    }
    fn title(&self) -> String {
        "Save snapshot".into()
    }
    fn footer(&self, _: &Model) -> String {
        "Enter save  ·  Esc cancel".into()
    }

    fn content(&self, m: &mut Model, _: &Tree, w: i32) -> Vec<Line> {
        let t = &m.theme;
        let mut out = vec![txt("File:")];
        let s = textutil::sanitize(&self.ed.text());
        for l in wrap(&(s + "█"), w) {
            out.push(styled(l, Style::new(t.fg, Default::default()).with(BOLD)));
        }
        if m.scanning {
            out.push(txt(""));
            out.push(styled("The scan is still running: the snapshot will be marked incomplete.", Style::new(t.warn, Default::default())));
        }
        out
    }

    fn key(&mut self, m: &mut Model, _: &Tree, k: &str) -> (bool, Cmd) {
        match k {
            "esc" => (true, Cmd::None),
            "enter" => {
                let p = self.ed.text().trim().to_string();
                if p.is_empty() {
                    return (false, Cmd::None);
                }
                let s = m.save_snapshot(p);
                let i = m.info("Saving snapshot…");
                (true, Cmd::batch(vec![s, i]))
            }
            _ => (false, Cmd::None),
        }
    }
}

/// Describe o and d, which change meaning on a headless system.
pub(crate) fn reveal_verb() -> &'static str {
    if platform::headless() {
        "show folder in map"
    } else {
        "reveal in file manager"
    }
}

pub(crate) fn delete_verb() -> String {
    if platform::headless() {
        return "delete permanently".into();
    }
    format!("move to {}", platform::TRASH_NAME)
}
