// Package app wires the command-line modes together: interactive TUI,
// non-interactive reports, snapshots and comparisons. TUI and CLI share the
// same scanner and inventory packages.
package app

import (
	"context"
	"fmt"
	"io"
	"log/slog"
	"os"
	"os/signal"
	"time"

	"github.com/andydixon/mapsize/internal/config"
	"github.com/andydixon/mapsize/internal/export"
	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/scan"
	"github.com/andydixon/mapsize/internal/textutil"
)

// Config is the fully-resolved command line.
type Config struct {
	Paths        []string
	ScanOpts     scan.Options
	NoUI         bool
	JSON, CSV    bool
	Depth        int
	Top          int
	LargestFiles int
	Save         string
	Load         string
	Compare      [2]string
	Duplicates   bool
	ReadOnly     bool
	Theme        string
	SizeMode     inventory.SizeMode
	Mouse        bool
	Color        string
	Settings     config.Settings
	Stdout       io.Writer
	Stderr       io.Writer
}

// Report reports whether any non-interactive output was requested.
func (c *Config) Report() bool {
	return c.NoUI || c.JSON || c.CSV || c.Top > 0 || c.LargestFiles > 0 || c.Duplicates
}

// Run executes the configured mode.
func Run(cfg *Config) error {
	if cfg.Stdout == nil {
		cfg.Stdout = os.Stdout
	}
	if cfg.Stderr == nil {
		cfg.Stderr = os.Stderr
	}
	if cfg.Compare[0] != "" {
		return runCompare(cfg)
	}
	if !cfg.Report() {
		return runTUI(cfg)
	}
	return runReport(cfg)
}

// startSource begins a scan (or loads a snapshot). The scanner is nil when
// the tree came from a snapshot.
func startSource(ctx context.Context, cfg *Config) (*scan.Scanner, *inventory.Tree, error) {
	if cfg.Load != "" {
		t, err := loadSnapshot(cfg.Load)
		return nil, t, err
	}
	root := "."
	if len(cfg.Paths) > 0 {
		root = cfg.Paths[0]
	}
	slog.Info("scan start", "workers", cfg.ScanOpts.Workers, "follow", cfg.ScanOpts.Follow.String())
	s, err := scan.Start(ctx, root, cfg.ScanOpts)
	if err != nil {
		return nil, nil, err
	}
	return s, s.Tree, nil
}

func runReport(cfg *Config) error {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()
	s, t, err := startSource(ctx, cfg)
	if err != nil {
		return err
	}
	if s != nil {
		waitWithProgress(s, cfg.Stderr)
	}
	t.RLock()
	defer t.RUnlock()
	if s != nil && cfg.Save != "" {
		if err := saveSnapshot(t, cfg.Save); err != nil {
			return err
		}
		fmt.Fprintf(cfg.Stderr, "Saved snapshot %s\n", cfg.Save)
	}
	out := cfg.Stdout
	switch {
	case cfg.JSON:
		return export.JSON(out, t, t.Root(), cfg.Depth)
	case cfg.CSV:
		return export.CSV(out, t, t.Root(), cfg.Depth)
	}
	if cfg.Duplicates {
		if err := runDuplicates(ctx, cfg, t); err != nil {
			return err
		}
	}
	if cfg.Top > 0 {
		ids := t.SortedChildren(t.Root(), cfg.SizeMode)
		if len(ids) > cfg.Top {
			ids = ids[:cfg.Top]
		}
		export.Table(out, t, ids, cfg.SizeMode)
		fmt.Fprintln(out)
	}
	if cfg.LargestFiles > 0 {
		ids := t.Top(t.Root(), cfg.LargestFiles,
			func(n *inventory.Node) bool {
				return n.Kind == inventory.KindFile && n.Flags&inventory.FlagHardlinkDup == 0
			},
			func(n *inventory.Node) int64 { return n.Own(cfg.SizeMode) })
		export.Table(out, t, ids, cfg.SizeMode)
		fmt.Fprintln(out)
	}
	export.Summary(out, t)
	return nil
}

// waitWithProgress blocks until the scan finishes, printing a progress line
// to w if it is a terminal.
func waitWithProgress(s *scan.Scanner, w io.Writer) {
	tty := isTerminal(w)
	tick := time.NewTicker(250 * time.Millisecond)
	defer tick.Stop()
	start := time.Now()
	for {
		select {
		case <-s.Done():
			if tty {
				fmt.Fprint(w, "\r\033[K")
			}
			return
		case <-tick.C:
			if tty {
				p := s.Progress()
				el := time.Since(start)
				fmt.Fprintf(w, "\r\033[Kscanning… %s entries  %s/s  queue %d  %s",
					textutil.Count(p.Entries), textutil.Count(int64(float64(p.Entries)/el.Seconds())),
					p.Pending, textutil.Duration(el))
			}
		}
	}
}

func isTerminal(w io.Writer) bool {
	f, ok := w.(*os.File)
	if !ok {
		return false
	}
	fi, err := f.Stat()
	return err == nil && fi.Mode()&os.ModeCharDevice != 0
}
