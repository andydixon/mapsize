//! The model: all UI state, the message handler and the frame cache.
//!
//! Threading: update and view run on the event-loop thread. The scan
//! controller mutates the inventory concurrently, so every read of tree data
//! happens under the tree's read lock, taken once per entry point (view,
//! handle_key, mouse handlers) and passed down as `&Tree`; helpers never lock
//! again (a recursive read lock can deadlock against a waiting writer). The
//! model itself is only touched on the event-loop thread.

use super::actions::DupState;
use super::canvas::Style;
use super::canvas::BOLD;
use super::data::Kids;
use super::mapview::TmLayout;
use super::modal::ModalFrame;
use super::search::SearchState;
use super::theme::{load_theme, Theme};
use super::views::View;
use crate::cancel::Cancel;
use crate::config::Settings;
use crate::duplicate;
use crate::filter::FilterResult;
use crate::inventory::{NodeId, SizeMode, Tree};
use crate::scan::Scanner;
use crate::snapshot;
use crate::textutil;
use crate::treemap::Rect;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// Begins a scan (or returns a loaded tree with no scanner). Called again
/// on rescan.
pub type StartFn = Arc<dyn Fn(Cancel) -> Result<(Option<Scanner>, Arc<RwLock<Tree>>), crate::Error> + Send + Sync>;

/// Configures the interactive session.
pub struct Options {
    pub read_only: bool,
    pub theme: String,
    pub size_mode: SizeMode,
    pub mouse: bool,
    pub color: String, // auto, truecolor, 256, 16, none
    pub save: String,  // save a snapshot here when the scan completes
    pub settings: Settings,
    pub start: StartFn,
    pub diff: Option<snapshot::Diff>, // compare mode
}

/// The colour capability of the terminal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Profile {
    TrueColor,
    Ansi256,
    Ansi,
    Ascii,
}

/// A key press. name follows Bubble Tea's keystroke names ("a", "G",
/// "ctrl+c", "shift+tab", "space", "pgdown", …); text is the inserted
/// text for printable keys.
#[derive(Clone, Debug, Default)]
pub(crate) struct Key {
    pub name: String,
    pub text: String,
    pub ctrl_alt: bool,
}

