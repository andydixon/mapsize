package tui

import (
	"fmt"
	"sort"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/snapshot"
	"github.com/andydixon/mapsize/internal/textutil"
	"github.com/andydixon/mapsize/internal/treemap"
)

// ---- Directory list -------------------------------------------------------

type listView struct {
	tb      table
	lastSel int64
}

func (*listView) name() string { return "List" }

// sync reconciles the table cursor with the shared selection: if the
// selection changed elsewhere (treemap, jump) the cursor follows it;
// otherwise the cursor (moved by keys or clicks) drives the selection.
func (v *listView) sync(m *Model) {
	k := m.kidsOf(m.zoom)
	if m.sel != v.lastSel {
		for i, id := range k.ids {
			if int64(id) == m.sel {
				v.tb.cursor = i
			}
		}
	} else if v.tb.cursor < len(k.ids) {
		m.sel = int64(k.ids[v.tb.cursor])
	}
	v.lastSel = m.sel
}

func (v *listView) paint(m *Model, cv *Canvas, r treemap.Rect) {
	t := m.theme
	k := m.kidsOf(m.zoom)
	v.sync(m)
	total := m.sizeOf(m.zoom)
	rows := make([]tableRow, len(k.ids))
	for i, id := range k.ids {
		n := m.tree.Node(id)
		name := textutil.Sanitize(n.Name)
		files := ""
		if n.IsDir() {
			name += "/"
			files = textutil.Count(int64(n.Files))
		}
		mod := ""
		if n.MTime > 0 {
			mod = time.Unix(0, n.MTime).Format("2006-01-02")
		}
		col := m.colorOf(int64(id))
		nameSt := &Style{FG: col.Mix(t.Fg, 0.45), BG: t.Bg}
		if n.IsDir() {
			nameSt.Attr = Bold
		}
		frac := 0.0
		if k.sizes[0] > 0 {
			frac = float64(k.sizes[i]) / float64(k.sizes[0])
		}
		rows[i] = tableRow{
			cells:  []string{textutil.Size(k.sizes[i]), textutil.Percent(k.sizes[i], total), "", files, mod, name},
			styles: []*Style{nil, {FG: t.Muted, BG: t.Bg}, nil, {FG: t.Muted, BG: t.Bg}, {FG: t.Muted, BG: t.Bg}, nameSt},
			bar:    frac, barCol: col,
		}
		if m.opts.Diff != nil {
			rows[i].cells[4] = textutil.SignedSize(m.opts.Diff.Delta(id, m.sizeMode))
		}
	}
	cols := []column{{"SIZE", 10, true}, {"%", 5, true}, {"", 12, false}, {"FILES", 10, true}, {"MODIFIED", 10, false}, {"NAME", 0, false}}
	if m.opts.Diff != nil {
		cols[4] = column{"CHANGE", 12, true}
	}
	if r.W < 70 {
		cols = []column{{"SIZE", 10, true}, {"%", 5, true}, {"NAME", 0, false}}
		for i := range rows {
			rows[i].cells = []string{rows[i].cells[0], rows[i].cells[1], rows[i].cells[5]}
			rows[i].styles = []*Style{nil, rows[i].styles[1], rows[i].styles[5]}
		}
	}
	v.tb.paint(m, cv, r, cols, rows, "")
	v.sync(m)
}

func (v *listView) key(m *Model, k string) (bool, tea.Cmd) {
	v.sync(m)
	defer v.sync(m)
	return v.tb.key(k, len(m.kidsOf(m.zoom).ids)), nil
}

func (v *listView) wheel(m *Model, up bool) tea.Cmd {
	v.sync(m)
	v.tb.move(map[bool]int{true: -3, false: 3}[up], len(m.kidsOf(m.zoom).ids))
	v.sync(m)
	return nil
}

// ---- Extension statistics ------------------------------------------------

type extView struct {
	tb    table
	cache []inventory.ExtStat
	key_  [2]int
}

func (*extView) name() string { return "Types" }

