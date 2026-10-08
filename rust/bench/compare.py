#!/usr/bin/env python3
"""Times the Go and Rust mapsize binaries on the same command lines.

    rust/bench/compare.py GO_BIN RUST_BIN PATH [RUNS]

Runs are interleaved (go, rust, go, rust, …) after one untimed warm-up each,
so both see the same page-cache state. Reports median and best wall time and
median peak RSS per scenario as a Markdown table. Linux only (/usr/bin/time).
"""
import os, statistics, subprocess, sys, tempfile

go, rust, path = sys.argv[1:4]
runs = int(sys.argv[4]) if len(sys.argv) > 4 else 7
tmp = tempfile.mkdtemp(prefix="mapsize-bench-")
snap = {"go": os.path.join(tmp, "go.msz"), "rust": os.path.join(tmp, "rust.msz")}
bins = {"go": go, "rust": rust}

def timed(argv):
    r = subprocess.run(["/usr/bin/time", "-f", "%e %M"] + argv, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    if r.returncode != 0:
        sys.exit(f"failed: {' '.join(argv)}\n{r.stderr}")
    secs, kb = r.stderr.strip().splitlines()[-1].split()
    return float(secs), int(kb) / 1024

scenarios = [(f"scan, {w} workers", lambda b, w=w: [bins[b], "--no-ui", "--workers", str(w), path]) for w in (1, 8, 32)]
scenarios += [
    ("scan + save snapshot", lambda b: [bins[b], "--no-ui", "--save", snap[b], path]),
    ("load own snapshot", lambda b: [bins[b], "--no-ui", snap[b]]),
    ("load Go snapshot", lambda b: [bins[b], "--no-ui", snap["go"]]),
    ("scan + JSON export", lambda b: [bins[b], "--json", path]),
    ("scan + top 50 largest files", lambda b: [bins[b], "--largest-files", "50", path]),
]

print(f"Path: `{path}`, {runs} interleaved runs per binary, warm cache\n")
print("| scenario | Go median | Rust median | Go best | Rust best | Go RSS | Rust RSS | speed-up |")
print("|---|---:|---:|---:|---:|---:|---:|---:|")
for name, mk in scenarios:
    argv = {k: mk(k) for k in bins}
    res = {k: [] for k in bins}
    for k in bins:
        timed(argv[k])  # warm-up (also creates the snapshot files)
    for _ in range(runs):
        for k in bins:
            res[k].append(timed(argv[k]))
    med = {k: statistics.median(t for t, _ in v) for k, v in res.items()}
    best = {k: min(t for t, _ in v) for k, v in res.items()}
    rss = {k: statistics.median(m for _, m in v) for k, v in res.items()}
    print(f"| {name} | {med['go']:.2f} s | {med['rust']:.2f} s | {best['go']:.2f} s | {best['rust']:.2f} s "
          f"| {rss['go']:.0f} MB | {rss['rust']:.0f} MB | {med['go'] / med['rust']:.2f}× |", flush=True)
