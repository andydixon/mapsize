# Performance

All numbers below were measured, not estimated. Re-run them on your own
hardware before relying on them; filesystem caches dominate scan timings.

## Test machine (2026-10-02)

| | |
|---|---|
| CPU | Intel Core i7-7700 @ 3.60 GHz, 4 cores / 8 threads |
| RAM | 32 GiB |
| OS | Linux 7.0, Go 1.27.1 |
| Filesystem | ext4 on Linux software RAID (`/dev/md2`) over two 7200 rpm SATA HDDs |

## Scanner

Dataset: `~/go/pkg/mod` — 388,307 files + 65,518 directories (458,510
inventory nodes), 10.5 GiB allocated. **Warm cache only**: dropping the page
cache needs root, which was not available, so these measure CPU and syscall
cost, not disk seeks. Cold-cache behaviour on HDDs will be dominated by seek
time; expect fewer workers to be optimal there.

`mapsize --no-ui --workers N ~/go/pkg/mod`, wall time, 3 runs each
(`/usr/bin/time`):

| workers | run 1 | run 2 | run 3 | entries/s (best) | peak RSS |
|---:|---:|---:|---:|---:|---:|
| 1  | 1.77 s | 1.67 s | 1.65 s | ~278,000 | 117 MB |
| 2  | 1.06 s | 0.99 s | 1.00 s | ~463,000 | 120 MB |
| 4  | 0.59 s | 0.59 s | 0.62 s | ~777,000 | 117 MB |
| 8 (balanced default) | 0.48 s | 0.50 s | 0.51 s | ~955,000 | 121 MB |
| 16 | 0.45 s | 0.44 s | 0.43 s | ~1,066,000 | 121 MB |
| 32 | 0.43 s | 0.44 s | 0.42 s | ~1,092,000 | 122 MB |

For comparison, `du -s` on the same tree (warm): 1.59 s, 34 MB RSS.

CPU profile at 8 workers: ~60 % of samples in `statx`/`getdents` syscalls;
path reconstruction for job dispatch ~7 %; aggregation is negligible.

### Memory

`go test ./internal/scan -run TestMemoryPerNode` with
`MAPSIZE_MEM_ROOT=~/go/pkg/mod`: 458,510 nodes, 69.1 MiB live heap,
**~158 bytes per node** (node record ~112 bytes plus name storage and
directory side tables). Extrapolated, 10 M entries need ~1.6 GiB.

## Snapshots

Same 458,510-node tree:

| operation | result |
|---|---:|
| snapshot file size (gzip level 1) | 6.7 MB (~15 bytes/node) |
| scan + save | 1.03 s |
| load (`mapsize --no-ui file.msz`), 3 runs | 0.28 / 0.29 / 0.32 s, 81 MB RSS |
| warm-cache rescan for comparison | 0.55 s |

Loading decompresses and verifies SHA-256 on a separate goroutine, overlapping
with parsing. Against a warm cache the win is ~1.9×; against a cold cache or
a network filesystem, where scans are dominated by I/O latency, it is far
larger (not yet measured here).

## UI

`go test ./internal/tui -bench Big` on a synthetic in-memory tree of
1,008,201 nodes (200 directories × 40 subdirectories × 100 files, plus one
flat directory with 200,000 files), 200×60 terminal:

| operation | time per op |
|---|---:|
| cold frame at root (sort children, LOD, layout incl. nested previews, paint) | 33.8 ms |
| resize to a new size (layout + paint, child lists cached) | 2.0 ms |
| arrow-key move + re-render | 1.5 ms |
| zoomed into the 200,000-entry directory, cold | 29.5 ms |
| render of a small tree (2.4 K nodes) at 200×60 | 0.66 ms |
| resize relayout of the small tree | 0.45 ms |

During a scan the size caches are invalidated at most every 400 ms, so a
cold frame (worst case above ~34 ms) costs under 10 % of one core; other
frames reuse caches. Resize events only record the new size; layout happens
once per rendered frame, so a resize storm cannot queue up work.

### Geometry

`go test ./internal/treemap -bench .`:

| operation | time |
|---|---:|
| squarified layout, 300 items | 89 µs |
| spatial neighbour lookup among 300 blocks | 2.1 µs |

### Filtering (1,008,201 nodes)

| query | time |
|---|---:|
| `size > 1MB` | 31 ms |
| `*.dat AND size > 500k` | 119 ms |
| `path contains sub3` | 112 ms |

Filters run on a background goroutine with cancellation and a 120 ms
keystroke debounce; the UI never waits for them.

## Known costs / future work

* Path predicates now build each path from the cached parent directory path
  (244 ms → 112 ms); globs are dominated by `filepath.Match`.
* Node records could shrink (owner/group/mode/link count are only needed for
  display and some filters) if memory on 10 M+ entry trees matters.
* Cold-cache HDD benchmarks still need to be run with root access.
