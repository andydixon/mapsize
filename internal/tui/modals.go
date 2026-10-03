package tui

import (
	"fmt"
	"io/fs"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/andydixon/mapsize/internal/export"
	"github.com/andydixon/mapsize/internal/filter"
	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/platform"
	"github.com/andydixon/mapsize/internal/textutil"
)

// ---- Item information -------------------------------------------------------

type infoModal struct {
	id    inventory.NodeID
	btime int64 // live birth time (0 if unknown)
	live  bool  // metadata re-read from the filesystem
}

func newInfoModal(m *Model, id inventory.NodeID) modal {
	md := &infoModal{id: id}
	if !m.snapshot {
		// Birth time is not kept in the inventory; read it live.
		if meta, err := platform.Lstat(m.tree.Path(id)); err == nil {
			md.btime, md.live = meta.BTime, true
		}
	}
	return md
}

func (*infoModal) width() int { return 72 }

func (md *infoModal) title() string { return "Item information" }

func (md *infoModal) footer(m *Model) string {
	s := "Esc close  ·  c copy path  ·  o " + revealVerb() + "  ·  Space go to"
	if !m.opts.ReadOnly && !m.snapshot {
		s += "  ·  d " + deleteVerb()
	}
	return s
}

func (md *infoModal) content(m *Model, w int) []line {
	t := m.theme
	tr := m.tree
	n := tr.Node(md.id)
	ks := Style{FG: t.Muted}
	vs := Style{FG: t.Fg}
	bold := Style{FG: t.Fg, Attr: Bold}
	warn := Style{FG: t.Warn, Attr: Bold}
	var out []line
	add := func(k, v string) { out = append(out, kv(k, v, ks, vs)) }
	addS := func(k, v string, st Style) { out = append(out, kv(k, v, ks, st)) }
	blank := func() { out = append(out, txt("")) }

	col := m.colorOf(int64(md.id))
	title := Style{FG: col.Mix(RGB(255, 255, 255), 0.3), Attr: Bold}
	for _, l := range wrap(textutil.Sanitize(n.Name), w) {
		out = append(out, styled(l, title))
	}
	blank()
	out = append(out, styled("Path", ks))
	for _, l := range wrap(textutil.Sanitize(tr.Path(md.id)), w) {
		out = append(out, styled(l, vs))
	}
	if textutil.Sanitize(n.Name) != n.Name {
		out = append(out, styled("Name contains control or invalid characters (shown escaped)", warn))
	}
	blank()
	if n.IsDir() {
		addS("Total logical", fmt.Sprintf("%s  (%s bytes)", textutil.Size(n.TotSize), textutil.Count(n.TotSize)), bold)
		addS("Total allocated", fmt.Sprintf("%s  (%s bytes)", textutil.Size(n.TotAlloc), textutil.Count(n.TotAlloc)), bold)
		add("Files", textutil.Count(int64(n.Files)))
		add("Directories", textutil.Count(int64(n.Dirs)))
		if p := n.Parent; p != inventory.NoNode {
			add("Of parent", textutil.Percent(n.Total(m.sizeMode), tr.Node(p).Total(m.sizeMode)))
		}
		add("Of scan", textutil.Percent(n.Total(m.sizeMode), tr.Node(0).Total(m.sizeMode)))
		k := m.kidsOf(md.id)
		if len(k.ids) > 0 {
			add("Largest child", fmt.Sprintf("%s (%s)", textutil.Sanitize(tr.Node(k.ids[0]).Name), textutil.Size(k.sizes[0])))
		}
		if n.Errors > 0 {
			addS("Errors beneath", textutil.Count(int64(n.Errors)), Style{FG: t.Err, Attr: Bold})
		}
		state := "complete"
		switch {
		case n.Flags&inventory.FlagIncomplete != 0:
			state = "incomplete (not fully read)"
		case n.Flags&inventory.FlagScanned == 0 && m.scanning:
			state = "scanning…"
		case n.Flags&(inventory.FlagSkippedFS|inventory.FlagVirtualFS|inventory.FlagLoop) != 0:
			state = "not descended"
		}
		add("Scan state", state)
	} else {
		addS("Logical size", fmt.Sprintf("%s  (%s bytes)", textutil.Size(n.Size), textutil.Count(n.Size)), bold)
		alloc := fmt.Sprintf("%s  (%s bytes)", textutil.Size(n.Alloc), textutil.Count(n.Alloc))
		if n.Flags&inventory.FlagAllocUnknown != 0 {
			alloc = "unknown on this platform"
		}
		addS("Allocated size", alloc, bold)
		if p := n.Parent; p != inventory.NoNode {
			add("Of parent", textutil.Percent(n.Own(m.sizeMode), tr.Node(p).Total(m.sizeMode)))
		}
	}
	if r := m.search.result; r != nil {
		add("Matching filter", textutil.Size(r.Size(md.id)))
	}
	if d := m.opts.Diff; d != nil {
		if old, ok := d.OldSize(md.id, m.sizeMode); ok {
			add("In old snapshot", textutil.Size(old))
			addS("Change", textutil.SignedSize(n.Total(m.sizeMode)-old), bold)
		} else {
			addS("Change", "new since old snapshot", Style{FG: t.DiffNew, Attr: Bold})
		}
	}
	blank()
	add("Type", n.Kind.String())
	if !n.IsDir() {
		if e := tr.ExtName(n); e != "" {
			add("Extension", textutil.Sanitize(e))
		}
		add("Category", n.Cat.String())
	}
	blank()
	if n.MTime > 0 {
		add("Modified", fmtTime(n.MTime))
	}
	if md.btime > 0 {
		add("Created", fmtTime(md.btime))
	}
	blank()
	add("Mode", fs.FileMode(n.Mode).String())
	add("Owner", textutil.Sanitize(platform.UserName(n.UID)))
	add("Group", textutil.Sanitize(platform.GroupName(n.GID)))
	if !n.IsDir() {
		add("Hard links", fmt.Sprint(n.Nlink))
		sparse := "No"
		if n.Flags&inventory.FlagSparse != 0 {
			sparse = fmt.Sprintf("Yes (or compressed) — %s not allocated", textutil.Size(n.Size-n.Alloc))
		}
		add("Sparse", sparse)
	}
	if fl := export.FlagList(n.Flags &^ (inventory.FlagScanned | inventory.FlagSparse)); len(fl) > 0 {
		addS("Flags", strings.Join(fl, ", "), warn)
	}
	if n.Flags&inventory.FlagHardlinkDup != 0 {
		out = append(out, styled("Another path to this inode was counted; this one adds nothing to totals.", Style{FG: t.Muted}))
	}
	if m.snapshot {
		out = append(out, txt(""), styled("From snapshot — metadata as recorded, not live.", Style{FG: t.Muted}))
	}
	return out
}

