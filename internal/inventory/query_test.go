package inventory

import "testing"

func TestTopKAndRecompute(t *testing.T) {
	tr := New("/r", KindDir)
	d := tr.Add(0, "d", KindDir)
	for i := range 100 {
		id := tr.Add(d, "f", KindFile)
		n := tr.Node(id)
		n.Size, n.Alloc = int64(i), int64(i)
	}
	tr.Recompute()
	if got := tr.Node(0).TotSize; got != 4950 {
		t.Fatalf("total %d", got)
	}
	if tr.Node(0).Files != 100 || tr.Node(0).Dirs != 1 {
		t.Fatalf("counts %d %d", tr.Node(0).Files, tr.Node(0).Dirs)
	}
	top := tr.Top(0, 3, func(n *Node) bool { return n.Kind == KindFile }, func(n *Node) int64 { return n.Size })
	if len(top) != 3 || tr.Node(top[0]).Size != 99 || tr.Node(top[2]).Size != 97 {
		t.Fatalf("top %v", top)
	}
	if p := tr.Path(top[0]); p != "/r/d/f" {
		t.Fatalf("path %q", p)
	}
}

func TestExtAndCategory(t *testing.T) {
	cases := map[string]string{"a.QCOW2": "qcow2", ".bashrc": "", "noext": "", "x.tar.gz": "gz", "trail.": ""}
	for in, want := range cases {
		if got := Ext(in); got != want {
			t.Errorf("Ext(%q)=%q want %q", in, got, want)
		}
	}
	if CategoryFor("qcow2", false, false) != CatDiskImage || CategoryFor("bin", true, false) != CatExecutable {
		t.Fatal("category mapping")
	}
	if CategoryFor("", true, false) != CatCache {
		t.Fatal("cache hint")
	}
}
