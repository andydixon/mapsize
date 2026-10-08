//! The scan information view: statistics and the error list.

use super::canvas::{Canvas, Style, BOLD};
use super::model::{Cmd, Model};
use super::table::{col, Table, TableRow};
use super::{san, si_units};
use crate::inventory::{ErrKind, Tree, FLAG_ALLOC_UNKNOWN};
use crate::textutil;
use crate::treemap::Rect;

#[derive(Default)]
pub(crate) struct InfoView {
    pub tb: Table,
}

impl InfoView {
    pub fn focus_errors(&mut self) {
        self.tb.cursor = 0;
    }

    pub fn paint(&mut self, m: &mut Model, tr: &Tree, cv: &mut Canvas, r: Rect) {
        let t = m.theme.clone();
        cv.fill(r, " ", t.base());
        let st = &tr.stats;
        let root = tr.node(0);
        let lw = if r.w < 100 { r.w - 2 } else { 52.min(r.w / 2) };
        let mut y = r.y + 1;
        let x = r.x + 2;
        let label = Style::new(t.muted, t.bg);
        let value = Style::new(t.fg, t.bg).with(BOLD);
        let line = |cv: &mut Canvas, y: &mut i32, k: &str, val: &str, vs: Style| {
            if *y >= r.y + r.h {
                return;
            }
            cv.text(x, *y, k, 20, label);
            cv.text(x + 20, *y, val, lw - 20, vs);
            *y += 1;
        };
        let head = |cv: &mut Canvas, y: &mut i32, s: &str| {
            if *y >= r.y + r.h - 1 {
                return;
            }
            *y += 1;
            cv.text(x, *y, s, lw, Style::new(t.accent, t.bg).with(BOLD));
            *y += 1;
        };
        let (state, mut state_st) = ("Complete", Style::new(t.ok, t.bg).with(BOLD));
        let state = if m.scanning {
            state_st.fg = t.accent;
            "Scanning…"
        } else if st.cancelled {
            state_st.fg = t.warn;
            "Cancelled — totals incomplete"
        } else if st.incomplete() {
            state_st.fg = t.warn;
            "Complete, with unreadable areas"
        } else {
            state
        };
        cv.text(x, y, "SCAN INFORMATION", lw, Style::new(t.accent, t.bg).with(BOLD));
        y += 1;
        line(cv, &mut y, "Root", &san(&st.root), value);
        line(cv, &mut y, "State", state, state_st);
        if !st.from_snapshot.is_empty() {
            line(cv, &mut y, "Snapshot", &textutil::sanitize(&st.from_snapshot), value);
        }
        let files = st.files + st.symlinks + st.others;
        line(cv, &mut y, "Items scanned", &textutil::count(files + st.dirs), value);
        line(cv, &mut y, "  Files", &textutil::count(st.files), value);
        line(cv, &mut y, "  Directories", &textutil::count(st.dirs), value);
        line(cv, &mut y, "  Symlinks", &textutil::count(st.symlinks), value);
        line(cv, &mut y, "  Special", &textutil::count(st.others), value);
        line(cv, &mut y, "Logical size", &format!("{}  ({} bytes)", textutil::size(root.tot_size), textutil::count(root.tot_size)), value);
        line(cv, &mut y, "Allocated size", &format!("{}  ({} bytes)", textutil::size(root.tot_alloc), textutil::count(root.tot_alloc)), value);
        let el = st.elapsed();
        if st.from_snapshot.is_empty() {
            line(cv, &mut y, "Elapsed", &textutil::duration(el), value);
            let rate = (files + st.dirs) as f64 / el.as_secs_f64().max(0.001);
            line(cv, &mut y, "Rate", &(textutil::count(rate as i64) + " items/s"), value);
            let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
            line(cv, &mut y, "Workers", &format!("{}  ({} CPUs)", st.workers, cpus), value);
        }

        head(cv, &mut y, "ERRORS AND OMISSIONS");
        let mut any_issue = false;
        for (k, &c) in st.err_counts.iter().enumerate() {
            if c > 0 {
                line(cv, &mut y, ErrKind::from_u8(k as u8).name(), &textutil::count(c), Style::new(t.err, t.bg).with(BOLD));
                any_issue = true;
            }
        }
        for (k, v) in [("Unscanned dirs", st.unscanned), ("Excluded", st.excluded), ("Other FS skipped", st.skipped_mounts),
            ("Virtual FS skipped", st.virtual_skipped), ("Loops skipped", st.loops_skipped), ("Broken symlinks", st.broken_links),
            ("Hard-link dups", st.hardlink_dups)]
        {
            if v > 0 {
                line(cv, &mut y, k, &textutil::count(v), Style::new(t.warn, t.bg).with(BOLD));
                any_issue = true;
            }
        }
        if !any_issue {
            line(cv, &mut y, "None", "every entry was read", Style::new(t.ok, t.bg));
        }
        head(cv, &mut y, "OPTIONS");
        let excl = if st.excludes.is_empty() { "none".to_string() } else { textutil::sanitize(&st.excludes.join(", ")) };
        line(cv, &mut y, "Exclusions", &excl, value);
        line(cv, &mut y, "One filesystem", &st.one_file_system.to_string(), value);
        line(cv, &mut y, "Follow symlinks", &textutil::sanitize(&st.follow), value);
        line(cv, &mut y, "Size mode", m.size_mode.name(), value);
        line(cv, &mut y, "Units", if si_units() { "SI (1000)" } else { "IEC (1024)" }, value);
        if root.has(FLAG_ALLOC_UNKNOWN) {
            line(cv, &mut y, "Note", "allocated size unavailable on this platform", Style::new(t.warn, t.bg));
        }

        // Error list on the right (or below in narrow terminals).
        let er = if r.w < 100 {
            Rect { x: r.x, y: y + 1, w: r.w, h: r.y + r.h - y - 1 }
        } else {
            Rect { x: r.x + lw + 4, y: r.y, w: r.w - lw - 4, h: r.h }
        };
        if er.w < 30 || er.h < 4 {
            return;
        }
        let rows: Vec<TableRow> = st
            .errors
            .iter()
            .map(|e| {
                let mut p = tr.path_bytes(e.node);
                if !e.name.is_empty() {
                    p.push(b'/');
                    p.extend_from_slice(&e.name);
                }
                TableRow {
                    cells: vec![e.kind.name().into(), san(&p), textutil::sanitize(&e.msg)],
                    styles: vec![Some(Style::new(t.err, t.bg)), None, Some(Style::new(t.muted, t.bg))],
                    bar: -1.0,
                    ..Default::default()
                }
            })
            .collect();
        let mut title = format!("ERROR LIST ({})", textutil::count(st.total_errors()));
        if (st.errors.len() as i64) < st.total_errors() {
            title += &format!(" — first {} shown", textutil::count(st.errors.len() as i64));
        }
        let cols = [col("KIND", 18, false, false), col("PATH", 0, false, true), col("DETAIL", 22, false, false)];
        self.tb.paint(m, cv, er, &cols, &rows, &title);
    }

    pub fn key(&mut self, m: &mut Model, tr: &Tree, k: &str) -> (bool, Cmd) {
        let errs = &tr.stats.errors;
        if self.tb.key(k, errs.len()) {
            return (true, Cmd::None);
        }
        if self.tb.at() < errs.len() && (k == "enter" || k == "space") {
            m.jump_to(tr, errs[self.tb.at()].node);
            return (true, Cmd::None);
        }
        (false, Cmd::None)
    }
}
