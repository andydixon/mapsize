//! Synthetic directory-tree generator for testing and benchmarking mapsize:
//!
//!     cargo run --release --example treegen -- --out DIR [flags]
//!
//! Flags (`-flag` or `--flag`, value as `=V` or a separate argument):
//!   --out DIR          output directory (required, must not exist or be empty)
//!   --directories N    number of directories (1000)
//!   --files N          number of files (10000)
//!   --max-depth N      maximum directory depth (8)
//!   --sparse           make some large files sparse
//!   --unicode          use Unicode and unusual (but safe) names
//!   --hostile          include names with control characters/escape sequences (Unix only)
//!   --max-size N       largest regular file in bytes; sizes are log-uniform, content is written (1048576)
//!   --seed N           random seed; output is deterministic per seed (1)
//!   --hotspots N       directories that receive a large share of files (3)

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::exit;
use std::time::Instant;

const WORDS: &[&str] = &[
    "data",
    "cache",
    "backup",
    "photos",
    "src",
    "build",
    "logs",
    "tmp",
    "vm",
    "media",
    "docs",
    "lib",
    "node_modules",
    "archive",
];
const EXTS: &[&str] = &[
    ".txt", ".log", ".jpg", ".png", ".mp4", ".iso", ".qcow2", ".go", ".c", ".pdf", ".zip",
    ".tar.gz", ".db", ".so", "", ".json", ".mkv", ".flac",
];
const UNI: &[&str] = &[
    "日本語",
    "Ünïcödé",
    "emoji 🎉",
    "Ελληνικά",
    "кириллица",
    "space name",
    "quote'name",
    "dash-name",
];
const BOOLS: &[&str] = &["sparse", "unicode", "hostile"];

/// SplitMix64: tiny, fast and good enough for synthetic data.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn n(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn f64(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn pick<'a>(&mut self, s: &[&'a str]) -> &'a str {
        s[self.n(s.len())]
    }
}

fn fatal(msg: impl std::fmt::Display) -> ! {
    eprintln!("treegen: {msg}");
    exit(1);
}

fn usage() -> ! {
    eprintln!("usage: treegen --out DIR [--directories N] [--files N] [--max-depth N] [--sparse] [--unicode] [--hostile] [--max-size N] [--seed N] [--hotspots N]");
    exit(2);
}

fn name(r: &mut Rng, i: usize, unicode: bool, hostile: bool) -> String {
    if hostile && r.n(50) == 0 {
        format!("evil\x1b[31m{i}\x1b]0;pwned\x07")
    } else if unicode && r.n(5) == 0 {
        format!("{}-{i}", r.pick(UNI))
    } else {
        format!("{}-{i}", r.pick(WORDS))
    }
}

fn main() {
    let mut out = None;
    let (mut dirs, mut files, mut depth, mut max_size, mut seed, mut hotspots) =
        (1000usize, 10000usize, 8usize, 1usize << 20, 1u64, 3usize);
    let (mut sparse, mut unicode, mut hostile) = (false, false, false);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let Some(flag) = a.strip_prefix("--").or_else(|| a.strip_prefix('-')) else {
            usage()
        };
        let (key, inline) = match flag.split_once('=') {
            Some((k, v)) => (k, Some(v.to_string())),
            None => (flag, None),
        };
        if BOOLS.contains(&key) {
            let on = match inline.as_deref() {
                None | Some("true" | "1") => true,
                Some("false" | "0") => false,
                Some(v) => fatal(format!("invalid value {v:?} for -{key}")),
            };
            *match key {
                "sparse" => &mut sparse,
                "unicode" => &mut unicode,
                _ => &mut hostile,
            } = on;
            continue;
        }
        let v = inline
            .or_else(|| args.next())
            .unwrap_or_else(|| fatal(format!("flag needs an argument: -{key}")));
        let num = || {
            v.parse::<u64>()
                .unwrap_or_else(|_| fatal(format!("invalid value {v:?} for -{key}")))
        };
        match key {
            "out" => out = Some(PathBuf::from(&v)),
            "directories" => dirs = num() as usize,
            "files" => files = num() as usize,
            "max-depth" => depth = num() as usize,
            "max-size" => max_size = num() as usize,
            "seed" => seed = num(),
            "hotspots" => hotspots = num() as usize,
            _ => usage(),
        }
    }
    let Some(out) = out else { usage() };
    hostile &= cfg!(unix);
    fs::create_dir_all(&out).unwrap_or_else(|e| fatal(e));
    if fs::read_dir(&out).is_ok_and(|mut d| d.next().is_some()) {
        fatal(format!("{} is not empty", out.display()));
    }
    let mut r = Rng(seed);
    let start = Instant::now();

    // Pick a depth first, then a directory at that depth, so the tree is broad
    // near the top instead of one giant preferential-attachment chain.
    let mut all = vec![(out.clone(), 0usize)];
    let mut by_depth = vec![vec![0usize]];
    while all.len() < dirs + 1 {
        let lv = &by_depth[r.n(by_depth.len())];
        let (parent, d) = all[lv[r.n(lv.len())]].clone();
        if d >= depth {
            continue;
        }
        let p = parent.join(name(&mut r, all.len(), unicode, hostile));
        if fs::create_dir(&p).is_err() {
            continue; // name collision; try again
        }
        all.push((p, d + 1));
        if d + 1 == by_depth.len() {
            by_depth.push(Vec::new());
        }
        by_depth[d + 1].push(all.len() - 1);
    }
    let hot: Vec<usize> = (0..hotspots).map(|_| r.n(all.len())).collect();
    // Incompressible, so allocation is real everywhere.
    let buf: Vec<u8> = (0..max_size).map(|_| r.next() as u8).collect();
    let (mut bytes, mut created) = (0u64, 0usize);
    for i in 0..files {
        let d = if !hot.is_empty() && r.n(3) == 0 {
            &all[hot[r.n(hot.len())]].0
        } else {
            &all[r.n(all.len())].0
        };
        let n = name(&mut r, i, unicode, hostile);
        let p = d.join(n + r.pick(EXTS));
        // Log-uniform sizes: many tiny files, a few large ones.
        let mut size = (r.f64() * (max_size as f64).ln()).exp() as usize;
        if r.n(20) == 0 {
            size = 0;
        }
        let Ok(mut f) = OpenOptions::new().write(true).create_new(true).open(&p) else {
            continue;
        };
        if sparse && r.n(200) == 0 {
            // Sparse: a large logical size with almost nothing allocated.
            f.write_all(b"x")
                .and_then(|_| f.set_len((1 + r.n(8) as u64) << 30))
                .unwrap_or_else(|e| fatal(e));
        } else if size > 0 {
            f.write_all(&buf[..size]).unwrap_or_else(|e| fatal(e));
            bytes += size as u64;
        }
        created += 1;
        if i % 100_000 == 0 && i > 0 {
            eprintln!("{i} files…");
        }
    }
    eprintln!(
        "created {} directories, {created} files ({bytes} logical bytes) in {:.2?}",
        all.len() - 1,
        start.elapsed()
    );
}
