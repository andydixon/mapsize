# mapsize architecture

This document describes how mapsize is put together and the invariants each
part relies on. It is kept in sync with the code; if they disagree, the code
is wrong or this file is stale — fix one of them.

```
cmd/mapsize        CLI entry point, flag parsing, mode selection
cmd/treegen        synthetic test-tree generator
internal/brand     product name / file extensions (rename in one place)
internal/inventory flat node store, aggregation, indexes, categories
internal/scan      bounded concurrent scanner (controller + workers)
internal/platform  OS-specific metadata (build-tagged files)
internal/treemap   squarified layout, LOD, spatial navigation (pure geometry)
internal/filter    query lexer / parser / evaluator
internal/snapshot  versioned binary snapshot format + comparison
internal/duplicate staged duplicate detection
internal/export    JSON / CSV / text reports
internal/config    settings, themes, JSON config file
internal/tui       Bubble Tea model, canvas renderer, views, modals
internal/textutil  display sanitising, width-aware truncation, units
```

Dependency direction is strictly downward: `tui` → (`treemap`, `filter`,
`inventory`, `scan`, …); `scan` → (`inventory`, `platform`); `treemap` depends
on nothing but the standard library. The TUI framework (Bubble Tea v2) is only
imported by `internal/tui` and `cmd/mapsize`.

## Inventory representation

`inventory.Tree` is an append-only arena of `Node` records addressed by
`NodeID` (`uint32`). Nodes are stored in fixed-size chunks
(`[][]Node`, 65 536 nodes per chunk), so growing the arena never copies or
moves existing nodes and peak memory never doubles during growth.

A node holds: parent ID, first-child / next-sibling IDs (an intrusive child
list — no per-directory slices), name (the single path component, not the full
path), kind, flags, own logical size, own allocated size, mtime, mode,
uid/gid, link count, and aggregates (total logical, total allocated, file
count, directory count, error count). Directory-only data — per-category byte
totals used for colouring — lives in a side table indexed by a directory index,
so files do not pay for it.

Full paths are never stored; `Tree.Path(id)` walks parents. Extensions are
interned into a small table and stored as a `uint16`. All sizes are `int64`.

Node IDs are stable for the lifetime of a `Tree`: nodes are never removed or
reordered. A rescan produces a new `Tree`. This is what makes selection
persistence trivial (see below).

## Scanner worker architecture

```
               +------------------ controller goroutine ------------------+
               |  owns: Tree (write lock), pending FIFO of dir NodeIDs,   |
               |        hard-link table, visited-dir table               |
               +----------------------------------------------------------+
                  | jobs (bounded chan, cap = workers*2)   ^ results (bounded chan)
                  v                                        |
             worker 1 .. worker N  (fixed pool, N from --workers / mode)
```

* Workers do I/O only: open a directory, read entries in chunks of up to 1024,
  `lstat` each entry (relative to the directory fd where the platform allows),
  and send a `result` chunk back. Workers never touch the `Tree`.
* The controller is the only writer of the `Tree`. It applies result chunks,
  creates child nodes, assigns IDs, propagates aggregate deltas, and decides
  which subdirectories become new jobs.
* Worker count: `conservative` = 2, `balanced` = min(8, 2×CPU),
  `aggressive` = min(64, 8×CPU), or an explicit number. More is not always
  faster — on rotational media 2–4 is often best.

## Bounded directory scheduling (no deadlock under backpressure)

The classic deadlock is: workers block sending new work into a full queue
while the only consumer of that queue is also blocked. mapsize avoids it by
construction:

* Only the controller produces jobs. Workers only produce *results*.
* The controller's main loop is a single `select` that simultaneously offers
  the next pending job to the bounded `jobs` channel *and* receives from the
  bounded `results` channel. It never performs a blocking send without also
  being ready to receive. Therefore a worker blocked on `results <-` is always
  eventually served, and the controller is never blocked on `jobs <-` alone.
* Discovered-but-unscheduled directories wait in the controller's pending FIFO
  as 4-byte NodeIDs (paths are rebuilt at dispatch). Its size is bounded by the
  number of directory nodes already in the inventory, so it adds O(1) bytes per
  directory on top of memory we already spend.
* FIFO order means the scan is roughly breadth-first: every top-level directory
  starts growing early, which is what makes the live treemap informative.

Huge directories are streamed: a worker sends a result per 1024 entries, so a
directory with 5 M entries neither balloons worker memory nor stalls the UI.

Completion: the controller tracks `inflight` (dispatched directories whose
final chunk has not arrived). The scan is finished when `pending` is empty and
`inflight == 0`. Cancellation (`context.Context`) stops dispatch immediately;
workers check the context between chunks and between entries, and the
controller drains outstanding results so no goroutine leaks.

Stress tests (`internal/scan/scan_test.go`) run with 1 worker and job/result
channel capacity 1 over trees with thousands of directories to prove progress
under maximum backpressure.

