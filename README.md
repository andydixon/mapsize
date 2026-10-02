# mapsize

An interactive, full-terminal disk usage analyser built around a live,
navigable treemap. Think WinDirStat + ncdu + WizTree, in one Go binary that
runs over SSH.

> `mapsize` is a working name; branding lives in `internal/brand`.

## Status

Under active development — see [ROADMAP.md](ROADMAP.md) for what works today.

## Install

```sh
go install github.com/andydixon/mapsize/cmd/mapsize@latest
# or
make build   # → bin/mapsize
```

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

### Read-only mode

`--read-only` disables every modifying action and shows `READ ONLY` in the
status bar. It does not make mapsize a forensic acquisition tool: scanning
uses ordinary filesystem APIs, and duplicate detection reads file content
(which may update access times).

## Documentation

* [ARCHITECTURE.md](ARCHITECTURE.md) — design and invariants
* [DEVELOPMENT.md](DEVELOPMENT.md) — building, testing, profiling
* [PERFORMANCE.md](PERFORMANCE.md) — measured results
