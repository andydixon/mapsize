//! The generic modal frame: a centred, clamped, scrollable overlay.

use super::canvas::{rgb, Canvas, Style, BOLD, BOX_ASCII, BOX_ROUND, FAINT};
use super::model::{Cmd, Hit, HitAct, Key, Model};
use super::pad_right;
use super::search::LineEditor;
use crate::inventory::Tree;
use crate::textutil;
use crate::treemap::Rect;

/// A run of styled text; a line is a list of spans.
pub(crate) struct Span {
    pub s: String,
    pub st: Option<Style>, // None = modal default
}

pub(crate) type Line = Vec<Span>;

pub(crate) fn txt(s: impl Into<String>) -> Line {
    vec![Span { s: s.into(), st: None }]
}

pub(crate) fn styled(s: impl Into<String>, st: Style) -> Line {
    vec![Span { s: s.into(), st: Some(st) }]
}

pub(crate) fn kv(k: &str, v: impl Into<String>, kst: Style, vst: Style) -> Line {
    vec![Span { s: pad_right(k, 18), st: Some(kst) }, Span { s: v.into(), st: Some(vst) }]
}

/// An overlay. Modals receive all keys while open; Esc always closes them
/// (handled by the frame before the modal sees it, unless the modal is
/// editing text).
pub(crate) trait Modal {
    fn title(&self) -> String;
    /// Rebuilt every frame, so resizes reflow.
    fn content(&self, m: &mut Model, tr: &Tree, width: i32) -> Vec<Line>;
    fn footer(&self, m: &Model) -> String;
    fn key(&mut self, m: &mut Model, tr: &Tree, k: &str) -> (bool, Cmd);
    /// Preferred inner width.
    fn width(&self) -> i32;
    /// Modals that consume printable keys return their editor.
    fn editor(&mut self) -> Option<&mut LineEditor> {
        None
    }
}

/// Wraps a modal with scroll state.
pub(crate) struct ModalFrame {
    pub md: Box<dyn Modal>,
    pub scroll: i32,
    pub view_h: i32, // content rows visible in the last paint
    pub content_n: i32,
}

impl ModalFrame {
    pub fn clamp_scroll(&mut self) {
        self.scroll = 0.max(self.scroll.min(self.content_n - self.view_h));
    }
}

impl Model {
    pub fn open_modal(&mut self, md: Box<dyn Modal>) {
        self.modals.push(ModalFrame { md, scroll: 0, view_h: 0, content_n: 0 });
        self.dirty = true;
    }

    pub fn close_modal(&mut self) {
        self.modals.pop();
        self.dirty = true;
    }

    pub fn modal_key(&mut self, tr: &Tree, key: &Key) -> Cmd {
        let k = key.name.as_str();
        let idx = self.modals.len() - 1;
        let editing = self.modals[idx].md.editor().is_some();
        if editing && k != "esc" && k != "enter" {
            if let Some(ed) = self.modals[idx].md.editor() {
                ed.key(key);
            }
            return Cmd::None;
        }
        if !editing {
            let f = &mut self.modals[idx];
            let page = (f.view_h - 1).max(1);
            let scrolled = match k {
                "esc" | "q" => {
                    self.close_modal();
                    return Cmd::None;
                }
                "up" | "k" => Some(f.scroll - 1),
                "down" | "j" => Some(f.scroll + 1),
                "pgup" => Some(f.scroll - page),
                "pgdown" | "space" => Some(f.scroll + page),
                "home" => Some(0),
                "end" => Some(f.content_n),
                _ => None,
            };
            if let Some(s) = scrolled {
                f.scroll = s;
                f.clamp_scroll();
                return Cmd::None;
            }
        }
        // Modal-specific keys. The frame is taken out while the modal runs;
        // if it opens another modal, that one stacks above it.
        let mut f = self.modals.remove(idx);
        let (close, cmd) = f.md.key(self, tr, k);
        if close {
            self.dirty = true;
        } else {
            self.modals.insert(idx.min(self.modals.len()), f);
        }
        cmd
    }

