# Changelog

## 1.1.0 — 2026-10-03

### Fixed
- Map: folders that contain a single subfolder (`mnt/raid/media/`) now
  collapse into one label instead of each taking a nesting level, so the
  map at `/` reaches the shows, seasons and individual files. Nesting goes as
  deep as the block sizes allow (previously a fixed 3 levels), and nested
  blocks show their size and percentage of their parent folder.

## 1.0.0 — 2026-10-02

First stable release. The snapshot format (version 1) is now stable: later
1.x releases will keep reading version 1 snapshots.

### Engine
- Bounded concurrent scanner with incremental aggregation, cancellation,
  hard-link and symlink-loop handling, error accounting.
- Non-interactive reports: `--top`, `--largest-files`, `--json`, `--csv`,
  `--duplicates`, `--no-ui`.
- Versioned, checksummed snapshot format; `--compare`.

### Interface
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

### Packaging and documentation
- `mapsize(1)` manual page; `make install` installs binary and man page.
- `make release`: archives for Linux, macOS, Windows and the BSDs, plus
  .deb, .rpm, .apk and Arch packages (with the man page) and SHA256SUMS.
- Version stamped into release builds.
- Homebrew formula template and `make brew-formula`.

### Hardening
- Snapshot loader rejects over-deep trees, decompression bombs and
  out-of-range sizes; aggregates saturate instead of overflowing.
- Exclusion patterns from snapshot metadata are sanitised in reports.
- Duplicate, trash and filter results belonging to a replaced tree (after
  a rescan) are discarded.
- Linux trash refuses symlinked or foreign trash directories on shared
  filesystems and never overwrites an earlier trashed item.
- Symlinks are not followed on platforms without inode identity; the
  unscanned-directory count and post-cancel errors are reported correctly.
- Path filters evaluate incrementally (2× faster on 1M entries).
