//! A scrollable list with a cursor, shared by the list-style views.

use super::canvas::{rgb, Canvas, Color, Style, BOLD};
use super::model::{Cmd, Hit, HitAct, Model};
use super::{pad_left, trunc, trunc_left};
use crate::inventory::Tree;
use crate::textutil;
use crate::treemap::Rect;

/// Describes one table column.
pub(crate) struct Column {
    pub title: &'static str,
    pub width: i32, // 0 = flexible (takes the remaining space)
    pub right: bool,
    pub clip_left: bool, // truncate from the left (paths: the tail matters)
}

pub(crate) const fn col(title: &'static str, width: i32, right: bool, clip_left: bool) -> Column {
    Column {
        title,
        width,
        right,
        clip_left,
    }
}

/// One rendered row: cell strings plus optional per-cell styles.
#[derive(Default)]
pub(crate) struct TableRow {
    pub cells: Vec<String>,
    pub styles: Vec<Option<Style>>, // None = default
    pub bar: f64,                   // 0..1 for an optional bar column (-1 = none)
    pub bar_col: Color,
}

#[derive(Default)]
pub(crate) struct Table {
    pub cursor: i32,
    pub offset: i32,
    pub height: i32, // rows visible in the last paint
}

impl Table {
    /// Moves the cursor three rows for a mouse wheel step.
    pub fn scroll(&mut self, up: bool, n: usize) {
        self.mv(if up { -3 } else { 3 }, n);
    }

    fn mv(&mut self, d: i32, n: usize) {
        if n == 0 {
            self.cursor = 0;
            return;
        }
        self.cursor = (self.cursor + d).max(0).min(n as i32 - 1);
    }

    /// Handles navigation keys common to all tables.
    pub fn key(&mut self, k: &str, n: usize) -> bool {
        let page = (self.height - 1).max(1);
        match k {
            "up" | "k" => self.mv(-1, n),
            "down" | "j" => self.mv(1, n),
            "pgup" | "ctrl+b" => self.mv(-page, n),
            "pgdown" | "ctrl+f" => self.mv(page, n),
            "end" | "G" => self.mv(n as i32, n),
            "g" => self.cursor = 0,
            _ => return false,
        }
        true
    }

    /// The cursor as an index (it is never negative).
    pub fn at(&self) -> usize {
        self.cursor.max(0) as usize
    }

    pub fn paint(
        &mut self,
        m: &mut Model,
        cv: &mut Canvas,
        r: Rect,
        cols: &[Column],
        rows: &[TableRow],
        title: &str,
    ) {
        let t = m.theme.clone();
        cv.fill(r, " ", t.base());
        if r.h < 3 {
            return;
        }
        let mut y = r.y;
        if !title.is_empty() {
            cv.text(
                r.x + 1,
                y,
                title,
                r.w - 2,
                Style::new(t.accent, t.bg).with(BOLD),
            );
            y += 1;
        }
        // Resolve flexible width.
        let fixed: i32 = cols.iter().map(|c| c.width + 1).sum();
        let flex = (r.w - 2 - fixed).max(8);
        let width = |c: &Column| if c.width == 0 { flex } else { c.width };
        let mut x = r.x + 1;
        let hdr = Style::new(t.muted, t.bg).with(BOLD);
        for c in cols {
            let w = width(c);
            let s = if c.right {
                pad_left(c.title, w)
            } else {
                c.title.to_string()
            };
            cv.text(x, y, &s, w, hdr);
            x += w + 1;
        }
        y += 1;
        cv.hline(r.x + 1, y, r.w - 2, "─", Style::new(t.faint, t.bg));
        y += 1;
        self.height = r.y + r.h - y;
        let n = rows.len() as i32;
        if self.cursor >= n {
            self.cursor = (n - 1).max(0);
        }
        if self.cursor < self.offset {
            self.offset = self.cursor;
        }
        if self.cursor >= self.offset + self.height {
            self.offset = self.cursor - self.height + 1;
        }
        self.offset = 0.max(self.offset.min(n - self.height));
        let mut i = self.offset;
        while i < n && y < r.y + r.h {
            let row = &rows[i as usize];
            let sel = i == self.cursor;
            let mut base = t.base();
            if sel {
                base = Style::new(t.text_on(t.sel), t.sel).with(BOLD | m.mono_rev());
                cv.fill(
                    Rect {
                        x: r.x,
                        y,
                        w: r.w,
                        h: 1,
                    },
                    " ",
                    base,
                );
            }
            let mut x = r.x + 1;
            for (ci, c) in cols.iter().enumerate() {
                let w = width(c);
                let s = row.cells.get(ci).map_or("", |s| s.as_str());
                let st = match row.styles.get(ci) {
                    Some(Some(st)) if !sel => *st,
                    _ => base,
                };
                if c.title.is_empty() && row.bar >= 0.0 {
                    m.paint_bar(cv, x, y, w, row.bar, row.bar_col, sel);
                } else if c.right {
                    cv.text(x, y, &pad_left(s, w), w, st);
                } else if c.clip_left {
                    cv.text(x, y, &trunc_left(s, w), w, st);
                } else {
                    cv.text(x, y, &trunc(s, w), w, st);
                }
                x += w + 1;
            }
            m.hits.push(Hit {
                r: Rect {
                    x: r.x,
                    y,
                    w: r.w,
                    h: 1,
                },
                act: HitAct::Row(i as usize),
            });
            y += 1;
            i += 1;
        }
        if rows.is_empty() {
            cv.text_centered(r.x, r.y + r.h / 2, r.w, "Nothing to show", t.muted());
        }
        if n > self.height && self.height > 0 {
            let pos = format!(
                "{}/{}",
                textutil::count(self.cursor as i64 + 1),
                textutil::count(n as i64)
            );
            cv.text_right(r.x, r.y, r.w - 1, &pos, t.muted());
        }
    }
}

impl Model {
    /// Draws a proportional bar with eighth-block precision.
    #[allow(clippy::too_many_arguments)]
    pub fn paint_bar(
        &self,
        cv: &mut Canvas,
        x: i32,
        y: i32,
        w: i32,
        frac: f64,
        col: Color,
        sel: bool,
    ) {
        let t = &self.theme;
        let col = if col.is_default() { t.accent } else { col };
        let bg = if sel {
            t.sel.mix(rgb(0, 0, 0), 0.25)
        } else {
            t.bg.mix(t.fg, 0.08)
        };
        let mut eighths = (frac * (w * 8) as f64 + 0.5) as i64;
        const PARTS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
        for i in 0..w {
            let mut ch = if eighths >= 8 {
                "█"
            } else if eighths > 0 {
                PARTS[eighths as usize]
            } else {
                " "
            };
            if t.ascii {
                ch = if eighths >= 4 { "#" } else { " " };
            }
            eighths -= 8;
            let st = if t.mono {
                Style::default()
            } else {
                Style::new(col, bg)
            };
            cv.text(x + i, y, ch, 1, st);
        }
    }

    /// Lets mouse actions reuse key handling (lock already held).
    pub fn handle_key_name(&mut self, tr: &Tree, k: &str) -> Cmd {
        let (handled, cmd) = self.view_key(tr, k);
        if handled {
            return cmd;
        }
        match k {
            "enter" => self.inspect_selection(tr),
            "space" => self.zoom_into_selection(tr),
            _ => Cmd::None,
        }
    }
}
