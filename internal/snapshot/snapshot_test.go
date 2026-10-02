package snapshot

import (
	"bufio"
	"bytes"
	"encoding/binary"
	"testing"

	"github.com/andydixon/mapsize/internal/inventory"
)

func sample() *inventory.Tree {
	t := inventory.New("/srv", inventory.KindDir)
	a := t.Add(0, "a", inventory.KindDir)
	b := t.Add(0, "b", inventory.KindDir)
	f1 := t.Add(a, "x.iso", inventory.KindFile)
	t.Node(f1).Size, t.Node(f1).Alloc = 1000, 1024
	f2 := t.Add(b, "y.log", inventory.KindFile)
	t.Node(f2).Size, t.Node(f2).Alloc = 50, 4096
	t.Node(f2).Flags = inventory.FlagSparse
	t.Recompute()
	t.Stats.Files, t.Stats.Dirs = 2, 3
	t.Stats.AddError(inventory.ErrorRecord{Node: b, Kind: inventory.ErrPermission, Msg: "denied"})
	return t
}

func TestRoundTrip(t *testing.T) {
	src := sample()
	var buf bytes.Buffer
	if err := Save(&buf, src); err != nil {
		t.Fatal(err)
	}
	got, err := Load(bytes.NewReader(buf.Bytes()))
	if err != nil {
		t.Fatal(err)
	}
	if got.Len() != src.Len() {
		t.Fatalf("len %d want %d", got.Len(), src.Len())
	}
	for i := range src.Len() {
		a, b := src.Node(inventory.NodeID(i)), got.Node(inventory.NodeID(i))
		if a.Name != b.Name || a.TotSize != b.TotSize || a.TotAlloc != b.TotAlloc || a.Flags != b.Flags ||
			src.Path(inventory.NodeID(i)) != got.Path(inventory.NodeID(i)) || src.ExtName(a) != got.ExtName(b) {
			t.Fatalf("node %d differs: %+v vs %+v", i, a, b)
		}
	}
	if got.Stats.ErrCounts[inventory.ErrPermission] != 1 || len(got.Stats.Errors) != 1 {
		t.Fatalf("errors not restored: %+v", got.Stats)
	}
}

func TestCorruptionDetected(t *testing.T) {
	var buf bytes.Buffer
	Save(&buf, sample())
	data := buf.Bytes()
	// Flip a byte in the checksum trailer.
	bad := append([]byte(nil), data...)
	bad[len(bad)-1] ^= 0xff
	if _, err := Load(bytes.NewReader(bad)); err == nil {
		t.Fatal("checksum corruption not detected")
	}
	// Every truncation must fail cleanly.
	for i := 0; i < len(data)-1; i += 7 {
		if _, err := Load(bytes.NewReader(data[:i])); err == nil {
			t.Fatalf("truncation at %d accepted", i)
		}
	}
}

func FuzzLoad(f *testing.F) {
	var buf bytes.Buffer
	Save(&buf, sample())
	f.Add(buf.Bytes())
	f.Add([]byte("MAPSIZE\x00\x01\x00\x01\x00"))
	f.Fuzz(func(t *testing.T, data []byte) {
		tr, err := Load(bytes.NewReader(data))
		if err != nil {
			return
		}
		// A successfully loaded tree must be internally consistent.
		for i := 1; i < tr.Len(); i++ {
			if p := tr.Node(inventory.NodeID(i)).Parent; p >= inventory.NodeID(i) {
				t.Fatalf("parent %d >= child %d", p, i)
			}
		}
		_ = tr.Path(inventory.NodeID(tr.Len() - 1))
	})
}

func TestCompare(t *testing.T) {
	old := sample()
	nw := inventory.New("/srv", inventory.KindDir)
	a := nw.Add(0, "a", inventory.KindDir)
	f1 := nw.Add(a, "x.iso", inventory.KindFile)
	nw.Node(f1).Size, nw.Node(f1).Alloc = 5000, 5120
	c := nw.Add(0, "c", inventory.KindDir)
	f3 := nw.Add(c, "z", inventory.KindFile)
	nw.Node(f3).Size, nw.Node(f3).Alloc = 10, 4096
	nw.Recompute()

	d := Compare(old, nw)
	if d.StatusOf(f1, inventory.SizeAllocated) != Grew || d.Delta(f1, inventory.SizeAllocated) != 4096 {
		t.Fatalf("x.iso: %v %d", d.StatusOf(f1, 0), d.Delta(f1, 0))
	}
	if d.StatusOf(c, 0) != Added {
		t.Fatal("c should be new")
	}
	if len(d.Removed) != 1 || old.Node(d.Removed[0]).Name != "b" {
		t.Fatalf("removed %v", d.Removed)
	}
	ch := d.Changes(10, inventory.SizeAllocated)
	paths := map[string]Status{}
	for _, c := range ch {
		paths[c.Path] = c.Status
	}
	if paths["/srv/a/x.iso"] != Grew || paths["/srv/b"] != Removed || paths["/srv/c"] != Added {
		t.Fatalf("changes %+v", ch)
	}
	if _, ok := paths["/srv/a"]; ok {
		t.Fatal("/srv/a should be explained by its child")
	}
	if _, ok := paths["/srv/c/z"]; ok {
		t.Fatal("contents of new dirs should not be listed separately")
	}
}

func TestReadUvarintMatchesStdlib(t *testing.T) {
	for _, v := range []uint64{0, 1, 127, 128, 300, 1 << 35, 1<<64 - 1} {
		buf := binary.AppendUvarint(nil, v)
		got, err := readUvarint(bufio.NewReader(bytes.NewReader(buf)))
		if err != nil || got != v {
			t.Fatalf("%d: got %d %v", v, got, err)
		}
	}
	bad := bytes.Repeat([]byte{0xff}, 11)
	if _, err := readUvarint(bufio.NewReader(bytes.NewReader(bad))); err == nil {
		t.Fatal("overflow not detected")
	}
}
