package tui

import (
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/andydixon/mapsize/internal/inventory"
)

const doubleClick = 400 * time.Millisecond

func (m *Model) mouseClick(ev tea.Mouse) tea.Cmd {
	t := m.tree
	t.RLock()
	defer t.RUnlock()
	m.dirty = true
	if len(m.modals) > 0 {
		// Clicking outside the modal closes it.
		if len(m.hits) > 0 && !m.hits[0].r.Contains(ev.X, ev.Y) {
			m.closeModal()
		}
		return nil
	}
	if ev.Button == tea.MouseRight {
		for _, h := range m.hits {
			if h.r.Contains(ev.X, ev.Y) {
				h.fn(m, false)
				return m.inspectSelection()
			}
		}
		return nil
	}
	if ev.Button != tea.MouseLeft {
		return nil
	}
	for i := len(m.hits) - 1; i >= 0; i-- {
		h := m.hits[i]
		if h.r.Contains(ev.X, ev.Y) {
			key := int64(ev.Y)<<32 | int64(i)
			double := time.Since(m.lastClick) < doubleClick && m.lastClickI == key
			m.lastClick, m.lastClickI = time.Now(), key
			if double {
				m.lastClickI = noSel
			}
			return h.fn(m, double)
		}
	}
	return nil
}

// clickBlock selects a treemap block; a double click zooms (or inspects).
func (m *Model) clickBlock(id int64, double bool) tea.Cmd {
	m.sel = id
	if !double {
		return nil
	}
	if !isGroup(id) && !m.tree.Node(inventory.NodeID(id)).IsDir() {
		return m.inspectSelection()
	}
	return m.zoomIntoSelection()
}

func (m *Model) mouseWheel(ev tea.Mouse) tea.Cmd {
	t := m.tree
	t.RLock()
	defer t.RUnlock()
	m.dirty = true
	up := ev.Button == tea.MouseWheelUp
	if ev.Button != tea.MouseWheelUp && ev.Button != tea.MouseWheelDown {
		return nil
	}
	if len(m.modals) > 0 {
		f := m.modals[len(m.modals)-1].(*modalFrame)
		if up {
			f.scroll -= 3
		} else {
			f.scroll += 3
		}
		f.clampScroll()
		return nil
	}
	if up {
		// Zoom towards the block under the pointer.
		if l := m.tm; l != nil && m.view == 0 {
			for _, b := range l.blocks {
				if b.Contains(ev.X, ev.Y) {
					m.sel = b.ID
				}
			}
		}
	}
	return m.views[m.view].wheel(m, up)
}

func (m *Model) mouseMotion(ev tea.Mouse) {
	h := noSel
	if l := m.tm; l != nil && m.view == 0 && len(m.modals) == 0 {
		for _, b := range l.blocks {
			if b.Contains(ev.X, ev.Y) {
				h = b.ID
				break
			}
		}
	}
	if h != m.hover {
		m.hover = h
		m.dirty = true
	}
}
