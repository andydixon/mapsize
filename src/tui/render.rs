//! Frame composition: panel geometry, tabs, breadcrumbs, info and status
//! lines.

use super::canvas::{Attr, Canvas, Color, Depth, Style, BOLD, REVERSE, UNDERLINE};
use super::model::{is_group, Hit, HitAct, Model, Profile};
use super::{fmt_date, san, trunc_left, tw};
use crate::brand;
use crate::inventory::{Kind, Node, Tree, FLAG_SCANNED, NO_NODE};
use crate::textutil;
use crate::treemap::Rect;

/// Minimum usable terminal size.
pub(crate) const MIN_W: i32 = 40;
pub(crate) const MIN_H: i32 = 10;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum SizeClass {
    TooSmall,
    Small,
    Medium,
    Large,
}

/// The panel layout for one terminal size. It is recomputed from scratch for
/// every frame, so resizing can never leave stale geometry.
#[derive(Default)]
pub(crate) struct Geometry {
    pub tabs: Rect, // empty in the small layout
    pub crumbs: Rect,
    pub main: Rect,
    pub side: Rect,
    pub info: Rect,
    pub stat: Rect,
}

pub(crate) fn compute_geometry(w: i32, h: i32) -> (SizeClass, Geometry) {
    let mut g = Geometry::default();
    let class = if w < MIN_W || h < MIN_H {
        return (SizeClass::TooSmall, g);
    } else if w >= 150 && h >= 36 {
        SizeClass::Large
    } else if w >= 90 && h >= 22 {
        SizeClass::Medium
    } else {
        SizeClass::Small
    };
    let mut y = 0;
    if class >= SizeClass::Medium {
        g.tabs = Rect {
            x: 0,
            y: 0,
            w,
            h: 1,
        };
        y = 1;
    }
    g.crumbs = Rect { x: 0, y, w, h: 1 };
    y += 1;
    let mut bottom = h - 1;
    g.stat = Rect {
        x: 0,
        y: h - 1,
        w,
        h: 1,
    };
    if class >= SizeClass::Medium {
        g.info = Rect {
            x: 0,
            y: h - 2,
            w,
            h: 1,
        };
        bottom = h - 2;
    }
    g.main = Rect {
        x: 0,
        y,
        w,
        h: bottom - y,
    };
    if class == SizeClass::Large {
        let sw = 40.min(w / 5);
        g.side = Rect {
            x: 0,
            y,
            w: sw,
            h: g.main.h,
        };
        (g.main.x, g.main.w) = (sw, w - sw);
    }
    (class, g)
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

impl Model {
    pub fn render(&mut self) -> String {
        let t = self.theme.clone();
        let mut cv = Canvas::new(self.w, self.h, t.base());
        self.hits.clear();
        let (class, g) = compute_geometry(self.w, self.h);
        if class == SizeClass::TooSmall {
            self.render_too_small(&mut cv);
            return cv.render(self.depth());
        }
        let arc = self.tree.clone();
        let tr = arc.read().unwrap();

        self.ensure_selection(&tr);
        if g.tabs.w > 0 {
            self.paint_tabs(&tr, &mut cv, g.tabs);
        }
        self.paint_crumbs(&tr, &mut cv, g.crumbs);
        if !g.side.empty() {
            self.paint_side(&tr, &mut cv, g.side);
        }
        self.paint_view(&tr, &mut cv, g.main);
        if !g.info.empty() {
            self.paint_info_line(&tr, &mut cv, g.info);
        }
        self.paint_status(&tr, &mut cv, g.stat, class);
        if class == SizeClass::Small && self.search.editing {
            self.paint_search(&mut cv, g.stat);
        }
        if !self.modals.is_empty() {
            self.paint_modal(&tr, &mut cv);
        }
        cv.render(self.depth())
    }

    pub fn depth(&self) -> Depth {
        match self.profile {
            Profile::Ansi256 => Depth::C256,
            Profile::Ansi => Depth::C16,
            Profile::Ascii => Depth::None,
            Profile::TrueColor => Depth::True,
        }
    }

    fn render_too_small(&self, cv: &mut Canvas) {
        let st = self.theme.muted();
        let lines = [
            "Terminal too small".to_string(),
            format!("{}×{} — need {}×{}", self.w, self.h, MIN_W, MIN_H),
            "q quits".to_string(),
        ];
        let y0 = 0.max((self.h - lines.len() as i32) / 2);
        for (i, l) in lines.iter().enumerate() {
            cv.text_centered(0, y0 + i as i32, self.w, l, st);
        }
    }

    pub fn mono_rev(&self) -> Attr {
        if self.theme.mono {
            REVERSE
        } else {
            0
        }
    }

    /// Draws the view tabs and mode chips.
    fn paint_tabs(&mut self, tr: &Tree, cv: &mut Canvas, r: Rect) {
        let t = self.theme.clone();
        cv.fill(r, " ", t.header());
        let badge = if t.ascii {
            format!(" {} ", brand::NAME)
        } else {
            format!(" ◧ {} ", brand::NAME)
        };
        let mut x = cv.text(
            r.x,
            r.y,
            &badge,
            r.w,
            Style::new(t.text_on(t.accent), t.accent).with(BOLD | self.mono_rev()),
        );
        x += 1;
        let names: Vec<&'static str> = self.views.iter().map(|v| v.name()).collect();
        for (i, name) in names.iter().enumerate() {
            let label = format!(" {name} ");
            let st = if i == self.view {
                Style::new(t.fg, t.bg).with(BOLD | UNDERLINE)
            } else {
                Style::new(t.muted, t.header_bg)
            };
            if x + tw(&label) >= r.w - 30 {
                break;
            }
            let w = cv.text(x, r.y, &label, r.w - x, st);
            self.hits.push(Hit {
                r: Rect { x, y: r.y, w, h: 1 },
                act: HitAct::SetView(i),
            });
            x += w;
        }
        // Right-aligned chips.
        let mut chips: Vec<(String, Color)> = Vec::new();
        if self.opts.read_only {
            chips.push(("READ ONLY".into(), t.err));
        }
        if self.snapshot {
            chips.push(("SNAPSHOT".into(), t.accent));
        }
        if self.opts.diff.is_some() {
            chips.push(("COMPARE".into(), t.diff_new));
        }
        if self.search.query.is_some() {
            chips.push(("FILTER".into(), t.warn));
        }
        if !tr.stats.excludes.is_empty() {
            chips.push((format!("EXCL {}", tr.stats.excludes.len()), t.faint));
        }
        chips.push((self.size_mode.name().to_uppercase(), t.faint));
        let mut rx = r.x + r.w;
        for (s, bg) in chips.iter().rev() {
            let s = format!(" {s} ");
            let w = tw(&s);
            if rx - w - 1 < x {
                break;
            }
            rx -= w;
            cv.text(
                rx,
                r.y,
                &s,
                w,
                Style::new(t.text_on(*bg), *bg).with(BOLD | self.mono_rev()),
            );
            rx -= 1;
        }
    }

    /// Draws "/ › home › andy" breadcrumbs, truncated from the left, with the
    /// zoom root's size on the right.
    fn paint_crumbs(&mut self, tr: &Tree, cv: &mut Canvas, r: Rect) {
        let t = self.theme.clone();
        cv.fill(r, " ", t.header());
        let z = tr.node(self.zoom);
        let size = self.size_of(tr, self.zoom);
        let mut right = format!(" {}", textutil::size(size));
        let root = self.size_of(tr, 0);
        if self.zoom != 0 && root > 0 {
            right += &format!("  {} of scan", textutil::percent(size, root));
        }
        if let Some(res) = &self.search.result {
            right += &format!("  ({} matches)", textutil::count(res.count));
        }
        if z.is_dir() {
            right += &format!("  {} files", textutil::count(z.files as i64));
        }
        right += " ";
        let mut rw = tw(&right);
        if rw > r.w / 2 {
            right = format!(" {} ", textutil::size_compact(size));
            rw = tw(&right);
        }
        cv.text_right(
            r.x,
            r.y,
            r.w,
            &right,
            Style::new(t.header_fg, t.header_bg).with(BOLD),
        );

        let sep = if t.ascii { " > " } else { " › " };
        let segs: Vec<(u32, String)> = tr
            .ancestors(self.zoom)
            .into_iter()
            .map(|id| (id, san(&tr.node(id).name)))
            .collect();
        let avail = r.w - rw - 2;
        // Drop leading segments until the rest fits; keep at least the last.
        let total = |from: usize| {
            let mut w = 0;
            for i in from..segs.len() {
                w += tw(&segs[i].1);
                if i > from {
                    w += tw(sep);
                }
            }
            if from > 0 {
                w += tw(&format!("…{sep}"));
            }
            w
        };
        let mut from = 0;
        while from + 1 < segs.len() && total(from) > avail {
            from += 1;
        }
        let mut x = r.x + 1;
        if from > 0 {
            x += cv.text(
                x,
                r.y,
                &format!("…{sep}"),
                avail,
                Style::new(t.muted, t.header_bg),
            );
        }
        for i in from..segs.len() {
            if i > from {
                x += cv.text(
                    x,
                    r.y,
                    sep,
                    r.x + 1 + avail - x,
                    Style::new(t.faint, t.header_bg),
                );
            }
            let last = i == segs.len() - 1;
            let st = if last {
                Style::new(t.fg, t.header_bg).with(BOLD)
            } else {
                Style::new(t.muted, t.header_bg)
            };
            let s = if last {
                trunc_left(&segs[i].1, 1.max(r.x + 1 + avail - x))
            } else {
                segs[i].1.clone()
            };
            let w = cv.text(x, r.y, &s, r.x + 1 + avail - x, st);
            self.hits.push(Hit {
                r: Rect { x, y: r.y, w, h: 1 },
                act: HitAct::ZoomTo(segs[i].0),
            });
            x += w;
        }
    }

    /// Describes the selection (or shows the search editor).
    fn paint_info_line(&mut self, tr: &Tree, cv: &mut Canvas, r: Rect) {
        let t = self.theme.clone();
        cv.fill(r, " ", t.panel());
        if self.search.editing {
            self.paint_search(cv, r);
            return;
        }
        if self.search.query.is_some() {
            self.paint_filter_summary(tr, cv, r);
            return;
        }
        let d = self.describe_selection(tr);
        cv.text(r.x + 1, r.y, &d, r.w - 2, t.panel());
    }

    pub fn describe_selection(&mut self, tr: &Tree) -> String {
        let id = self.selected_node(tr);
        if is_group(self.sel) {
            if let Some(g) = self.group_info(self.sel) {
                return format!(
                    "▸ {} smaller items  ·  {}  ·  {} of {}  ·  Enter lists them",
                    textutil::count(g.ids().len() as i64),
                    textutil::size(g.size),
                    textutil::percent(g.size, self.size_of(tr, self.zoom)),
                    san(&tr.node(self.zoom).name)
                );
            }
        }
        if id == NO_NODE {
            return "Nothing selected".into();
        }
        let n = tr.node(id);
        let mut parts = vec![
            format!("▸ {}", san(&n.name)),
            textutil::size(self.size_of(tr, id)),
        ];
        if n.parent != NO_NODE {
            parts.push(
                textutil::percent(self.size_of(tr, id), self.size_of(tr, n.parent)) + " of parent",
            );
        }
        if n.is_dir() {
            parts.push(textutil::count(n.files as i64) + " files");
            parts.push(textutil::count(n.dirs as i64) + " dirs");
            if !n.has(FLAG_SCANNED) && self.scanning {
                parts.push("scanning…".into());
            }
        } else {
            parts.push(kind_label(n));
        }
        if n.mtime > 0 {
            parts.push("modified ".to_string() + &fmt_date(n.mtime));
        }
        if n.errors > 0 {
            parts.push(format!("{} errors", n.errors));
        }
        if let Some(d) = &self.opts.diff {
            parts.push(textutil::signed_size(d.delta(tr, id, self.size_mode)) + " vs old snapshot");
        }
        parts.join("  ·  ")
    }

    /// Draws the live status bar.
    fn paint_status(&mut self, tr: &Tree, cv: &mut Canvas, r: Rect, class: SizeClass) {
        let t = self.theme.clone();
        cv.fill(r, " ", t.status());
        let help = if class == SizeClass::Small {
            " ? "
        } else {
            " ? help "
        };
        let hw = cv.text(
            r.x + r.w - tw(help),
            r.y,
            help,
            r.w,
            Style::new(t.muted, t.status_bg),
        );
        let avail = r.w - hw - 1;
        if !self.toast.is_empty() {
            cv.text(r.x + 1, r.y, &self.toast, avail - 1, self.toast_style);
            return;
        }
        let st = &tr.stats;
        let root = tr.node(0);
        let files = st.files + st.symlinks + st.others;
        let mut parts: Vec<(String, Color)> = Vec::new();
        if self.scanning {
            let ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let frame = if t.ascii {
                "*"
            } else {
                SPINNER[(ms / 100) as usize % SPINNER.len()]
            };
            parts.push((format!("{frame} scanning"), t.accent));
        } else if st.cancelled {
            parts.push(("✗ cancelled".into(), t.warn));
        } else if self.snapshot {
            parts.push(("◉ snapshot".into(), t.accent));
        } else {
            parts.push(("✓ complete".into(), t.ok));
        }
        parts.push((textutil::count(files) + " files", t.status_fg));
        parts.push((textutil::size(root.total(self.size_mode)), t.status_fg));
        if self.scanning {
            let el = st.elapsed();
            let p = self
                .scanner
                .as_ref()
                .map(|s| s.progress())
                .unwrap_or_default();
            parts.push((
                textutil::count((p.entries as f64 / el.as_secs_f64().max(0.001)) as i64) + "/s",
                t.status_fg,
            ));
            if class != SizeClass::Small {
                parts.push((format!("{} workers", p.workers), t.muted));
                parts.push((format!("queue {}", textutil::count(p.pending)), t.muted));
            }
            parts.push((textutil::duration(el), t.muted));
        } else if !self.snapshot && class != SizeClass::Small {
            parts.push((textutil::duration(st.elapsed()), t.muted));
        }
        let e = st.total_errors();
        if e > 0 {
            parts.push((format!("{} errors", textutil::count(e)), t.err));
        }
        if st.excluded > 0 {
            parts.push((format!("{} excluded", textutil::count(st.excluded)), t.warn));
        }
        if st.incomplete() && !self.scanning && class != SizeClass::Small {
            parts.push(("totals incomplete".into(), t.warn));
        }
        let mut x = r.x + 1;
        for (i, (s, fg)) in parts.iter().enumerate() {
            let s = if i > 0 { format!("  {s}") } else { s.clone() };
            if x + tw(&s) > r.x + avail {
                break;
            }
            let mut st = Style::new(*fg, t.status_bg);
            if i == 0 {
                st.attr = BOLD;
            }
            x += cv.text(x, r.y, &s, avail - x, st);
        }
    }
}

pub(crate) fn kind_label(n: &Node) -> String {
    match n.kind {
        Kind::File => n.cat.name().to_string(),
        Kind::Symlink => "symlink".into(),
        k => k.name().to_string(),
    }
}
