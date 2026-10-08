#!/usr/bin/env bash
# Turn the binaries from scripts/build.sh into release assets: one archive
# per binary (with README, CHANGELOG, LICENSE and the manual page), .deb,
# .rpm, .apk and Arch packages for the Linux binaries, and SHA256SUMS.
#
#   scripts/release.sh VERSION [BIN_DIR [OUT_DIR]]   # defaults: dist, dist/release
#
# VERSION (e.g. 1.3.0 or v1.3.0) must match Cargo.toml, which is what the
# binaries report.
set -euo pipefail
cd "$(dirname "$0")/.."
root=$PWD
version=${1:?usage: scripts/release.sh VERSION [BIN_DIR [OUT_DIR]]}
version=${version#v}
bins=$(cd "${2:-dist}" && pwd)
out=${3:-dist/release}
cargo_version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[[ $version == "$cargo_version" ]] || { echo "release: VERSION $version does not match Cargo.toml ($cargo_version)" >&2; exit 1; }
rm -rf -- "$out" && mkdir -p -- "$out"
out=$(cd "$out" && pwd)
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT

shopt -s nullglob
for bin in "$bins"/mapsize-*; do
	b=$(basename "$bin")
	osarch=${b#mapsize-}
	osarch=${osarch%.exe}
	os=${osarch%-*} arch=${osarch#*-}
	ext=; [[ $os == windows ]] && ext=.exe
	name=mapsize-$version-$os-$arch
	mkdir -p "$work/$name"
	install -m 0755 "$bin" "$work/$name/mapsize$ext"
	cp README.md CHANGELOG.md LICENSE docs/mapsize.1 "$work/$name/"
	if [[ $os == windows ]]; then (cd "$work" && zip -qr "$out/$name.zip" "$name")
	else tar -C "$work" -czf "$out/$name.tar.gz" "$name"; fi
	if [[ $os == linux ]]; then
		nfarch=$arch; [[ $arch == arm ]] && nfarch=arm7
		cat >"$work/nfpm-$arch.yaml" <<-EOF2
		name: mapsize
		arch: $nfarch
		platform: linux
		version: $version
		maintainer: Andy Dixon <andy@dixon.cx>
		description: Interactive full-terminal disk usage analyser with a live treemap.
		homepage: https://github.com/andydixon/mapsize
		license: GPL-3.0-or-later
		contents:
		  - src: $work/$name/mapsize
		    dst: /usr/bin/mapsize
		    file_info: {mode: 0755}
		  - src: $root/docs/mapsize.1
		    dst: /usr/share/man/man1/mapsize.1
		    file_info: {mode: 0644}
		EOF2
		for fmt in deb rpm apk archlinux; do
			scripts/nfpm.sh pkg -f "$work/nfpm-$arch.yaml" -p $fmt -t "$out/" >/dev/null
		done
	fi
done
(cd "$out" && sha256sum -- * >SHA256SUMS)
echo "$(($(ls "$out" | wc -l) - 1)) assets and SHA256SUMS in $out"
