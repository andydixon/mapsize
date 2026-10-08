#!/usr/bin/env python3
"""Runs the Go `go test -bench` suite and the Rust bench example
interleaved for several rounds and prints the best time per op of each.

    rust/bench/micro.py GO_TUI_TEST GO_TREEMAP_TEST RUST_BENCH [ROUNDS]

Build the inputs with `go test -c ./internal/tui`, `go test -c
./internal/treemap` and `cargo build --release --example bench`. The Go tui
test binary must run from internal/tui, so run this from the repo root.
"""
import re, subprocess, sys

go_tui, go_tm, rust = sys.argv[1:4]
rounds = int(sys.argv[4]) if len(sys.argv) > 4 else 5
names = {  # rust name -> go name
    "treemap/Squarify(300)": "Squarify", "treemap/Neighbour(300)": "Neighbour",
    "tui/BigColdFrame": "BigColdFrame", "tui/BigResize": "BigResize",
    "tui/BigFlatDirCold": "BigFlatDirCold", "tui/BigNavigate": "BigNavigate",
    "tui/BigFilter/size > 1MB": "BigFilter/size_>_1MB",
    "tui/BigFilter/*.dat AND size > 500k": "BigFilter/*.dat_AND_size_>_500k",
    "tui/BigFilter/path contains sub3": "BigFilter/path_contains_sub3",
}
unit = {"ns": 1e-9, "µs": 1e-6, "ms": 1e-3, "s": 1}
go_best, rust_best = {}, {}

def keep(d, k, v):
    d[k] = min(d.get(k, v), v)

for _ in range(rounds):
    for exe, cwd in ((go_tui, "internal/tui"), (go_tm, "internal/treemap")):
        out = subprocess.run([exe, "-test.run=NONE", "-test.bench=.", "-test.benchtime=1s"], cwd=cwd, capture_output=True, text=True).stdout
        for m in re.finditer(r"^Benchmark(\S+?)-\d+\s+\d+\s+([\d.]+) ns/op", out, re.M):
            keep(go_best, m[1], float(m[2]) * 1e-9)
    out = subprocess.run([rust], capture_output=True, text=True).stdout
    for m in re.finditer(r"^(\S.*?)\s+\d+ ops\s+([\d.]+)(ns|µs|ms|s)/op", out, re.M):
        keep(rust_best, m[1].strip(), float(m[2]) * unit[m[3]])

def fmt(s):
    return f"{s * 1e6:.1f} µs" if s < 1e-3 else f"{s * 1e3:.2f} ms"

print(f"Best of {rounds} interleaved rounds\n")
print("| benchmark | Go | Rust | speed-up |\n|---|---:|---:|---:|")
for r, g in names.items():
    if r in rust_best and g in go_best:
        print(f"| {r} | {fmt(go_best[g])} | {fmt(rust_best[r])} | {go_best[g] / rust_best[r]:.2f}× |")