func fmtTime(ns int64) string { return time.Unix(0, ns).Format("2006-01-02 15:04:05") }

func (md *infoModal) key(m *Model, k string) (bool, tea.Cmd) {
	switch k {
	case "c":
		return false, m.copyPath(m.tree.Path(md.id))
	case "o":
		m.sel = int64(md.id)
		// Headless, o navigates the map, which the modal would cover.
		return platform.Headless(), m.reveal()
	case "enter":
		return true, nil
	case "g", "right":
		m.jumpTo(md.id)
		return true, nil
	case "d", "delete":
		m.sel = int64(md.id)
		return true, m.requestTrash()
	}
	return false, nil
}

// ---- LOD group ------------------------------------------------------------------

type groupModal struct{ g *group }

func newGroupModal(_ *Model, g *group) modal { return &groupModal{g} }

func (*groupModal) width() int           { return 70 }
func (*groupModal) title() string        { return "Smaller items (grouped)" }
func (*groupModal) footer(*Model) string { return "Esc close  ·  Space shows them in the list view" }

func (md *groupModal) content(m *Model, w int) []line {
	t := m.theme
	out := []line{
		txt(fmt.Sprintf("%s items too small to draw individually at this size,", textutil.Count(int64(len(md.g.ids))))),
		txt(fmt.Sprintf("totalling %s. Zoom in or enlarge the terminal to see them.", textutil.Size(md.g.size))),
		txt(""),
	}
	for i, id := range md.g.ids {
		if i == 200 {
			out = append(out, styled(fmt.Sprintf("… and %s more", textutil.Count(int64(len(md.g.ids)-200))), Style{FG: t.Muted}))
			break
		}
		n := m.tree.Node(id)
		name := textutil.Sanitize(n.Name)
		if n.IsDir() {
			name += "/"
		}
		out = append(out, line{{textutil.PadLeft(textutil.Size(m.sizeOf(id)), 11) + "  ", &Style{FG: t.Muted}},
			{textutil.Truncate(name, w-13), &Style{FG: m.colorOf(int64(id)).Mix(t.Fg, 0.4)}}})
	}
	return out
}

func (md *groupModal) key(m *Model, k string) (bool, tea.Cmd) {
	if k == "enter" {
		m.sel = int64(md.g.ids[0])
		m.setViewByName("List")
		return true, nil
	}
	return false, nil
}

// ---- Help ---------------------------------------------------------------------

type helpModal struct{}

func newHelpModal(*Model) modal { return helpModal{} }

func (helpModal) width() int           { return 78 }
func (helpModal) title() string        { return "Keyboard reference" }
func (helpModal) footer(*Model) string { return "Esc close  ·  ↑↓ scroll" }

