package tui

import (
	"context"
	"fmt"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/andydixon/mapsize/internal/duplicate"
	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/platform"
	"github.com/andydixon/mapsize/internal/textutil"
	"github.com/andydixon/mapsize/internal/treemap"
)

// ---- Trash ---------------------------------------------------------------

type trashDoneMsg struct {
	id  inventory.NodeID
	err error
}

// requestTrash asks for confirmation before moving the selection to the
// trash. Permanent deletion is deliberately not offered.
func (m *Model) requestTrash() tea.Cmd {
	switch {
	case m.opts.ReadOnly:
		return m.warn("Read-only mode: modifying actions are disabled")
	case m.snapshot:
		return m.warn("Snapshot view: nothing to delete on the live filesystem")
	case m.scanning:
		return m.warn("Wait for the scan to finish before trashing items")
	}
	id := m.selectedNode()
	if id == inventory.NoNode || id == m.tree.Root() {
		return nil
	}
	n := m.tree.Node(id)
	path := m.tree.Path(id)
	what := "file"
	if n.IsDir() {
		what = fmt.Sprintf("directory with %s files", textutil.Count(int64(n.Files)))
	}
	m.openModal(&confirmModal{
		ttl: "Move to trash?",
		body: []string{textutil.Sanitize(path), "",
			fmt.Sprintf("%s · %s", what, textutil.Size(n.Total(m.sizeMode))),
			"", "It will be moved to the " + platform.TrashName + ", not permanently deleted."},
		yes: "move to " + platform.TrashName,
		onYes: func(m *Model) tea.Cmd {
			return func() tea.Msg { return trashDoneMsg{id, platform.Trash(path)} }
		},
	})
	return nil
}

func (m *Model) trashDone(msg trashDoneMsg) tea.Cmd {
	if msg.err != nil {
		return m.warn("Trash failed: " + textutil.Sanitize(msg.err.Error()))
	}
	// The scan has finished, so the UI goroutine is the tree's only writer.
	t := m.tree
	t.Lock()
	n := t.Node(msg.id)
	n.Flags |= inventory.FlagDeleted
	d := inventory.Delta{Size: -n.TotSize, Alloc: -n.TotAlloc, Files: -int64(n.Files), Dirs: -int64(n.Dirs)}
	if n.IsDir() {
		d.Dirs--
		for i, v := range t.CatSizes(msg.id) {
			d.Cat[i] = -v
		}
	} else {
		d.Files--
		d.Cat[n.Cat] = -n.TotAlloc
	}
	t.Propagate(n.Parent, d)
	n.TotSize, n.TotAlloc = 0, 0
	name := n.Name
	t.Unlock()
	m.invalidate()
	return m.info("Moved to " + platform.TrashName + ": " + textutil.Sanitize(name))
}

// ---- Duplicates ----------------------------------------------------------

type dupState struct {
	running bool
	finder  *duplicate.Finder
	groups  []duplicate.Group
	err     error
	cancel  context.CancelFunc
	started time.Time
}

type (
	dupProgressMsg struct{}
	dupDoneMsg     struct {
		groups []duplicate.Group
		err    error
	}
)

func (m *Model) startDuplicates() tea.Cmd {
	if m.snapshot {
		return m.warn("Duplicate detection needs the live filesystem")
	}
	if m.scanning {
		return m.warn("Wait for the scan to finish first")
	}
	if m.dups != nil {
		m.ensureDupView()
		return nil
	}
	m.openModal(&confirmModal{
		ttl: "Find duplicate files?",
		body: []string{
			"Files are grouped by size, then sampled, then fully hashed with SHA-256.",
			"Only identical hashes are reported as duplicates.",
			"", "This reads file contents (it may take a while and can update access times).",
		},
		yes: "start",
		onYes: func(m *Model) tea.Cmd {
			ctx, cancel := context.WithCancel(context.Background())
			f := &duplicate.Finder{}
			m.dups = &dupState{running: true, finder: f, cancel: cancel, started: time.Now()}
			m.ensureDupView()
			t := m.tree
			return tea.Batch(m.startTicking(), func() tea.Msg {
				g, err := f.Find(ctx, t, duplicate.Options{MinSize: 1, Workers: 4})
				return dupDoneMsg{g, err}
			})
		},
	})
	return nil
}

func (m *Model) ensureDupView() {
	for i, v := range m.views {
		if _, ok := v.(*dupView); ok {
			m.setView(i)
			return
		}
	}
	m.views = append(m.views, &dupView{})
	m.setView(len(m.views) - 1)
}

