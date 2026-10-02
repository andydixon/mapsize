# Roadmap

Status markers: `[x]` done, `[~]` partial, `[ ]` not started.

## 0.1 — inventory engine
- [ ] Go project skeleton, Makefile, CI
- [ ] portable filesystem scanner, bounded worker pool, no-deadlock scheduler
- [ ] inventory node model, incremental aggregation
- [ ] cancellation, progress counters, error accounting
- [ ] CLI: `--top`, `--json`, `--csv`, `--no-ui`

## 0.2 — TUI shell
- [ ] full-screen Bubble Tea v2 app, canvas renderer, frame cache
- [ ] responsive layout (large / medium / small / too-small)
- [ ] resize handling, status bar, progressive display, directory list

## 0.3 — treemap
- [ ] squarified layout, edge-snapped cell rectangles
- [ ] renderer with borders, nested preview, adaptive labels
- [ ] level-of-detail grouping, category colouring

## 0.4 — interaction
- [ ] spatial arrow navigation, stable NodeID selection
- [ ] info modal (scrollable, resize-aware), zoom, breadcrumbs, mouse

## 0.5 — investigation
- [ ] search & filter language (AND/OR/NOT, parentheses, units, ages)
- [ ] extension statistics, largest files / dirs, error view

## 0.6 — snapshots
- [ ] versioned binary snapshot format, save/load, comparison, growth treemap

## 0.7 — duplicates
- [ ] staged duplicate detection (size → sampled hash → SHA-256), view

## 0.8 — operations
- [ ] read-only mode, JSON/CSV export
- [ ] move-to-trash (freedesktop, macOS ~/.Trash, Windows recycle bin)

## 0.9 — hardening (ongoing)
- [ ] profiling & benchmarks (see PERFORMANCE.md)
- [ ] memory reduction (name arena) if profiles justify it
- [ ] Windows allocated-size via GetCompressedFileSizeW (opt-in)
- [ ] configurable keybindings

## 1.0
- [ ] stable snapshot format v1 guarantee, packaged releases
