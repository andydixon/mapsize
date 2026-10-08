//! Mutating and long-running actions: trash/delete and duplicate search.

use super::canvas::{Canvas, Style, BOLD};
use super::modals::{new_info_modal, ConfirmModal};
use super::model::{Cmd, Model, Msg};
use super::san;
use super::table::{col, Table, TableRow};
use crate::cancel::Cancel;
use crate::duplicate::{self, Finder};
use crate::inventory::{Delta, NodeId, Tree, FLAG_DELETED, NO_NODE};
use crate::platform::{self, Expect};
use crate::textutil;
use crate::treemap::Rect;
use std::sync::{Arc, RwLock};

// ---- Trash ---------------------------------------------------------------

impl Model {
    /// Asks for confirmation before moving the selection to the trash. On a
    /// headless system (no desktop, so nobody empties the trash and the space
    /// is never freed) it deletes permanently instead, after a confirmation
    /// that says so.
    pub fn request_trash(&mut self, tr: &Tree) -> Cmd {
        if self.opts.read_only {
            return self.warn("Read-only mode: modifying actions are disabled");
        }
        if self.snapshot {
            return self.warn("Snapshot view: nothing to delete on the live filesystem");
        }
        if self.scanning {
            return self.warn("Wait for the scan to finish before deleting items");
        }
        let id = self.selected_node(tr);
        if id == NO_NODE || id == 0 {
            return Cmd::None;
        }
        let n = tr.node(id);
        let path = tr.path(id);
        let want = Expect { uid: n.uid, mode: n.mode };
        let what = if n.is_dir() { format!("directory with {} files", textutil::count(n.files as i64)) } else { "file".into() };
        let mut body = vec![san(&tr.path_bytes(id)), String::new(), format!("{what} · {}", textutil::size(n.total(self.size_mode))), String::new()];
        if platform::headless() {
            body.push("This is a headless system (no X or Wayland display), so there is no trash:".into());
            body.push("it will be permanently deleted and cannot be recovered.".into());
            self.open_modal(Box::new(ConfirmModal {
                ttl: "Delete permanently? Are you sure?".into(),
                body,
                yes: "delete permanently".into(),
                on_yes: Some(Box::new(move |m: &mut Model| {
                    let t = m.tree.clone();
                    Cmd::run(move || {
                        let err = platform::delete(&path, want).err().map(|e| e.to_string());
                        Msg::TrashDone { tree: t, id, err, done: "Deleted".into() }
                    })
                })),
            }));
            return Cmd::None;
        }
        body.push(format!("It will be moved to the {}, not permanently deleted.", platform::TRASH_NAME));
        self.open_modal(Box::new(ConfirmModal {
            ttl: "Move to trash?".into(),
            body,
            yes: format!("move to {}", platform::TRASH_NAME),
            on_yes: Some(Box::new(move |m: &mut Model| {
                let t = m.tree.clone();
                Cmd::run(move || {
                    let err = platform::trash(&path, want).err().map(|e| e.to_string());
                    Msg::TrashDone { tree: t, id, err, done: format!("Moved to {}", platform::TRASH_NAME) }
                })
            })),
        }));
        Cmd::None
    }

    pub fn trash_done(&mut self, tree: Arc<RwLock<Tree>>, id: NodeId, err: Option<String>, done: String) -> Cmd {
        if let Some(e) = err {
            return self.warn(format!("Delete failed: {}", textutil::sanitize(&e)));
        }
        if !Arc::ptr_eq(&tree, &self.tree) || self.scanning {
            // A rescan replaced the inventory; the new scan reflects the change.
            return self.info(done);
        }
        // The scan has finished, so the UI thread is the tree's only writer.
        let name = {
            let mut t = self.tree.write().unwrap();
            let n = t.node(id).clone();
            let mut d = Delta { size: -n.tot_size, alloc: -n.tot_alloc, files: -(n.files as i64), dirs: -(n.dirs as i64), ..Default::default() };
            if n.is_dir() {
                d.dirs -= 1;
                if let Some(cs) = t.cat_sizes(id) {
                    for (c, v) in d.cat.iter_mut().zip(cs) {
                        *c = -v;
                    }
                }
            } else {
                d.files -= 1;
                d.cat[n.cat as usize] = -n.tot_alloc;
            }
            t.propagate(n.parent, &d);
            let nm = t.node_mut(id);
            nm.flags |= FLAG_DELETED;
            (nm.tot_size, nm.tot_alloc) = (0, 0);
            n.name
        };
        self.invalidate();
        self.info(format!("{done}: {}", san(&name)))
    }

    // ---- Duplicates ----------------------------------------------------------

    pub fn start_duplicates(&mut self) -> Cmd {
        if self.snapshot {
            return self.warn("Duplicate detection needs the live filesystem");
        }
        if self.scanning {
            return self.warn("Wait for the scan to finish first");
        }
        if self.dups.is_some() {
            self.ensure_dup_view();
            return Cmd::None;
        }
        self.open_modal(Box::new(ConfirmModal {
            ttl: "Find duplicate files?".into(),
            body: vec![
                "Files are grouped by size, then sampled, then fully hashed with SHA-256.".into(),
                "Only identical hashes are reported as duplicates.".into(),
                String::new(),
                "This reads file contents (it may take a while and can update access times).".into(),
            ],
            yes: "start".into(),
            on_yes: Some(Box::new(|m: &mut Model| {
                let cancel = Cancel::new();
                let f = Arc::new(Finder::default());
                m.dups = Some(DupState { running: true, finder: f.clone(), groups: Vec::new(), err: None, cancel: cancel.clone() });
                m.ensure_dup_view();
                let t = m.tree.clone();
                let tick = m.start_ticking();
                Cmd::batch(vec![
                    tick,
                    Cmd::run(move || {
                        let g = f.find(&cancel, &t, duplicate::Options { min_size: 1, workers: 4 });
                        Msg::DupDone { finder: f, groups: g }
                    }),
                ])
            })),
        }));
        Cmd::None
    }

