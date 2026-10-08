# Roadmap

Status markers: `[x]` done, `[~]` partial, `[ ]` not started.

## 0.1 — inventory engine
- [x] project skeleton, Makefile, CI
- [x] portable filesystem scanner, bounded worker pool, no-deadlock scheduler
- [x] inventory node model, incremental aggregation
- [x] cancellation, progress counters, error accounting
- [x] CLI: `--top`, `--json`, `--csv`, `--no-ui`

## 0.2 — TUI shell
- [x] full-screen terminal app, canvas renderer, frame cache
- [x] responsive layout (large / medium / small / too-small)
- [x] resize handling, status bar, progressive display, directory list

## 0.3 — treemap
- [x] squarified layout, edge-snapped cell rectangles
- [x] renderer with borders, nested preview, adaptive labels
- [x] level-of-detail grouping, category colouring

## 0.4 — interaction
- [x] spatial arrow navigation, stable NodeID selection
- [x] info modal (scrollable, resize-aware), zoom, breadcrumbs, mouse

## 0.5 — investigation
- [x] search & filter language (AND/OR/NOT, parentheses, units, ages)
- [x] extension statistics, largest files / dirs, error view

## 0.6 — snapshots
- [x] versioned binary snapshot format, save/load, comparison, growth treemap

## 0.7 — duplicates
- [x] staged duplicate detection (size → sampled hash → SHA-256), view

## 0.8 — operations
- [x] read-only mode, JSON/CSV export
- [x] move-to-trash: freedesktop trash (tested on Linux); macOS ~/.Trash and Windows Recycle Bin implemented but not yet tested on those systems

## 0.9 — hardening
- [~] profiling & benchmarks (warm-cache only so far) (see PERFORMANCE.md)
- [ ] memory reduction (name arena) if profiles justify it
- [ ] Windows allocated-size via GetCompressedFileSizeW (opt-in)
- [ ] configurable keybindings
- [ ] cold-cache HDD scan benchmarks (need root to drop caches)
- [x] incremental path-predicate evaluation for filters
- [ ] permanent deletion (deliberately not offered yet; trash only)

## 1.0 (released 2026-10-02)
- [x] stable snapshot format v1 guarantee
- [x] packaged releases (`make release`: archives, deb/rpm/apk/Arch, checksums)
- [x] manual page

## After 1.0
Unchecked items above carry over; candidates for 1.1 are cold-cache
benchmarks, configurable keybindings and testing trash/reveal on macOS and
Windows.
