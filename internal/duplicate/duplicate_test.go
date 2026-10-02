package duplicate

import (
	"context"
	"os"
	"path/filepath"
	"testing"

	"github.com/andydixon/mapsize/internal/scan"
)

func TestFind(t *testing.T) {
	root := t.TempDir()
	big := make([]byte, 200_000)
	for i := range big {
		big[i] = byte(i * 7)
	}
	w := func(name string, b []byte) {
		if err := os.WriteFile(filepath.Join(root, name), b, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	w("a.bin", big)
	w("b.bin", big)
	// Same size, same samples, different middle-of-nowhere byte: must not match.
	c := append([]byte(nil), big...)
	c[20_000] ^= 1
	w("c.bin", c)
	w("small1", []byte("hello"))
	w("small2", []byte("hello"))
	w("small3", []byte("world")) // same size, different content

	s, err := scan.Start(context.Background(), root, scan.Options{Workers: 2})
	if err != nil {
		t.Fatal(err)
	}
	<-s.Done()
	var f Finder
	groups, err := f.Find(context.Background(), s.Tree, Options{})
	if err != nil {
		t.Fatal(err)
	}
	if len(groups) != 2 {
		t.Fatalf("groups=%d want 2", len(groups))
	}
	names := func(g Group) (out []string) {
		for _, id := range g.Files {
			out = append(out, s.Tree.Node(id).Name)
		}
		return
	}
	if g := names(groups[0]); len(g) != 2 || groups[0].Size != 200_000 {
		t.Fatalf("big group %v", g)
	}
	if g := names(groups[1]); len(g) != 2 {
		t.Fatalf("small group %v", g)
	}
	if p := f.Progress(); p.Candidates != 6 || p.VerifiedGroups != 2 {
		t.Fatalf("progress %+v", p)
	}
}
