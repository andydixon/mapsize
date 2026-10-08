# Development

Requires a stable Rust toolchain (`rustup`); the dependencies are pinned in
`Cargo.lock`. No C toolchain beyond what Rust itself needs.

```sh
make build       # target/release/mapsize
make test        # cargo test --release
make lint        # clippy, warnings are errors
make fmt-check   # rustfmt
make benchmark   # in-process benchmarks (layout, render, filter): examples/bench.rs
make treegen     # synthetic test-tree generator: examples/treegen.rs
make release     # local release assets in dist/release (see below)
```

CI (`.github/workflows/ci.yml`) runs rustfmt and clippy, the tests on Linux
(x86-64 and ARM64), macOS (Intel and Apple silicon) and Windows, and
cross-builds every release target, running the ARM and RISC-V Linux
binaries under qemu.

## Platforms

`scripts/build.sh` builds `dist/mapsize-OS-ARCH`:

| OS | Architectures | Rust target | Built with |
|---|---|---|---|
| Linux | amd64, arm64, 386, arm (v7), riscv64 | `*-unknown-linux-musl` (static) | cargo-zigbuild + zig |
| Windows | amd64, arm64 | `x86_64-pc-windows-gnu`, `aarch64-pc-windows-gnullvm` | cargo-zigbuild + zig |
| macOS | amd64, arm64 | `*-apple-darwin` | cargo on a Mac (or zigbuild) |
| FreeBSD, NetBSD | amd64 | `x86_64-unknown-{freebsd,netbsd}` | cross + Docker |

Platform differences live in `src/platform.rs` (and `cfg` blocks where the
terminal code needs them); nothing else switches on the OS.

## Releasing

Example for 1.2.0; substitute the real version throughout.

1. Update the version in `Cargo.toml` (then `cargo check` to refresh
   `Cargo.lock`) and `docs/mapsize.1`, and turn `## Unreleased` in
   CHANGELOG.md into `## 1.2.0 — DATE`. Commit.
2. Tag and push:
   ```sh
   git tag -a v1.2.0 -m "mapsize 1.2.0"
   git push origin master v1.2.0
   ```
3. The tag starts `.github/workflows/release.yml`, which builds every
   target, packages them (archives, .deb/.rpm/.apk/Arch packages,
   SHA256SUMS) and publishes the GitHub release with the changelog section
   as its notes. Watch it with `gh run watch`. `make release` builds the
   same assets locally into `dist/release`, for whichever targets the host
   can build (`scripts/build.sh`, then `scripts/release.sh`).
4. Check the release page once the workflow has finished.
5. Update Homebrew. This needs the tag pushed (step 2), because it
   checksums GitHub's source tarball for that tag:
   ```sh
   make brew-formula VERSION=v1.2.0      # writes dist/homebrew/mapsize.rb
   git clone https://github.com/andydixon/homebrew-tap /tmp/tap
   cp dist/homebrew/mapsize.rb /tmp/tap/Formula/mapsize.rb
   git -C /tmp/tap commit -am "mapsize 1.2.0" && git -C /tmp/tap push
   ```
   Users get it with `brew upgrade mapsize`. Template:
   `packaging/homebrew/mapsize.rb.in`.
6. Publish to the apt, dnf and pacman repository at repo.dixon.cx. This
   builds signed .deb, .rpm and Arch packages from the tag and rebuilds the
   repository indexes (needs cargo-zigbuild and zig, docker, gpg,
   apt-ftparchive and the signing key in `~/.config/repo.dixon.cx/gnupg`):
   ```sh
   packaging/repo.sh v1.2.0
   ```
   The same script, with its own project block, publishes vault.

The binary reports the version in `Cargo.toml`; `scripts/release.sh`
refuses to package a tag that does not match it.

The snapshot format is stable at version 1: a format change must bump
`snapshot.Version` and keep the reader for version 1.

## Test data

`treegen` creates synthetic trees:

```sh
cargo run --release --example treegen -- --out /tmp/t --directories 50000 --files 1000000 --max-depth 20 --sparse --unicode
target/release/mapsize /tmp/t
```

## Benchmarks

```sh
make benchmark                          # in-process: layout, render, navigation, filters
scripts/bench-scan.sh /path 1 2 4 8     # wall time and peak RSS per worker count
sudo DROP_CACHES=1 scripts/bench-scan.sh /path   # cold-cache runs
```

Paste results into PERFORMANCE.md with the machine description. Never edit
numbers by hand.

## Terminal restoration checks

```sh
MAPSIZE_DEBUG_PANIC=ui target/release/mapsize .        # panics on the first key press
MAPSIZE_DEBUG_PANIC=scanner target/release/mapsize /   # panics in a scanner thread (aborts)
stty -a                                     # echo/icanon must be back on
```

## Profiling

Build with symbols (`CARGO_PROFILE_RELEASE_STRIP=false
CARGO_PROFILE_RELEASE_DEBUG=1 cargo build --release`) and use the platform
profiler, for example `perf record -g target/release/mapsize --no-ui /path`
on Linux or Instruments on macOS. `examples/bench.rs` times the hot UI paths
in-process.

## Layout

See ARCHITECTURE.md. Rules of thumb:

* `treemap` and `filter` must not depend on `tui`.
* OS-specific code lives in `platform` (`cfg` attributes); the terminal code
  has the only other `cfg` blocks.
* All strings derived from filenames go through `textutil::sanitize` before
  reaching a terminal.