func (v *extView) stats(m *Model) []inventory.ExtStat {
	k := [2]int{int(m.zoom), m.dataVer*2 + int(m.sizeMode)}
	if v.cache == nil || v.key_ != k {
		v.cache = m.tree.ExtStats(m.zoom)
		sort.Slice(v.cache, func(i, j int) bool {
			a, b := v.cache[i], v.cache[j]
			as, bs := a.Alloc, b.Alloc
			if m.sizeMode == inventory.SizeLogical {
				as, bs = a.Size, b.Size
			}
			if as != bs {
				return as > bs
			}
			return a.Ext < b.Ext
		})
		v.key_ = k
	}
	return v.cache
}

func (v *extView) paint(m *Model, cv *Canvas, r treemap.Rect) {
	t := m.theme
	st := v.stats(m)
	var total int64
	for _, s := range st {
		total += pick(m, s)
	}
	rows := make([]tableRow, len(st))
	for i, s := range st {
		ext := "." + s.Ext
		if s.Ext == "" {
			ext = "(none)"
		}
		col := t.Cat[s.Cat]
		if c, ok := t.Ext[s.Ext]; ok {
			col = c
		}
		frac := 0.0
		if v0 := pick(m, st[0]); v0 > 0 {
			frac = float64(pick(m, s)) / float64(v0)
		}
		rows[i] = tableRow{
			cells:  []string{textutil.Sanitize(ext), s.Cat.String(), textutil.Count(s.Count), textutil.Size(pick(m, s)), textutil.Percent(pick(m, s), total), ""},
			styles: []*Style{{FG: col, BG: t.Bg, Attr: Bold}, {FG: t.Muted, BG: t.Bg}, nil, nil, {FG: t.Muted, BG: t.Bg}, nil},
			bar:    frac, barCol: col,
		}
	}
	cols := []column{{"EXTENSION", 14, false}, {"CATEGORY", 12, false}, {"FILES", 11, true}, {"SIZE", 10, true}, {"%", 5, true}, {"", 0, false}}
	v.tb.paint(m, cv, r, cols, rows, fmt.Sprintf("File types under %s — Enter filters the map by extension", textutil.Sanitize(m.tree.Node(m.zoom).Name)))
}

func pick(m *Model, s inventory.ExtStat) int64 {
	if m.sizeMode == inventory.SizeLogical {
		return s.Size
	}
	return s.Alloc
}

func (v *extView) key(m *Model, k string) (bool, tea.Cmd) {
	st := v.stats(m)
	if v.tb.key(k, len(st)) {
		return true, nil
	}
	if (k == "enter" || k == "space") && v.tb.cursor < len(st) {
		e := st[v.tb.cursor].Ext
		q := "ext = \"" + e + "\""
		if e == "" {
			q = "type = file AND ext = \"\""
		}
		m.setViewByName("Map")
		return true, m.setFilter(q)
	}
	return false, nil
}

func (v *extView) wheel(m *Model, up bool) tea.Cmd {
	v.tb.move(map[bool]int{true: -3, false: 3}[up], len(v.stats(m)))
	return nil
}

// ---- Top-N investigation views --------------------------------------------

type topMode int

const (
	topFiles topMode = iota
	topDirs
	topOld
	topNew
	topCount
	topSparse
	topHardlinks
	numTopModes
)

var topNames = [...]string{"Largest files", "Largest directories (own files)", "Oldest large files",
	"Newest large files", "Most files", "Sparse files", "Hard-linked files"}

type topView struct {
	tb    table
	mode  topMode
	ids   []inventory.NodeID
	vals  []int64
	key_  [3]int
	limit int
}

func (*topView) name() string { return "Top" }

const topLimit = 500

