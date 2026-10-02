# mapsize

An interactive, full-terminal disk usage analyser built around a live,
navigable treemap. Think WinDirStat + ncdu + WizTree, in one Go binary that
runs over SSH.

> `mapsize` is a working name; branding lives in `internal/brand`.

## Status

Version 1.0. See [CHANGELOG.md](CHANGELOG.md), [ROADMAP.md](ROADMAP.md) for
what comes next, and [PERFORMANCE.md](PERFORMANCE.md) for measured numbers.
Tested on Linux; macOS and Windows builds are produced but trash and
file-manager integration have not yet been tested there.

## What it does

* **Live treemap.** A squarified, space-filling treemap fills the terminal
  and grows while the scan runs. Directories show nested previews of their
  contents; colours are file categories (video, archives, disk images, code,
  …). Items too small to draw collapse into a "N smaller items" block until
  you zoom in.
* **Spatial navigation.** Arrow keys move to the block that is actually to
  the left/right/above/below. Enter inspects, Space zooms in, Backspace zooms
  out, Home returns to the root. The selection survives rescans, resizes and
  filtering.
* **Resizes properly.** Every resize recomputes the layout for the new
  geometry (it never stretches the old one); large terminals get a side
  panel, small ones a compact layout, tiny ones a polite message.
* **Search and filter** with a small query language — the treemap shows only
  matching data:
  `ubuntu` · `*.iso` · `size > 5GB` · `ext IN (iso,qcow2,vmdk)` ·
  `age > 365d` · `path contains cache AND NOT type = dir` · `owner = andy`
* **Investigation views** (Tab): directory list, file types, top lists
  (largest files/dirs, oldest/newest large files, most entries, sparse and
  hard-linked files), scan information with every error, duplicates.
* **Honest numbers.** Allocated vs logical size, hard links counted once,
  sparse files flagged, permission errors and skipped areas always shown;
  incomplete totals are labelled as such.
* **Snapshots**: save a scan, reopen it instantly, compare two — the treemap
  colours growth and shrinkage.
* **Safe with hostile filenames**: control characters and escape sequences
  in names are displayed escaped and never reach the terminal.

## Keys

| Key | Action |
|---|---|
| ↑ ↓ ← → / hjkl | move spatially through the map |
| Enter | item information |
| Space / → | zoom into directory |
| Backspace / ← | zoom out |
| Home | scan root |
| / | search / filter (Esc clears) |
| Tab, Shift+Tab, 1–7 | switch views |
| x, g, e | file types, top lists, scan info & errors |
| D | find duplicates |
| a | toggle allocated / logical size |
| c, o | copy path, reveal in file manager |
| d | move to trash (asks; disabled with `--read-only`) |
| s, r | save snapshot, rescan |
| T | cycle theme |
| ? | help |
| Ctrl+C | cancel scan, then quit |
| q | quit |

Mouse: click selects, double-click zooms, right-click inspects, wheel zooms
the map or scrolls lists. Everything works without a mouse (`--no-mouse`).

## Install

```sh
go install github.com/andydixon/mapsize/cmd/mapsize@v1.0.0
# or
make build   # → bin/mapsize
sudo make install   # binary + man page under /usr/local (PREFIX=… to change)

# Homebrew (macOS and Linux)
brew install andydixon/tap/mapsize
```

Full reference: `man mapsize` after installing, or `make man` /
`man -l docs/mapsize.1` from the source tree.

## Usage

```sh
mapsize .                       # scan and explore interactively
mapsize --read-only /mnt/evidence
mapsize /srv --save srv.msz     # explore, and save a snapshot when done
mapsize srv.msz                 # reopen a snapshot (no rescan)
mapsize --compare old.msz new.msz

# non-interactive
mapsize /srv --top 50
mapsize /srv --largest-files 20
mapsize /srv --json --depth 3
mapsize /srv --csv > inventory.csv
mapsize /srv --no-ui --save snapshot.msz
mapsize /srv --duplicates --no-ui
```

### Sizes and units

* By default sizes are **allocated** (disk usage, like `du`); `--apparent`
  switches to logical size. Both are shown in the info modal.
* Units are **IEC** (KiB, MiB, GiB — powers of 1024) everywhere; `--si`
  switches everything to SI (kB, MB, GB — powers of 1000). Never mixed.
* Hard links are counted once (first path seen); see ARCHITECTURE.md.

### Filesystem boundaries and exclusions

* Default: other filesystems *are* descended (like `du`), except Linux
  virtual filesystems (proc, sysfs, cgroup, …) which are never scanned.
  Mount points are marked.
* `--one-file-system` / `-x`: stay on the root's filesystem.
* `--follow-symlinks=none|same-filesystem|all` (default `none`). Loops are
  detected and nothing is counted twice.
* `--exclude PATTERN` (repeatable): glob on entry name, or a path prefix if
  the pattern contains a separator. Exclusions are always shown in the UI.

### Configuration

Optional JSON file at `$XDG_CONFIG_HOME/mapsize/config.json`
(macOS: `~/Library/Application Support/mapsize/config.json`, Windows:
`%AppData%\mapsize\config.json`):

```json
{
  "workers": "balanced",
  "theme": "default",
  "units": "iec",
  "excludes": ["/proc", "*.tmp"],
  "category_colors": {"Video": "#e06c75"},
  "extension_colors": {"qcow2": "#c678dd"}
}
```

### Colour

Colour support is auto-detected; `--color truecolor|256|16|none` overrides
it (handy over SSH, where `COLORTERM` is often not forwarded). Themes:
`default`, `dark`, `high-contrast`, `mono` (`--theme`, or `T` to cycle).
`NO_COLOR` is respected. On the Linux console, or with `MAPSIZE_ASCII=1`,
box drawing falls back to ASCII.

### Read-only mode

`--read-only` disables every modifying action and shows `READ ONLY` in the
header. (Saving a snapshot file is still allowed; it writes only where you
tell it to.) It does not make mapsize a forensic acquisition tool: scanning
uses ordinary filesystem APIs, and duplicate detection reads file content
(which may update access times).

### Deletion

`d` moves the selected item to the system trash (freedesktop.org trash on
Linux, `~/.Trash` on macOS, the Recycle Bin on Windows) after confirmation,
using native file operations — never a shell. Permanent deletion is not
offered. Trash is unavailable while scanning, for snapshots, and in
read-only mode.

## Documentation

* [docs/mapsize.1](docs/mapsize.1) — manual page
* [ARCHITECTURE.md](ARCHITECTURE.md) — design and invariants
* [DEVELOPMENT.md](DEVELOPMENT.md) — building, testing, profiling
* [PERFORMANCE.md](PERFORMANCE.md) — measured results

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
