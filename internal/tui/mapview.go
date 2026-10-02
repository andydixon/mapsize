package tui

import (
	tea "charm.land/bubbletea/v2"

	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/textutil"
	"github.com/andydixon/mapsize/internal/treemap"
)

// tmLayout is the treemap geometry for one (zoom, rect, data version). It is
// recomputed from scratch whenever any of those change — after a resize the
// layout algorithm runs again for the new rectangle rather than scaling the
// old one.
type tmLayout struct {
	zoom    inventory.NodeID
	rect    treemap.Rect
	ver     int
	blocks  []treemap.Block // top level: navigable
	nested  []nested        // previews inside directories: drawn only
	groups  map[int64]*group
	visible map[int64]bool
	inner   map[int64]bool // top-level blocks with nested previews
}

type nested struct {
	id    int64
	r     treemap.Rect
	depth int
	alt   bool
	inner bool // has nested children drawn inside it
}

type group struct {
	parent inventory.NodeID
	ids    []inventory.NodeID
	size   int64
}

// LOD parameters.
const (
	topMinArea   = 12 // cells: room for a 4×3 framed block
	topMaxItems  = 400
	nestMinArea  = 3
	nestMaxItems = 120
	maxNestDepth = 3
	nestMinW     = 4
	nestMinH     = 2
	nestRecurseW = 14
	nestRecurseH = 5
)

func (m *Model) layoutTreemap(r treemap.Rect) *tmLayout {
	if l := m.tm; l != nil && l.zoom == m.zoom && l.rect == r && l.ver == m.dataVer {
		return l
	}
	l := &tmLayout{zoom: m.zoom, rect: r, ver: m.dataVer, groups: map[int64]*group{}, visible: map[int64]bool{}, inner: map[int64]bool{}}
	l.blocks = m.layoutLevel(l, m.zoom, r, topMinArea, topMaxItems)
	for _, b := range l.blocks {
		l.visible[b.ID] = true
		if b.ID < 0 || b.W < 3 || b.H < 2 {
			continue
		}
		if n := m.tree.Node(inventory.NodeID(b.ID)); n.IsDir() {
			in := b.Inset(1)
			if in.W >= nestMinW && in.H >= nestMinH {
				before := len(l.nested)
				m.layoutNested(l, inventory.NodeID(b.ID), in, 2)
				l.inner[b.ID] = len(l.nested) > before
			}
		}
	}
	m.tm = l
	return l
}

// layoutLevel lays out the children of parent in r with level-of-detail
// grouping: children too small to be useful collapse into one group block.
func (m *Model) layoutLevel(l *tmLayout, parent inventory.NodeID, r treemap.Rect, minArea, maxItems int) []treemap.Block {
	k := m.kidsOf(parent)
	if len(k.ids) == 0 || r.Empty() {
		return nil
	}
	sizes := make([]float64, len(k.sizes))
	for i, s := range k.sizes {
		sizes[i] = float64(s)
	}
	vis := treemap.Visible(sizes, float64(k.total), r.Area(), minArea, maxItems)
	items := make([]treemap.Item, 0, vis+1)
	for i := range vis {
		items = append(items, treemap.Item{ID: int64(k.ids[i]), Size: sizes[i]})
	}
	if vis < len(k.ids) {
		g := &group{parent: parent, ids: k.ids[vis:]}
		for _, s := range k.sizes[vis:] {
			g.size += s
		}
		gid := groupID(parent)
		l.groups[gid] = g
		items = append(items, treemap.Item{ID: gid, Size: float64(g.size)})
	}
	return treemap.Squarify(items, r)
}

func (m *Model) layoutNested(l *tmLayout, parent inventory.NodeID, r treemap.Rect, depth int) {
	// Deeper levels need bigger blocks to stay legible rather than noisy.
	blocks := m.layoutLevel(l, parent, r, nestMinArea*(depth-1)*(depth-1), nestMaxItems)
	for i, b := range blocks {
		nb := nested{id: b.ID, r: b.Rect, depth: depth, alt: i%2 == 1}
		idx := len(l.nested)
		l.nested = append(l.nested, nb)
		if depth < maxNestDepth && b.ID >= 0 && b.W >= nestRecurseW && b.H >= nestRecurseH &&
			m.tree.Node(inventory.NodeID(b.ID)).IsDir() {
			l.nested[idx].inner = true
			m.layoutNested(l, inventory.NodeID(b.ID), treemap.Rect{X: b.X, Y: b.Y + 1, W: b.W - 1, H: b.H - 2}, depth+1)
		}
	}
}

