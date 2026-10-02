//go:build unix

package duplicate

import (
	"context"
	"os"
	"path/filepath"
	"syscall"
	"testing"
	"time"

	"github.com/andydixon/mapsize/internal/scan"
)

// A candidate replaced by a FIFO (or a symlink to one) after the scan must
// be skipped, not block the search forever in open(2).
func TestFindSkipsSwappedFIFO(t *testing.T) {
	root := t.TempDir()
	for _, n := range []string{"a", "b", "c"} {
		os.WriteFile(filepath.Join(root, n), []byte("same"), 0o644)
	}
	s, err := scan.Start(context.Background(), root, scan.Options{Workers: 1})
	if err != nil {
		t.Fatal(err)
	}
	<-s.Done()
	os.Remove(filepath.Join(root, "b"))
	if err := syscall.Mkfifo(filepath.Join(root, "b"), 0o644); err != nil {
		t.Skip("mkfifo:", err)
	}
	fifo2 := filepath.Join(t.TempDir(), "p")
	syscall.Mkfifo(fifo2, 0o644)
	os.Remove(filepath.Join(root, "c"))
	os.Symlink(fifo2, filepath.Join(root, "c"))

	done := make(chan error, 1)
	var f Finder
	go func() { _, err := f.Find(context.Background(), s.Tree, Options{}); done <- err }()
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("duplicate search blocked on a FIFO")
	}
	if p := f.Progress(); p.Skipped != 2 {
		t.Fatalf("skipped=%d want 2", p.Skipped)
	}
}
