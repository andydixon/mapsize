//! The search/filter editor and background filter evaluation.

use super::canvas::{Canvas, Style, BOLD};
use super::model::{Cmd, Key, Model, Msg};
use super::{tw, trunc};
use crate::cancel::Cancel;
use crate::filter::{self, FilterResult, Query};
use crate::inventory::Tree;
use crate::textutil;
use crate::treemap::Rect;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// A minimal single-line text editor.
#[derive(Default)]
pub(crate) struct LineEditor {
    pub buf: Vec<char>,
    pub cur: usize,
}

impl LineEditor {
    pub fn text(&self) -> String {
        self.buf.iter().collect()
    }

    pub fn set(&mut self, s: &str) {
        self.buf = s.chars().collect();
        self.cur = self.buf.len();
    }

    /// Applies an editing key; it reports whether the text changed and
    /// whether the key was handled.
    pub fn key(&mut self, k: &Key) -> (bool, bool) {
        match k.name.as_str() {
            "left" | "ctrl+b" => self.cur = self.cur.saturating_sub(1),
            "right" | "ctrl+f" => self.cur = self.buf.len().min(self.cur + 1),
            "home" | "ctrl+a" => self.cur = 0,
            "end" | "ctrl+e" => self.cur = self.buf.len(),
            "backspace" | "ctrl+h" => {
                if self.cur > 0 {
                    self.buf.remove(self.cur - 1);
                    self.cur -= 1;
                    return (true, true);
                }
            }
            "delete" | "ctrl+d" => {
                if self.cur < self.buf.len() {
                    self.buf.remove(self.cur);
                    return (true, true);
                }
            }
            "ctrl+u" => {
                self.buf.drain(..self.cur);
                self.cur = 0;
                return (true, true);
            }
            "ctrl+k" => {
                self.buf.truncate(self.cur);
                return (true, true);
            }
            "ctrl+w" | "alt+backspace" => {
                let mut i = self.cur;
                while i > 0 && self.buf[i - 1] == ' ' {
                    i -= 1;
                }
                while i > 0 && self.buf[i - 1] != ' ' {
                    i -= 1;
                }
                self.buf.drain(i..self.cur);
                self.cur = i;
                return (true, true);
            }
            _ => {
                if k.text.is_empty() || k.ctrl_alt {
                    return (false, false);
                }
                let rs: Vec<char> = k.text.chars().filter(|&r| r >= ' ' && r != '\x7f').collect();
                if rs.is_empty() || self.buf.len() + rs.len() > 1024 {
                    return (false, true);
                }
                let n = rs.len();
                self.buf.splice(self.cur..self.cur, rs);
                self.cur += n;
                return (true, true);
            }
        }
        (false, true)
    }
}

#[derive(Default)]
pub(crate) struct SearchState {
    pub editing: bool,
    pub ed: LineEditor,
    pub query: Option<Arc<Query>>,
    pub result: Option<FilterResult>,
    pub err: String,
    pub gen: u64,
    pub busy: bool,
    pub applied_at: Option<Instant>,
    pub cancel: Option<Cancel>,
}

impl SearchState {
    pub fn begin(&mut self) {
        self.editing = true;
        if let Some(q) = &self.query {
            let s = q.to_string();
            self.ed.set(&s);
        }
    }
}

impl Model {
    pub fn search_key(&mut self, k: &Key) -> Cmd {
        match k.name.as_str() {
            "esc" => {
                self.search.editing = false;
                self.clear_filter();
                return Cmd::None;
            }
            "enter" => {
                self.search.editing = false;
                if !self.search.err.is_empty() {
                    let e = format!("Invalid query: {}", self.search.err);
                    return self.warn(e);
                }
                return Cmd::None;
            }
            _ => {}
        }
        let (changed, _) = self.search.ed.key(k);
        if changed {
            return self.query_changed();
        }
        Cmd::None
    }

    /// Parses the editor contents and schedules a debounced apply.
    pub fn query_changed(&mut self) -> Cmd {
        let s = &mut self.search;
        s.gen += 1;
        let text = s.ed.text();
        if text.is_empty() {
            s.err.clear();
            self.drop_filter();
            return Cmd::None;
        }
        match filter::parse(&text, SystemTime::now()) {
            Err(e) => {
                s.err = e;
                Cmd::None
            }
            Ok(q) => {
                s.err.clear();
                s.query = Some(Arc::new(q));
                Cmd::Tick(Duration::from_millis(120), Msg::FilterDebounce(s.gen))
            }
        }
    }

    /// Replaces the filter with q and applies it immediately.
    pub fn set_filter(&mut self, q: &str) -> Cmd {
        self.search.ed.set(q);
        self.search.editing = false;
        let _ = self.query_changed();
        if !self.search.err.is_empty() {
            let e = format!("Invalid query: {}", self.search.err);
            return self.warn(e);
        }
        self.apply_filter()
    }

