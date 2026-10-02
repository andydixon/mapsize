# Development

Requires Go (see `go.mod`). No CGO, no external tools.

```sh
make build      # bin/mapsize, bin/treegen
make test       # go test ./...
make race       # go test -race ./...
make vet fmt-check
make benchmark  # go test -bench
make fuzz       # short fuzz runs of snapshot / treemap / filter
make release    # cross-compiled binaries in bin/release
```

## Releasing

```sh
git tag -a v1.0.0 -m "mapsize 1.0.0"
make release          # bin/release: archives, .deb/.rpm/.apk/Arch, SHA256SUMS
```

The version comes from `git describe` and is stamped into
`brand.Version`; untagged builds report the fallback in
`internal/brand/brand.go`. Packages need network access the first time
(`nfpm` is fetched with `go run`). Bump the version in `docs/mapsize.1` and
`brand.go` and add a CHANGELOG section before tagging.

### Homebrew

After pushing the tag:

```sh
make brew-formula     # → bin/homebrew/mapsize.rb with the tarball's sha256
```

Copy the result to `Formula/mapsize.rb` in the tap repository
`github.com/andydixon/homebrew-tap`; users then run
`brew install andydixon/tap/mapsize`. The formula builds from source with
Go, stamps the version and installs the man page. Template:
`packaging/homebrew/mapsize.rb.in`.

The snapshot format is stable at version 1: a format change must bump
`snapshot.Version` and keep the reader for version 1.

## Test data

`treegen` creates synthetic trees:

```sh
bin/treegen --out /tmp/t --directories 50000 --files 1000000 --max-depth 20 --sparse --unicode
bin/mapsize /tmp/t
```

## Benchmarks

```sh
make benchmark                          # Go micro-benchmarks (layout, render, filter, scan)
scripts/bench-scan.sh /path 1 2 4 8     # wall time and peak RSS per worker count
sudo DROP_CACHES=1 scripts/bench-scan.sh /path   # cold-cache runs
MAPSIZE_MEM_ROOT=/path go test ./internal/scan -run TestMemoryPerNode -v
```

Paste results into PERFORMANCE.md with the machine description. Never edit
numbers by hand.

## Terminal restoration checks

```sh
MAPSIZE_DEBUG_PANIC=ui bin/mapsize .        # panics on the first key press
MAPSIZE_DEBUG_PANIC=scanner bin/mapsize /   # panics in a scanner goroutine
stty -a                                     # echo/icanon must be back on
```

## Profiling

```sh
bin/mapsize --no-ui --cpuprofile cpu.prof --memprofile mem.prof /path
go tool pprof -http=: cpu.prof
```

`--log FILE --log-level debug` writes a log without disturbing the TUI.
Paths are logged only at `debug`/`trace` level.

## Layout

See ARCHITECTURE.md. Rules of thumb:

* `internal/treemap` and `internal/filter` must not import UI packages.
* Only `internal/platform` may contain OS-specific code (build tags).
* All strings derived from filenames go through `textutil.Sanitize` before
  reaching a terminal.
