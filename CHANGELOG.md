# Changelog

## Unreleased
- Bounded concurrent scanner with incremental aggregation, cancellation,
  hard-link and symlink-loop handling, error accounting.
- Non-interactive reports: `--top`, `--largest-files`, `--json`, `--csv`,
  `--duplicates`, `--no-ui`.
- Versioned, checksummed snapshot format; `--compare`.
- Interactive TUI (Bubble Tea v2): live squarified treemap with nested
  previews and level-of-detail grouping, spatial arrow navigation, zoom,
  breadcrumbs, responsive layouts, info/help/confirm/save modals, mouse.
- Views: directory list, file types, top lists, scan info & errors,
  duplicates, snapshot changes.
- Search/filter language with live treemap filtering.
- Themes (default, dark, high-contrast, mono), `--color` override,
  perceptual 256-colour quantizer, NO_COLOR support.
- Move to trash (freedesktop / macOS / Windows), `--read-only`.
- `treegen` synthetic tree generator; benchmarks in PERFORMANCE.md.
