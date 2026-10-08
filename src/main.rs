//! mapsize: interactive terminal disk usage analyser.

use mapsize::app::{self, Config};
use mapsize::inventory::SizeMode;
use mapsize::{brand, config, scan, snapshot, textutil};
use std::path::Path;

/// Flags taking a value (Go's flag package: `-f v`, `-f=v`, `--f v`, `--f=v`).
const VALUE_FLAGS: &[&str] = &[
    "workers",
    "follow-symlinks",
    "depth",
    "top",
    "largest-files",
    "save",
    "snapshot",
    "load",
    "scan",
    "theme",
    "color",
    "log",
    "log-level",
    "exclude",
];
/// Boolean flags (`-f`, `-f=false`).
const BOOL_FLAGS: &[&str] = &[
    "one-file-system",
    "x",
    "read-only",
    "no-ui",
    "json",
    "csv",
    "duplicates",
    "compare",
    "si",
    "apparent",
    "no-mouse",
    "version",
];

fn usage() {
    let n = brand::NAME;
    let e = brand::SNAPSHOT_EXT;
    eprint!(
        "{n} — interactive terminal disk usage analyser

Usage:
  {n} [flags] [PATH]             scan PATH (default .) interactively
  {n} [flags] SNAPSHOT{e}       open a saved snapshot
  {n} --compare OLD NEW          compare two snapshots
  {n} PATH --top 50 | --json | --csv | --no-ui --save FILE

Flags:
  --workers MODE          concurrency: conservative, balanced, aggressive or a number
  --follow-symlinks MODE  none, same-filesystem or all
  -x, --one-file-system   do not cross filesystem boundaries
  --exclude PATTERN       exclude PATTERN (glob on name, or path prefix if it contains a separator); repeatable
  --read-only             disable all modifying actions
  --no-ui                 scan without the interactive interface and print a summary
  --json, --csv           print the inventory as JSON / CSV
  --depth N               maximum depth for --json/--csv (-1 = unlimited)
  --top N                 print the N largest entries of the root
  --largest-files N       print the N largest files
  --duplicates            find duplicate files (non-interactive)
  --save FILE             save a snapshot to FILE after scanning (alias --snapshot)
  --load FILE             open a saved snapshot FILE instead of scanning
  --scan PATH             path to scan (alternative to a positional argument)
  --compare               compare two snapshots: --compare OLD NEW
  --si                    use SI units (kB, MB) instead of IEC (KiB, MiB)
  --apparent              size by logical (apparent) size instead of disk usage
  --theme NAME            colour theme: default, dark, high-contrast, mono
  --color MODE            colour support: auto, truecolor, 256, 16, none
  --no-mouse              disable mouse support
  --log FILE              write a debug log to FILE
  --log-level LEVEL       log level: error, warn, info, debug, trace
  --version               print version and exit
"
    );
}

fn main() {
    std::process::exit(run(std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect()));
}

fn fail(msg: impl std::fmt::Display) -> i32 {
    eprintln!("{}: {msg}", brand::NAME);
    2
}