func (v *topView) compute(m *Model) {
	k := [3]int{int(m.zoom), m.dataVer*2 + int(m.sizeMode), int(v.mode)}
	if v.ids != nil && v.key_ == k {
		return
	}
	v.key_ = k
	tr := m.tree
	isFile := func(n *inventory.Node) bool {
		return n.Kind == inventory.KindFile && n.Flags&(inventory.FlagHardlinkDup|inventory.FlagDeleted) == 0
	}
	matches := func(id inventory.NodeID) bool {
		return m.search.result == nil || m.search.result.Size(id) > 0
	}
	tk := inventory.NewTopK(topLimit)
	val := map[inventory.NodeID]int64{}
	const largeFile = 1 << 20
	switch v.mode {
	case topDirs:
		own := map[inventory.NodeID]int64{}
		tr.Walk(m.zoom, func(id inventory.NodeID, n *inventory.Node) bool {
			if isFile(n) && matches(id) {
				own[n.Parent] += n.Own(m.sizeMode)
			}
			return true
		})
		for id, s := range own {
			tk.Offer(id, s)
			val[id] = s
		}
	case topCount:
		tr.Walk(m.zoom, func(id inventory.NodeID, n *inventory.Node) bool {
			if n.IsDir() && matches(id) {
				c := int64(tr.ChildCount(id))
				tk.Offer(id, c)
				val[id] = c
			}
			return true
		})
	default:
		tr.Walk(m.zoom, func(id inventory.NodeID, n *inventory.Node) bool {
			if !isFile(n) || !matches(id) {
				return true
			}
			sz := n.Own(m.sizeMode)
			switch v.mode {
			case topFiles:
				tk.Offer(id, sz)
				val[id] = sz
			case topOld, topNew:
				if sz >= largeFile && n.MTime > 0 {
					key := n.MTime
					if v.mode == topOld {
						key = -key
					}
					tk.Offer(id, key)
					val[id] = sz
				}
			case topSparse:
				if n.Flags&inventory.FlagSparse != 0 {
					tk.Offer(id, n.Size-n.Alloc)
					val[id] = n.Size - n.Alloc
				}
			case topHardlinks:
				if n.Flags&inventory.FlagHardlinked != 0 {
					tk.Offer(id, n.Size)
					val[id] = n.Size
				}
			}
			return true
		})
	}
	v.ids = tk.Sorted()
	v.vals = v.vals[:0]
	for _, id := range v.ids {
		v.vals = append(v.vals, val[id])
	}
}

func (v *topView) paint(m *Model, cv *Canvas, r treemap.Rect) {
	t := m.theme
	v.compute(m)
	rows := make([]tableRow, len(v.ids))
	var first int64
	if len(v.vals) > 0 {
		first = v.vals[0]
	}
	for i, id := range v.ids {
		n := m.tree.Node(id)
		val := textutil.Size(v.vals[i])
		if v.mode == topCount {
			val = textutil.Count(v.vals[i])
		}
		mod := ""
		if n.MTime > 0 {
			mod = time.Unix(0, n.MTime).Format("2006-01-02")
		}
		frac := 0.0
		if first > 0 && v.mode != topOld && v.mode != topNew {
			frac = float64(v.vals[i]) / float64(first)
		}
		col := m.colorOf(int64(id))
		rows[i] = tableRow{
			cells:  []string{val, mod, "", textutil.Sanitize(m.tree.Path(id))},
			styles: []*Style{nil, {FG: t.Muted, BG: t.Bg}, nil, {FG: col.Mix(t.Fg, 0.45), BG: t.Bg}},
			bar:    frac, barCol: col,
		}
	}
	title := fmt.Sprintf("◂ %s ▸   (←/→ change list · Enter details · Space jump to it)", topNames[v.mode])
	if m.search.result != nil {
		title += "  · filtered"
	}
	valTitle := "SIZE"
	switch v.mode {
	case topCount:
		valTitle = "ENTRIES"
	case topSparse:
		valTitle = "SAVED"
	}
	cols := []column{{valTitle, 10, true}, {"MODIFIED", 10, false}, {"", 10, false}, {"PATH", 0, false}}
	v.tb.paint(m, cv, r, cols, rows, title)
}

