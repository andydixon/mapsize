BIN     := bin
PKG     := ./...
VERSION ?= $(shell git describe --tags --always --dirty 2>/dev/null || echo dev)
LDFLAGS := -s -w
PREFIX  ?= /usr/local

.PHONY: build install man brew-formula test race vet fmt fmt-check benchmark fuzz release clean treegen

build:
	go build -trimpath -ldflags '$(LDFLAGS)' -o $(BIN)/mapsize ./cmd/mapsize
	go build -trimpath -ldflags '$(LDFLAGS)' -o $(BIN)/treegen ./cmd/treegen

install: build
	install -Dm755 $(BIN)/mapsize $(DESTDIR)$(PREFIX)/bin/mapsize
	install -Dm644 docs/mapsize.1 $(DESTDIR)$(PREFIX)/share/man/man1/mapsize.1

man:
	man -l docs/mapsize.1

test:
	go test $(PKG)

race:
	go test -race $(PKG)

vet:
	go vet $(PKG)

fmt:
	gofmt -w .

fmt-check:
	@test -z "$$(gofmt -l .)" || (gofmt -l . && echo 'run make fmt' && exit 1)

benchmark:
	go test -run=NONE -bench=. -benchmem $(PKG)

fuzz:
	go test -run=NONE -fuzz=FuzzLoad -fuzztime=30s ./internal/snapshot
	go test -run=NONE -fuzz=FuzzSquarify -fuzztime=30s ./internal/treemap
	go test -run=NONE -fuzz=FuzzParse -fuzztime=30s ./internal/filter

# Cross-built archives plus .deb/.rpm/.apk/Arch packages in $(BIN)/release.
REL     := $(BIN)/release
NFPM    := go run github.com/goreleaser/nfpm/v2/cmd/nfpm@v2.41.1
release:
	rm -rf $(REL) && mkdir -p $(REL)
	@set -e; v=$(VERSION); v=$${v#v}; \
	for t in linux/amd64 linux/arm64 linux/386 linux/arm darwin/amd64 darwin/arm64 \
	         windows/amd64 windows/arm64 freebsd/amd64 freebsd/arm64 openbsd/amd64 netbsd/amd64; do \
		os=$${t%/*}; arch=$${t#*/}; ext=; [ $$os = windows ] && ext=.exe; \
		name=mapsize-$$v-$$os-$$arch; d=$(REL)/$$name; echo "$$os/$$arch"; mkdir -p $$d; \
		CGO_ENABLED=0 GOOS=$$os GOARCH=$$arch GOARM=7 go build -trimpath \
			-ldflags '$(LDFLAGS) -X github.com/andydixon/mapsize/internal/brand.Version='$$v \
			-o $$d/mapsize$$ext ./cmd/mapsize; \
		cp README.md CHANGELOG.md LICENSE docs/mapsize.1 $$d/; \
		if [ $$os = windows ]; then (cd $(REL) && zip -qr $$name.zip $$name); \
		else tar -C $(REL) -czf $(REL)/$$name.tar.gz $$name; fi; \
		if [ $$os = linux ]; then mkdir -p $(REL)/pkg && cp $$d/mapsize $(REL)/pkg/; \
			a=$$arch; [ $$a = arm ] && a=arm7; \
			for f in deb rpm apk archlinux; do \
			VERSION=$$v ARCH=$$a $(NFPM) pkg -f nfpm.yaml -p $$f -t $(REL)/; done; rm -rf $(REL)/pkg; fi; \
		rm -rf $$d; \
	done; \
	cd $(REL) && sha256sum * > SHA256SUMS

# Homebrew formula for the tagged release. The tag must already be pushed:
# the checksum is of GitHub's source tarball for that tag.
BREW_REPO := https://github.com/andydixon/mapsize
brew-formula:
	@set -e; v=$(VERSION); v=$${v#v}; \
	case $$v in *-*) echo "brew-formula: HEAD is not exactly a release tag ($$v)"; exit 1;; esac; \
	url=$(BREW_REPO)/archive/refs/tags/v$$v.tar.gz; \
	sum=$$(curl -fsSL "$$url" | sha256sum | cut -d' ' -f1) || { echo "cannot fetch $$url (tag pushed?)"; exit 1; }; \
	mkdir -p $(BIN)/homebrew; \
	sed -e "s/@VERSION@/$$v/g" -e "s/@SHA256@/$$sum/" packaging/homebrew/mapsize.rb.in > $(BIN)/homebrew/mapsize.rb; \
	echo "wrote $(BIN)/homebrew/mapsize.rb (v$$v, sha256 $$sum)"

clean:
	rm -rf $(BIN)
