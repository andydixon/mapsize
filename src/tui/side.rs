//! The large-layout side panel: directory summary, category breakdown and
//! the selected item.

use super::canvas::{Canvas, Style, BOLD};
use super::modal::wrap;
use super::model::{is_group, Model};
use super::render::kind_label;
use super::san;
use crate::inventory::{Category, Tree, NO_NODE};
use crate::textutil;
use crate::treemap::Rect;

impl Model {
    pub fn paint_side(&mut self, tr: &Tree, cv: &mut Canvas, r: Rect) {
        let t = self.theme.clone();
        let ps = t.panel();
        cv.fill(r, " ", ps);
        cv.fill(
            Rect {
                x: r.x + r.w - 1,
                y: r.y,
                w: 1,
                h: r.h,
            },
            "│",
            Style::new(t.faint, t.panel_bg),
        );
        let w = r.w - 3;
        let x = r.x + 1;
        let mut y = r.y + 1;
        let end = r.y + r.h;
        let head = |cv: &mut Canvas, y: &mut i32, s: &str| {
            if *y < end {
                cv.text(x, *y, s, w, Style::new(t.accent, t.panel_bg).with(BOLD));
                *y += 1;
            }
        };
        let kvl = |cv: &mut Canvas, y: &mut i32, k: &str, v: &str, vs: Style| {
            if *y < end {
                cv.text(x, *y, k, w, Style::new(t.muted, t.panel_bg));
                cv.text_right(x, *y, w, v, vs);
                *y += 1;
            }
        };
        let val = Style::new(t.fg, t.panel_bg).with(BOLD);
        let plain = Style::new(t.fg, t.panel_bg);
        let z = tr.node(self.zoom);

        head(cv, &mut y, "DIRECTORY");
        for l in wrap(&san(&z.name), w) {
            if y < end {
                cv.text(x, y, &l, w, Style::new(t.fg, t.panel_bg).with(BOLD));
                y += 1;
            }
        }
        y += 1;
        kvl(
            cv,
            &mut y,
            "Size",
            &textutil::size(self.size_of(tr, self.zoom)),
            val,
        );
        kvl(cv, &mut y, "Logical", &textutil::size(z.tot_size), plain);
        kvl(cv, &mut y, "Allocated", &textutil::size(z.tot_alloc), plain);
        kvl(cv, &mut y, "Files", &textutil::count(z.files as i64), plain);
        kvl(
            cv,
            &mut y,
            "Directories",
            &textutil::count(z.dirs as i64),
            plain,
        );
        if z.errors > 0 {
            kvl(
                cv,
                &mut y,
                "Errors",
                &textutil::count(z.errors as i64),
                Style::new(t.err, t.panel_bg).with(BOLD),
            );
        }
        y += 1;

        // Category breakdown with proportional bars.
        if let Some(cs) = tr.cat_sizes(self.zoom) {
            head(cv, &mut y, "CATEGORIES");
            let mut cats: Vec<(Category, i64)> = cs
                .iter()
                .enumerate()
                .filter(|(_, &v)| v > 0)
                .map(|(i, &v)| (Category::from_u8(i as u8), v))
                .collect();
            let total: i64 = cats.iter().map(|c| c.1).sum();
            cats.sort_by(|a, b| b.1.cmp(&a.1));
            for &(c, v) in &cats {
                if y >= end - 8 {
                    break;
                }
                let col = t.cat[c as usize];
                let sw = if t.mono || t.ascii { "   " } else { "██ " };
                cv.text(x, y, sw, 3, Style::new(col, t.panel_bg));
                cv.text(x + 3, y, c.name(), w - 3, plain);
                cv.text_right(
                    x,
                    y,
                    w,
                    &textutil::percent(v, total),
                    Style::new(t.muted, t.panel_bg),
                );
                y += 1;
                self.paint_bar(cv, x + 3, y, w - 3, v as f64 / cats[0].1 as f64, col, false);
                y += 1;
            }
            y += 1;
        }

        // Selected item.
        if y < end - 4 {
            head(cv, &mut y, "SELECTED");
            let mut desc = Vec::new();
            if is_group(self.sel) {
                if let Some(g) = self.group_info(self.sel) {
                    desc.push(format!(
                        "{} smaller items",
                        textutil::count(g.ids().len() as i64)
                    ));
                    desc.push(textutil::size(g.size));
                }
            } else {
                let id = self.selected_node(tr);
                if id != NO_NODE {
                    let n = tr.node(id);
                    desc.extend(wrap(&san(&n.name), w));
                    desc.push(
                        textutil::size(self.size_of(tr, id))
                            + "  ·  "
                            + &textutil::percent(self.size_of(tr, id), self.size_of(tr, self.zoom)),
                    );
                    if n.is_dir() {
                        desc.push(textutil::count(n.files as i64) + " files");
                    } else {
                        desc.push(kind_label(n));
                    }
                }
            }
            for d in desc {
                if y < end {
                    cv.text(x, y, &d, w, plain);
                    y += 1;
                }
            }
        }
    }
}
