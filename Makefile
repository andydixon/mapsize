PREFIX  ?= /usr/local
VERSION ?= $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

.PHONY: build install man test lint fmt fmt-check benchmark treegen release brew-formula clean

build:
	cargo build --release --locked

install: build
	install -Dm755 target/release/mapsize $(DESTDIR)$(PREFIX)/bin/mapsize
	install -Dm644 docs/mapsize.1 $(DESTDIR)$(PREFIX)/share/man/man1/mapsize.1

man:
	man -l docs/mapsize.1

test:
	cargo test --release --locked

lint:
	cargo clippy --locked --all-targets -- -D warnings

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

benchmark:
	cargo run --release --locked --example bench

treegen:
	cargo build --release --locked --example treegen

# Binaries for every target this host can build (see scripts/build.sh), then
# archives, Linux packages and SHA256SUMS in dist/release. Tagged releases are
# built on GitHub's runners by .github/workflows/release.yml instead.
release:
	scripts/build.sh || true
	scripts/release.sh $(VERSION) dist dist/release

# Homebrew formula for the tagged release. The tag must already be pushed:
# the checksum is of GitHub's source tarball for that tag.
BREW_REPO := https://github.com/andydixon/mapsize
brew-formula:
	@set -e; v=$(VERSION); v=$${v#v}; \
	url=$(BREW_REPO)/archive/refs/tags/v$$v.tar.gz; \
	sum=$$(curl -fsSL "$$url" | sha256sum | cut -d' ' -f1) || { echo "cannot fetch $$url (tag pushed?)"; exit 1; }; \
	mkdir -p dist/homebrew; \
	sed -e '/^#/d' -e "s/@VERSION@/$$v/g" -e "s/@SHA256@/$$sum/" packaging/homebrew/mapsize.rb.in > dist/homebrew/mapsize.rb; \
	echo "wrote dist/homebrew/mapsize.rb (v$$v, sha256 $$sum)"

clean:
	cargo clean
	rm -rf dist
