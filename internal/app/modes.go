package app

import (
	"context"
	"fmt"

	"github.com/andydixon/mapsize/internal/duplicate"
	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/scan"
	"github.com/andydixon/mapsize/internal/snapshot"
	"github.com/andydixon/mapsize/internal/textutil"
	"github.com/andydixon/mapsize/internal/tui"
)

func runTUI(cfg *Config) error {
	return tui.Run(tui.Options{
		ReadOnly: cfg.ReadOnly, Theme: cfg.Theme, SizeMode: cfg.SizeMode, Mouse: cfg.Mouse, Color: cfg.Color,
		Save: cfg.Save, Settings: cfg.Settings,
		Start: func(ctx context.Context) (*scan.Scanner, *inventory.Tree, error) { return startSource(ctx, cfg) },
	})
}

func loadSnapshot(path string) (*inventory.Tree, error) { return snapshot.LoadFile(path) }

func saveSnapshot(t *inventory.Tree, path string) error { return snapshot.SaveFile(path, t) }

func runCompare(cfg *Config) error {
	old, err := snapshot.LoadFile(cfg.Compare[0])
	if err != nil {
		return fmt.Errorf("%s: %w", cfg.Compare[0], err)
	}
	nw, err := snapshot.LoadFile(cfg.Compare[1])
	if err != nil {
		return fmt.Errorf("%s: %w", cfg.Compare[1], err)
	}
	d := snapshot.Compare(old, nw)
	k := cfg.Top
	if k <= 0 {
		k = 30
	}
	out := cfg.Stdout
	ro, rn := old.Node(0), nw.Node(0)
	fmt.Fprintf(out, "%s → %s\n", textutil.Sanitize(old.Stats.FromSnapshot), textutil.Sanitize(nw.Stats.FromSnapshot))
	fmt.Fprintf(out, "Total %s → %s (%s)\n\n", textutil.Size(ro.Total(cfg.SizeMode)), textutil.Size(rn.Total(cfg.SizeMode)),
		textutil.SignedSize(rn.Total(cfg.SizeMode)-ro.Total(cfg.SizeMode)))
	for _, c := range d.Changes(k, cfg.SizeMode) {
		fmt.Fprintf(out, "%-10s %14s  %s\n", c.Status, textutil.SignedSize(c.Delta), textutil.Sanitize(c.Path))
	}
	return nil
}

func runDuplicates(ctx context.Context, cfg *Config, t *inventory.Tree) error {
	// The caller holds RLock; Find takes its own read lock, which is fine
	// for a RWMutex only if no writer is waiting — the scan is finished.
	var f duplicate.Finder
	groups, err := f.Find(ctx, t, duplicate.Options{MinSize: 1})
	if err != nil {
		return err
	}
	out := cfg.Stdout
	var wasted int64
	for _, g := range groups {
		wasted += g.Wasted()
		fmt.Fprintf(out, "%s × %d  (sha256 %x…)\n", textutil.Size(g.Size), len(g.Files), g.Hash[:6])
		for _, id := range g.Files {
			fmt.Fprintf(out, "    %s\n", textutil.Sanitize(t.Path(id)))
		}
	}
	p := f.Progress()
	fmt.Fprintf(out, "\n%d verified duplicate groups, %s reclaimable (%d candidates by size, %d unreadable/changed)\n\n",
		len(groups), textutil.Size(wasted), p.Candidates, p.Skipped)
	return nil
}
