//! Wires the command-line modes together: interactive TUI, non-interactive
//! reports, snapshots and comparisons. TUI and CLI share the same scanner
//! and inventory.

use crate::cancel::Cancel;
use crate::config::Settings;
use crate::inventory::{Kind, SizeMode, Tree, FLAG_HARDLINK_DUP};
use crate::scan::{self, Scanner};
use crate::{duplicate, export, snapshot, textutil, tui, Error};
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// The fully-resolved command line.
pub struct Config {
    pub paths: Vec<String>,
    pub scan_opts: scan::Options,
    pub no_ui: bool,
    pub json: bool,
    pub csv: bool,
    pub depth: i64,
    pub top: usize,
    pub largest_files: usize,
    pub save: String,
    pub load: String,
    pub compare: Option<(String, String)>,
    pub duplicates: bool,
    pub read_only: bool,
    pub theme: String,
    pub size_mode: SizeMode,
    pub mouse: bool,
    pub color: String,
    pub settings: Settings,
}

impl Config {
    pub fn new(settings: Settings) -> Config {
        Config {
            paths: Vec::new(),
            scan_opts: Default::default(),
            no_ui: false,
            json: false,
            csv: false,
            depth: -1,
            top: 0,
            largest_files: 0,
            save: String::new(),
            load: String::new(),
            compare: None,
            duplicates: false,
            read_only: false,
            theme: settings.theme.clone(),
            size_mode: SizeMode::Allocated,
            mouse: settings.mouse,
            color: "auto".into(),
            settings,
        }
    }

    /// Whether any non-interactive output was requested.
    pub fn report(&self) -> bool {
        self.no_ui
            || self.json
            || self.csv
            || self.top > 0
            || self.largest_files > 0
            || self.duplicates
    }
}

/// Executes the configured mode.
pub fn run(cfg: Config) -> Result<(), Error> {
    if cfg.compare.is_some() {
        return run_compare(cfg);
    }
    if !cfg.report() {
        return run_tui(cfg);
    }
    run_report(cfg)
}

/// Begins a scan (or loads a snapshot). The scanner is None when the tree
/// came from a snapshot.
fn start_source(
    cancel: Cancel,
    load: &str,
    paths: &[String],
    opts: &scan::Options,
) -> Result<(Option<Scanner>, Arc<RwLock<Tree>>), Error> {
    if !load.is_empty() {
        return Ok((
            None,
            Arc::new(RwLock::new(snapshot::load_file(Path::new(load))?)),
        ));
    }
    let root = paths.first().map_or(".", |s| s.as_str());
    let s = scan::start(root.as_bytes(), opts.clone(), cancel)?;
    let t = s.tree.clone();
    Ok((Some(s), t))
}

/// A token cancelled by SIGINT (Ctrl+C), for report mode.
fn interrupt_cancel() -> Cancel {
    static CANCEL: std::sync::OnceLock<Cancel> = std::sync::OnceLock::new();
    extern "C" fn on_sigint(_: libc::c_int) {
        if let Some(c) = CANCEL.get() {
            // Only an atomic store happens here; the channel drop is deferred.
            c.cancel_flag_only();
        }
    }
    let c = CANCEL.get_or_init(Cancel::new).clone();
    unsafe { libc::signal(libc::SIGINT, on_sigint as *const () as libc::sighandler_t) };
    c
}

fn run_report(cfg: Config) -> Result<(), Error> {
    let cancel = interrupt_cancel();
    let (s, tree) = start_source(cancel.clone(), &cfg.load, &cfg.paths, &cfg.scan_opts)?;
    if let Some(s) = &s {
        wait_with_progress(s, &cancel);
    }
    let mut out = std::io::stdout().lock();
    if cfg.duplicates && !cfg.json && !cfg.csv {
        // find takes its own read lock; never nest read locks.
        run_duplicates(&mut out, &cancel, &tree)?;
    }
    let t = tree.read().unwrap();
    if s.is_some() && !cfg.save.is_empty() {
        snapshot::save_file(Path::new(&cfg.save), &t)?;
        eprintln!("Saved snapshot {}", cfg.save);
    }
    if cfg.json {
        return Ok(export::json(&mut out, &t, t.root(), cfg.depth)?);
    }
    if cfg.csv {
        return Ok(export::csv(&mut out, &t, t.root(), cfg.depth)?);
    }
    if cfg.top > 0 {
        let mut ids = t.sorted_children(t.root(), cfg.size_mode);
        ids.truncate(cfg.top);
        export::table(&mut out, &t, &ids, cfg.size_mode)?;
        writeln!(out)?;
    }
    if cfg.largest_files > 0 {
        let m = cfg.size_mode;
        let ids = t.top(
            t.root(),
            cfg.largest_files,
            |n| n.kind == Kind::File && n.flags & FLAG_HARDLINK_DUP == 0,
            |n| n.own(m),
        );
        export::table(&mut out, &t, &ids, m)?;
        writeln!(out)?;
    }
    export::summary(&mut out, &t)?;
    Ok(())
}