func (v *topView) key(m *Model, k string) (bool, tea.Cmd) {
	v.compute(m)
	switch k {
	case "left", "h":
		v.mode = (v.mode + numTopModes - 1) % numTopModes
		v.tb.cursor = 0
		return true, nil
	case "right", "l":
		v.mode = (v.mode + 1) % numTopModes
		v.tb.cursor = 0
		return true, nil
	}
	if v.tb.key(k, len(v.ids)) {
		return true, nil
	}
	if v.tb.cursor < len(v.ids) {
		id := v.ids[v.tb.cursor]
		switch k {
		case "enter":
			m.openModal(newInfoModal(m, id))
			return true, nil
		case "space":
			m.jumpTo(id)
			return true, nil
		case "c", "o", "d":
			m.sel = int64(id)
		}
	}
	return false, nil
}

func (v *topView) wheel(m *Model, up bool) tea.Cmd {
	v.tb.move(map[bool]int{true: -3, false: 3}[up], len(v.ids))
	return nil
}

// ---- Snapshot comparison -----------------------------------------------------

type changesView struct {
	tb   table
	rows []snapshot.Change
	key_ int
}

func (*changesView) name() string { return "Changes" }

func (v *changesView) paint(m *Model, cv *Canvas, r treemap.Rect) {
	t := m.theme
	d := m.opts.Diff
	if v.rows == nil || v.key_ != int(m.sizeMode) {
		v.rows = d.Changes(1000, m.sizeMode)
		v.key_ = int(m.sizeMode)
	}
	var maxAbs int64 = 1
	for _, c := range v.rows {
		maxAbs = max(maxAbs, abs64(c.Delta))
	}
	rows := make([]tableRow, len(v.rows))
	for i, c := range v.rows {
		col := map[snapshot.Status]Color{snapshot.Added: t.DiffNew, snapshot.Removed: t.Muted,
			snapshot.Grew: t.DiffGrow, snapshot.Shrank: t.DiffShrink}[c.Status]
		rows[i] = tableRow{
			cells:  []string{textutil.SignedSize(c.Delta), c.Status.String(), textutil.Size(c.Old), textutil.Size(c.New), "", textutil.Sanitize(c.Path)},
			styles: []*Style{{FG: col, BG: t.Bg, Attr: Bold}, {FG: col, BG: t.Bg}, {FG: t.Muted, BG: t.Bg}, nil, nil, nil},
			bar:    float64(abs64(c.Delta)) / float64(maxAbs), barCol: col,
		}
	}
	cols := []column{{"CHANGE", 13, true}, {"STATUS", 9, false}, {"OLD", 10, true}, {"NEW", 10, true}, {"", 10, false}, {"PATH", 0, false}}
	ro, rn := d.Old.Node(0).Total(m.sizeMode), d.New.Node(0).Total(m.sizeMode)
	title := fmt.Sprintf("Changes  %s → %s  (%s)", textutil.Size(ro), textutil.Size(rn), textutil.SignedSize(rn-ro))
	v.tb.paint(m, cv, r, cols, rows, title)
}

func abs64(v int64) int64 {
	if v < 0 {
		return -v
	}
	return v
}

func (v *changesView) key(m *Model, k string) (bool, tea.Cmd) {
	if v.tb.key(k, len(v.rows)) {
		return true, nil
	}
	if v.tb.cursor < len(v.rows) {
		c := v.rows[v.tb.cursor]
		if c.Node == inventory.NoNode {
			if k == "enter" || k == "space" {
				return true, m.info("Removed since the old snapshot: " + textutil.Sanitize(c.Path))
			}
			return false, nil
		}
		switch k {
		case "enter":
			m.openModal(newInfoModal(m, c.Node))
			return true, nil
		case "space":
			m.jumpTo(c.Node)
			return true, nil
		}
	}
	return false, nil
}

func (v *changesView) wheel(m *Model, up bool) tea.Cmd {
	v.tb.move(map[bool]int{true: -3, false: 3}[up], len(v.rows))
	return nil
}