func (m *Model) dupMsg(msg tea.Msg) tea.Cmd {
	if d, ok := msg.(dupDoneMsg); ok && m.dups != nil {
		m.dups.running = false
		m.dups.groups, m.dups.err = d.groups, d.err
		m.dirty = true
		var wasted int64
		for _, g := range d.groups {
			wasted += g.Wasted()
		}
		return m.info(fmt.Sprintf("Duplicates: %d verified groups, %s reclaimable", len(d.groups), textutil.Size(wasted)))
	}
	return nil
}

type dupView struct {
	tb   table
	rows []dupRow
}

type dupRow struct {
	group int
	id    inventory.NodeID // NoNode for a group header
}

func (*dupView) name() string { return "Dups" }

func (v *dupView) paint(m *Model, cv *Canvas, r treemap.Rect) {
	t := m.theme
	d := m.dups
	if d == nil {
		cv.TextCentered(r.X, r.Y+r.H/2, r.W, "Press D to search for duplicates", t.muted())
		return
	}
	if d.running {
		p := d.finder.Progress()
		cv.Fill(r, " ", t.base())
		y := r.Y + r.H/2 - 2
		cv.TextCentered(r.X, y, r.W, "Finding duplicates — "+p.Stage+"…", Style{FG: t.Accent, BG: t.Bg, Attr: Bold})
		cv.TextCentered(r.X, y+2, r.W, fmt.Sprintf("%s candidate files in %s size groups", textutil.Count(p.Candidates),
			textutil.Count(p.CandidateGroups)), t.base())
		if p.BytesToHash > 0 {
			frac := float64(p.BytesHashed) / float64(p.BytesToHash)
			bw := min(60, r.W-10)
			m.paintBar(cv, r.X+(r.W-bw)/2, y+4, bw, frac, t.Accent, false)
			cv.TextCentered(r.X, y+5, r.W, fmt.Sprintf("%s of %s hashed · %s skipped", textutil.Size(p.BytesHashed),
				textutil.Size(p.BytesToHash), textutil.Count(p.Skipped)), t.muted())
		}
		return
	}
	v.rows = v.rows[:0]
	var tr []tableRow
	for gi, g := range d.groups {
		v.rows = append(v.rows, dupRow{gi, inventory.NoNode})
		tr = append(tr, tableRow{
			cells:  []string{textutil.Size(g.Wasted()), fmt.Sprintf("%d × %s", len(g.Files), textutil.Size(g.Size)), fmt.Sprintf("verified identical · sha256 %x…", g.Hash[:8])},
			styles: []*Style{{FG: t.Warn, BG: t.Bg, Attr: Bold}, {FG: t.Fg, BG: t.Bg, Attr: Bold}, {FG: t.OK, BG: t.Bg}}, bar: -1,
		})
		for _, id := range g.Files {
			v.rows = append(v.rows, dupRow{gi, id})
			tr = append(tr, tableRow{cells: []string{"", "", "  " + textutil.Sanitize(m.tree.Path(id))}, bar: -1})
		}
	}
	title := fmt.Sprintf("Duplicates — %d groups (Enter details · Space go to)", len(d.groups))
	if d.err != nil {
		title += " — stopped: " + textutil.Sanitize(d.err.Error())
	}
	v.tb.paint(m, cv, r, []column{{"WASTED", 10, true}, {"COPIES", 16, false}, {"", 0, false}}, tr, title)
}

func (v *dupView) key(m *Model, k string) (bool, tea.Cmd) {
	if v.tb.key(k, len(v.rows)) {
		return true, nil
	}
	if v.tb.cursor < len(v.rows) {
		r := v.rows[v.tb.cursor]
		if r.id == inventory.NoNode && m.dups != nil && r.group < len(m.dups.groups) {
			r.id = m.dups.groups[r.group].Files[0]
		}
		if r.id != inventory.NoNode {
			switch k {
			case "enter":
				m.openModal(newInfoModal(m, r.id))
				return true, nil
			case "space":
				m.jumpTo(r.id)
				return true, nil
			case "c", "o", "d":
				m.sel = int64(r.id)
			}
		}
	}
	if k == "esc" && m.dups != nil && m.dups.running {
		m.dups.cancel()
		return true, nil
	}
	return false, nil
}

func (v *dupView) wheel(m *Model, up bool) tea.Cmd {
	v.tb.move(map[bool]int{true: -3, false: 3}[up], len(v.rows))
	return nil
}