/// Blocks until the scan finishes, printing a progress line to stderr if it
/// is a terminal.
fn wait_with_progress(s: &Scanner, cancel: &Cancel) {
    let tty = std::io::stderr().is_terminal();
    let start = Instant::now();
    let mut forwarded = false;
    loop {
        match s.done().recv_timeout(Duration::from_millis(250)) {
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            _ => {
                if tty {
                    eprint!("\r\x1b[K");
                }
                return;
            }
        }
        if cancel.is_cancelled() && !forwarded {
            cancel.cancel(); // complete the signal-handler cancellation
            forwarded = true;
        }
        if tty {
            let p = s.progress();
            let el = start.elapsed();
            eprint!(
                "\r\x1b[Kscanning… {} entries  {}/s  queue {}  {}",
                textutil::count(p.entries),
                textutil::count((p.entries as f64 / el.as_secs_f64()) as i64),
                p.pending,
                textutil::duration(el)
            );
        }
    }
}

fn run_tui(cfg: Config) -> Result<(), Error> {
    let (load, paths, opts) = (cfg.load.clone(), cfg.paths.clone(), cfg.scan_opts.clone());
    tui::run(tui::Options {
        read_only: cfg.read_only,
        theme: cfg.theme,
        size_mode: cfg.size_mode,
        mouse: cfg.mouse,
        color: cfg.color,
        save: cfg.save,
        settings: cfg.settings,
        diff: None,
        start: Arc::new(move |c| start_source(c, &load, &paths, &opts)),
    })
}

fn run_compare(cfg: Config) -> Result<(), Error> {
    let (a, b) = cfg.compare.clone().unwrap();
    let old = snapshot::load_file(Path::new(&a)).map_err(|e| format!("{a}: {e}"))?;
    let nw = snapshot::load_file(Path::new(&b)).map_err(|e| format!("{b}: {e}"))?;
    let d = snapshot::compare(old, &nw);
    if !cfg.report() {
        let tree = Arc::new(RwLock::new(nw));
        return tui::run(tui::Options {
            read_only: true,
            theme: cfg.theme,
            size_mode: cfg.size_mode,
            mouse: cfg.mouse,
            color: cfg.color,
            save: String::new(),
            settings: cfg.settings,
            diff: Some(d),
            start: Arc::new(move |_| Ok((None, tree.clone()))),
        });
    }
    let k = if cfg.top > 0 { cfg.top } else { 30 };
    let m = cfg.size_mode;
    let mut out = std::io::stdout().lock();
    let (ro, rn) = (d.old.node(0), nw.node(0));
    writeln!(
        out,
        "{} → {}",
        textutil::sanitize(&d.old.stats.from_snapshot),
        textutil::sanitize(&nw.stats.from_snapshot)
    )?;
    writeln!(
        out,
        "Total {} → {} ({})\n",
        textutil::size(ro.total(m)),
        textutil::size(rn.total(m)),
        textutil::signed_size(rn.total(m) - ro.total(m))
    )?;
    for c in d.changes(&nw, k, m) {
        writeln!(
            out,
            "{:<10} {:>14}  {}",
            c.status.name(),
            textutil::signed_size(c.delta),
            textutil::sanitize(&c.path)
        )?;
    }
    Ok(())
}

fn run_duplicates(out: &mut dyn Write, cancel: &Cancel, tree: &RwLock<Tree>) -> Result<(), Error> {
    let f = duplicate::Finder::default();
    let groups = f
        .find(
            cancel,
            tree,
            duplicate::Options {
                min_size: 1,
                workers: 0,
            },
        )
        .ok_or("cancelled")?;
    let t = tree.read().unwrap();
    let mut wasted = 0;
    for g in &groups {
        wasted += g.wasted();
        let h: String = g.hash[..6].iter().map(|b| format!("{b:02x}")).collect();
        writeln!(
            out,
            "{} × {}  (sha256 {h}…)",
            textutil::size(g.size),
            g.files.len()
        )?;
        for &id in &g.files {
            writeln!(out, "    {}", textutil::sanitize_bytes(&t.path_bytes(id)))?;
        }
    }
    let p = f.progress();
    writeln!(
        out,
        "\n{} verified duplicate groups, {} reclaimable ({} candidates by size, {} unreadable/changed)\n",
        groups.len(), textutil::size(wasted), p.candidates, p.skipped
    )?;
    Ok(())
}
