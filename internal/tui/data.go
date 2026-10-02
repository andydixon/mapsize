package tui

import (
	"sort"

	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/snapshot"
)

// kids is a cached, size-sorted child list (zero-size entries removed).
type kids struct {
	ids   []inventory.NodeID
	sizes []int64
	total int64
}

// sizeOf returns the size that drives layout: the filtered size when a
// filter is active, otherwise the aggregate in the current size mode.
// Callers hold the tree read lock.
func (m *Model) sizeOf(id inventory.NodeID) int64 {
	if r := m.search.result; r != nil {
		return r.Size(id)
	}
	n := m.tree.Node(id)
	if n.Flags&inventory.FlagDeleted != 0 {
		return 0
	}
	return n.Total(m.sizeMode)
}

// kidsOf returns the sorted children of id, cached until data changes.
func (m *Model) kidsOf(id inventory.NodeID) *kids {
	if m.kidsVer != m.dataVer {
		clear(m.kidsCache)
		m.kidsVer = m.dataVer
	}
	if k, ok := m.kidsCache[id]; ok {
		return k
	}
	k := &kids{}
	m.tree.Children(id, func(c inventory.NodeID, _ *inventory.Node) {
		if s := m.sizeOf(c); s > 0 {
			k.ids = append(k.ids, c)
			k.sizes = append(k.sizes, s)
			k.total += s
		}
	})
	sort.Sort(byKidSize{k})
	m.kidsCache[id] = k
	return k
}

type byKidSize struct{ *kids }

func (b byKidSize) Len() int { return len(b.ids) }
func (b byKidSize) Less(i, j int) bool {
	if b.sizes[i] != b.sizes[j] {
		return b.sizes[i] > b.sizes[j]
	}
	return b.ids[i] < b.ids[j]
}
func (b byKidSize) Swap(i, j int) {
	b.ids[i], b.ids[j] = b.ids[j], b.ids[i]
	b.sizes[i], b.sizes[j] = b.sizes[j], b.sizes[i]
}

// selectedNode returns the selected inventory node (NoNode for groups).
func (m *Model) selectedNode() inventory.NodeID {
	if m.sel == noSel || isGroup(m.sel) || m.sel >= int64(m.tree.Len()) {
		return inventory.NoNode
	}
	return inventory.NodeID(m.sel)
}

// ensureSelection keeps the selection meaningful after any change: if the
// selected node is no longer a direct, visible child of the zoom root it
// falls back to the visible ancestor, then the group containing it, then
// the largest child. Tree read lock held.
func (m *Model) ensureSelection() {
	k := m.kidsOf(m.zoom)
	if len(k.ids) == 0 {
		m.sel = noSel
		return
	}
	if !m.userSel {
		// Until the user picks something, follow the largest block (it
		// changes while a scan is running).
		m.sel = int64(k.ids[0])
		return
	}
	if isGroup(m.sel) {
		if groupParent(m.sel) == m.zoom {
			return
		}
		m.sel = noSel
	}
	id := m.selectedNode()
	if id != inventory.NoNode {
		// Walk up to the direct child of the zoom root.
		for cur := id; cur != inventory.NoNode; cur = m.tree.Node(cur).Parent {
			if m.tree.Node(cur).Parent == m.zoom {
				if m.sizeOf(cur) > 0 {
					m.sel = int64(cur)
					return
				}
				break
			}
		}
	}
	m.sel = int64(k.ids[0])
}

// visibleSel maps the selection onto the treemap blocks: a node collapsed
// into the LOD group selects the group.
func (m *Model) visibleSel(blocks map[int64]bool) int64 {
	if blocks[m.sel] {
		return m.sel
	}
	if !isGroup(m.sel) && blocks[groupID(m.zoom)] {
		return groupID(m.zoom)
	}
	return m.sel
}

// zoomTo makes dir the treemap root.
func (m *Model) zoomTo(dir inventory.NodeID) {
	if dir == m.zoom {
		return
	}
	prev := m.zoom
	m.zoom = dir
	// Coming back out, keep the directory we left selected.
	if m.tree.IsAncestor(dir, prev) {
		m.sel = int64(prev)
	} else {
		m.sel = noSel
	}
	m.userSel = m.sel != noSel
	m.zoomed = true
	m.dirty = true
}

func (m *Model) setView(i int) {
	if i >= 0 && i < len(m.views) {
		m.view = i
		m.dirty = true
	}
}

// colorOf returns the base colour for a block.
func (m *Model) colorOf(id int64) Color {
	t := m.theme
	if isGroup(id) {
		return t.Group
	}
	nid := inventory.NodeID(id)
	n := m.tree.Node(nid)
	if d := m.opts.Diff; d != nil {
		switch d.StatusOf(nid, m.sizeMode) {
		case snapshot.Added:
			return t.DiffNew
		case snapshot.Grew:
			old, _ := d.OldSize(nid, m.sizeMode)
			return t.DiffSame.Mix(t.DiffGrow, ratio(n.Total(m.sizeMode)-old, old))
		case snapshot.Shrank:
			old, _ := d.OldSize(nid, m.sizeMode)
			return t.DiffSame.Mix(t.DiffShrink, ratio(old-n.Total(m.sizeMode), old))
		}
		return t.DiffSame
	}
	if !n.IsDir() {
		if t.Ext != nil {
			if c, ok := t.Ext[m.tree.ExtName(n)]; ok {
				return c
			}
		}
		return t.Cat[n.Cat]
	}
	cs := m.tree.CatSizes(nid)
	best, bestV := -1, int64(0)
	for i, v := range cs {
		if v > bestV {
			best, bestV = i, v
		}
	}
	if best < 0 {
		return t.EmptyDir
	}
	return t.Cat[best]
}

// ratio maps a relative change to 0.35..1 for colour intensity.
func ratio(delta, base int64) float64 {
	if base <= 0 {
		return 1
	}
	r := float64(delta) / float64(base)
	return min(1, 0.35+r*0.65)
}
