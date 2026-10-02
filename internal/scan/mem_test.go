package scan

import (
	"context"
	"os"
	"runtime"
	"testing"
	"time"
)

// TestMemoryPerNode reports heap use per inventory node for a real tree.
// Set MAPSIZE_MEM_ROOT to a directory to run it.
func TestMemoryPerNode(t *testing.T) {
	root := os.Getenv("MAPSIZE_MEM_ROOT")
	if root == "" {
		t.Skip("set MAPSIZE_MEM_ROOT")
	}
	var before, after runtime.MemStats
	runtime.GC()
	runtime.ReadMemStats(&before)
	start := time.Now()
	s, err := Start(context.Background(), root, Options{})
	if err != nil {
		t.Fatal(err)
	}
	<-s.Done()
	el := time.Since(start)
	runtime.GC()
	runtime.ReadMemStats(&after)
	n := s.Tree.Len()
	heap := after.HeapAlloc - before.HeapAlloc
	t.Logf("nodes=%d scan=%s heap=%.1f MiB (%.0f bytes/node)", n, el.Round(time.Millisecond),
		float64(heap)/(1<<20), float64(heap)/float64(n))
	runtime.KeepAlive(s)
}