    /// Evaluates the current query in the background. The tree is
    /// read-locked by the worker; stale results are discarded by generation.
    pub fn apply_filter(&mut self) -> Cmd {
        let Some(q) = self.search.query.clone() else { return Cmd::None };
        if let Some(c) = self.search.cancel.take() {
            c.cancel();
        }
        let c = Cancel::new();
        self.search.cancel = Some(c.clone());
        self.search.busy = true;
        self.search.applied_at = Some(Instant::now());
        let (t, mode, gen) = (self.tree.clone(), self.size_mode, self.search.gen);
        Cmd::run(move || {
            let res = {
                let g = t.read().unwrap();
                filter::apply(&c, &g, q, mode)
            };
            Msg::Filter { gen, res }
        })
    }

    pub fn filter_result(&mut self, gen: u64, res: Option<FilterResult>) -> Cmd {
        let s = &mut self.search;
        if gen != s.gen || s.query.is_none() {
            return Cmd::None;
        }
        s.busy = false;
        let Some(res) = res else { return Cmd::None };
        s.result = Some(res);
        self.invalidate();
        Cmd::None
    }

    /// Removes the active filter but keeps the editor open.
    pub fn drop_filter(&mut self) {
        let s = &mut self.search;
        if let Some(c) = &s.cancel {
            c.cancel();
        }
        (s.query, s.result, s.busy) = (None, None, false);
        self.invalidate();
    }

    pub fn clear_filter(&mut self) {
        self.drop_filter();
        self.search.ed.set("");
        self.search.err.clear();
        self.search.gen += 1;
    }

    pub fn paint_search(&self, cv: &mut Canvas, r: Rect) {
        let t = &self.theme;
        let s = &self.search;
        let st = Style::new(t.fg, t.panel_bg);
        cv.fill(r, " ", st);
        let mut x = r.x + 1;
        x += cv.text(x, r.y, "/ ", 2, Style::new(t.accent, t.panel_bg).with(BOLD));
        let mut status_st = Style::new(t.muted, t.panel_bg);
        let status = if !s.err.is_empty() {
            status_st.fg = t.err;
            format!("✗ {}", s.err)
        } else if s.busy {
            "filtering…".into()
        } else if let Some(res) = &s.result {
            format!("{} matches · {}", textutil::count(res.count), textutil::size(res.total))
        } else if s.ed.buf.is_empty() {
            "name, *.iso, size > 1GB, ext IN (iso,vmdk), age > 365d … Enter keep · Esc clear".into()
        } else {
            String::new()
        };
        let sw = tw(&status).min(r.w / 2);
        let avail = r.w - (x - r.x) - sw - 3;
        // Scroll the text so the cursor stays visible.
        let text = &s.ed.buf;
        let start = if s.ed.cur as i32 > avail - 1 { (s.ed.cur as i32 - avail + 1) as usize } else { 0 };
        let end = text.len().min(start + avail.max(0) as usize).max(start);
        let vis: String = text[start..end].iter().collect();
        cv.text(x, r.y, &textutil::sanitize(&vis), avail, st);
        let before: String = text[start..s.ed.cur].iter().collect();
        let cx = x + tw(&textutil::sanitize(&before));
        let under = if s.ed.cur < text.len() { textutil::sanitize(&text[s.ed.cur].to_string()) } else { " ".into() };
        cv.text(cx, r.y, &under, 2, Style::new(t.panel_bg, t.fg).with(self.mono_rev()));
        cv.text_right(r.x, r.y, r.w - 1, &trunc(&status, sw), status_st);
    }

    pub fn paint_filter_summary(&self, tr: &Tree, cv: &mut Canvas, r: Rect) {
        let t = &self.theme;
        let s = &self.search;
        let mut x = r.x + 1;
        x += cv.text(x, r.y, " FILTER ", 8, Style::new(t.text_on(t.warn), t.warn).with(BOLD | self.mono_rev()));
        let q = s.query.as_ref().map(|q| q.to_string()).unwrap_or_default();
        x += cv.text(x + 1, r.y, &textutil::sanitize(&q), r.w / 2, Style::new(t.fg, t.panel_bg).with(BOLD)) + 1;
        let mut info = "filtering…".to_string();
        if let (Some(res), false) = (&s.result, s.busy) {
            info = format!(
                "{} matches · {} of {} · / edit · Esc clear",
                textutil::count(res.count),
                textutil::size(res.total),
                textutil::size(tr.node(0).total(self.size_mode))
            );
        }
        cv.text(x + 2, r.y, &info, r.x + r.w - x - 3, Style::new(t.muted, t.panel_bg));
    }
}
