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
HDDs. Go 1.27.1, Rust 1.91.1 (`lto`, `codegen-units = 1`). The host was
shared and heavily loaded during the runs (load average 30–50 on 8 threads),
so absolute times are inflated 2–4× compared with PERFORMANCE.md. To keep the
comparison fair, Go and Rust runs were interleaved and the tables report
best-of (micro) or median and best (end-to-end). Re-run on an idle machine
before quoting absolute numbers:

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
| treemap/Squarify(300) | 299.3 µs | 22.2 µs | 13.48× |
| treemap/Neighbour(300) | 5.0 µs | 2.8 µs | 1.76× |
| tui/BigColdFrame | 86.88 ms | 29.32 ms | 2.96× |
| tui/BigResize | 7.15 ms | 4.76 ms | 1.50× |
| tui/BigFlatDirCold | 86.33 ms | 22.90 ms | 3.77× |
| tui/BigNavigate | 4.85 ms | 4.07 ms | 1.19× |
| tui/BigFilter/size > 1MB | 78.62 ms | 59.51 ms | 1.32× |
| tui/BigFilter/*.dat AND size > 500k | 302.03 ms | 154.90 ms | 1.95× |
| tui/BigFilter/path contains sub3 | 337.79 ms | 136.85 ms | 2.47× |

### End to end (`~/go/pkg/mod`: 388k files + 65k dirs, warm cache)

| scenario | Go median | Rust median | Go best | Rust best | Go RSS | Rust RSS | speed-up |
|---|---:|---:|---:|---:|---:|---:|---:|
| scan, 1 workers | 4.68 s | 5.51 s | 4.34 s | 4.86 s | 114 MB | 75 MB | 0.85× |
| scan, 8 workers | 2.03 s | 2.03 s | 1.61 s | 1.42 s | 113 MB | 78 MB | 1.00× |
| scan, 32 workers | 3.09 s | 2.59 s | 2.85 s | 2.07 s | 116 MB | 89 MB | 1.19× |
| scan + save snapshot | 4.69 s | 3.60 s | 4.02 s | 3.29 s | 117 MB | 80 MB | 1.30× |
| load own snapshot | 1.20 s | 0.71 s | 0.71 s | 0.48 s | 85 MB | 78 MB | 1.69× |
| load Go snapshot | 1.03 s | 1.14 s | 0.84 s | 0.48 s | 85 MB | 78 MB | 0.90× |
| scan + JSON export | 6.10 s | 4.74 s | 5.44 s | 4.16 s | 152 MB | 79 MB | 1.29× |
| scan + top 50 largest files | 4.49 s | 4.31 s | 3.62 s | 3.65 s | 117 MB | 79 MB | 1.04× |

Reading the numbers:

* **Scanning** is syscall-bound (`statx`/`getdents` were about 60% of Go's
  profile), so wall time is roughly at parity. Rust edges ahead at high
  worker counts. The single-worker case is about 15% slower, but that run was
  the noisiest of the set.
* **Memory**: peak RSS is 30–48% lower (78 MB vs 113 MB while scanning
  458k nodes). The JSON export streams without Go's per-node allocations
  (79 MB vs 152 MB).
* **Snapshots**: loading is about 1.7× faster on the median. The two binaries
  load each other's files.
* **UI**: cold frames are 3–3.8× faster, and squarified layout is about 13×
  faster. Navigation and resize gain less (1.2–1.5×) because they mostly
  reuse cached layout.
* **Filters**: 1.3–2.5× faster. This required caching lower-cased directory
  paths in a `Vec` with reused buffers, and a `*.ext` fast path in the
  `filepath.Match` port.
