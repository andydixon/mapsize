package tui

import (
	"context"
	"fmt"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/andydixon/mapsize/internal/filter"
	"github.com/andydixon/mapsize/internal/textutil"
	"github.com/andydixon/mapsize/internal/treemap"
)

// lineEditor is a minimal single-line text editor.
type lineEditor struct {
	buf []rune
	cur int
}

func (e *lineEditor) String() string { return string(e.buf) }

func (e *lineEditor) set(s string) { e.buf, e.cur = []rune(s), len([]rune(s)) }

// key applies an editing key; it reports whether the text changed.
func (e *lineEditor) key(msg tea.KeyPressMsg) (changed, handled bool) {
	switch msg.Keystroke() {
	case "left", "ctrl+b":
		e.cur = max(0, e.cur-1)
	case "right", "ctrl+f":
		e.cur = min(len(e.buf), e.cur+1)
	case "home", "ctrl+a":
		e.cur = 0
	case "end", "ctrl+e":
		e.cur = len(e.buf)
	case "backspace", "ctrl+h":
		if e.cur > 0 {
			e.buf = append(e.buf[:e.cur-1], e.buf[e.cur:]...)
			e.cur--
			return true, true
		}
	case "delete", "ctrl+d":
		if e.cur < len(e.buf) {
			e.buf = append(e.buf[:e.cur], e.buf[e.cur+1:]...)
			return true, true
		}
	case "ctrl+u":
		e.buf, e.cur = e.buf[e.cur:], 0
		return true, true
	case "ctrl+k":
		e.buf = e.buf[:e.cur]
		return true, true
	case "ctrl+w", "alt+backspace":
		i := e.cur
		for i > 0 && e.buf[i-1] == ' ' {
			i--
		}
		for i > 0 && e.buf[i-1] != ' ' {
			i--
		}
		e.buf = append(e.buf[:i], e.buf[e.cur:]...)
		e.cur = i
		return true, true
	default:
		txt := msg.Text
		if txt == "" || msg.Mod&(tea.ModCtrl|tea.ModAlt) != 0 {
			return false, false
		}
		var rs []rune
		for _, r := range txt {
			if r >= 0x20 && r != 0x7f {
				rs = append(rs, r)
			}
		}
		if len(rs) == 0 || len(e.buf)+len(rs) > 1024 {
			return false, true
		}
		e.buf = append(e.buf[:e.cur], append(rs, e.buf[e.cur:]...)...)
		e.cur += len(rs)
		return true, true
	}
	return false, true
}

type searchState struct {
	editing   bool
	ed        lineEditor
	query     *filter.Query
	result    *filter.Result
	err       string
	gen       int
	busy      bool
	appliedAt time.Time
	cancel    context.CancelFunc
}

func (s *searchState) begin() {
	s.editing = true
	if s.query != nil {
		s.ed.set(s.query.String())
	}
}

func (m *Model) searchKey(msg tea.KeyPressMsg) tea.Cmd {
	s := &m.search
	switch msg.Keystroke() {
	case "esc":
		s.editing = false
		m.clearFilter()
		return nil
	case "enter":
		s.editing = false
		if s.err != "" {
			return m.warn("Invalid query: " + s.err)
		}
		return nil
	}
	changed, _ := s.ed.key(msg)
	if changed {
		return m.queryChanged()
	}
	return nil
}

// queryChanged parses the editor contents and schedules a debounced apply.
func (m *Model) queryChanged() tea.Cmd {
	s := &m.search
	s.gen++
	text := s.ed.String()
	if text == "" {
		s.err = ""
		m.dropFilter()
		return nil
	}
	q, err := filter.Parse(text, time.Now())
	if err != nil {
		s.err = err.Error()
		return nil
	}
	s.err = ""
	s.query = q
	gen := s.gen
	return tea.Tick(120*time.Millisecond, func(time.Time) tea.Msg { return filterDebounceMsg{gen} })
}

// setFilter replaces the filter with q and applies it immediately.
func (m *Model) setFilter(q string) tea.Cmd {
	m.search.ed.set(q)
	m.search.editing = false
	m.queryChanged()
	if m.search.err != "" {
		return m.warn("Invalid query: " + m.search.err)
	}
	return m.applyFilter()
}

