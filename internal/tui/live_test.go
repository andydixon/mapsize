package tui

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/scan"
)

// TestLiveScanRendering renders, navigates, filters and resizes while the
// scanner mutates the inventory. Run with -race to verify the locking
// discipline.
func TestLiveScanRendering(t *testing.T) {
	root := t.TempDir()
	for i := range 30 {
		d := filepath.Join(root, fmt.Sprintf("d%02d", i), "sub")
		if err := os.MkdirAll(d, 0o755); err != nil {
			t.Fatal(err)
		}
		for j := range 40 {
			os.WriteFile(filepath.Join(d, fmt.Sprintf("f%d.log", j)), make([]byte, (i+1)*(j+1)*100), 0o644)
		}
	}
	start := func(ctx context.Context) (*scan.Scanner, *inventory.Tree, error) {
		s, err := scan.Start(ctx, root, scan.Options{Workers: 4, ChunkSize: 3})
		if err != nil {
			return nil, nil, err
		}
		return s, s.Tree, nil
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	s, tr, err := start(ctx)
	if err != nil {
		t.Fatal(err)
	}
	m := newModel(Options{Start: start}, s, tr, cancel)
	sizes := [][2]int{{120, 40}, {80, 24}, {200, 60}, {45, 12}}
	deadline := time.After(20 * time.Second)
	for i := 0; ; i++ {
		send(m, size(sizes[i%len(sizes)][0], sizes[i%len(sizes)][1]), tickMsg(time.Now()))
		send(m, key([]string{"right", "down", "left", "up"}[i%4]))
		if i%5 == 0 {
			send(m, key("tab"))
		}
		if i == 3 {
			m.search.ed.set("ext = log")
			m.queryChanged()
			send(m, m.applyFilter()())
		}
		m.dirty = true
		m.View()
		select {
		case <-s.Done():
			send(m, scanDoneMsg{s})
			m.View()
			if m.scanning {
				t.Fatal("scan should be finished")
			}
			return
		case <-deadline:
			t.Fatal("scan did not finish")
		default:
		}
	}
}
