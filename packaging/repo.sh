#!/usr/bin/env bash
# Build Linux packages (.deb, .rpm, Arch .pkg.tar.zst) for a release tag and
# publish them to the signed apt, dnf and pacman repositories served at
# https://repo.dixon.cx.
#
#   packaging/repo.sh          # the newest tag
#   packaging/repo.sh v1.0.0   # a given tag
#
# Builds from a clean export of the tag, so the working tree doesn't matter.
# Every index is rebuilt over all the packages in the repository, so other
# projects publishing there with their copy of this script are kept.
#
# Needs: cargo with cargo-zigbuild and zig (static musl cross-builds), curl
# (to fetch the pinned nfpm), docker, gpg, apt-ftparchive (apt-utils), flock.
#   REPO_DIR        where the repository lives (default below)
#   REPO_GNUPGHOME  GnuPG home holding the signing key (default below)
#   ARCHS           package architectures (default below)
#
# The same script, with its own project block, is in ../vault/packaging.
set -euo pipefail

# ---- project ---------------------------------------------------------------
NAME=mapsize         # the command
PACKAGE=mapsize
CONFLICTS=
SUMMARY="Interactive full-terminal disk usage analyser with a live treemap"
HOMEPAGE=https://github.com/andydixon/mapsize
LICENSE=GPL-3.0-or-later
MANPAGES=docs/*.1
DOCS="README.md CHANGELOG.md LICENSE"
# Package architecture -> Rust target (static musl binaries).
triple() {
	case $1 in
	amd64) echo x86_64-unknown-linux-musl ;;
	arm64) echo aarch64-unknown-linux-musl ;;
	arm) echo armv7-unknown-linux-musleabihf ;;
	386) echo i686-unknown-linux-musl ;;
	riscv64) echo riscv64gc-unknown-linux-musl ;;
	*) return 1 ;;
	esac
}
# build ARCH OUT: build the binary for ARCH to OUT, in the source tree.
build() {
	local t
	t=$(triple "$1") || die "no Rust target for $1"
	RUSTFLAGS="--remap-path-prefix=$PWD=$NAME" cargo zigbuild --quiet --release --locked --target "$t"
	install -D -m 0755 "target/$t/release/$NAME" "$2"
}
# ----------------------------------------------------------------------------

REPO_DIR=${REPO_DIR:-/home/andy/domains/repo.dixon.cx/public_html}
REPO_GNUPGHOME=${REPO_GNUPGHOME:-$HOME/.config/repo.dixon.cx/gnupg}
ARCHS=${ARCHS:-"amd64 arm64 arm 386 riscv64"}
NFPM="$(cd "$(dirname "$0")/.." && pwd)/scripts/nfpm.sh"
SUITE=stable
COMPONENT=main
ARCH_REPO=dixon # the pacman repository name: [dixon]

say() { echo "repo: $*" >&2; }
die() { say "$*"; exit 1; }

for tool in cargo cargo-zigbuild zig curl docker gpg apt-ftparchive flock; do
	command -v $tool >/dev/null || die "needs $tool"
done
[[ -d $REPO_DIR ]] || die "$REPO_DIR does not exist"
export GNUPGHOME=$REPO_GNUPGHOME
KEY=$(gpg --list-secret-keys --with-colons 2>/dev/null | awk -F: '/^fpr/{print $10; exit}')
[[ -n $KEY ]] || die "no signing key in $GNUPGHOME"

cd "$(git rev-parse --show-toplevel)"
TAG=${1:-$(git describe --tags --abbrev=0)}
git rev-parse -q --verify "refs/tags/$TAG" >/dev/null || die "no tag $TAG"
VERSION=${TAG#v}
export VERSION

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
git archive "$TAG" | tar -x -C "$WORK" --one-top-level=src
gpg --batch --armor --export-secret-keys "$KEY" >"$WORK/signing.asc"

# ---- build -----------------------------------------------------------------
say "building $PACKAGE $VERSION for $ARCHS"
mkdir -p "$WORK/out"
for arch in $ARCHS; do
	nfarch=$arch
	[[ $arch == arm ]] && nfarch=arm7
	(cd "$WORK/src" && build "$arch" "$WORK/bin-$arch/$NAME")
	{
		cat <<-EOF
		name: $PACKAGE
		arch: $nfarch
		platform: linux
		version: $VERSION
		release: 1
		maintainer: Andy Dixon <andy@dixon.cx>
		description: $SUMMARY
		homepage: $HOMEPAGE
		license: $LICENSE
		conflicts: [${CONFLICTS:-}]
		contents:
		  - src: $WORK/bin-$arch/$NAME
		    dst: /usr/bin/$NAME
		    file_info: {mode: 0755}
		EOF
		for m in $(cd "$WORK/src" && ls $MANPAGES); do
			printf '  - src: %s\n    dst: /usr/share/man/man1/%s\n    file_info: {mode: 0644}\n' \
				"$WORK/src/$m" "$(basename "$m")"
		done
		for d in $DOCS; do
			printf '  - src: %s\n    dst: /usr/share/doc/%s/%s\n    file_info: {mode: 0644}\n' \
				"$WORK/src/$d" "$PACKAGE" "$d"
		done
		printf 'rpm:\n  signature:\n    key_file: %s\n' "$WORK/signing.asc"
	} >"$WORK/nfpm-$arch.yaml"
	for fmt in deb rpm archlinux; do
		$NFPM pkg -f "$WORK/nfpm-$arch.yaml" -p $fmt -t "$WORK/out/" >/dev/null
	done
done

# ---- publish ---------------------------------------------------------------
# One publisher at a time: the indexes cover every project's packages.
exec 9>"$REPO_DIR/../.publish.lock"
flock 9
say "publishing to $REPO_DIR"
gpg --batch --armor --export "$KEY" >"$REPO_DIR/dixon.asc"
gpg --batch --export "$KEY" >"$REPO_DIR/dixon.gpg"
sign() { gpg --batch --yes --local-user "$KEY" "$@"; }

# apt: pool/main/<letter>/<name>/, indexed by apt-ftparchive.
pool=$REPO_DIR/apt/pool/$COMPONENT/${PACKAGE:0:1}/$PACKAGE
mkdir -p "$pool"
cp "$WORK"/out/*.deb "$pool/"
(
	cd "$REPO_DIR/apt"
	# Built beside the live suite and swapped in, so clients never see half.
	dists=dists/.$SUITE.new
	rm -rf "$dists"
	debarchs=$(find pool -name '*.deb' | sed -E 's/.*_([^_]+)\.deb$/\1/' | sort -u | tr '\n' ' ')
	for a in $debarchs; do
		d=$dists/$COMPONENT/binary-$a
		mkdir -p "$d"
		apt-ftparchive --arch "$a" packages pool >"$d/Packages"
		gzip -9kn "$d/Packages"
	done
	apt-ftparchive \
		-o APT::FTPArchive::Release::Origin=repo.dixon.cx \
		-o APT::FTPArchive::Release::Label=repo.dixon.cx \
		-o APT::FTPArchive::Release::Suite=$SUITE \
		-o APT::FTPArchive::Release::Codename=$SUITE \
		-o APT::FTPArchive::Release::Components=$COMPONENT \
		-o "APT::FTPArchive::Release::Architectures=${debarchs% }" \
		release "$dists" >"$WORK/Release"
	mv "$WORK/Release" "$dists/Release"
	sign --clearsign -o "$dists/InRelease" "$dists/Release"
	sign --armor --detach-sign -o "$dists/Release.gpg" "$dists/Release"
	rm -rf "dists/.$SUITE.old"
	[[ -d dists/$SUITE ]] && mv "dists/$SUITE" "dists/.$SUITE.old"
	mv "$dists" "dists/$SUITE"
	rm -rf "dists/.$SUITE.old"
)

# dnf: every architecture in one directory; dnf picks its own.
mkdir -p "$REPO_DIR/rpm"
cp "$WORK"/out/*.rpm "$REPO_DIR/rpm/"
docker image inspect repo-tools:fedora >/dev/null 2>&1 ||
	docker build -q -t repo-tools:fedora - >/dev/null <<-'EOF'
	FROM fedora:42
	RUN dnf -y install createrepo_c && dnf clean all
	EOF
docker run --rm --user "$(id -u):$(id -g)" -v "$REPO_DIR/rpm:/repo" repo-tools:fedora \
	createrepo_c --quiet --update --general-compress-type=gz /repo
sign --armor --detach-sign -o "$REPO_DIR/rpm/repodata/repomd.xml.asc" "$REPO_DIR/rpm/repodata/repomd.xml"
cat >"$REPO_DIR/rpm/dixon.repo" <<EOF
[dixon]
name=repo.dixon.cx
baseurl=https://repo.dixon.cx/rpm
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=https://repo.dixon.cx/dixon.asc
EOF

# pacman: one directory per architecture, each package signed, then
# repo-add (from an Arch container) and a signed database.
added=
for pkg in "$WORK"/out/*.pkg.tar.zst; do
	a=$(basename "$pkg" .pkg.tar.zst)
	a=${a##*-}
	mkdir -p "$REPO_DIR/arch/$a"
	cp "$pkg" "$REPO_DIR/arch/$a/"
	sign --detach-sign -o "$REPO_DIR/arch/$a/$(basename "$pkg").sig" "$pkg"
	added+=" $a"
done
for a in $(tr ' ' '\n' <<<"$added" | sort -u); do
	dir=$REPO_DIR/arch/$a
	# A fresh database of every package here; -p keeps the newest version.
	rm -f "$dir/$ARCH_REPO".{db,files}{,.tar.gz}{,.sig}
	docker run --rm --user "$(id -u):$(id -g)" -e HOME=/tmp -v "$dir:/repo" -w /repo archlinux:latest \
		sh -c "repo-add --quiet --prevent-downgrade $ARCH_REPO.db.tar.gz *.pkg.tar.zst" 2>&1 |
		{ grep -v "newer version\|^$" >&2 || true; }
	for db in $ARCH_REPO.db $ARCH_REPO.files; do
		sign --detach-sign -o "$dir/$db.tar.gz.sig" "$dir/$db.tar.gz"
		ln -sfn "$db.tar.gz.sig" "$dir/$db.sig"
	done
	rm -f "$dir"/*.old "$dir"/*.old.sig
done

say "published $PACKAGE $VERSION: $(cd "$WORK/out" && ls | wc -l) packages"
