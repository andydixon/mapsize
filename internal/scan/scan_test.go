package scan

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"testing"
	"time"

	"github.com/andydixon/mapsize/internal/inventory"
)

// mkTree creates dirs×files under root: root/d0/d1/... fan-out tree.
func mkTree(t testing.TB, root string, fanout, depth, files int) (nfiles, ndirs int64, bytes int64) {
	var rec func(dir string, d int)
	rec = func(dir string, d int) {
		for i := range files {
			sz := (i + 1) * 100
			if err := os.WriteFile(filepath.Join(dir, fmt.Sprintf("f%d.txt", i)), make([]byte, sz), 0o644); err != nil {
				t.Fatal(err)
			}
			nfiles++
			bytes += int64(sz)
		}
		if d == depth {
			return
		}
		for i := range fanout {
			sub := filepath.Join(dir, fmt.Sprintf("d%d", i))
			if err := os.Mkdir(sub, 0o755); err != nil {
				t.Fatal(err)
			}
			ndirs++
			rec(sub, d+1)
		}
	}
	rec(root, 0)
	return
}

func runScan(t testing.TB, root string, o Options) *inventory.Tree {
	s, err := Start(context.Background(), root, o)
	if err != nil {
		t.Fatal(err)
	}
	select {
	case <-s.Done():
	case <-time.After(60 * time.Second):
		t.Fatal("scan did not finish (deadlock?)")
	}
	return s.Tree
}

func TestScanCountsAndAggregates(t *testing.T) {
	root := t.TempDir()
	nf, nd, bytes := mkTree(t, root, 3, 3, 4)
	tr := runScan(t, root, Options{Workers: 4})
	r := tr.Node(tr.Root())
	if int64(r.Files) != nf || int64(r.Dirs) != nd {
		t.Fatalf("files=%d dirs=%d, want %d %d", r.Files, r.Dirs, nf, nd)
	}
	// Logical total = file bytes + directory entries' own sizes.
	var dirBytes int64
	for i := 0; i < tr.Len(); i++ {
		if n := tr.Node(inventory.NodeID(i)); n.IsDir() {
			dirBytes += n.Size
		}
	}
	if r.TotSize != bytes+dirBytes {
		t.Fatalf("TotSize=%d want %d", r.TotSize, bytes+dirBytes)
	}
	if !tr.Stats.Complete || tr.Stats.Incomplete() {
		t.Fatalf("stats: %+v", tr.Stats)
	}
	// Every directory's aggregate equals the sum of its children.
	checkAggregates(t, tr)
	// Incremental aggregation must match a full recompute.
	before := snapshotTotals(tr)
	tr.Recompute()
	if after := snapshotTotals(tr); fmt.Sprint(before) != fmt.Sprint(after) {
		t.Fatal("incremental aggregates differ from recompute")
	}
}

func snapshotTotals(tr *inventory.Tree) [][4]int64 {
	out := make([][4]int64, tr.Len())
	for i := range out {
		n := tr.Node(inventory.NodeID(i))
		out[i] = [4]int64{n.TotSize, n.TotAlloc, int64(n.Files), int64(n.Dirs)}
	}
	return out
}

func checkAggregates(t *testing.T, tr *inventory.Tree) {
	t.Helper()
	for i := 0; i < tr.Len(); i++ {
		id := inventory.NodeID(i)
		n := tr.Node(id)
		if !n.IsDir() {
			continue
		}
		sum := n.Size
		tr.Children(id, func(_ inventory.NodeID, c *inventory.Node) { sum += c.TotSize })
		if sum != n.TotSize {
			t.Fatalf("%s: TotSize %d != own+children %d", tr.Path(id), n.TotSize, sum)
		}
	}
}

// TestBackpressureNoDeadlock forces maximum contention: one worker, channel
// capacities of one, tiny chunks, and thousands of directories.
func TestBackpressureNoDeadlock(t *testing.T) {
	root := t.TempDir()
	nf, nd, _ := mkTree(t, root, 6, 4, 3) // 1554 dirs
	for _, w := range []int{1, 2, 16} {
		tr := runScan(t, root, Options{Workers: w, JobBuffer: 1, ResultBuf: 1, ChunkSize: 1, ApplyBatch: 1})
		r := tr.Node(tr.Root())
		if int64(r.Files) != nf || int64(r.Dirs) != nd {
			t.Fatalf("workers=%d: files=%d dirs=%d want %d %d", w, r.Files, r.Dirs, nf, nd)
		}
	}
}

func TestCancel(t *testing.T) {
	root := t.TempDir()
	mkTree(t, root, 8, 3, 2)
	ctx, cancel := context.WithCancel(context.Background())
	s, err := Start(ctx, root, Options{Workers: 2, JobBuffer: 1, ResultBuf: 1, ChunkSize: 1})
	if err != nil {
		t.Fatal(err)
	}
	cancel()
	select {
	case <-s.Done():
	case <-time.After(10 * time.Second):
		t.Fatal("cancel did not stop scan")
	}
	tr := s.Tree
	tr.RLock()
	defer tr.RUnlock()
	if !tr.Stats.Cancelled || !tr.Stats.Incomplete() {
		t.Fatalf("expected cancelled+incomplete: %+v", tr.Stats)
	}
	// Workers must have exited: give the runtime a moment and check goroutines.
	time.Sleep(50 * time.Millisecond)
	if n := runtime.NumGoroutine(); n > 10 {
		t.Logf("goroutines after cancel: %d", n)
	}
}

