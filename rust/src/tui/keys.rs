//! Key dispatch and the actions keys trigger.

use super::model::{is_group, Cmd, Key, Model, Msg};
use super::modals::{new_group_modal, new_help_modal, new_info_modal, new_save_modal};
use super::theme::{load_theme, theme_names};
use super::views::View;
use super::{fmt_stamp_now, san};
use crate::brand;
use crate::inventory::{NodeId, SizeMode, Tree, NO_NODE};
use crate::platform;
use crate::snapshot;
use crate::textutil;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::rc::Rc;

impl Model {
    /// Dispatches a key press. It holds the tree read lock for the whole
    /// handler; helpers it calls must not lock again (a recursive read lock
    /// can deadlock against a waiting writer).
    pub fn handle_key(&mut self, key: &Key) -> Cmd {
        let k = key.name.as_str();
        if k == "ctrl+c" {
            return self.ctrl_c_pressed();
        }
        let arc = self.tree.clone();
        let tr = arc.read().unwrap();
        self.ensure_selection(&tr);
        if !self.modals.is_empty() {
            return self.modal_key(&tr, key);
        }
        if self.search.editing {
            return self.search_key(key);
        }
        let (handled, cmd) = self.view_key(&tr, k);
        if handled {
            return cmd;
        }
        match k {
            "q" => self.quit = true,
            "?" | "f1" => {
                let md = new_help_modal();
                self.open_modal(md);
            }
            "/" => self.search.begin(),
            "esc" => {
                if self.search.query.is_some() {
                    self.clear_filter();
                } else if self.zoom != 0 {
                    self.zoom_out(&tr);
                }
            }
            "tab" => self.set_view((self.view + 1) % self.views.len()),
            "shift+tab" => self.set_view((self.view + self.views.len() - 1) % self.views.len()),
            "1" | "2" | "3" | "4" | "5" | "6" | "7" => self.set_view((k.as_bytes()[0] - b'1') as usize),
            "enter" => return self.inspect_selection(&tr),
            "space" | "right" => return self.zoom_into_selection(&tr),
            "backspace" | "left" => self.zoom_out(&tr),
            "home" => self.zoom_to(&tr, 0),
            "a" => {
                self.size_mode = if self.size_mode == SizeMode::Allocated { SizeMode::Logical } else { SizeMode::Allocated };
                self.invalidate();
                let mut cmds = Vec::new();
                if self.search.query.is_some() {
                    cmds.push(self.apply_filter());
                }
                let msg = format!("Sizing by {} size", self.size_mode.name());
                cmds.push(self.info(msg));
                return Cmd::batch(cmds);
            }
            "r" => return self.rescan(&tr),
            "s" => {
                let md = new_save_modal(&tr);
                self.open_modal(md);
            }
            "e" => {
                self.set_view_by_name("Scan");
                if let View::Info(v) = &mut self.views[self.view] {
                    v.focus_errors();
                }
            }
            "x" => self.set_view_by_name("Types"),
            "g" => self.set_view_by_name("Top"),
            "D" => return self.start_duplicates(),
            "c" => return self.copy_selected(&tr),
            "o" => return self.reveal(&tr),
            "d" | "delete" => return self.request_trash(&tr),
            "T" => {
                let names = theme_names();
                let cur = names.iter().position(|n| *n == self.theme.name).unwrap_or(0);
                let ascii = self.theme.ascii;
                let mut th = load_theme(names[(cur + 1) % names.len()], &self.opts.settings);
                th.ascii = ascii;
                self.theme = Rc::new(th);
                let msg = format!("Theme: {}", self.theme.name);
                return self.info(msg);
            }
            _ => {}
        }
        Cmd::None
    }

    pub fn inspect_selection(&mut self, tr: &Tree) -> Cmd {
        if is_group(self.sel) {
            if let Some(g) = self.group_info(self.sel) {
                self.open_modal(new_group_modal(g));
            }
            return Cmd::None;
        }
        let id = self.selected_node(tr);
        if id != NO_NODE {
            let md = new_info_modal(self, tr, id);
            self.open_modal(md);
        }
        Cmd::None
    }