// groupInfo returns the LOD group for a group ID in the current layout.
func (m *Model) groupInfo(id int64) (*group, bool) {
	if m.tm == nil {
		return nil, false
	}
	g, ok := m.tm.groups[id]
	return g, ok
}

type mapView struct{}

func (*mapView) name() string { return "Map" }

func (v *mapView) paint(m *Model, cv *Canvas, r treemap.Rect) {
	t := m.theme
	l := m.layoutTreemap(r)
	if len(l.blocks) == 0 {
		msg := "Empty directory"
		switch {
		case m.search.query != nil:
			msg = "No matches here — Esc clears the filter"
		case m.scanning:
			msg = "Scanning…"
		case !m.tree.Node(m.zoom).IsDir():
			msg = textutil.Sanitize(m.tree.Node(m.zoom).Name) + " — " + textutil.Size(m.sizeOf(m.zoom))
		}
		cv.TextCentered(r.X, r.Y+r.H/2, r.W, msg, t.muted())
		return
	}
	sel := m.visibleSel(l.visible)
	total := m.sizeOf(m.zoom)
	for _, b := range l.blocks {
		m.paintTop(cv, b, b.ID == sel, b.ID == m.hover, total)
	}
	for _, nb := range l.nested {
		m.paintNested(cv, nb)
	}
	for _, b := range l.blocks {
		id := b.ID
		m.hits = append(m.hits, hit{b.Rect, func(m *Model, double bool) tea.Cmd { return m.clickBlock(id, double) }})
	}
}

// blockName returns a sanitised display name for a block.
func (m *Model) blockName(id int64) string {
	if isGroup(id) {
		if g, ok := m.groupInfo(id); ok {
			return textutil.Count(int64(len(g.ids))) + " smaller items"
		}
		return "smaller items"
	}
	n := m.tree.Node(inventory.NodeID(id))
	name := textutil.Sanitize(n.Name)
	if n.IsDir() && inventory.NodeID(id) != m.tree.Root() {
		name += "/"
	}
	return name
}

func (m *Model) blockSize(id int64) int64 {
	if isGroup(id) {
		if g, ok := m.groupInfo(id); ok {
			return g.size
		}
		return 0
	}
	return m.sizeOf(inventory.NodeID(id))
}

func (m *Model) paintTop(cv *Canvas, b treemap.Block, sel, hov bool, total int64) {
	t := m.theme
	base := m.colorOf(b.ID)
	fill := base.Mix(t.Bg, 0.62)
	fillCh := " "
	if isGroup(b.ID) {
		fillCh = "░"
		if t.ASCII {
			fillCh = "."
		}
	}
	name := m.blockName(b.ID)
	size := m.blockSize(b.ID)
	if b.W < 3 || b.H < 2 {
		// Too small for a frame: a solid tile.
		st := Style{FG: base, BG: base.Mix(t.Bg, 0.3)}
		ch := fillCh
		if sel {
			st = Style{FG: t.textOn(t.Sel), BG: t.Sel, Attr: Bold | m.monoRev()}
			ch = "◆"
			if t.ASCII {
				ch = "#"
			}
		} else if t.Mono {
			ch = "▒"
		}
		cv.Fill(b.Rect, " ", st)
		cv.Text(b.X+(b.W-1)/2, b.Y+(b.H-1)/2, ch, 1, st)
		return
	}
	cv.Fill(b.Rect, fillCh, Style{FG: base.Mix(t.Bg, 0.35), BG: fill})
	frame := Style{FG: base.Mix(t.Bg, 0.15), BG: fill}
	box := t.box()
	title := Style{FG: base.Mix(RGB(255, 255, 255), 0.35), BG: fill, Attr: Bold}
	switch {
	case sel:
		box = t.selBox()
		frame = Style{FG: t.Sel, BG: fill, Attr: Bold}
		title = Style{FG: t.textOn(t.Sel), BG: t.Sel, Attr: Bold | m.monoRev()}
	case hov:
		frame.FG = base.Mix(RGB(255, 255, 255), 0.5)
	}
	if t.Mono && !sel {
		title.Attr = Bold
	}
	cv.Box(b.Rect, box, frame)
	cv.Text(b.X+1, b.Y, textutil.Truncate(" "+name+" ", b.W-2), b.W-2, title)

	label := textutil.Size(size)
	if pct := textutil.Percent(size, total); b.W-2 >= textutil.Width(label)+textutil.Width(pct)+5 {
		label += " · " + pct
	} else if b.W-2 < textutil.Width(label)+2 {
		label = textutil.SizeCompact(size)
	}
	label = " " + label + " "
	if b.H >= 3 && textutil.Width(label) <= b.W-2 {
		cv.TextRight(b.X+1, b.Y+b.H-1, b.W-2, label, Style{FG: frame.FG, BG: fill, Attr: frame.Attr})
	}
	// Interior labels for blocks without nested previews.
	in := b.Inset(1)
	if in.Empty() || m.hasNested(b.ID) {
		return
	}
	n := inventory.NoNode
	if !isGroup(b.ID) {
		n = inventory.NodeID(b.ID)
	}
	lines := []string{}
	if n != inventory.NoNode {
		nd := m.tree.Node(n)
		if !nd.IsDir() {
			lines = append(lines, kindLabel(m.tree, nd))
		} else if nd.Files > 0 {
			lines = append(lines, textutil.Count(int64(nd.Files))+" files")
		}
	}
	if b.H < 4 || in.W < 6 {
		return
	}
	y0 := in.Y + (in.H-len(lines))/2
	for i, s := range lines {
		cv.TextCentered(in.X, y0+i, in.W, s, Style{FG: base.Mix(t.Fg, 0.4), BG: fill})
	}
}

