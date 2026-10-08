# mapsize — Rust port

A port of the Go implementation (repo root) to Rust, with the same CLI, TUI,
keys, themes and snapshot format. `.msz` snapshots are byte-compatible in
both directions. The code is Linux/Unix only; the Windows code paths were
not ported.

```
cd rust
cargo build --release          # target/release/mapsize
cargo test --release           # 69 tests (ported from the Go suites, fuzz tests excepted)
```

Module layout mirrors `internal/`: `inventory`, `scan`, `platform`, `treemap`,
`filter`, `snapshot` (+ `compare`), `duplicate`, `export`, `config`,
`textutil`, `tui`, `app`. The tree is shared as `Arc<RwLock<Tree>>` with the
same single-writer invariants as ARCHITECTURE.md; `cancel::Cancel` replaces
`context.Context`; crossterm replaces Bubble Tea as the event loop (the
canvas renderer is ported as-is, plus line-diffed frame output).

## Known differences from Go

* `--log`/`--log-level` are accepted but there is no debug log yet.
* A scanner thread panic restores the terminal and aborts the process instead
  of returning an "internal error" message.
* The colour profile is detected from `NO_COLOR`/`TERM`/`COLORTERM` only (no
  terminal query); `--color` still overrides.
* JSON export does not HTML-escape `<>&`; `started` is written in UTC.
* The filter lexer no longer hangs on queries containing bytes Go treats as
  white space but never skips (e.g. `voilà`, whose UTF-8 contains 0xA0).
* Huge sizes saturate rather than wrap in filters and snapshot comparison.

## Benchmarks

Machine: Intel Core i7-7700 (4C/8T), 32 GiB, ext4 on md RAID over 7200 rpm
HDDs. Go 1.27.1, Rust 1.91.1 (`lto`, `codegen-units = 1`). The host is
shared, and its load average was 5–9 on 8 threads during the runs. Go's
absolute times are close to those in PERFORMANCE.md. Go and Rust runs were
interleaved; the tables report best-of (micro) or median and best
(end-to-end). To re-run:

```
go test -c -o /tmp/tui.test ./internal/tui && go test -c -o /tmp/treemap.test ./internal/treemap
(cd rust && cargo build --release --example bench)
rust/bench/micro.py /tmp/tui.test /tmp/treemap.test rust/target/release/examples/bench 5
rust/bench/compare.py bin/mapsize rust/target/release/mapsize ~/go/pkg/mod 7
```

### In-process (Go `go test -bench` suite vs `examples/bench.rs`)

The tree is 1,008,201 synthetic nodes, rendered at 200×60.

| benchmark | Go | Rust | speed-up |
|---|---:|---:|---:|
| treemap/Squarify(300) | 84.3 µs | 8.4 µs | 10.07× |
| treemap/Neighbour(300) | 2.1 µs | 1.1 µs | 1.94× |
| tui/BigColdFrame | 36.01 ms | 11.34 ms | 3.18× |
| tui/BigResize | 2.02 ms | 1.50 ms | 1.35× |
| tui/BigFlatDirCold | 32.00 ms | 9.37 ms | 3.41× |
| tui/BigNavigate | 1.50 ms | 1.31 ms | 1.14× |
| tui/BigFilter/size > 1MB | 29.44 ms | 26.75 ms | 1.10× |
| tui/BigFilter/*.dat AND size > 500k | 121.40 ms | 67.84 ms | 1.79× |
| tui/BigFilter/path contains sub3 | 101.86 ms | 63.80 ms | 1.60× |

### End to end (`~/go/pkg/mod`: 388k files + 65k dirs, warm cache)

| scenario | Go median | Rust median | Go best | Rust best | Go RSS | Rust RSS | speed-up |
|---|---:|---:|---:|---:|---:|---:|---:|
| scan, 1 workers | 1.89 s | 1.52 s | 1.72 s | 1.44 s | 112 MB | 75 MB | 1.24× |
| scan, 8 workers | 0.63 s | 0.47 s | 0.58 s | 0.43 s | 117 MB | 76 MB | 1.34× |
| scan, 32 workers | 0.60 s | 0.46 s | 0.48 s | 0.43 s | 118 MB | 80 MB | 1.30× |
| scan + save snapshot | 1.22 s | 0.75 s | 1.13 s | 0.71 s | 119 MB | 77 MB | 1.63× |
| load own snapshot | 0.34 s | 0.23 s | 0.30 s | 0.23 s | 85 MB | 78 MB | 1.48× |
| load Go snapshot | 0.36 s | 0.26 s | 0.30 s | 0.23 s | 85 MB | 78 MB | 1.38× |
| scan + JSON export | 1.27 s | 0.86 s | 1.04 s | 0.68 s | 153 MB | 76 MB | 1.48× |
| scan + top 50 largest files | 0.70 s | 0.60 s | 0.64 s | 0.47 s | 114 MB | 76 MB | 1.17× |

Reading the numbers:

* **Scanning** is 1.2–1.3× faster at every worker count. It is syscall-bound
  (`statx`/`getdents`), so the gain comes from lower per-entry overhead in the
  controller (no GC, no per-chunk allocation churn), not from faster I/O.
* **Memory**: peak RSS is about a third lower (76 MB vs 117 MB while
  scanning 458k nodes). The JSON export halves memory (76 MB vs 153 MB) because
  it streams without per-node allocations.
* **Snapshots**: scan + save is 1.6× faster and load is 1.4–1.5× faster. The
  two binaries load each other's files.
* **UI**: cold frames are 3.2–3.4× faster, and squarified layout is about 10×
  faster. Navigation and resize gain less (1.1–1.35×) because they mostly
  reuse cached layout.
* **Filters**: 1.1–1.8× faster. This required caching lower-cased directory
  paths in a `Vec` with reused buffers, and a `*.ext` fast path in the
  `filepath.Match` port.
