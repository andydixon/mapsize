package tui

import (
	"fmt"
	"sort"

	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/textutil"
	"github.com/andydixon/mapsize/internal/treemap"
)

// paintSide draws the large-layout side panel: directory summary, category
// breakdown and the selected item.
func (m *Model) paintSide(cv *Canvas, r treemap.Rect) {
	t := m.theme
	ps := t.panel()
	cv.Fill(r, " ", ps)
	cv.Fill(treemap.Rect{X: r.X + r.W - 1, Y: r.Y, W: 1, H: r.H}, "│", Style{FG: t.Faint, BG: t.PanelBg})
	w := r.W - 3
	x := r.X + 1
	y := r.Y + 1
	end := r.Y + r.H
	head := func(s string) {
		if y < end {
			cv.Text(x, y, s, w, Style{FG: t.Accent, BG: t.PanelBg, Attr: Bold})
			y++
		}
	}
	kvl := func(k, v string, vs Style) {
		if y < end {
			cv.Text(x, y, k, w, Style{FG: t.Muted, BG: t.PanelBg})
			cv.TextRight(x, y, w, v, vs)
			y++
		}
	}
	val := Style{FG: t.Fg, BG: t.PanelBg, Attr: Bold}
	tr := m.tree
	z := tr.Node(m.zoom)

	head("DIRECTORY")
	for _, l := range wrap(textutil.Sanitize(z.Name), w) {
		if y < end {
			cv.Text(x, y, l, w, Style{FG: t.Fg, BG: t.PanelBg, Attr: Bold})
			y++
		}
	}
	y++
	kvl("Size", textutil.Size(m.sizeOf(m.zoom)), val)
	kvl("Logical", textutil.Size(z.TotSize), Style{FG: t.Fg, BG: t.PanelBg})
	kvl("Allocated", textutil.Size(z.TotAlloc), Style{FG: t.Fg, BG: t.PanelBg})
	kvl("Files", textutil.Count(int64(z.Files)), Style{FG: t.Fg, BG: t.PanelBg})
	kvl("Directories", textutil.Count(int64(z.Dirs)), Style{FG: t.Fg, BG: t.PanelBg})
	if z.Errors > 0 {
		kvl("Errors", textutil.Count(int64(z.Errors)), Style{FG: t.Err, BG: t.PanelBg, Attr: Bold})
	}
	y++

	// Category breakdown with proportional bars.
	if cs := tr.CatSizes(m.zoom); cs != nil {
		head("CATEGORIES")
		type cat struct {
			c inventory.Category
			v int64
		}
		var cats []cat
		var total int64
		for i, v := range cs {
			if v > 0 {
				cats = append(cats, cat{inventory.Category(i), v})
				total += v
			}
		}
		sort.Slice(cats, func(i, j int) bool { return cats[i].v > cats[j].v })
		for _, c := range cats {
			if y >= end-8 {
				break
			}
			col := t.Cat[c.c]
			sw := "██ "
			if t.Mono || t.ASCII {
				sw = "   "
			}
			cv.Text(x, y, sw, 3, Style{FG: col, BG: t.PanelBg})
			cv.Text(x+3, y, c.c.String(), w-3, Style{FG: t.Fg, BG: t.PanelBg})
			cv.TextRight(x, y, w, textutil.Percent(c.v, total), Style{FG: t.Muted, BG: t.PanelBg})
			y++
			m.paintBar(cv, x+3, y, w-3, float64(c.v)/float64(cats[0].v), col, false)
			y++
		}
		y++
	}

	// Selected item.
	if y < end-4 {
		head("SELECTED")
		desc := []string{}
		if isGroup(m.sel) {
			if g, ok := m.groupInfo(m.sel); ok {
				desc = append(desc, fmt.Sprintf("%s smaller items", textutil.Count(int64(len(g.ids)))), textutil.Size(g.size))
			}
		} else if id := m.selectedNode(); id != inventory.NoNode {
			n := tr.Node(id)
			desc = append(desc, wrap(textutil.Sanitize(n.Name), w)...)
			desc = append(desc, textutil.Size(m.sizeOf(id))+"  ·  "+textutil.Percent(m.sizeOf(id), m.sizeOf(m.zoom)))
			if n.IsDir() {
				desc = append(desc, textutil.Count(int64(n.Files))+" files")
			} else {
				desc = append(desc, kindLabel(tr, n))
			}
		}
		for _, d := range desc {
			if y < end {
				cv.Text(x, y, d, w, Style{FG: t.Fg, BG: t.PanelBg})
				y++
			}
		}
	}
}
