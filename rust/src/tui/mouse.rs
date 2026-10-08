//! Mouse handling: clicks on recorded hit regions, wheel zoom/scroll and
//! hover.

use super::model::{is_group, Button, Cmd, HitAct, Model, NO_SEL};
use crate::inventory::{NodeId, Tree};
use std::time::{Duration, Instant};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

impl Model {
    pub fn mouse_click(&mut self, x: i32, y: i32, button: Button) -> Cmd {
        let arc = self.tree.clone();
        let tr = arc.read().unwrap();
        self.dirty = true;
        if !self.modals.is_empty() {
            // Clicking outside the modal closes it.
            if self.hits.first().is_some_and(|h| !h.r.contains(x, y)) {
                self.close_modal();
            }
            return Cmd::None;
        }
        if button == Button::Right {
            if let Some(act) = self.hits.iter().find(|h| h.r.contains(x, y)).map(|h| h.act) {
                let _ = self.run_hit(&tr, act, false);
                return self.inspect_selection(&tr);
            }
            return Cmd::None;
        }
        if button != Button::Left {
            return Cmd::None;
        }
        for i in (0..self.hits.len()).rev() {
            if self.hits[i].r.contains(x, y) {
                let key = (y as i64) << 32 | i as i64;
                let double = self.last_click.is_some_and(|t| t.elapsed() < DOUBLE_CLICK) && self.last_click_i == key;
                (self.last_click, self.last_click_i) = (Some(Instant::now()), key);
                if double {
                    self.last_click_i = NO_SEL;
                }
                let act = self.hits[i].act;
                return self.run_hit(&tr, act, double);
            }
        }
        Cmd::None
    }

    fn run_hit(&mut self, tr: &Tree, act: HitAct, double: bool) -> Cmd {
        match act {
            HitAct::None => Cmd::None,
            HitAct::SetView(i) => {
                self.set_view(i);
                Cmd::None
            }
            HitAct::ZoomTo(id) => {
                self.zoom_to(tr, id);
                Cmd::None
            }
            HitAct::Block(id) => self.click_block(tr, id, double),
            HitAct::Row(i) => {
                if let Some(tb) = self.views[self.view].table_mut() {
                    tb.cursor = i as i32;
                }
                if double {
                    return self.handle_key_name(tr, "enter");
                }
                Cmd::None
            }
        }
    }

    /// Selects a treemap block; a double click zooms (or inspects).
    fn click_block(&mut self, tr: &Tree, id: i64, double: bool) -> Cmd {
        self.sel = id;
        if !double {
            return Cmd::None;
        }
        if !is_group(id) && !tr.node(id as NodeId).is_dir() {
            return self.inspect_selection(tr);
        }
        self.zoom_into_selection(tr)
    }

    pub fn mouse_wheel(&mut self, x: i32, y: i32, up: bool) -> Cmd {
        let arc = self.tree.clone();
        let tr = arc.read().unwrap();
        self.dirty = true;
        if let Some(f) = self.modals.last_mut() {
            f.scroll += if up { -3 } else { 3 };
            f.clamp_scroll();
            return Cmd::None;
        }
        if up {
            // Zoom towards the block under the pointer.
            if let Some(l) = self.tm.clone() {
                if self.view == 0 {
                    for b in &l.blocks {
                        if b.rect.contains(x, y) {
                            self.sel = b.id;
                        }
                    }
                }
            }
        }
        self.view_wheel(&tr, up)
    }

    pub fn mouse_motion(&mut self, x: i32, y: i32) {
        let mut h = NO_SEL;
        if let Some(l) = &self.tm {
            if self.view == 0 && self.modals.is_empty() {
                if let Some(b) = l.blocks.iter().find(|b| b.rect.contains(x, y)) {
                    h = b.id;
                }
            }
        }
        if h != self.hover {
            self.hover = h;
            self.dirty = true;
        }
    }
}