## Concurrent aggregation strategy

Because the controller is the single writer, aggregation needs no atomics.
For each result chunk it sums the chunk's file sizes, allocated sizes, counts
and per-category bytes **once**, then walks the ancestor chain adding that one
delta. Cost is O(depth) per 1024-entry chunk rather than O(depth) per file —
for a 1 M-file tree at depth 20 that is ~20 K ancestor updates instead of
~20 M. There is no reconciliation pass: totals are exact at every moment for
the data seen so far.

The UI reads the tree under `Tree.RLock`; the controller applies batches of
results under `Tree.Lock`, holding it for at most a few milliseconds (it
releases the lock between batches). This is the only lock in the data path.

**Concurrency invariants**

1. Only the scan controller (or a snapshot loader before publication) mutates
   a `Tree`.
2. Every read of `Tree` node data from another goroutine holds `RLock`.
3. Nodes are never deleted; a `NodeID` once observed stays valid.
4. Workers share nothing mutable except the bounded channels and atomic
   progress counters.
5. No goroutine ever takes `RLock` while already holding it. Go's
   `RWMutex` blocks new readers once a writer waits, so a recursive read
   lock can deadlock against the scan controller. The TUI takes the lock
   once per entry point (`View`, `handleKey`, mouse handlers); helpers they
   call assume it is held. Background commands (filter, duplicates,
   snapshot save) take their own lock on their own goroutine.
6. After a scan finishes, the UI goroutine may become the writer (move to
   trash updates aggregates under `Lock`). Trash is refused while scanning.

## Hard links and sizes

* *Logical size* = `st_size`. *Allocated size* = `st_blocks × 512` on Unix.
  On Windows allocated size is reported as unknown (equal to logical, flagged)
  because getting it costs an extra handle-open per file.
* For regular files with link count > 1, the controller keys `(dev, inode)`.
  The first path seen contributes to directory aggregates; later paths are
  flagged `HardlinkDup` and contribute **zero** to aggregates (their own size
  is still shown). Which path is "first" depends on scan order and is not
  deterministic. This matches `du`'s default semantics.
* Windows does not expose link counts from directory enumeration, so hard
  links are not deduplicated there. This is documented, not hidden.

## Symlinks, mount points, exclusions

* `--follow-symlinks=none` (default): symlinks are leaf nodes counted at their
  own (tiny) size. `same-filesystem` / `all` follow directory symlinks; every
  descended directory's `(dev, inode)` is recorded and a directory already
  visited is never descended twice — loops terminate and nothing is counted
  twice. On platforms without inode identity (Windows, other portable
  fallbacks) loop detection is impossible, so symlinks are never followed
  there and scan info says so.
* A directory whose device differs from its parent is flagged `MountPoint`.
  With `--one-file-system` it is not descended. On Linux, virtual filesystems
  (proc, sysfs, cgroup, debugfs, …) are never descended — `/proc/kcore` would
  otherwise dominate any scan of `/` — and are reported in scan info.
* `--exclude PATTERN`: patterns containing a path separator match full paths;
  others match the entry name with glob semantics. Excluded entries are
  counted and the UI shows that exclusions are active.

## TUI event flow

```
scan controller ──(applies under Lock)──> Tree
       │ atomic progress counters
       ▼
tea.Tick (10 Hz while scanning) ──> tickMsg ──> Update marks frame dirty
key/mouse/resize ──> Update ──> state change ──> frame dirty
View(): if dirty → RLock tree → layout (if layout dirty) → paint canvas → string
```

There is never one message per file: the scanner does not talk to the UI at
all except for a single `scanDoneMsg`. The UI polls at a fixed rate.

`View()` is called by Bubble Tea after *every* `Update`. The model therefore
caches the rendered frame and only repaints when something visible changed.

Expensive per-directory work (sorting children by size) is cached per data
version. During a scan the version advances at most every 400 ms, so the
layout does not jitter at frame rate even though labels and counters update
at the 10 Hz refresh.

## Rendering: the canvas

All drawing goes to an in-memory `Canvas` of cells (grapheme, fg, bg, attrs).
Views and modals paint into the canvas; the canvas is serialised to a string
with minimal SGR changes. Bubble Tea's renderer diffs frames and downsamples
true-colour to the detected profile (256 / 16 / none), so the drawing code
uses RGB throughout. For 256-colour terminals the canvas quantizes colours
itself (redmean distance over the 6×6×6 cube and grey ramp, never the
palette-dependent first 16 entries): the stock conversion maps dark tints to
saturated olive/navy and destroys the shading. For 16 colours, dark shades
map to black, greys to the grey entries, and saturated colours keep their
hue, so shadows stay quiet and categories stay distinguishable. `--color` overrides
detection (useful over SSH, where `COLORTERM` is often not forwarded).
`NO_COLOR` selects the `mono` theme, which relies on attributes and glyphs
only. Selection never relies on colour alone: the selected rectangle uses a
double-line border, bold reversed title and a status-line description.

