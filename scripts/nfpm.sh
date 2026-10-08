#!/usr/bin/env bash
# Run the pinned nfpm packager, downloading and checksumming it on first use
# (into target/tools). Linux x86-64 and ARM64 hosts only.
set -euo pipefail
version=2.41.1
case $(uname -m) in
x86_64) arch=x86_64 sum=b3cf95aa6dabed836d09ad7f0c190a13c74c5b1304db60846f0f702ee407f430 ;;
aarch64) arch=arm64 sum=17350a838c8e2c422c6e573ed379b18424565d2de8a2b1cb1b20211976124eb5 ;;
*) echo "nfpm.sh: unsupported host $(uname -m)" >&2; exit 1 ;;
esac
dir=$(cd "$(dirname "$0")/.." && pwd)/target/tools/nfpm-$version
if [[ ! -x $dir/nfpm ]]; then
	mkdir -p "$dir"
	tgz=$dir/nfpm.tar.gz
	curl -fsSL -o "$tgz" "https://github.com/goreleaser/nfpm/releases/download/v$version/nfpm_${version}_Linux_$arch.tar.gz"
	echo "$sum  $tgz" | sha256sum -c --quiet
	tar -xzf "$tgz" -C "$dir" nfpm
	rm -f "$tgz"
fi
exec "$dir/nfpm" "$@"
