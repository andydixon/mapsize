package tui

import (
	"context"
	"fmt"
	"os"
	"runtime/debug"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/colorprofile"

	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/scan"
	"github.com/andydixon/mapsize/internal/snapshot"
	"github.com/andydixon/mapsize/internal/textutil"
)

var profiles = map[string]colorprofile.Profile{
	"truecolor": colorprofile.TrueColor, "24bit": colorprofile.TrueColor,
	"256": colorprofile.ANSI256, "16": colorprofile.ANSI, "none": colorprofile.ASCII,
}

func saveFile(path string, t *inventory.Tree) error { return snapshot.SaveFile(path, t) }

// Run starts the interactive UI. The terminal is restored on every exit
// path: normal quit, Ctrl+C, errors and panics (Bubble Tea recovers panics
// in Update/View; scanner goroutine panics are routed here via
// scan.OnPanic and reported after the terminal is restored).
func Run(opts Options) error {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	s, t, err := opts.Start(ctx)
	if err != nil {
		return err
	}
	m := newModel(opts, s, t, cancel)
	var popts []tea.ProgramOption
	if p, ok := profiles[opts.Color]; ok {
		m.profile, m.forced = p, true
		popts = append(popts, tea.WithColorProfile(p))
	} else if opts.Color != "" && opts.Color != "auto" {
		return fmt.Errorf("invalid --color %q (auto, truecolor, 256, 16, none)", opts.Color)
	}
	p := tea.NewProgram(m, popts...)
	prev := scan.OnPanic
	scan.OnPanic = func(v any, stack []byte) {
		p.Send(fatalMsg{v, stack})
		select {} // park the broken goroutine; the program is quitting
	}
	defer func() { scan.OnPanic = prev }()
	if debugPanic == "scanner" {
		go func() {
			time.Sleep(500 * time.Millisecond)
			defer func() {
				if v := recover(); v != nil {
					scan.OnPanic(v, debug.Stack())
				}
			}()
			panic("MAPSIZE_DEBUG_PANIC=scanner: deliberate panic to test terminal restoration")
		}()
	}

	_, err = p.Run()
	cancel()
	if m.fatal != nil {
		fmt.Fprintf(os.Stderr, "mapsize: internal error in scanner: %s\n\n%s\n",
			textutil.Sanitize(fmt.Sprint(m.fatal.v)), m.fatal.stack)
		return fmt.Errorf("scanner crashed")
	}
	return err
}