// applyFilter evaluates the current query in the background. The tree is
// read-locked by the worker; stale results are discarded by generation.
func (m *Model) applyFilter() tea.Cmd {
	s := &m.search
	if s.query == nil {
		return nil
	}
	if s.cancel != nil {
		s.cancel()
	}
	ctx, cancel := context.WithCancel(context.Background())
	s.cancel = cancel
	s.busy = true
	s.appliedAt = time.Now()
	t, q, mode, gen := m.tree, s.query, m.sizeMode, s.gen
	return func() tea.Msg {
		t.RLock()
		res, err := filter.Apply(ctx, t, q, mode)
		t.RUnlock()
		return filterMsg{gen: gen, res: res, err: err}
	}
}

func (m *Model) filterResult(msg filterMsg) tea.Cmd {
	s := &m.search
	if msg.gen != s.gen || s.query == nil {
		return nil
	}
	s.busy = false
	if msg.err != nil {
		return nil
	}
	s.result = msg.res
	m.invalidate()
	return nil
}

// dropFilter removes the active filter but keeps the editor open.
func (m *Model) dropFilter() {
	s := &m.search
	if s.cancel != nil {
		s.cancel()
	}
	s.query, s.result, s.busy = nil, nil, false
	m.invalidate()
}

func (m *Model) clearFilter() {
	m.dropFilter()
	m.search.ed.set("")
	m.search.err = ""
	m.search.gen++
}

func (m *Model) paintSearch(cv *Canvas, r treemap.Rect) {
	t := m.theme
	s := &m.search
	st := Style{FG: t.Fg, BG: t.PanelBg}
	cv.Fill(r, " ", st)
	x := r.X + 1
	x += cv.Text(x, r.Y, "/ ", 2, Style{FG: t.Accent, BG: t.PanelBg, Attr: Bold})
	status, statusSt := "", Style{FG: t.Muted, BG: t.PanelBg}
	switch {
	case s.err != "":
		status, statusSt.FG = "✗ "+s.err, t.Err
	case s.busy:
		status = "filtering…"
	case s.result != nil:
		status = fmt.Sprintf("%s matches · %s", textutil.Count(s.result.Count), textutil.Size(s.result.Total))
	case len(s.ed.buf) == 0:
		status = "name, *.iso, size > 1GB, ext IN (iso,vmdk), age > 365d … Enter keep · Esc clear"
	}
	sw := min(textutil.Width(status), r.W/2)
	avail := r.W - (x - r.X) - sw - 3
	// Scroll the text so the cursor stays visible.
	text := s.ed.buf
	start := 0
	if s.ed.cur > avail-1 {
		start = s.ed.cur - avail + 1
	}
	vis := string(text[start:min(len(text), start+avail)])
	cv.Text(x, r.Y, textutil.Sanitize(vis), avail, st)
	cx := x + textutil.Width(textutil.Sanitize(string(text[start:s.ed.cur])))
	under := " "
	if s.ed.cur < len(text) {
		under = textutil.Sanitize(string(text[s.ed.cur]))
	}
	cv.Text(cx, r.Y, under, 2, Style{FG: t.PanelBg, BG: t.Fg, Attr: m.monoRev()})
	cv.TextRight(r.X, r.Y, r.W-1, textutil.Truncate(status, sw), statusSt)
}

func (m *Model) paintFilterSummary(cv *Canvas, r treemap.Rect) {
	t := m.theme
	s := &m.search
	x := r.X + 1
	x += cv.Text(x, r.Y, " FILTER ", 8, Style{FG: t.textOn(t.Warn), BG: t.Warn, Attr: Bold | m.monoRev()})
	x += cv.Text(x+1, r.Y, textutil.Sanitize(s.query.String()), r.W/2, Style{FG: t.Fg, BG: t.PanelBg, Attr: Bold}) + 1
	info := "filtering…"
	if s.result != nil && !s.busy {
		info = fmt.Sprintf("%s matches · %s of %s · / edit · Esc clear", textutil.Count(s.result.Count),
			textutil.Size(s.result.Total), textutil.Size(m.tree.Node(0).Total(m.sizeMode)))
	}
	cv.Text(x+2, r.Y, info, r.X+r.W-x-3, Style{FG: t.Muted, BG: t.PanelBg})
}
