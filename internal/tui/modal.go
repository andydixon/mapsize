package tui

import (
	"strings"

	tea "charm.land/bubbletea/v2"

	"github.com/andydixon/mapsize/internal/textutil"
	"github.com/andydixon/mapsize/internal/treemap"
)

// span is a run of styled text; a line is a slice of spans.
type span struct {
	s  string
	st *Style // nil = modal default
}

type line []span

func txt(s string) line                   { return line{{s, nil}} }
func styled(s string, st Style) line      { return line{{s, &st}} }
func kv(k, v string, kst, vst Style) line { return line{{textutil.PadRight(k, 18), &kst}, {v, &vst}} }

// modal is an overlay. Modals receive all keys while open; Esc always
// closes them (handled by the frame before the modal sees it, unless the
// modal is editing text).
type modal interface {
	title() string
	content(m *Model, width int) []line // rebuilt every frame, so resizes reflow
	footer(m *Model) string
	key(m *Model, k string) (closeModal bool, cmd tea.Cmd)
	width() int // preferred inner width
}

// modalFrame wraps a modal with scroll state.
type modalFrame struct {
	modal
	scroll   int
	viewH    int // content rows visible in the last paint
	contentN int
}

func (m *Model) openModal(md modal) {
	m.modals = append(m.modals, &modalFrame{modal: md})
	m.dirty = true
}

func (m *Model) closeModal() {
	if len(m.modals) > 0 {
		m.modals = m.modals[:len(m.modals)-1]
	}
	m.dirty = true
}

// textInputModal is implemented by modals that consume printable keys.
type textInputModal interface {
	editing() bool
	editor() *lineEditor
}

func (m *Model) modalKey(msg tea.KeyPressMsg) tea.Cmd {
	k := keyName(msg)
	f := m.modals[len(m.modals)-1].(*modalFrame)
	if ti, ok := f.modal.(textInputModal); ok && ti.editing() && k != "esc" && k != "enter" {
		ti.editor().key(msg)
		return nil
	}
	if ti, ok := f.modal.(textInputModal); !ok || !ti.editing() {
		page := max(f.viewH-1, 1)
		switch k {
		case "esc", "q":
			m.closeModal()
			return nil
		case "up", "k":
			f.scroll--
		case "down", "j":
			f.scroll++
		case "pgup":
			f.scroll -= page
		case "pgdown", "space":
			f.scroll += page
		case "home":
			f.scroll = 0
		case "end":
			f.scroll = f.contentN
		default:
			goto modalSpecific
		}
		f.clampScroll()
		return nil
	}
modalSpecific:
	closeIt, cmd := f.key(m, k)
	if closeIt {
		// Close this modal specifically (it may have opened another).
		for i, x := range m.modals {
			if x == f {
				m.modals = append(m.modals[:i], m.modals[i+1:]...)
				break
			}
		}
		m.dirty = true
	}
	return cmd
}

func (f *modalFrame) clampScroll() {
	f.scroll = max(0, min(f.scroll, f.contentN-f.viewH))
}

// paintModal dims the base screen and draws the top modal centred,
// clamped to the terminal with a one-cell margin. Content that does not fit
// scrolls.
func (m *Model) paintModal(cv *Canvas) {
	t := m.theme
	// Dim everything underneath.
	cv.Restyle(treemap.Rect{X: 0, Y: 0, W: cv.W, H: cv.H}, func(s Style) Style {
		if t.Mono {
			s.Attr |= Faint
			return s
		}
		s.FG = s.FG.Mix(t.Bg, 0.6)
		s.BG = s.BG.Mix(RGB(0, 0, 0), 0.45)
		s.Attr &^= Bold
		return s
	})
	f := m.modals[len(m.modals)-1].(*modalFrame)
	maxW := cv.W - 2
	w := min(f.width()+4, maxW)
	w = max(w, min(30, maxW))
	inner := w - 4
	lines := f.content(m, inner)
	foot := f.footer(m)
	chrome := 4 // border top/bottom, title rule
	if foot != "" {
		chrome += 2
	}
	h := min(len(lines)+chrome, cv.H-2)
	h = max(h, min(chrome+1, cv.H))
	x0, y0 := (cv.W-w)/2, (cv.H-h)/2
	r := treemap.Rect{X: x0, Y: y0, W: w, H: h}
	bg := t.PanelBg
	if bg == 0 {
		bg = t.Bg
	}
	base := Style{FG: t.Fg, BG: bg}
	// Drop shadow.
	if !t.Mono {
		cv.Restyle(treemap.Rect{X: x0 + 1, Y: y0 + h, W: w, H: 1}, func(s Style) Style { s.BG = RGB(0, 0, 0); return s })
		cv.Restyle(treemap.Rect{X: x0 + w, Y: y0 + 1, W: 1, H: h}, func(s Style) Style { s.BG = RGB(0, 0, 0); return s })
	}
	cv.Fill(r, " ", base)
	border := Style{FG: t.Accent, BG: bg, Attr: Bold}
	box := BoxRound
	if t.ASCII {
		box = BoxASCII
	}
	cv.Box(r, box, border)
	cv.Text(x0+2, y0+1, strings.ToUpper(f.title()), inner, Style{FG: t.Accent, BG: bg, Attr: Bold})
	rule := "─"
	if t.ASCII {
		rule = "-"
	}
	cv.HLine(x0+1, y0+2, w-2, rule, Style{FG: t.Faint, BG: bg})
	f.viewH = h - chrome
	f.contentN = len(lines)
	f.clampScroll()
	for i := 0; i < f.viewH && f.scroll+i < len(lines); i++ {
		x := x0 + 2
		for _, sp := range lines[f.scroll+i] {
			st := base
			if sp.st != nil {
				st = *sp.st
				if st.BG == 0 {
					st.BG = bg
				}
			}
			x += cv.Text(x, y0+3+i, sp.s, x0+2+inner-x, st)
		}
	}
	if len(lines) > f.viewH && f.viewH > 0 {
		ind := textutil.Count(int64(f.scroll+1)) + "–" + textutil.Count(int64(min(f.scroll+f.viewH, len(lines)))) +
			"/" + textutil.Count(int64(len(lines)))
		if f.scroll > 0 {
			ind = "▲ " + ind
		}
		if f.scroll+f.viewH < len(lines) {
			ind += " ▼"
		}
		cv.TextRight(x0+1, y0+1, w-3, ind, Style{FG: t.Muted, BG: bg})
	}
	if foot != "" {
		cv.HLine(x0+1, y0+h-3, w-2, rule, Style{FG: t.Faint, BG: bg})
		cv.Text(x0+2, y0+h-2, foot, inner, Style{FG: t.Muted, BG: bg})
	}
	m.hits = append(m.hits[:0], hit{r, func(*Model, bool) tea.Cmd { return nil }})
}

// wrap breaks s into chunks of at most w cells (used for long paths).
func wrap(s string, w int) []string {
	if w <= 0 {
		return nil
	}
	var out []string
	var cur strings.Builder
	cw := 0
	textutil.Clusters(s, func(c string, cwid int) bool {
		if cw+cwid > w {
			out = append(out, cur.String())
			cur.Reset()
			cw = 0
		}
		cur.WriteString(c)
		cw += cwid
		return true
	})
	if cur.Len() > 0 || len(out) == 0 {
		out = append(out, cur.String())
	}
	return out
}
