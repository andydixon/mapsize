package tui

import (
	tea "charm.land/bubbletea/v2"

	"github.com/andydixon/mapsize/internal/textutil"
	"github.com/andydixon/mapsize/internal/treemap"
)

// column describes one table column.
type column struct {
	title string
	width int // 0 = flexible (takes the remaining space)
	right bool
}

// tableRow is one rendered row: cell strings plus optional per-cell styles.
type tableRow struct {
	cells  []string
	styles []*Style // nil = default
	bar    float64  // 0..1 for an optional bar column (-1 = none)
	barCol Color
}

// table is a scrollable list with a cursor, shared by the list-style views.
type table struct {
	cursor, offset int
	height         int // rows visible in the last paint
}

func (tb *table) move(d, n int) {
	if n == 0 {
		tb.cursor = 0
		return
	}
	tb.cursor = min(max(tb.cursor+d, 0), n-1)
}

// key handles navigation keys common to all tables.
func (tb *table) key(k string, n int) bool {
	page := max(tb.height-1, 1)
	switch k {
	case "up", "k":
		tb.move(-1, n)
	case "down", "j":
		tb.move(1, n)
	case "pgup", "ctrl+b":
		tb.move(-page, n)
	case "pgdown", "ctrl+f":
		tb.move(page, n)
	case "end", "G":
		tb.move(n, n)
	case "g":
		tb.cursor = 0
	default:
		return false
	}
	return true
}

func (tb *table) paint(m *Model, cv *Canvas, r treemap.Rect, cols []column, rows []tableRow, title string) {
	t := m.theme
	cv.Fill(r, " ", t.base())
	if r.H < 3 {
		return
	}
	y := r.Y
	if title != "" {
		cv.Text(r.X+1, y, title, r.W-2, Style{FG: t.Accent, BG: t.Bg, Attr: Bold})
		y++
	}
	// Resolve flexible width.
	fixed := 0
	for _, c := range cols {
		fixed += c.width + 1
	}
	flex := max(r.W-2-fixed, 8)
	x := r.X + 1
	hdr := Style{FG: t.Muted, BG: t.Bg, Attr: Bold}
	for _, c := range cols {
		w := c.width
		if w == 0 {
			w = flex
		}
		s := c.title
		if c.right {
			s = textutil.PadLeft(s, w)
		}
		cv.Text(x, y, s, w, hdr)
		x += w + 1
	}
	y++
	cv.HLine(r.X+1, y, r.W-2, "─", Style{FG: t.Faint, BG: t.Bg})
	y++
	tb.height = r.Y + r.H - y
	if tb.cursor >= len(rows) {
		tb.cursor = max(len(rows)-1, 0)
	}
	if tb.cursor < tb.offset {
		tb.offset = tb.cursor
	}
	if tb.cursor >= tb.offset+tb.height {
		tb.offset = tb.cursor - tb.height + 1
	}
	tb.offset = max(0, min(tb.offset, len(rows)-tb.height))
	for i := tb.offset; i < len(rows) && y < r.Y+r.H; i++ {
		row := rows[i]
		sel := i == tb.cursor
		base := t.base()
		if sel {
			base = Style{FG: t.textOn(t.Sel), BG: t.Sel, Attr: Bold | m.monoRev()}
			cv.Fill(treemap.Rect{X: r.X, Y: y, W: r.W, H: 1}, " ", base)
		}
		x := r.X + 1
		for ci, c := range cols {
			w := c.width
			if w == 0 {
				w = flex
			}
			s := ""
			if ci < len(row.cells) {
				s = row.cells[ci]
			}
			st := base
			if !sel && ci < len(row.styles) && row.styles[ci] != nil {
				st = *row.styles[ci]
			}
			if c.title == "" && row.bar >= 0 && c.width > 0 {
				m.paintBar(cv, x, y, w, row.bar, row.barCol, sel)
			} else if c.right {
				cv.Text(x, y, textutil.PadLeft(s, w), w, st)
			} else {
				cv.Text(x, y, textutil.Truncate(s, w), w, st)
			}
			x += w + 1
		}
		idx := i
		m.hits = append(m.hits, hit{treemap.Rect{X: r.X, Y: y, W: r.W, H: 1}, func(m *Model, double bool) tea.Cmd {
			tb.cursor = idx
			if double {
				return m.handleKeyName("enter")
			}
			return nil
		}})
		y++
	}
	if len(rows) == 0 {
		cv.TextCentered(r.X, r.Y+r.H/2, r.W, "Nothing to show", t.muted())
	}
	if len(rows) > tb.height && tb.height > 0 {
		pos := textutil.Count(int64(tb.cursor+1)) + "/" + textutil.Count(int64(len(rows)))
		cv.TextRight(r.X, r.Y, r.W-1, pos, t.muted())
	}
}

// paintBar draws a proportional bar with eighth-block precision.
func (m *Model) paintBar(cv *Canvas, x, y, w int, frac float64, col Color, sel bool) {
	t := m.theme
	if col == 0 {
		col = t.Accent
	}
	bg := t.Bg.Mix(t.Fg, 0.08)
	if sel {
		bg = t.Sel.Mix(RGB(0, 0, 0), 0.25)
	}
	eighths := int(frac*float64(w*8) + 0.5)
	parts := []string{"", "▏", "▎", "▍", "▌", "▋", "▊", "▉"}
	for i := 0; i < w; i++ {
		ch := " "
		switch {
		case eighths >= 8:
			ch = "█"
		case eighths > 0:
			ch = parts[eighths]
		}
		if t.ASCII {
			ch = map[bool]string{true: "#", false: " "}[eighths >= 4]
		}
		eighths -= 8
		st := Style{FG: col, BG: bg}
		if t.Mono {
			st = Style{}
		}
		cv.Text(x+i, y, ch, 1, st)
	}
}

// handleKeyName lets mouse actions reuse key handling (lock already held).
func (m *Model) handleKeyName(k string) tea.Cmd {
	if handled, cmd := m.views[m.view].key(m, k); handled {
		return cmd
	}
	switch k {
	case "enter":
		return m.inspectSelection()
	case "space":
		return m.zoomIntoSelection()
	}
	return nil
}

func plural(n int, s string) string {
	if n == 1 {
		return "1 " + s
	}
	return textutil.Count(int64(n)) + " " + s + "s"
}