func TestSymlinkLoops(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("symlinks need privileges on Windows")
	}
	root := t.TempDir()
	a := filepath.Join(root, "a")
	os.MkdirAll(filepath.Join(a, "b"), 0o755)
	os.WriteFile(filepath.Join(a, "b", "x"), make([]byte, 5000), 0o644)
	os.Symlink("..", filepath.Join(a, "b", "up")) // loop
	os.Symlink("../a", filepath.Join(a, "self"))  // loop
	os.Symlink("nowhere", filepath.Join(root, "broken"))
	for _, f := range []FollowMode{FollowNone, FollowSameFS, FollowAll} {
		tr := runScan(t, root, Options{Workers: 4, Follow: f})
		r := tr.Node(tr.Root())
		if r.Files == 0 || r.TotSize <= 5000 {
			t.Fatalf("follow=%v: bad totals %+v", f, r)
		}
		if tr.Stats.BrokenLinks != 1 {
			t.Fatalf("follow=%v: broken links %d", f, tr.Stats.BrokenLinks)
		}
		if f != FollowNone && tr.Stats.LoopsSkipped == 0 {
			t.Fatalf("follow=%v: expected loops to be detected", f)
		}
		// The 5000-byte file must be counted exactly once.
		var count int
		for i := 0; i < tr.Len(); i++ {
			if n := tr.Node(inventory.NodeID(i)); n.Name == "x" {
				count++
			}
		}
		if count != 1 {
			t.Fatalf("follow=%v: file x seen %d times", f, count)
		}
	}
}

func TestHardlinksCountedOnce(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("hard-link identity not available on Windows")
	}
	root := t.TempDir()
	p := filepath.Join(root, "orig")
	os.WriteFile(p, make([]byte, 100000), 0o644)
	os.Mkdir(filepath.Join(root, "d"), 0o755)
	if err := os.Link(p, filepath.Join(root, "d", "link")); err != nil {
		t.Skip(err)
	}
	tr := runScan(t, root, Options{Workers: 2})
	r := tr.Node(tr.Root())
	if tr.Stats.HardlinkDups != 1 {
		t.Fatalf("dups=%d", tr.Stats.HardlinkDups)
	}
	if r.TotSize >= 200000 {
		t.Fatalf("hard link double counted: %d", r.TotSize)
	}
}

func TestPermissionErrorsSurface(t *testing.T) {
	if runtime.GOOS == "windows" || os.Geteuid() == 0 {
		t.Skip("needs non-root unix")
	}
	root := t.TempDir()
	locked := filepath.Join(root, "locked")
	os.Mkdir(locked, 0o755)
	os.WriteFile(filepath.Join(locked, "f"), []byte("x"), 0o644)
	os.Chmod(locked, 0)
	defer os.Chmod(locked, 0o755)
	tr := runScan(t, root, Options{Workers: 2})
	if tr.Stats.ErrCounts[inventory.ErrPermission] != 1 || !tr.Stats.Incomplete() {
		t.Fatalf("expected one permission error: %+v", tr.Stats.ErrCounts)
	}
	if tr.Node(tr.Root()).Errors != 1 {
		t.Fatal("error not aggregated to root")
	}
}

func TestExcludes(t *testing.T) {
	root := t.TempDir()
	os.MkdirAll(filepath.Join(root, "keep"), 0o755)
	os.MkdirAll(filepath.Join(root, "skip"), 0o755)
	os.WriteFile(filepath.Join(root, "keep", "a.tmp"), []byte("x"), 0o644)
	os.WriteFile(filepath.Join(root, "keep", "b.txt"), []byte("x"), 0o644)
	os.WriteFile(filepath.Join(root, "skip", "c.txt"), []byte("x"), 0o644)
	tr := runScan(t, root, Options{Workers: 2, Excludes: []string{"*.tmp", filepath.Join(root, "skip")}})
	if tr.Stats.Excluded != 2 || tr.Node(0).Files != 1 {
		t.Fatalf("excluded=%d files=%d", tr.Stats.Excluded, tr.Node(0).Files)
	}
	// Relative path patterns resolve against the working directory.
	t.Chdir(root)
	tr = runScan(t, root, Options{Workers: 2, Excludes: []string{"./skip"}})
	if tr.Stats.Excluded != 1 {
		t.Fatalf("relative exclude: excluded=%d", tr.Stats.Excluded)
	}
}

func TestUnicodeAndHostileNames(t *testing.T) {
	root := t.TempDir()
	names := []string{"日本語.txt", "emoji-🎉.png", "esc\x1b[31mred", "nl\nname", "sp ace"}
	if runtime.GOOS == "windows" {
		names = names[:2]
	}
	for _, n := range names {
		if err := os.WriteFile(filepath.Join(root, n), []byte("x"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	tr := runScan(t, root, Options{Workers: 2})
	got := map[string]bool{}
	tr.Children(0, func(_ inventory.NodeID, n *inventory.Node) { got[n.Name] = true })
	for _, n := range names {
		if !got[n] {
			t.Errorf("missing %q (raw names must be preserved)", n)
		}
	}
}

func TestSingleFileRoot(t *testing.T) {
	p := filepath.Join(t.TempDir(), "file")
	os.WriteFile(p, make([]byte, 1234), 0o644)
	tr := runScan(t, p, Options{})
	if tr.Node(0).TotSize != 1234 || tr.Node(0).Kind != inventory.KindFile {
		t.Fatalf("%+v", tr.Node(0))
	}
}

func BenchmarkScan(b *testing.B) {
	root := b.TempDir()
	mkTree(b, root, 5, 4, 20)
	for _, w := range []int{1, 4, 16} {
		b.Run(fmt.Sprintf("workers=%d", w), func(b *testing.B) {
			for b.Loop() {
				runScan(b, root, Options{Workers: w})
			}
		})
	}
}