fn run(args: Vec<String>) -> i32 {
    let (settings, cfg_err) = config::load();
    let mut vals: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    let mut bools: std::collections::HashMap<&str, bool> = std::collections::HashMap::new();
    let mut excludes = settings.excludes.clone();
    let mut positional = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--" {
            positional.extend(it.by_ref());
            break;
        }
        let Some(flag) = a
            .strip_prefix("--")
            .or_else(|| a.strip_prefix('-'))
            .filter(|f| !f.is_empty() && a != "-")
        else {
            positional.push(a);
            continue;
        };
        let (name, inline) = match flag.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (flag, None),
        };
        if name == "h" || name == "help" {
            usage();
            return 0;
        }
        if let Some(&n) = BOOL_FLAGS.iter().find(|&&f| f == name) {
            match inline.as_deref().map(str::parse::<bool>) {
                None => bools.insert(n, true),
                Some(Ok(v)) => bools.insert(n, v),
                Some(Err(_)) => {
                    return fail(format!(
                        "invalid boolean value {:?} for -{n}",
                        inline.unwrap()
                    ))
                }
            };
        } else if let Some(&n) = VALUE_FLAGS.iter().find(|&&f| f == name) {
            let Some(v) = inline.or_else(|| it.next()) else {
                return fail(format!("flag needs an argument: -{n}"));
            };
            if n == "exclude" {
                excludes.push(v);
            } else {
                vals.insert(n, v);
            }
        } else {
            usage();
            return fail(format!("flag provided but not defined: -{name}"));
        }
    }
    let b = |n: &str| bools.get(n).copied();
    let v = |n: &str| vals.get(n).cloned();
    let int = |n: &str, d: i64| -> Result<i64, String> {
        v(n).map_or(Ok(d), |s| {
            s.parse()
                .map_err(|_| format!("invalid value {s:?} for flag -{n}: parse error"))
        })
    };

    if b("version") == Some(true) {
        println!(
            "{} {} ({}/{})",
            brand::NAME,
            brand::VERSION,
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        return 0;
    }
    if let Some(e) = cfg_err {
        eprintln!("{}: warning: config: {e} (using defaults)", brand::NAME);
    }
    textutil::set_si(b("si").unwrap_or(settings.units == "si"));
    if let Some(l) = v("log-level") {
        if !["error", "warn", "info", "debug", "trace"].contains(&l.as_str()) {
            return fail(format!("invalid --log-level {l:?}"));
        }
    }
    // ponytail: --log is accepted for CLI compatibility; the Rust port has no debug logging yet.

    let (depth, top, largest) = match (int("depth", -1), int("top", 0), int("largest-files", 0)) {
        (Ok(d), Ok(t), Ok(l)) => (d, t.max(0) as usize, l.max(0) as usize),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return fail(e),
    };
    let mut cfg = Config {
        no_ui: b("no-ui").unwrap_or(false),
        json: b("json").unwrap_or(false),
        csv: b("csv").unwrap_or(false),
        depth,
        top,
        largest_files: largest,
        duplicates: b("duplicates").unwrap_or(false),
        read_only: b("read-only").unwrap_or(false),
        theme: v("theme").unwrap_or_else(|| settings.theme.clone()),
        mouse: !b("no-mouse").unwrap_or(!settings.mouse),
        color: v("color").unwrap_or_else(|| "auto".into()),
        load: v("load").unwrap_or_default(),
        save: v("save").or_else(|| v("snapshot")).unwrap_or_default(),
        size_mode: if b("apparent").unwrap_or(settings.size_mode == "logical") {
            SizeMode::Logical
        } else {
            SizeMode::Allocated
        },
        ..Config::new(settings.clone())
    };
    if let Some(p) = v("scan") {
        positional.insert(0, p);
    }
    if b("compare") == Some(true) {
        if positional.len() != 2 {
            return fail("--compare needs exactly two snapshot files");
        }
        cfg.compare = Some((positional[0].clone(), positional[1].clone()));
    } else {
        if positional.len() > 1 {
            return fail("only one path may be scanned at a time");
        }
        if positional.len() == 1
            && cfg.load.is_empty()
            && snapshot::is_snapshot(Path::new(&positional[0]))
        {
            cfg.load = positional.remove(0);
        } else {
            cfg.paths = positional;
        }
    }
    let workers = match scan::workers(&v("workers").unwrap_or_else(|| settings.workers.clone())) {
        Ok(w) => w,
        Err(e) => return fail(e),
    };
    let follow = match scan::parse_follow(
        &v("follow-symlinks").unwrap_or_else(|| settings.follow_symlinks.clone()),
    ) {
        Ok(f) => f,
        Err(e) => return fail(e),
    };
    cfg.scan_opts = scan::Options {
        workers,
        follow,
        one_file_system: b("one-file-system").unwrap_or(settings.one_file_system)
            || b("x").unwrap_or(false),
        excludes,
        ..Default::default()
    };
    match app::run(cfg) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{}: {}", brand::NAME, textutil::sanitize(&e.to_string()));
            1
        }
    }
}
