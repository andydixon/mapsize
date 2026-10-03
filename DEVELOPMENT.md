# Development

Requires Go (see `go.mod`). No CGO, no external tools.

```sh
make build      # bin/mapsize, bin/treegen
make test       # go test ./...
make race       # go test -race ./...
make vet fmt-check
make benchmark  # go test -bench
make fuzz       # short fuzz runs of snapshot / treemap / filter
make release    # release archives and packages in bin/release
```

## Releasing

Example for 1.2.0; substitute the real version throughout.

1. Update the version in `internal/brand/brand.go` and `docs/mapsize.1`,
   and add a `## 1.2.0` section to CHANGELOG.md. Commit.
2. Tag and push:
   ```sh
   git tag -a v1.2.0 -m "mapsize 1.2.0"
   git push origin master v1.2.0
   ```
3. Build everything (archives, .deb/.rpm/.apk/Arch packages, SHA256SUMS
   into `bin/release`; the first run downloads `nfpm`):
   ```sh
   make release VERSION=v1.2.0
   ```
4. Publish the GitHub release with the changelog section as notes:
   ```sh
   awk '/^## 1.2.0/{f=1;next} /^## /{f=0} f' CHANGELOG.md > /tmp/notes.md
   gh release create v1.2.0 --title v1.2.0 --notes-file /tmp/notes.md bin/release/*
   ```
5. Update Homebrew. This needs the tag pushed (step 2), because it
   checksums GitHub's source tarball for that tag:
   ```sh
   make brew-formula VERSION=v1.2.0      # writes bin/homebrew/mapsize.rb
   git clone https://github.com/andydixon/homebrew-tap /tmp/tap
   cp bin/homebrew/mapsize.rb /tmp/tap/Formula/mapsize.rb
   git -C /tmp/tap commit -am "mapsize 1.2.0" && git -C /tmp/tap push
   ```
   Users get it with `brew upgrade mapsize`. Template:
   `packaging/homebrew/mapsize.rb.in`.
6. Publish to the apt, dnf and pacman repository at repo.dixon.cx. This
   builds signed .deb, .rpm and Arch packages from the tag and rebuilds the
   repository indexes (needs docker, gpg, apt-ftparchive and the signing key
   in `~/.config/repo.dixon.cx/gnupg`):
   ```sh
   packaging/repo.sh v1.2.0
   ```
   The same script, with its own project block, publishes vault.

Always pass `VERSION=`: otherwise it comes from `git describe`, which only
matches the tag when HEAD is exactly the tagged commit. `make release` and
the Homebrew formula stamp the version into the binary; `make build` and
`go install` use the value in `brand.go`, which is why step 1 updates it.

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
