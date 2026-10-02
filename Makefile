BIN     := bin
PKG     := ./...
VERSION ?= $(shell git describe --tags --always --dirty 2>/dev/null || echo dev)
LDFLAGS := -s -w

.PHONY: build test race vet fmt fmt-check benchmark fuzz release clean treegen

build:
	go build -trimpath -ldflags '$(LDFLAGS)' -o $(BIN)/mapsize ./cmd/mapsize
	go build -trimpath -ldflags '$(LDFLAGS)' -o $(BIN)/treegen ./cmd/treegen

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

release:
	@for t in linux/amd64 linux/arm64 darwin/amd64 darwin/arm64 windows/amd64 windows/arm64; do \
		os=$${t%/*}; arch=$${t#*/}; ext=; [ $$os = windows ] && ext=.exe; \
		echo "$$os/$$arch"; \
		CGO_ENABLED=0 GOOS=$$os GOARCH=$$arch go build -trimpath -ldflags '$(LDFLAGS)' \
			-o $(BIN)/release/mapsize-$(VERSION)-$$os-$$arch$$ext ./cmd/mapsize || exit 1; \
	done

clean:
	rm -rf $(BIN)
