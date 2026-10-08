# Performance

All numbers below were measured, not estimated. Re-run them on your own
hardware before relying on them; filesystem caches dominate scan timings.

## Test machine (2026-10-08)

| | |
|---|---|
| CPU | Intel Core i7-7700 @ 3.60 GHz, 4 cores / 8 threads |
| RAM | 32 GiB |
| OS | Linux 7.0, Rust 1.91.1 (release profile: LTO, one codegen unit) |
| Filesystem | ext4 on Linux software RAID (`/dev/md2`) over two 7200 rpm SATA HDDs |

The host is shared; its load average was 5–9 during these runs. Each figure
is the median (and best) of 7 runs.

## Scanner

Dataset: `~/go/pkg/mod`, a large module cache: 388,307 files + 65,518
directories (458,510 inventory nodes), 10.5 GiB allocated. **Warm cache
only**: dropping the page cache needs root, so these measure CPU and syscall
cost, not disk seeks. Cold-cache behaviour on HDDs will be dominated by seek
time; expect fewer workers to be optimal there.

`mapsize --no-ui --workers N PATH`:

| workers | median | best | peak RSS |
|---:|---:|---:|---:|
| 1  | 1.52 s | 1.44 s | 75 MB |
| 8 (balanced default) | 0.47 s | 0.43 s | 76 MB |
| 32 | 0.46 s | 0.43 s | 80 MB |

At 8 workers that is about 1,000,000 entries a second, in under 80 MB for
458k nodes. Most of the time is spent in the `statx` and `getdents` system
calls.

| other runs over the same tree | median | peak RSS |
|---|---:|---:|
| scan + `--largest-files 50` | 0.60 s | 76 MB |
| scan + `--json` (streamed to /dev/null) | 0.86 s | 76 MB |

## Snapshots

| operation | median | peak RSS |
|---|---:|---:|
| scan + save | 0.75 s | 77 MB |
| load (`mapsize --no-ui file.msz`) | 0.23 s | 78 MB |
| load a snapshot written by mapsize 1.2.0 | 0.26 s | 78 MB |

Loading decompresses and verifies SHA-256 on one thread while another parses.
Against a warm cache that is about twice as fast as rescanning; against a
cold cache or a network filesystem, where scans are dominated by I/O
latency, the gain is far larger (not yet measured here).

## UI

`make benchmark` (`examples/bench.rs`), best of 5 runs, on a synthetic
in-memory tree of 1,008,201 nodes (200 directories × 40 subdirectories × 100
files, plus one flat directory with 200,000 files), 200×60 terminal:

| operation | time per op |
|---|---:|
| cold frame at root (sort children, LOD, layout incl. nested previews, paint) | 11.34 ms |
| resize to a new size (layout + paint, child lists cached) | 1.50 ms |
| arrow-key move + re-render | 1.31 ms |
| zoomed into the 200,000-entry directory, cold | 9.37 ms |

During a scan the size caches are invalidated at most every 400 ms, so even
a cold frame costs a few percent of one core. Resize events only record the
new size; layout happens once per rendered frame, so a resize storm cannot
queue up work.

### Geometry

| operation | time |
|---|---:|
| squarified layout, 300 items | 8.4 µs |
| spatial neighbour lookup among 300 blocks | 1.1 µs |

### Filtering (1,008,201 nodes)

| query | time |
|---|---:|
| `size > 1MB` | 26.75 ms |
| `*.dat AND size > 500k` | 67.84 ms |
| `path contains sub3` | 63.80 ms |

Filters run on a background thread with cancellation and a 120 ms keystroke
debounce; the UI never waits for them. Path predicates build each path from
the cached parent directory path into a reused buffer, and `*.ext` globs are
a suffix test.

## Compared with mapsize 1.2.0

mapsize 1.2.0 was the last release of the previous implementation, written
in Go. Both were run alternately on the same machine, over the same tree, in
the same session:

| | 1.2.0 | current | |
|---|---:|---:|---|
| scan, 8 workers (median) | 0.63 s | 0.47 s | 1.34× faster |
| scan, 1 worker (median) | 1.89 s | 1.52 s | 1.24× faster |
| peak RSS while scanning | 117 MB | 76 MB | |
| scan + save snapshot | 1.22 s | 0.75 s | 1.63× faster |
| load snapshot | 0.34 s | 0.23 s | 1.48× faster |
| scan + JSON export | 1.27 s, 153 MB | 0.86 s, 76 MB | 1.48× faster |
| cold frame, 1M nodes | 36.01 ms | 11.34 ms | 3.18× faster |
| squarified layout, 300 items | 84.3 µs | 8.4 µs | 10.07× faster |
| arrow key + re-render | 1.50 ms | 1.31 ms | 1.14× faster |
| filter `*.dat AND size > 500k` | 121.40 ms | 67.84 ms | 1.79× faster |

## Known costs / future work

* Node records could shrink (owner/group/mode/link count are only needed for
  display and some filters) if memory on 10 M+ entry trees matters.
* Cold-cache HDD benchmarks still need to be run with root access.