func (m *Model) hasNested(id int64) bool { return m.tm != nil && m.tm.inner[id] }

func (m *Model) paintNested(cv *Canvas, nb nested) {
	t := m.theme
	base := m.colorOf(nb.id)
	f := 0.38 - 0.1*float64(nb.depth-2)
	if nb.alt {
		f += 0.07
	}
	bg := base.Mix(t.Bg, f)
	shadow := bg.Mix(RGB(0, 0, 0), 0.45)
	st := Style{FG: t.textOn(bg), BG: bg}
	ch := " "
	if isGroup(nb.id) {
		ch = "░"
		if t.ASCII {
			ch = "."
		}
		st.FG = bg.Mix(t.Fg, 0.25)
	}
	r := nb.r
	cv.Fill(r, ch, st)
	if t.Mono {
		// No colour: draw a light outline so neighbours stay distinguishable.
		if r.W >= 2 && r.H >= 2 {
			cv.Box(r, BoxRound, Style{Attr: Faint})
		}
	} else {
		if r.W >= 4 {
			cv.Fill(treemap.Rect{X: r.X + r.W - 1, Y: r.Y, W: 1, H: r.H}, " ", Style{BG: shadow})
		}
		if r.H >= 3 {
			cv.Fill(treemap.Rect{X: r.X, Y: r.Y + r.H - 1, W: r.W, H: 1}, " ", Style{BG: shadow})
		}
	}
	lw := r.W - 1
	if t.Mono {
		lw = r.W - 2
	}
	if lw < 5 {
		return
	}
	lx := r.X
	if t.Mono {
		lx++
	}
	ly := r.Y
	if t.Mono && r.H >= 3 {
		ly++
	}
	name := m.blockName(nb.id)
	if textutil.Width(name) > lw && lw < 8 {
		return // a stub like "te…" is noise, not information
	}
	cv.Text(lx, ly, textutil.Truncate(name, lw), lw, Style{FG: st.FG, BG: bg, Attr: Bold})
	if !nb.inner && r.H >= 3 && lw >= 5 {
		cv.Text(lx, ly+1, textutil.Truncate(textutil.SizeCompact(m.blockSize(nb.id)), lw), lw, Style{FG: bg.Mix(st.FG, 0.7), BG: bg})
	}
}

func (v *mapView) key(m *Model, k string) (bool, tea.Cmd) {
	dirs := map[string]treemap.Direction{"left": treemap.Left, "right": treemap.Right, "up": treemap.Up, "down": treemap.Down,
		"h": treemap.Left, "l": treemap.Right, "k": treemap.Up, "j": treemap.Down}
	if d, ok := dirs[k]; ok {
		if l := m.tm; l != nil {
			if id, ok := treemap.Neighbour(l.blocks, m.visibleSel(l.visible), d); ok {
				m.sel = id
			}
		}
		return true, nil
	}
	return false, nil
}

func (v *mapView) wheel(m *Model, up bool) tea.Cmd {
	if up {
		return m.zoomIntoSelection()
	}
	m.zoomOut()
	return nil
}