    /// Dims the base screen and draws the top modal centred, clamped to the
    /// terminal with a one-cell margin. Content that does not fit scrolls.
    pub fn paint_modal(&mut self, tr: &Tree, cv: &mut Canvas) {
        let t = self.theme.clone();
        // Dim everything underneath.
        cv.restyle(Rect { x: 0, y: 0, w: cv.w, h: cv.h }, |mut s| {
            if t.mono {
                s.attr |= FAINT;
                return s;
            }
            s.fg = s.fg.mix(t.bg, 0.6);
            s.bg = s.bg.mix(rgb(0, 0, 0), 0.45);
            s.attr &= !BOLD;
            s
        });
        let mut f = self.modals.pop().unwrap();
        let max_w = cv.w - 2;
        let w = (f.md.width() + 4).min(max_w).max(30.min(max_w));
        let inner = w - 4;
        let lines = f.md.content(self, tr, inner);
        let foot = f.md.footer(self);
        let mut chrome = 4; // border top/bottom, title rule
        if !foot.is_empty() {
            chrome += 2;
        }
        let h = (lines.len() as i32 + chrome).min(cv.h - 2).max((chrome + 1).min(cv.h));
        let (x0, y0) = ((cv.w - w) / 2, (cv.h - h) / 2);
        let r = Rect { x: x0, y: y0, w, h };
        let bg = if t.panel_bg.is_default() { t.bg } else { t.panel_bg };
        let base = Style::new(t.fg, bg);
        // Drop shadow.
        if !t.mono {
            let black = |mut s: Style| {
                s.bg = rgb(0, 0, 0);
                s
            };
            cv.restyle(Rect { x: x0 + 1, y: y0 + h, w, h: 1 }, black);
            cv.restyle(Rect { x: x0 + w, y: y0 + 1, w: 1, h }, black);
        }
        cv.fill(r, " ", base);
        let border = Style::new(t.accent, bg).with(BOLD);
        cv.draw_box(r, if t.ascii { BOX_ASCII } else { BOX_ROUND }, border);
        cv.text(x0 + 2, y0 + 1, &f.md.title().to_uppercase(), inner, Style::new(t.accent, bg).with(BOLD));
        let rule = if t.ascii { "-" } else { "─" };
        cv.hline(x0 + 1, y0 + 2, w - 2, rule, Style::new(t.faint, bg));
        f.view_h = h - chrome;
        f.content_n = lines.len() as i32;
        f.clamp_scroll();
        let mut i = 0;
        while i < f.view_h && ((f.scroll + i) as usize) < lines.len() {
            let mut x = x0 + 2;
            for sp in &lines[(f.scroll + i) as usize] {
                let st = match sp.st {
                    Some(mut st) => {
                        if st.bg.is_default() {
                            st.bg = bg;
                        }
                        st
                    }
                    None => base,
                };
                x += cv.text(x, y0 + 3 + i, &sp.s, x0 + 2 + inner - x, st);
            }
            i += 1;
        }
        let n = lines.len() as i32;
        if n > f.view_h && f.view_h > 0 {
            let mut ind = format!(
                "{}–{}/{}",
                textutil::count(f.scroll as i64 + 1),
                textutil::count((f.scroll + f.view_h).min(n) as i64),
                textutil::count(n as i64)
            );
            if f.scroll > 0 {
                ind = format!("▲ {ind}");
            }
            if f.scroll + f.view_h < n {
                ind += " ▼";
            }
            cv.text_right(x0 + 1, y0 + 1, w - 3, &ind, Style::new(t.muted, bg));
        }
        if !foot.is_empty() {
            cv.hline(x0 + 1, y0 + h - 3, w - 2, rule, Style::new(t.faint, bg));
            cv.text(x0 + 2, y0 + h - 2, &foot, inner, Style::new(t.muted, bg));
        }
        self.modals.push(f);
        self.hits.clear();
        self.hits.push(Hit { r, act: HitAct::None });
    }
}

/// Breaks s into chunks of at most w cells (used for long paths).
pub(crate) fn wrap(s: &str, w: i32) -> Vec<String> {
    if w <= 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cw = 0;
    textutil::clusters(s, |c, cwid| {
        if cw + cwid as i32 > w {
            out.push(std::mem::take(&mut cur));
            cw = 0;
        }
        cur.push_str(c);
        cw += cwid as i32;
        true
    });
    if !cur.is_empty() || out.is_empty() {
        out.push(cur);
    }
    out
}
