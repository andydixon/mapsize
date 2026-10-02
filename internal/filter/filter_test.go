package filter

import (
	"context"
	"testing"
	"time"

	"github.com/andydixon/mapsize/internal/inventory"
)

func tree() *inventory.Tree {
	t := inventory.New("/srv", inventory.KindDir)
	vms := t.Add(0, "vms", inventory.KindDir)
	cache := t.Add(0, "cache", inventory.KindDir)
	add := func(p inventory.NodeID, name string, size int64, age time.Duration) inventory.NodeID {
		id := t.Add(p, name, inventory.KindFile)
		n := t.Node(id)
		n.Size, n.Alloc = size, size
		n.MTime = time.Now().Add(-age).UnixNano()
		n.Cat = inventory.CategoryFor(inventory.Ext(name), false, false)
		return id
	}
	add(vms, "ubuntu.qcow2", 80<<30, 400*24*time.Hour)
	add(vms, "win.vmdk", 40<<30, time.Hour)
	add(vms, "notes.txt", 1000, time.Hour)
	add(cache, "blob.iso", 700<<20, 10*24*time.Hour)
	add(cache, "x.tmp", 10, time.Hour)
	t.Recompute()
	return t
}

func names(t *testing.T, tr *inventory.Tree, q string) map[string]bool {
	t.Helper()
	pq, err := Parse(q, time.Now())
	if err != nil {
		t.Fatalf("Parse(%q): %v", q, err)
	}
	r, err := Apply(context.Background(), tr, pq, inventory.SizeLogical)
	if err != nil {
		t.Fatal(err)
	}
	out := map[string]bool{}
	for i := range tr.Len() {
		if r.IsMatch(inventory.NodeID(i)) {
			out[tr.Node(inventory.NodeID(i)).Name] = true
		}
	}
	return out
}

func TestQueries(t *testing.T) {
	tr := tree()
	cases := map[string][]string{
		"ubuntu":                     {"ubuntu.qcow2"},
		"*.iso":                      {"blob.iso"},
		"size > 1GB":                 {"vms", "ubuntu.qcow2", "win.vmdk"},
		"size > 1GB AND type = file": {"ubuntu.qcow2", "win.vmdk"},
		"ext = qcow2":                {"ubuntu.qcow2"},
		"size > 500MB AND ext IN (iso,qcow2,vmdk)":  {"ubuntu.qcow2", "win.vmdk", "blob.iso"},
		"path contains cache":                       {"cache", "blob.iso", "x.tmp"},
		"age > 365d":                                {"ubuntu.qcow2"},
		"NOT type = dir AND size < 1k":              {"notes.txt", "x.tmp"},
		"(ext = iso OR ext = tmp) type=file":        {"blob.iso", "x.tmp"},
		"category = vm":                             {"ubuntu.qcow2", "win.vmdk", "blob.iso"},
		"name matches '^w.*k$'":                     {"win.vmdk"},
		"ext != qcow2 && type = file && size > 1GB": {"win.vmdk"},
		"name = 'notes.txt'":                        {"notes.txt"},
	}
	for q, want := range cases {
		got := names(t, tr, q)
		if len(got) != len(want) {
			t.Errorf("%q: got %v want %v", q, got, want)
			continue
		}
		for _, w := range want {
			if !got[w] {
				t.Errorf("%q: got %v want %v", q, got, want)
			}
		}
	}
}

func TestFilteredSizesNoDoubleCount(t *testing.T) {
	tr := tree()
	q, _ := Parse("name = vms OR ext = qcow2", time.Now())
	r, _ := Apply(context.Background(), tr, q, inventory.SizeLogical)
	if want := tr.Node(1).TotSize; r.Total != want {
		t.Fatalf("total %d want %d (matching dir must not double count matching child)", r.Total, want)
	}
}

func TestParseErrors(t *testing.T) {
	for _, q := range []string{"", "size >", "size > banana", "(ext = iso", "ext = iso)", "size contains 5",
		"name matches '['", "type = blob", "'unterminated", "a & b", "category = nope"} {
		if _, err := Parse(q, time.Now()); err == nil {
			t.Errorf("Parse(%q) should fail", q)
		}
	}
}

func TestParseSize(t *testing.T) {
	cases := map[string]int64{"1GB": 1e9, "1GiB": 1 << 30, "1.5k": 1536, "500MB": 5e8, "10": 10}
	for in, want := range cases {
		if got, err := ParseSize(in); err != nil || got != want {
			t.Errorf("ParseSize(%q)=%d,%v want %d", in, got, err, want)
		}
	}
}

func FuzzParse(f *testing.F) {
	for _, s := range []string{"size > 1GB", "(a OR b) AND NOT c", "ext IN (a,b)", "'x", "name matches '.*'"} {
		f.Add(s)
	}
	tr := tree()
	f.Fuzz(func(t *testing.T, s string) {
		q, err := Parse(s, time.Now())
		if err != nil {
			return
		}
		Apply(context.Background(), tr, q, inventory.SizeAllocated)
	})
}