    pub fn ensure_dup_view(&mut self) {
        if let Some(i) = self.views.iter().position(|v| matches!(v, super::views::View::Dups(_))) {
            self.set_view(i);
            return;
        }
        self.views.push(super::views::View::Dups(DupView::default()));
        self.set_view(self.views.len() - 1);
    }

    pub fn dup_done(&mut self, finder: Arc<Finder>, groups: Option<Vec<duplicate::Group>>) -> Cmd {
        let Some(d) = self.dups.as_mut().filter(|d| Arc::ptr_eq(&d.finder, &finder)) else { return Cmd::None };
        d.running = false;
        d.err = groups.is_none().then(|| "cancelled".to_string());
        d.groups = groups.unwrap_or_default();
        let (n, wasted) = (d.groups.len(), d.groups.iter().map(|g| g.wasted()).sum::<i64>());
        self.dirty = true;
        self.info(format!("Duplicates: {n} verified groups, {} reclaimable", textutil::size(wasted)))
    }
}

pub(crate) struct DupState {
    pub running: bool,
    pub finder: Arc<Finder>,
    pub groups: Vec<duplicate::Group>,
    pub err: Option<String>,
    pub cancel: Cancel,
}

#[derive(Default)]
pub(crate) struct DupView {
    pub tb: Table,
    pub rows: Vec<(usize, NodeId)>, // (group, file); NO_NODE for a group header
}

impl DupView {
    pub fn paint(&mut self, m: &mut Model, tr: &Tree, cv: &mut Canvas, r: Rect) {
        let t = m.theme.clone();
        let Some(d) = &m.dups else {
            cv.text_centered(r.x, r.y + r.h / 2, r.w, "Press D to search for duplicates", t.muted());
            return;
        };
        if d.running {
            let p = d.finder.progress();
            cv.fill(r, " ", t.base());
            let y = r.y + r.h / 2 - 2;
            cv.text_centered(r.x, y, r.w, &format!("Finding duplicates — {}…", p.stage), Style::new(t.accent, t.bg).with(BOLD));
            cv.text_centered(r.x, y + 2, r.w,
                &format!("{} candidate files in {} size groups", textutil::count(p.candidates), textutil::count(p.candidate_groups)), t.base());
            if p.bytes_to_hash > 0 {
                let frac = p.bytes_hashed as f64 / p.bytes_to_hash as f64;
                let bw = 60.min(r.w - 10);
                m.paint_bar(cv, r.x + (r.w - bw) / 2, y + 4, bw, frac, t.accent, false);
                cv.text_centered(r.x, y + 5, r.w,
                    &format!("{} of {} hashed · {} skipped", textutil::size(p.bytes_hashed), textutil::size(p.bytes_to_hash), textutil::count(p.skipped)),
                    t.muted());
            }
            return;
        }
        self.rows.clear();
        let mut rows = Vec::new();
        for (gi, g) in d.groups.iter().enumerate() {
            self.rows.push((gi, NO_NODE));
            let hash: String = g.hash[..8].iter().map(|b| format!("{b:02x}")).collect();
            rows.push(TableRow {
                cells: vec![textutil::size(g.wasted()), format!("{} × {}", g.files.len(), textutil::size(g.size)),
                    format!("verified identical · sha256 {hash}…")],
                styles: vec![Some(Style::new(t.warn, t.bg).with(BOLD)), Some(Style::new(t.fg, t.bg).with(BOLD)), Some(Style::new(t.ok, t.bg))],
                bar: -1.0,
                ..Default::default()
            });
            for &id in &g.files {
                self.rows.push((gi, id));
                rows.push(TableRow { cells: vec![String::new(), String::new(), format!("  {}", san(&tr.path_bytes(id)))], bar: -1.0, ..Default::default() });
            }
        }
        let mut title = format!("Duplicates — {} groups (Enter details · Space go to)", d.groups.len());
        if let Some(e) = &d.err {
            title += &format!(" — stopped: {}", textutil::sanitize(e));
        }
        let cols = [col("WASTED", 10, true, false), col("COPIES", 16, false, false), col("", 0, false, false)];
        self.tb.paint(m, cv, r, &cols, &rows, &title);
    }

    pub fn key(&mut self, m: &mut Model, tr: &Tree, k: &str) -> (bool, Cmd) {
        if self.tb.key(k, self.rows.len()) {
            return (true, Cmd::None);
        }
        if self.tb.at() < self.rows.len() {
            let (g, mut id) = self.rows[self.tb.at()];
            if id == NO_NODE {
                if let Some(d) = m.dups.as_ref().filter(|d| g < d.groups.len()) {
                    id = d.groups[g].files[0];
                }
            }
            if id != NO_NODE {
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
        }
        if k == "esc" {
            if let Some(d) = m.dups.as_ref().filter(|d| d.running) {
                d.cancel.cancel();
                return (true, Cmd::None);
            }
        }
        (false, Cmd::None)
    }
}
