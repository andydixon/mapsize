# Changelog

## Unreleased

### Changed
- Rewritten in Rust. Flags, keys, views, themes, the configuration file and
  the snapshot format are unchanged: snapshots from earlier releases open as
  before, and older releases open new ones. Measured against 1.2.0 on the
  same machine, scans are about 1.3× faster with a third less memory, cold
  treemap frames about 3× faster, and filters up to 1.8× faster (see
  PERFORMANCE.md).
- Release builds: Linux (x86-64, ARM64, 32-bit x86, ARMv7, RISC-V 64),
  macOS (Intel, Apple silicon), Windows (x86-64, ARM64), FreeBSD and NetBSD
  (x86-64), built and published by CI. OpenBSD and FreeBSD/ARM64 builds are
  no longer produced.
- `--log` and `--log-level` are accepted but no log is written yet;
  `--cpuprofile` and `--memprofile` are removed.
- Install from source with `cargo install`; the Homebrew formula builds with
  Rust.

### Fixed
- A search containing certain non-ASCII text (for example `voilà`) no longer
  hangs.

## 1.2.0 — 2026-10-03

### Changed
- On a headless Linux or BSD system (no `DISPLAY` or `WAYLAND_DISPLAY`), `d`
  deletes permanently after an "are you sure" confirmation instead of moving
  to a trash nobody empties, and `o` shows the folder in the map instead of
  trying to open a file manager. Desktop sessions, macOS and Windows still
  use the trash and the file manager.

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
