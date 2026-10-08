#!/usr/bin/env bash
# Build mapsize release binaries for the supported operating systems and
# CPU architectures.
#
#   scripts/build.sh                                   # every target this host can build, into dist/
#   TARGETS="linux/amd64 darwin/arm64" scripts/build.sh
#   OUT=/tmp/bins scripts/build.sh
#
# Binaries are named mapsize-OS-ARCH[.exe]: stripped release builds from the
# locked dependencies (Cargo.lock), with build paths remapped so the output
# does not depend on where the tree lives. Linux binaries are static (musl).
#
# How each target is built, and what the host needs:
#   zig    cargo-zigbuild and zig on PATH   (Linux, Windows)
#   mac    a Mac, or cargo-zigbuild and zig (macOS)
#   cross  `cross` and Docker               (FreeBSD, NetBSD)
# A requested target the host cannot build is reported, the others are still
# built, and the script exits non-zero. The release workflow
# (.github/workflows/release.yml) builds every target on GitHub's runners.
set -euo pipefail
cd "$(dirname "$0")/.."
root=$PWD

table() {
	# os/arch         Rust target                       method
	cat <<-'EOF2'
	linux/amd64       x86_64-unknown-linux-musl         zig
	linux/arm64       aarch64-unknown-linux-musl        zig
	linux/386         i686-unknown-linux-musl           zig
	linux/arm         armv7-unknown-linux-musleabihf    zig
	linux/riscv64     riscv64gc-unknown-linux-musl      zig
	windows/amd64     x86_64-pc-windows-gnu             zig
	windows/arm64     aarch64-pc-windows-gnullvm        zig
	darwin/amd64      x86_64-apple-darwin               mac
	darwin/arm64      aarch64-apple-darwin              mac
	freebsd/amd64     x86_64-unknown-freebsd            cross
	netbsd/amd64      x86_64-unknown-netbsd             cross
	EOF2
}

out=${OUT:-dist}
targets=${TARGETS:-$(table | awk '{print $1}')}
mkdir -p -- "$out"
out=$(cd -- "$out" && pwd -P)
case $out in
/ | "$(cd ~ && pwd -P)" | "$root") echo "refusing OUT=$out: use a dedicated directory" >&2; exit 2 ;;
esac
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$root=mapsize --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=cargo"
have() { command -v "$1" >/dev/null; }

status=0
for want in $targets; do
	row=$(table | awk -v t="$want" '$1 == t')
	[[ -n $row ]] || { echo "UNKNOWN $want" >&2; status=1; continue; }
	read -r _ triple method <<<"$row"
	ext=; [[ $want == windows/* ]] && ext=.exe
	name=mapsize-${want/\//-}$ext
	case $method in
	zig) have cargo-zigbuild && have zig && cmd=(cargo zigbuild) ||
		{ echo "SKIPPED $want: needs cargo-zigbuild and zig" >&2; status=1; continue; } ;;
	mac)
		if [[ $(uname -s) == Darwin ]]; then cmd=(cargo build)
		elif have cargo-zigbuild && have zig; then cmd=(cargo zigbuild)
		else echo "SKIPPED $want: needs a Mac, or cargo-zigbuild and zig" >&2; status=1; continue; fi ;;
	cross) have cross && have docker && cmd=(cross build) ||
		{ echo "SKIPPED $want: needs cross and docker" >&2; status=1; continue; } ;;
	esac
	if ! "${cmd[@]}" --quiet --release --locked --target "$triple"; then
		echo "FAILED $want ($triple)" >&2; status=1; continue
	fi
	cp -- "target/$triple/release/mapsize$ext" "$out/$name"
	if [[ $triple == *-musl* ]] && have file; then
		file -L "$out/$name" | grep -q 'static' || { echo "FAILED $want: not statically linked" >&2; status=1; continue; }
	fi
	echo "built  $name ($triple)"
done
exit $status