## Treemap algorithm

`treemap.Squarify` implements Bruls, Huizing & van Wijk's squarified layout
on float rectangles: items sorted by size descending are added to the current
row while the worst aspect ratio improves; then the row is laid out along the
shorter side. Float rectangles are snapped to the cell grid by rounding the
*edges* (not the widths), so neighbouring rectangles share edges exactly — no
gaps, no overlaps. Layout is deterministic (stable sort by size, then ID).

Nested levels are laid out recursively inside each block's interior (inside
its border) while the interior is large enough to be useful.

## Level-of-detail model

A 160×50 terminal has 8 000 cells; the tree may have millions of entries.
For each directory being laid out, children are reduced to the top K by size
using a bounded heap (O(n log K)). Children whose proportional area would be
smaller than a minimum useful block (a few cells), or beyond K, are merged
into one synthetic *group* block ("312 smaller items, 8.4 GiB"). Zooming into
a directory gives its children the whole screen, so they become individually
visible. Groups are selectable and inspectable.

## Spatial navigation algorithm

`treemap.Neighbour(rects, selected, dir)` is pure geometry:

1. Candidates must lie in the requested half-plane: for RIGHT, `c.X ≥
   sel.X + sel.W − tolerance` (with the candidate's centre to the right of the
   selected centre).
2. Each candidate is scored by the primary-axis gap (edge to edge) plus a
   penalty for the orthogonal offset. Candidates overlapping the selected
   rectangle's orthogonal span get the offset term set to zero and are
   preferred over any non-overlapping candidate.
3. Among overlapping candidates, the one whose overlap is largest relative to
   the source, then nearest to the source's centre line, wins.
4. Ties are broken by top-left position, then NodeID — fully deterministic.

## Terminal resize architecture

`tea.WindowSizeMsg` only records the new size and marks layout dirty. The next
`View()` recomputes panel geometry for the new size class (large / medium /
small / too-small), reruns the treemap layout for the new treemap rectangle
(never stretches old coordinates), and repaints. Because work happens only in
`View()` and the frame cache absorbs repeated calls, a storm of resize events
costs at most one layout per rendered frame. Modal geometry is derived from
the current size every frame, clamped to the screen, and becomes scrollable
when content does not fit.

## Selection persistence

Selection is a `NodeID` (or, for the synthetic LOD group, `-1 - parentID`).
Until the user moves the selection it follows the largest block, which
changes while a scan runs; after that it is sticky. After any relayout the renderer looks the ID up in the
new rectangle list. If it is absent (filtered out, collapsed into a group,
zoom changed) selection falls back to: the group containing it, then its
nearest visible ancestor, then the largest block.

## Modal architecture

A modal is a small interface: `Title()`, `Lines(width)`, `Footer()`, and
`HandleKey`. A generic frame renders any modal centred, clamped to the screen
(min 1-cell margin), with a scrollable viewport when content exceeds height.
The base screen is dimmed before the modal is painted. While a modal is open
it receives all keys; `Esc` always closes it.

## OS abstraction

`internal/platform` exposes `ReadDir(path, func(chunk []Entry) error)` and
`Meta` structs. Implementations:

* `readdir_linux.go`   — `getdents` via `os.File.ReadDir` + `statx` relative to
  the directory fd (one syscall per entry, gives birth time).
* `readdir_unix.go`    — darwin/BSD: `fstatat` relative to the directory fd.
* `readdir_windows.go` — `os.File.ReadDir` + `Lstat`; reparse points
  (junctions, symlinks) are never descended by default.
* `fs_*.go`            — virtual-filesystem detection, owner name lookup.

No other package switches on `runtime.GOOS`.

## Security handling for hostile filenames

Filenames and snapshot contents are untrusted.

* Every string that reaches the terminal passes through
  `textutil.Sanitize`, which replaces C0/C1 control characters (including
  ESC, CSI, BEL, DEL) with visible escapes (`\x1b`), replaces invalid UTF-8
  with U+FFFD, and neutralises bidi override characters. The canvas also
  refuses control characters as a second line of defence. Internal names keep
  the raw bytes.
* Text is clipped by display width (grapheme clusters, East-Asian wide
  characters) so labels never overflow their rectangle.
* Snapshot decoding validates magic, version, every length, every parent
  reference (a parent must precede its child, so cycles are impossible),
  depth (≤ 4096 levels, bounding recursion in exporters), per-node sizes
  (≤ 1 EiB) and node count (at most one node per byte of snapshot file, 1 Mi floor); caps the decompressed body at 64× the file size
  (decompression bombs fail fast); and verifies a SHA-256 trailer.
  Aggregates are recomputed with saturating arithmetic rather than trusted.
* No shell is ever invoked. "Open/reveal" and trash use direct `exec` with
  argument vectors or native APIs; paths are passed as single arguments.
* `--read-only` disables every mutating action in the UI.