func (helpModal) content(m *Model, _ int) []line {
	t := m.theme
	h := Style{FG: t.Accent, Attr: Bold}
	k := Style{FG: t.Sel, Attr: Bold}
	d := Style{FG: t.Fg}
	var out []line
	sec := func(s string) {
		if len(out) > 0 {
			out = append(out, txt(""))
		}
		out = append(out, styled(s, h))
	}
	row := func(keys, desc string) { out = append(out, line{{"  " + textutil.PadRight(keys, 19), &k}, {desc, &d}}) }
	sec("NAVIGATION")
	row("↑ ↓ ← →  hjkl", "move spatially through the treemap")
	row("Enter", "inspect the selected item")
	row("Space  →", "zoom into the selected directory")
	row("Backspace  ←", "zoom out one level")
	row("Home", "back to the scan root")
	row("click / dbl-click", "select / zoom (mouse)")
	row("wheel", "zoom in/out (map), scroll (lists)")
	sec("SEARCH & FILTER")
	row("/", "search or filter; the map shows only matches")
	row("Esc", "clear the filter")
	out = append(out, styled("    ubuntu   *.iso   size > 5GB   ext IN (iso,qcow2)   age > 365d", Style{FG: t.Muted}))
	out = append(out, styled("    path contains cache AND NOT type = dir   owner = andy   flag = sparse", Style{FG: t.Muted}))
	fields := strings.Fields(filter.Fields)
	out = append(out, styled("    fields: "+strings.Join(fields[:7], " "), Style{FG: t.Muted}))
	out = append(out, styled("            "+strings.Join(fields[7:], " "), Style{FG: t.Muted}))
	out = append(out, styled("    sizes: KiB/MiB/GiB (1024), kB/MB/GB (1000); ages: s min h d w mo y", Style{FG: t.Muted}))
	sec("VIEWS")
	row("Tab  Shift+Tab", "next / previous view")
	row("1 – 7", "jump to a view")
	row("x", "file types (extension statistics)")
	row("g", "top lists: largest, oldest, sparse, hard links …")
	row("e", "scan information and errors")
	row("D", "find duplicate files (reads file contents)")
	sec("APPLICATION")
	row("a", "toggle allocated / logical (apparent) size")
	row("c", "copy path to clipboard")
	row("o", revealVerb())
	if !m.opts.ReadOnly {
		row("d", deleteVerb()+" (asks first)")
	}
	row("s", "save snapshot")
	row("r", "rescan")
	row("T", "cycle colour theme")
	row("Ctrl+C", "cancel scan; press again to quit")
	row("q", "quit")
	out = append(out, txt(""))
	units := "IEC (KiB = 1024 bytes)"
	if textutil.SI {
		units = "SI (kB = 1000 bytes)"
	}
	out = append(out, styled("Sizes: "+m.sizeMode.String()+" · units: "+units, Style{FG: t.Muted}))
	return out
}

func (helpModal) key(*Model, string) (bool, tea.Cmd) { return false, nil }

// ---- Confirmation -------------------------------------------------------------

type confirmModal struct {
	ttl   string
	body  []string
	yes   string
	onYes func(m *Model) tea.Cmd
}

func (c *confirmModal) width() int {
	w := 40
	for _, b := range c.body {
		w = max(w, textutil.Width(b))
	}
	return min(w, 90)
}
func (c *confirmModal) title() string        { return c.ttl }
func (c *confirmModal) footer(*Model) string { return "y " + c.yes + "  ·  n / Esc cancel" }
func (c *confirmModal) content(m *Model, w int) []line {
	var out []line
	for _, b := range c.body {
		for _, l := range wrap(b, w) {
			out = append(out, txt(l))
		}
	}
	return out
}
func (c *confirmModal) key(m *Model, k string) (bool, tea.Cmd) {
	switch k {
	case "y", "Y":
		return true, c.onYes(m)
	case "n", "N", "enter":
		return true, nil
	}
	return false, nil
}

// ---- Save snapshot ---------------------------------------------------------------

type saveModal struct{ ed lineEditor }

func newSaveModal(m *Model) modal {
	md := &saveModal{}
	md.ed.set(defaultSnapshotName(m.tree.Stats.Root))
	return md
}

func (*saveModal) editing() bool          { return true }
func (md *saveModal) editor() *lineEditor { return &md.ed }
func (*saveModal) width() int             { return 70 }
func (*saveModal) title() string          { return "Save snapshot" }
func (*saveModal) footer(*Model) string   { return "Enter save  ·  Esc cancel" }

func (md *saveModal) content(m *Model, w int) []line {
	t := m.theme
	out := []line{txt("File:")}
	s := textutil.Sanitize(md.ed.String())
	for _, l := range wrap(s+"█", w) {
		out = append(out, styled(l, Style{FG: t.Fg, Attr: Bold}))
	}
	if m.scanning {
		out = append(out, txt(""), styled("The scan is still running: the snapshot will be marked incomplete.", Style{FG: t.Warn}))
	}
	return out
}

func (md *saveModal) key(m *Model, k string) (bool, tea.Cmd) {
	switch k {
	case "esc":
		return true, nil
	case "enter":
		p := strings.TrimSpace(md.ed.String())
		if p == "" {
			return false, nil
		}
		return true, tea.Batch(m.saveSnapshot(p), m.info("Saving snapshot…"))
	}
	return false, nil
}

// revealVerb and deleteVerb describe o and d, which change meaning on a
// headless system.
func revealVerb() string {
	if platform.Headless() {
		return "show folder in map"
	}
	return "reveal in file manager"
}

func deleteVerb() string {
	if platform.Headless() {
		return "delete permanently"
	}
	return "move to " + platform.TrashName
}