impl Key {
    /// A key from its keystroke name, as the Go tests build them: single
    /// characters insert themselves, "space" inserts " ".
    pub fn from_name(s: &str) -> Key {
        let text = match s {
            "space" => " ".to_string(),
            _ if s.chars().count() == 1 => s.to_string(),
            _ => String::new(),
        };
        Key { name: s.to_string(), text, ctrl_alt: s.starts_with("ctrl+") || s.starts_with("alt+") }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Button {
    Left,
    Right,
    Middle,
}

/// Messages delivered to update.
pub(crate) enum Msg {
    Size(i32, i32),
    Tick,
    ScanDone(Arc<RwLock<Tree>>), // identifies the scan by its tree
    Filter { gen: u64, res: Option<FilterResult> },
    FilterDebounce(u64),
    DupDone { finder: Arc<duplicate::Finder>, groups: Option<Vec<duplicate::Group>> },
    TrashDone { tree: Arc<RwLock<Tree>>, id: NodeId, err: Option<String>, done: String },
    SaveDone { path: String, err: Option<String> },
    Key(Key),
    Click { x: i32, y: i32, button: Button },
    Wheel { x: i32, y: i32, up: bool },
    Motion { x: i32, y: i32 },
    Quit, // SIGTERM
}

/// Side effects requested by update, executed by the event loop.
pub(crate) enum Cmd {
    None,
    Batch(Vec<Cmd>),
    /// Runs on a background thread; its message is delivered to update.
    Run(Box<dyn FnOnce() -> Msg + Send>),
    /// Delivers the message after the delay.
    Tick(Duration, Msg),
    /// Puts text on the clipboard (OSC 52).
    Clipboard(String),
}

impl Cmd {
    pub fn run(f: impl FnOnce() -> Msg + Send + 'static) -> Cmd {
        Cmd::Run(Box::new(f))
    }

    pub fn batch(v: Vec<Cmd>) -> Cmd {
        let v: Vec<Cmd> = v.into_iter().filter(|c| !matches!(c, Cmd::None)).collect();
        if v.is_empty() {
            Cmd::None
        } else {
            Cmd::Batch(v)
        }
    }

    /// Runs background commands synchronously and returns their messages
    /// (ticks and clipboard writes are dropped). For tests and benchmarks.
    pub fn run_sync(self) -> Vec<Msg> {
        match self {
            Cmd::Run(f) => vec![f()],
            Cmd::Batch(v) => v.into_iter().flat_map(Cmd::run_sync).collect(),
            _ => Vec::new(),
        }
    }
}

/// A clickable region recorded while painting.
pub(crate) struct Hit {
    pub r: Rect,
    pub act: HitAct,
}

#[derive(Clone, Copy)]
pub(crate) enum HitAct {
    None,
    SetView(usize),
    ZoomTo(NodeId),
    Block(i64),
    Row(usize), // a row of the current view's table
}

pub(crate) fn group_id(parent: NodeId) -> i64 {
    -1 - parent as i64
}
pub(crate) fn is_group(id: i64) -> bool {
    id < 0
}
pub(crate) fn group_parent(id: i64) -> NodeId {
    (-1 - id) as NodeId
}

pub(crate) const NO_SEL: i64 = 1 << 62;

/// MAPSIZE_DEBUG_PANIC=ui panics on the first key press; =scanner panics in
/// a background thread shortly after start. Lets developers verify terminal
/// restoration on crashes.
pub(crate) fn debug_panic() -> String {
    std::env::var("MAPSIZE_DEBUG_PANIC").unwrap_or_default()
}

pub(crate) struct Model {
    pub opts: Options,
    pub theme: Rc<Theme>,

    pub scanner: Option<Scanner>,
    pub tree: Arc<RwLock<Tree>>,
    pub cancel: Cancel,
    pub scanning: bool,
    pub snapshot: bool, // tree was loaded from a snapshot (not the live filesystem)
    pub ctrl_c: bool,   // a Ctrl+C already cancelled the scan

    pub w: i32,
    pub h: i32,
    pub profile: Profile,
    pub size_mode: SizeMode,

    pub views: Vec<View>,
    pub view: usize,

    pub zoom: NodeId,
    pub sel: i64, // selected block: NodeId, or group_id(parent) for an LOD group
    // user_sel is set once the user has chosen a selection; before that the
    // selection follows the largest block.
    pub user_sel: bool,
    pub zoomed: bool, // zoom changed during this message (zoom_to set user_sel)

    pub search: SearchState,
    pub modals: Vec<ModalFrame>,

    pub toast: String,
    pub toast_style: Style,
    pub toast_until: Instant,

    // Frame and data caches.
    pub dirty: bool,
    pub frame: String,
    pub data_ver: u64, // bumped when sizes/filters change; invalidates caches
    pub last_data: Option<Instant>, // last time data_ver advanced during a scan
    pub kids_cache: HashMap<NodeId, Rc<Kids>>,
    pub kids_ver: u64,
    pub tm: Option<Rc<TmLayout>>,
    pub hits: Vec<Hit>, // clickable regions from the last frame
    pub hover: i64,
    pub last_click: Option<Instant>,
    pub last_click_i: i64,
    pub ticking: bool,

    pub pending_zoom: Option<Vec<u8>>, // path to re-zoom after a rescan
    pub dups: Option<DupState>,

    pub quit: bool,
}

impl Model {
    pub fn new(opts: Options, s: Option<Scanner>, t: Arc<RwLock<Tree>>, cancel: Cancel) -> Model {
        let mut name = opts.theme.clone();
        if (name.is_empty() || name == "default") && std::env::var_os("NO_COLOR").is_some() {
            name = "mono".into();
        }
        let mut theme = load_theme(&name, &opts.settings);
        if std::env::var("TERM").is_ok_and(|v| v.eq_ignore_ascii_case("linux"))
            || std::env::var_os("MAPSIZE_ASCII").is_some_and(|v| !v.is_empty())
        {
            theme.ascii = true;
        }
        let mut views = vec![View::Map, View::List(Default::default()), View::Ext(Default::default()),
            View::Top(Default::default()), View::Info(Default::default())];
        if opts.diff.is_some() {
            views.push(View::Changes(Default::default()));
        }
        let scanning = s.is_some();
        Model {
            size_mode: opts.size_mode,
            opts,
            theme: Rc::new(theme),
            scanner: s,
            tree: t,
            cancel,
            scanning,
            snapshot: !scanning,
            ctrl_c: false,
            w: 0,
            h: 0,
            profile: Profile::TrueColor,
            views,
            view: 0,
            zoom: 0,
            sel: NO_SEL,
            user_sel: false,
            zoomed: false,
            search: SearchState::default(),
            modals: Vec::new(),
            toast: String::new(),
            toast_style: Style::default(),
            toast_until: Instant::now(),
            dirty: true,
            frame: String::new(),
            data_ver: 0,
            last_data: None,
            kids_cache: HashMap::new(),
            kids_ver: 0,
            tm: None,
            hits: Vec::new(),
            hover: NO_SEL,
            last_click: None,
            last_click_i: NO_SEL,
            ticking: false,
            pending_zoom: None,
            dups: None,
            quit: false,
        }
    }

    /// Starts ticking and waits for scan completion.
    pub fn init(&mut self) -> Cmd {
        if self.scanning {
            let w = self.wait_scan();
            let t = self.start_ticking();
            return Cmd::batch(vec![w, t]);
        }
        Cmd::None
    }

    fn wait_scan(&self) -> Cmd {
        let Some(s) = &self.scanner else { return Cmd::None };
        let (rx, tree) = (s.done().clone(), s.tree.clone());
        Cmd::run(move || {
            let _ = rx.recv();
            Msg::ScanDone(tree)
        })
    }

    pub fn refresh_interval(&self) -> Duration {
        let hz = if self.opts.settings.refresh_hz <= 0 { 10 } else { self.opts.settings.refresh_hz };
        Duration::from_secs(1) / hz as u32
    }

    /// Returns a tick command unless one is already pending.
    pub fn start_ticking(&mut self) -> Cmd {
        if self.ticking {
            return Cmd::None;
        }
        self.ticking = true;
        Cmd::Tick(self.refresh_interval(), Msg::Tick)
    }

    fn needs_ticks(&self) -> bool {
        self.scanning || Instant::now() < self.toast_until || self.dups.as_ref().is_some_and(|d| d.running)
    }

    /// Shows a transient status message.
    pub fn notify(&mut self, s: String, st: Style) -> Cmd {
        self.toast = s;
        self.toast_style = st;
        self.toast_until = Instant::now() + Duration::from_secs(4);
        self.dirty = true;
        self.start_ticking()
    }

    pub fn info(&mut self, s: impl Into<String>) -> Cmd {
        let st = Style::new(self.theme.fg, self.theme.status_bg);
        self.notify(s.into(), st)
    }

    pub fn warn(&mut self, s: impl Into<String>) -> Cmd {
        let st = Style::new(self.theme.warn, self.theme.status_bg).with(BOLD);
        self.notify(s.into(), st)
    }

    /// Marks size-dependent caches stale.
    pub fn invalidate(&mut self) {
        self.data_ver += 1;
        self.dirty = true;
    }

    /// Handles one message.
    pub fn update(&mut self, msg: Msg) -> Cmd {
        match msg {
            Msg::Size(w, h) => {
                // Only record the size; layout is recomputed lazily in view,
                // so a storm of resize events costs at most one layout per
                // frame.
                if w != self.w || h != self.h {
                    (self.w, self.h) = (w, h);
                    self.dirty = true;
                }
                Cmd::None
            }
            Msg::Tick => {
                self.ticking = false;
                if self.scanning {
                    self.dirty = true;
                    if self.last_data.is_none_or(|t| t.elapsed() >= Duration::from_millis(400)) {
                        self.invalidate();
                        self.last_data = Some(Instant::now());
                    }
                    self.resolve_pending_zoom();
                }
                if !self.toast.is_empty() && Instant::now() > self.toast_until {
                    self.toast.clear();
                    self.dirty = true;
                }
                let mut cmds = Vec::new();
                if self.scanning
                    && self.search.query.is_some()
                    && self.search.applied_at.is_none_or(|t| t.elapsed() > Duration::from_secs(1))
                {
                    cmds.push(self.apply_filter());
                }
                if self.dups.as_ref().is_some_and(|d| d.running) {
                    self.dirty = true;
                }
                if self.needs_ticks() {
                    cmds.push(self.start_ticking());
                }
                Cmd::batch(cmds)
            }
            Msg::ScanDone(t) => {
                if !Arc::ptr_eq(&t, &self.tree) {
                    return Cmd::None; // a superseded scan
                }
                self.scan_finished()
            }
            Msg::Filter { gen, res } => self.filter_result(gen, res),
            Msg::FilterDebounce(gen) => {
                if gen == self.search.gen {
                    return self.apply_filter();
                }
                Cmd::None
            }
            Msg::DupDone { finder, groups } => self.dup_done(finder, groups),
            Msg::TrashDone { tree, id, err, done } => self.trash_done(tree, id, err, done),
            Msg::SaveDone { path, err } => match err {
                Some(e) => self.warn(format!("Snapshot save failed: {}", textutil::sanitize(&e))),
                None => self.info(format!("Saved snapshot {}", textutil::sanitize(&path))),
            },
            Msg::Key(k) => {
                if debug_panic() == "ui" {
                    panic!("MAPSIZE_DEBUG_PANIC=ui: deliberate panic to test terminal restoration");
                }
                self.dirty = true;
                let prev = self.sel;
                self.zoomed = false;
                let cmd = self.handle_key(&k);
                self.note_selection(prev);
                cmd
            }
            Msg::Click { x, y, button } => {
                let prev = self.sel;
                self.zoomed = false;
                let cmd = self.mouse_click(x, y, button);
                self.note_selection(prev);
                cmd
            }
            Msg::Wheel { x, y, up } => self.mouse_wheel(x, y, up),
            Msg::Motion { x, y } => {
                self.mouse_motion(x, y);
                Cmd::None
            }
            Msg::Quit => {
                self.quit = true;
                Cmd::None
            }
        }
    }

    /// Records that the user chose a selection explicitly.
    fn note_selection(&mut self, prev: i64) {
        if !self.zoomed && self.sel != prev && self.sel != NO_SEL {
            self.user_sel = true;
        }
    }

    fn scan_finished(&mut self) -> Cmd {
        self.scanning = false;
        self.invalidate();
        self.resolve_pending_zoom();
        let (total, files, dirs, errs, el, cancelled) = {
            let t = self.tree.read().unwrap();
            let st = &t.stats;
            (t.node(0).total(self.size_mode), st.files + st.symlinks + st.others, st.dirs, st.total_errors(), st.elapsed(), st.cancelled)
        };
        let mut cmds = Vec::new();
        let rate = (files + dirs) as f64 / el.as_secs_f64().max(0.001);
        let mut msg = format!(
            "✓ Scan complete  {}  ·  {} files  ·  {} dirs  ·  {}  ·  {}/s",
            textutil::duration(el), textutil::count(files), textutil::count(dirs), textutil::size(total), textutil::count(rate as i64)
        );
        if cancelled {
            msg = "✗ Scan cancelled — totals are incomplete".into();
        }
        if errs > 0 {
            msg += &format!("  ·  {} errors (e)", textutil::count(errs));
        }
        if cancelled || errs > 0 {
            cmds.push(self.warn(msg));
        } else {
            let st = Style::new(self.theme.ok, self.theme.status_bg).with(BOLD);
            cmds.push(self.notify(msg, st));
        }
        if !self.opts.save.is_empty() && !cancelled {
            let p = self.opts.save.clone();
            cmds.push(self.save_snapshot(p));
        }
        if self.search.query.is_some() {
            cmds.push(self.apply_filter());
        }
        Cmd::batch(cmds)
    }

    /// First press cancels a running scan, second quits.
    pub fn ctrl_c_pressed(&mut self) -> Cmd {
        if self.scanning && !self.ctrl_c {
            self.ctrl_c = true;
            self.cancel.cancel();
            return self.warn("Cancelling scan… press Ctrl+C again to quit");
        }
        self.quit = true;
        Cmd::None
    }

    /// Called from handle_key, which holds the old tree's read lock (tr).
    pub fn rescan(&mut self, tr: &Tree) -> Cmd {
        if self.snapshot {
            return self.warn("Snapshot loaded — nothing to rescan");
        }
        self.pending_zoom = Some(tr.path_bytes(self.zoom));
        self.cancel.cancel();
        let cancel = Cancel::new();
        let (s, t) = match (self.opts.start)(cancel.clone()) {
            Ok(v) => v,
            Err(e) => {
                cancel.cancel();
                return self.warn(format!("Rescan failed: {}", textutil::sanitize(&e.to_string())));
            }
        };
        self.scanning = s.is_some();
        (self.scanner, self.tree, self.cancel, self.ctrl_c) = (s, t, cancel, false);
        (self.zoom, self.sel) = (0, NO_SEL);
        self.kids_cache.clear();
        self.tm = None;
        if let Some(d) = self.dups.take() {
            d.cancel.cancel();
            // Drop the duplicates view: its rows referred to the old tree.
            if let Some(i) = self.views.iter().position(|v| matches!(v, View::Dups(_))) {
                self.views.remove(i);
                self.view = 0;
            }
        }
        self.search.result = None;
        self.search.gen += 1; // results computed against the old tree are discarded
        self.invalidate();
        let w = self.wait_scan();
        let tk = self.start_ticking();
        let i = self.info("Rescanning…");
        let f = self.apply_filter();
        Cmd::batch(vec![w, tk, i, f])
    }

    /// Re-zooms into the remembered directory once the new scan has
    /// discovered it. Takes the read lock itself.
    pub fn resolve_pending_zoom(&mut self) {
        let Some(pz) = self.pending_zoom.clone() else { return };
        let arc = self.tree.clone();
        let t = arc.read().unwrap();
        let root = t.path_bytes(0);
        let rel: &[u8] = if pz == root {
            b""
        } else if let Some(r) = pz.strip_prefix(root.as_slice()).filter(|r| root.ends_with(b"/") || r.starts_with(b"/")) {
            r
        } else {
            self.pending_zoom = None;
            return;
        };
        let mut id = 0;
        for part in rel.split(|&b| b == b'/').filter(|p| !p.is_empty()) {
            match t.children(id).find(|(_, n)| &*n.name == part) {
                Some((c, _)) => id = c,
                None => return, // not discovered yet
            }
        }
        self.zoom = id;
        self.pending_zoom = None;
        self.dirty = true;
    }

    /// Renders the frame. Expensive work happens only when something
    /// changed; otherwise the cached frame is returned.
    pub fn view(&mut self) -> &str {
        if self.dirty || self.frame.is_empty() {
            self.frame = self.render();
            self.dirty = false;
        }
        &self.frame
    }
}