    pub fn zoom_into_selection(&mut self, tr: &Tree) -> Cmd {
        if is_group(self.sel) {
            // Groups cannot be zoomed: show their members in the list view.
            if let Some(g) = self.group_info(self.sel) {
                if let Some(&first) = g.ids().first() {
                    self.sel = first as i64;
                    self.set_view_by_name("List");
                }
            }
            return Cmd::None;
        }
        let id = self.selected_node(tr);
        if id == NO_NODE {
            return Cmd::None;
        }
        if !tr.node(id).is_dir() {
            return self.info("Not a directory — Enter shows details");
        }
        if self.kids_of(tr, id).ids.is_empty() {
            return self.info("Directory is empty");
        }
        self.zoom_to(tr, id);
        Cmd::None
    }

    pub fn zoom_out(&mut self, tr: &Tree) {
        let p = tr.node(self.zoom).parent;
        if p != NO_NODE {
            self.zoom_to(tr, p);
        }
    }

    /// Zooms to a node's parent directory and selects it.
    pub fn jump_to(&mut self, tr: &Tree, id: NodeId) {
        let p = tr.node(id).parent;
        if p == NO_NODE {
            self.zoom_to(tr, id);
            return;
        }
        self.zoom_to(tr, p);
        self.sel = id as i64;
        self.set_view_by_name("Map");
    }

    fn copy_selected(&mut self, tr: &Tree) -> Cmd {
        let id = self.selected_node(tr);
        if id == NO_NODE {
            return Cmd::None;
        }
        self.copy_path(&tr.path_bytes(id))
    }

    /// Puts p on the clipboard (OSC 52). Paths that would not survive
    /// display unchanged are refused: a newline in a hostile filename runs
    /// commands when pasted into a shell, and bidi controls disguise them.
    pub fn copy_path(&mut self, p: &[u8]) -> Cmd {
        let s = san(p);
        if s.as_bytes() != p {
            return self.warn("Not copied: path contains control or bidi characters");
        }
        let i = self.info("Copied path to clipboard (OSC 52)");
        Cmd::batch(vec![Cmd::Clipboard(s), i])
    }

    pub fn reveal(&mut self, tr: &Tree) -> Cmd {
        if self.snapshot {
            return self.warn("Snapshot — the files may not exist on this machine");
        }
        let id = self.selected_node(tr);
        if id == NO_NODE {
            return Cmd::None;
        }
        if platform::headless() {
            // No file manager: show the folder in the map instead.
            if tr.node(id).is_dir() && !self.kids_of(tr, id).ids.is_empty() {
                self.zoom_to(tr, id);
                self.set_view_by_name("Map");
            } else {
                self.jump_to(tr, id);
            }
            let msg = format!("Showing {}", san(&tr.path_bytes(self.zoom)));
            return self.info(msg);
        }
        if let Err(e) = platform::reveal(&tr.path(id)) {
            return self.warn(format!("Open failed: {}", textutil::sanitize(&e.to_string())));
        }
        self.info("Opened in file manager")
    }

    pub fn save_snapshot(&mut self, path: String) -> Cmd {
        let t = self.tree.clone();
        Cmd::run(move || {
            let err = {
                let g = t.read().unwrap();
                snapshot::save_file(Path::new(&path), &g).err().map(|e| e.to_string())
            };
            Msg::SaveDone { path, err }
        })
    }
}

pub(crate) fn default_snapshot_name(root: &[u8]) -> String {
    let p = Path::new(OsStr::from_bytes(root));
    let base = match p.file_name() {
        Some(b) => String::from_utf8_lossy(b.as_bytes()).into_owned(),
        None => "root".into(),
    };
    let wd = std::env::current_dir().unwrap_or_default();
    wd.join(format!("{base}-{}{}", fmt_stamp_now(), brand::SNAPSHOT_EXT)).to_string_lossy().into_owned()
}
