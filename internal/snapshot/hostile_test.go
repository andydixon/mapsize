package snapshot

import (
	"bytes"
	"compress/gzip"
	"crypto/sha256"
	"encoding/binary"
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/andydixon/mapsize/internal/brand"
	"github.com/andydixon/mapsize/internal/export"
	"github.com/andydixon/mapsize/internal/inventory"
)

// craft builds a well-formed (correctly checksummed) but hostile snapshot.
func craft(n int, meta string, rec func(i int, w *writer)) []byte {
	var out bytes.Buffer
	out.WriteString(brand.SnapshotMagic)
	out.Write(binary.LittleEndian.AppendUint16(nil, Version))
	out.Write(binary.LittleEndian.AppendUint16(nil, flagGzipped))
	gz, _ := gzip.NewWriterLevel(&out, gzip.BestCompression)
	h := sha256.New()
	w := &writer{w: io.MultiWriter(gz, h)}
	w.uvarint(uint64(len(meta)))
	w.bytes([]byte(meta))
	w.uvarint(1)
	w.str("")
	w.uvarint(uint64(n))
	for i := range n {
		rec(i, w)
	}
	w.uvarint(0)
	gz.Close()
	out.Write(h.Sum(nil))
	return out.Bytes()
}

func node(w *writer, pd uint64, kind inventory.Kind, name string, size int64) {
	w.uvarint(pd)
	w.bytes([]byte{byte(kind), 0})
	w.uvarint(0)
	w.str(name)
	w.varint(size)
	w.varint(size)
	w.varint(0)
	for range 5 {
		w.uvarint(0)
	}
}

func chain(n int) []byte {
	return craft(n, `{"root":"/r"}`, func(i int, w *writer) {
		if i == 0 {
			node(w, 0, inventory.KindDir, "", 0)
		} else {
			node(w, 1, inventory.KindDir, "a", 0)
		}
	})
}

func TestDeepChainRejected(t *testing.T) {
	if _, err := Load(bytes.NewReader(chain(MaxDepth + 1))); err != nil {
		t.Fatalf("depth at the limit must load: %v", err)
	}
	_, err := Load(bytes.NewReader(chain(MaxDepth + 2)))
	if err == nil || !strings.Contains(err.Error(), "deeper") {
		t.Fatalf("over-deep chain accepted: %v", err)
	}
}

func TestDecompressionBombLimited(t *testing.T) {
	flat := craft(100_000, `{"root":"/r"}`, func(i int, w *writer) {
		if i == 0 {
			node(w, 0, inventory.KindDir, "", 0)
		} else {
			node(w, uint64(i), inventory.KindFile, "f", 0)
		}
	})
	_, err := LoadLimit(bytes.NewReader(flat), 64<<10)
	if err == nil || !strings.Contains(err.Error(), "limit") {
		t.Fatalf("bomb not limited: %v", err)
	}
	if got := bodyLimit(int64(len(flat))); got < 64<<20 {
		t.Fatalf("bodyLimit floor: %d", got)
	}
}

func TestHugeSizesDoNotWrap(t *testing.T) {
	data := craft(3, `{"root":"/r"}`, func(i int, w *writer) {
		if i == 0 {
			node(w, 0, inventory.KindDir, "", 0)
		} else {
			node(w, uint64(i), inventory.KindFile, "f", MaxSize)
		}
	})
	tr, err := Load(bytes.NewReader(data))
	if err != nil {
		t.Fatal(err)
	}
	if tr.Node(0).TotSize < 0 {
		t.Fatal("aggregate wrapped negative")
	}
	over := craft(2, `{"root":"/r"}`, func(i int, w *writer) {
		if i == 0 {
			node(w, 0, inventory.KindDir, "", 0)
		} else {
			node(w, 1, inventory.KindFile, "f", MaxSize+1)
		}
	})
	if _, err := Load(bytes.NewReader(over)); err == nil {
		t.Fatal("size above MaxSize accepted")
	}
}

func TestHostileMetadataSanitisedInSummary(t *testing.T) {
	data := craft(1, `{"root":"/r\u001b[2J","excludes":["\u001b]0;PWNED\u0007"],"excluded":1}`, func(i int, w *writer) {
		node(w, 0, inventory.KindDir, "", 0)
	})
	tr, err := Load(bytes.NewReader(data))
	if err != nil {
		t.Fatal(err)
	}
	tr.Stats.Excluded = 1
	var b bytes.Buffer
	export.Summary(&b, tr)
	if strings.ContainsAny(b.String(), "\x1b\x07") {
		t.Fatalf("raw control characters in summary: %q", b.String())
	}
}

// Blocks of one directory plus K-1 files repeat byte for byte, so a small
// file describes millions of nodes; LoadFile must refuse before allocating.
func TestNodeAmplificationLimited(t *testing.T) {
	const k, n = 1000, 1_200_001
	data := craft(n, `{"root":"/r"}`, func(i int, w *writer) {
		switch {
		case i == 0:
			node(w, 0, inventory.KindDir, "", 0)
		case i == 1:
			node(w, 1, inventory.KindDir, "d", 0)
		case (i-1)%k == 0:
			node(w, k, inventory.KindDir, "d", 0)
		default:
			node(w, uint64((i-1)%k), inventory.KindFile, "f", 0)
		}
	})
	if _, err := Load(bytes.NewReader(data)); err != nil {
		t.Fatalf("crafted tree is otherwise valid: %v", err)
	}
	p := filepath.Join(t.TempDir(), "amp.msz")
	os.WriteFile(p, data, 0o644)
	_, err := LoadFile(p)
	if err == nil || !strings.Contains(err.Error(), "node count") {
		t.Fatalf("%d nodes from a %d-byte file accepted: %v", n, len(data), err)
	}
}
