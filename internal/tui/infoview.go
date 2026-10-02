package tui

import (
	"fmt"
	"runtime"
	"strings"

	tea "charm.land/bubbletea/v2"

	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/textutil"
	"github.com/andydixon/mapsize/internal/treemap"
)

// infoView shows scan statistics and the error list.
type infoView struct{ tb table }

func (*infoView) name() string { return "Scan" }

func (v *infoView) focusErrors(m *Model) { v.tb.cursor = 0 }

func (v *infoView) paint(m *Model, cv *Canvas, r treemap.Rect) {
	t := m.theme
	cv.Fill(r, " ", t.base())
	st := &m.tree.Stats
	root := m.tree.Node(m.tree.Root())
	lw := min(52, r.W/2)
	if r.W < 100 {
		lw = r.W - 2
	}
	y := r.Y + 1
	x := r.X + 2
	label := Style{FG: t.Muted, BG: t.Bg}
	value := Style{FG: t.Fg, BG: t.Bg, Attr: Bold}
	line := func(k, val string, vs Style) {
		if y >= r.Y+r.H {
			return
		}
		cv.Text(x, y, k, 20, label)
		cv.Text(x+20, y, val, lw-20, vs)
		y++
	}
	head := func(s string) {
		if y >= r.Y+r.H-1 {
			return
		}
		y++
		cv.Text(x, y, s, lw, Style{FG: t.Accent, BG: t.Bg, Attr: Bold})
		y++
	}
	state := "Complete"
	stateSt := Style{FG: t.OK, BG: t.Bg, Attr: Bold}
	switch {
	case m.scanning:
		state, stateSt.FG = "Scanning…", t.Accent
	case st.Cancelled:
		state, stateSt.FG = "Cancelled — totals incomplete", t.Warn
	case st.Incomplete():
		state, stateSt.FG = "Complete, with unreadable areas", t.Warn
	}
	cv.Text(x, y, "SCAN INFORMATION", lw, Style{FG: t.Accent, BG: t.Bg, Attr: Bold})
	y++
	line("Root", textutil.Sanitize(st.Root), value)
	line("State", state, stateSt)
	if st.FromSnapshot != "" {
		line("Snapshot", textutil.Sanitize(st.FromSnapshot), value)
	}
	files := st.Files + st.Symlinks + st.Others
	line("Items scanned", textutil.Count(files+st.Dirs), value)
	line("  Files", textutil.Count(st.Files), value)
	line("  Directories", textutil.Count(st.Dirs), value)
	line("  Symlinks", textutil.Count(st.Symlinks), value)
	line("  Special", textutil.Count(st.Others), value)
	line("Logical size", fmt.Sprintf("%s  (%s bytes)", textutil.Size(root.TotSize), textutil.Count(root.TotSize)), value)
	line("Allocated size", fmt.Sprintf("%s  (%s bytes)", textutil.Size(root.TotAlloc), textutil.Count(root.TotAlloc)), value)
	el := st.Elapsed()
	if st.FromSnapshot == "" {
		line("Elapsed", textutil.Duration(el), value)
		line("Rate", textutil.Count(int64(float64(files+st.Dirs)/max(el.Seconds(), 0.001)))+" items/s", value)
		line("Workers", fmt.Sprintf("%d  (%d CPUs)", st.Workers, runtime.NumCPU()), value)
	}

	head("ERRORS AND OMISSIONS")
	anyIssue := false
	for k, c := range st.ErrCounts {
		if c > 0 {
			line(inventory.ErrKind(k).String(), textutil.Count(c), Style{FG: t.Err, BG: t.Bg, Attr: Bold})
			anyIssue = true
		}
	}
	type row struct {
		k string
		v int64
	}
	for _, rr := range []row{{"Unscanned dirs", st.Unscanned}, {"Excluded", st.Excluded}, {"Other FS skipped", st.SkippedMounts},
		{"Virtual FS skipped", st.VirtualSkipped}, {"Loops skipped", st.LoopsSkipped}, {"Broken symlinks", st.BrokenLinks},
		{"Hard-link dups", st.HardlinkDups}} {
		if rr.v > 0 {
			line(rr.k, textutil.Count(rr.v), Style{FG: t.Warn, BG: t.Bg, Attr: Bold})
			anyIssue = true
		}
	}
	if !anyIssue {
		line("None", "every entry was read", Style{FG: t.OK, BG: t.Bg})
	}
	head("OPTIONS")
	excl := "none"
	if len(st.Excludes) > 0 {
		excl = textutil.Sanitize(strings.Join(st.Excludes, ", "))
	}
	line("Exclusions", excl, value)
	line("One filesystem", fmt.Sprint(st.OneFileSystem), value)
	line("Follow symlinks", textutil.Sanitize(st.Follow), value)
	line("Size mode", m.sizeMode.String(), value)
	units := "IEC (1024)"
	if textutil.SI {
		units = "SI (1000)"
	}
	line("Units", units, value)
	if root.Flags&inventory.FlagAllocUnknown != 0 {
		line("Note", "allocated size unavailable on this platform", Style{FG: t.Warn, BG: t.Bg})
	}

	// Error list on the right (or below in narrow terminals).
	er := treemap.Rect{X: r.X + lw + 4, Y: r.Y, W: r.W - lw - 4, H: r.H}
	if r.W < 100 {
		er = treemap.Rect{X: r.X, Y: y + 1, W: r.W, H: r.Y + r.H - y - 1}
	}
	if er.W < 30 || er.H < 4 {
		return
	}
	rows := make([]tableRow, len(st.Errors))
	for i, e := range st.Errors {
		p := m.tree.Path(e.Node)
		if e.Name != "" {
			p += "/" + e.Name
		}
		rows[i] = tableRow{cells: []string{e.Kind.String(), textutil.Sanitize(p), textutil.Sanitize(e.Msg)},
			styles: []*Style{{FG: t.Err, BG: t.Bg}, nil, {FG: t.Muted, BG: t.Bg}}, bar: -1}
	}
	title := fmt.Sprintf("ERROR LIST (%s)", textutil.Count(st.TotalErrors()))
	if int64(len(st.Errors)) < st.TotalErrors() {
		title += fmt.Sprintf(" — first %s shown", textutil.Count(int64(len(st.Errors))))
	}
	v.tb.paint(m, cv, er, []column{{"KIND", 18, false, false}, {"PATH", 0, false, true}, {"DETAIL", 22, false, false}}, rows, title)
}

func (v *infoView) key(m *Model, k string) (bool, tea.Cmd) {
	errs := m.tree.Stats.Errors
	if v.tb.key(k, len(errs)) {
		return true, nil
	}
	if v.tb.cursor < len(errs) && (k == "enter" || k == "space") {
		m.jumpTo(errs[v.tb.cursor].Node)
		return true, nil
	}
	return false, nil
}

func (v *infoView) wheel(m *Model, up bool) tea.Cmd {
	v.tb.scroll(up, len(m.tree.Stats.Errors))
	return nil
}
