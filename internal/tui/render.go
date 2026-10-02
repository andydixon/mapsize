package tui

import (
	"fmt"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/colorprofile"

	"github.com/andydixon/mapsize/internal/brand"
	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/textutil"
	"github.com/andydixon/mapsize/internal/treemap"
)

// Minimum usable terminal size.
const minW, minH = 40, 10

type sizeClass int

const (
	tooSmall sizeClass = iota
	small
	medium
	large
)

// geometry is the panel layout for one terminal size. It is recomputed from
// scratch for every frame, so resizing can never leave stale geometry.
type geometry struct {
	class                  sizeClass
	tabs, crumbs           treemap.Rect // tabs is empty in the small layout
	main, side, info, stat treemap.Rect
}

func computeGeometry(w, h int) geometry {
	g := geometry{}
	switch {
	case w < minW || h < minH:
		g.class = tooSmall
		return g
	case w >= 150 && h >= 36:
		g.class = large
	case w >= 90 && h >= 22:
		g.class = medium
	default:
		g.class = small
	}
	y := 0
	if g.class >= medium {
		g.tabs = treemap.Rect{X: 0, Y: 0, W: w, H: 1}
		y = 1
	}
	g.crumbs = treemap.Rect{X: 0, Y: y, W: w, H: 1}
	y++
	bottom := h - 1
	g.stat = treemap.Rect{X: 0, Y: h - 1, W: w, H: 1}
	if g.class >= medium {
		g.info = treemap.Rect{X: 0, Y: h - 2, W: w, H: 1}
		bottom = h - 2
	}
	g.main = treemap.Rect{X: 0, Y: y, W: w, H: bottom - y}
	if g.class == large {
		sw := min(40, w/5)
		g.side = treemap.Rect{X: 0, Y: y, W: sw, H: g.main.H}
		g.main.X, g.main.W = sw, w-sw
	}
	return g
}

func (m *Model) render() string {
	t := m.theme
	cv := NewCanvas(m.w, m.h, t.base())
	m.hits = m.hits[:0]
	g := computeGeometry(m.w, m.h)
	if g.class == tooSmall {
		m.renderTooSmall(cv)
		return cv.Render(m.depth())
	}
	m.tree.RLock()
	defer m.tree.RUnlock()

	m.ensureSelection()
	if g.tabs.W > 0 {
		m.paintTabs(cv, g.tabs)
	}
	m.paintCrumbs(cv, g.crumbs)
	if !g.side.Empty() {
		m.paintSide(cv, g.side)
	}
	m.views[m.view].paint(m, cv, g.main)
	if !g.info.Empty() {
		m.paintInfoLine(cv, g.info)
	}
	m.paintStatus(cv, g.stat, g.class)
	if g.class == small && m.search.editing {
		m.paintSearch(cv, g.stat)
	}
	if len(m.modals) > 0 {
		m.paintModal(cv)
	}
	return cv.Render(m.depth())
}

func (m *Model) depth() colorDepth {
	switch m.profile {
	case colorprofile.ANSI256:
		return depth256
	case colorprofile.ANSI:
		return depth16
	}
	return depthTrue
}

func (m *Model) renderTooSmall(cv *Canvas) {
	st := m.theme.muted()
	lines := []string{"Terminal too small", fmt.Sprintf("%d×%d — need %d×%d", m.w, m.h, minW, minH), "q quits"}
	y0 := max(0, (m.h-len(lines))/2)
	for i, l := range lines {
		cv.TextCentered(0, y0+i, m.w, l, st)
	}
}

// paintTabs draws the view tabs and mode chips.
func (m *Model) paintTabs(cv *Canvas, r treemap.Rect) {
	t := m.theme
	cv.Fill(r, " ", t.header())
	badge := " ◧ " + brand.Name + " "
	if t.ASCII {
		badge = " " + brand.Name + " "
	}
	x := cv.Text(r.X, r.Y, badge, r.W, Style{FG: t.textOn(t.Accent), BG: t.Accent, Attr: Bold | m.monoRev()})
	x++
	for i, v := range m.views {
		label := " " + v.name() + " "
		st := Style{FG: t.Muted, BG: t.HeaderBg}
		if i == m.view {
			st = Style{FG: t.Fg, BG: t.Bg, Attr: Bold | Underline}
		}
		if x+textutil.Width(label) >= r.W-30 {
			break
		}
		w := cv.Text(x, r.Y, label, r.W-x, st)
		idx := i
		m.hits = append(m.hits, hit{treemap.Rect{X: x, Y: r.Y, W: w, H: 1}, func(m *Model, _ bool) tea.Cmd { m.setView(idx); return nil }})
		x += w
	}
	// Right-aligned chips.
	var chips []struct {
		s  string
		bg Color
	}
	add := func(s string, bg Color) {
		chips = append(chips, struct {
			s  string
			bg Color
		}{s, bg})
	}
	if m.opts.ReadOnly {
		add("READ ONLY", t.Err)
	}
	if m.snapshot {
		add("SNAPSHOT", t.Accent)
	}
	if m.opts.Diff != nil {
		add("COMPARE", t.DiffNew)
	}
	if m.search.query != nil {
		add("FILTER", t.Warn)
	}
	if len(m.tree.Stats.Excludes) > 0 {
		add(fmt.Sprintf("EXCL %d", len(m.tree.Stats.Excludes)), t.Faint)
	}
	add(strings.ToUpper(m.sizeMode.String()), t.Faint)
	rx := r.X + r.W
	for i := len(chips) - 1; i >= 0; i-- {
		s := " " + chips[i].s + " "
		w := textutil.Width(s)
		if rx-w-1 < x {
			break
		}
		rx -= w
		cv.Text(rx, r.Y, s, w, Style{FG: t.textOn(chips[i].bg), BG: chips[i].bg, Attr: Bold | m.monoRev()})
		rx--
	}
}

func (m *Model) monoRev() Attr {
	if m.theme.Mono {
		return Reverse
	}
	return 0
}

// paintCrumbs draws "/ › home › andy" breadcrumbs, truncated from the left,
// with the zoom root's size on the right.
func (m *Model) paintCrumbs(cv *Canvas, r treemap.Rect) {
	t := m.theme
	cv.Fill(r, " ", t.header())
	tr := m.tree
	z := tr.Node(m.zoom)
	size := m.sizeOf(m.zoom)
	right := " " + textutil.Size(size)
	if root := m.sizeOf(tr.Root()); m.zoom != tr.Root() && root > 0 {
		right += "  " + textutil.Percent(size, root) + " of scan"
	}
	if m.search.result != nil {
		right += fmt.Sprintf("  (%s matches)", textutil.Count(m.search.result.Count))
	}
	if z.IsDir() {
		right += fmt.Sprintf("  %s files", textutil.Count(int64(z.Files)))
	}
	right += " "
	rw := textutil.Width(right)
	if rw > r.W/2 {
		right = " " + textutil.SizeCompact(size) + " "
		rw = textutil.Width(right)
	}
	cv.TextRight(r.X, r.Y, r.W, right, Style{FG: t.HeaderFg, BG: t.HeaderBg, Attr: Bold})

	sep := " › "
	if t.ASCII {
		sep = " > "
	}
	chain := tr.Ancestors(m.zoom)
	type seg struct {
		id inventory.NodeID
		s  string
	}
	segs := make([]seg, len(chain))
	for i, id := range chain {
		segs[i] = seg{id, textutil.Sanitize(tr.Node(id).Name)}
	}
	avail := r.W - rw - 2
	// Drop leading segments until the rest fits; keep at least the last.
	total := func(from int) int {
		w := 0
		for i := from; i < len(segs); i++ {
			w += textutil.Width(segs[i].s)
			if i > from {
				w += textutil.Width(sep)
			}
		}
		if from > 0 {
			w += textutil.Width("…" + sep)
		}
		return w
	}
	from := 0
	for from < len(segs)-1 && total(from) > avail {
		from++
	}
	x := r.X + 1
	if from > 0 {
		x += cv.Text(x, r.Y, "…"+sep, avail, Style{FG: t.Muted, BG: t.HeaderBg})
	}
	for i := from; i < len(segs); i++ {
		if i > from {
			x += cv.Text(x, r.Y, sep, r.X+1+avail-x, Style{FG: t.Faint, BG: t.HeaderBg})
		}
		st := Style{FG: t.Muted, BG: t.HeaderBg}
		if i == len(segs)-1 {
			st = Style{FG: t.Fg, BG: t.HeaderBg, Attr: Bold}
		}
		s := segs[i].s
		if i == len(segs)-1 {
			s = textutil.TruncateLeft(s, max(1, r.X+1+avail-x))
		}
		w := cv.Text(x, r.Y, s, r.X+1+avail-x, st)
		id := segs[i].id
		m.hits = append(m.hits, hit{treemap.Rect{X: x, Y: r.Y, W: w, H: 1}, func(m *Model, _ bool) tea.Cmd { m.zoomTo(id); return nil }})
		x += w
	}
}

// paintInfoLine describes the selection (or shows the search editor).
func (m *Model) paintInfoLine(cv *Canvas, r treemap.Rect) {
	t := m.theme
	cv.Fill(r, " ", t.panel())
	if m.search.editing {
		m.paintSearch(cv, r)
		return
	}
	if m.search.query != nil {
		m.paintFilterSummary(cv, r)
		return
	}
	cv.Text(r.X+1, r.Y, m.describeSelection(), r.W-2, t.panel())
}

func (m *Model) describeSelection() string {
	id := m.selectedNode()
	tr := m.tree
	if isGroup(m.sel) {
		if g, ok := m.groupInfo(m.sel); ok {
			return fmt.Sprintf("▸ %s smaller items  ·  %s  ·  %s of %s  ·  Enter lists them",
				textutil.Count(int64(len(g.ids))), textutil.Size(g.size), textutil.Percent(g.size, m.sizeOf(m.zoom)),
				textutil.Sanitize(tr.Node(m.zoom).Name))
		}
	}
	if id == inventory.NoNode {
		return "Nothing selected"
	}
	n := tr.Node(id)
	parts := []string{"▸ " + textutil.Sanitize(n.Name)}
	parts = append(parts, textutil.Size(m.sizeOf(id)))
	if p := n.Parent; p != inventory.NoNode {
		parts = append(parts, textutil.Percent(m.sizeOf(id), m.sizeOf(p))+" of parent")
	}
	if n.IsDir() {
		parts = append(parts, textutil.Count(int64(n.Files))+" files", textutil.Count(int64(n.Dirs))+" dirs")
		if n.Flags&inventory.FlagScanned == 0 && m.scanning {
			parts = append(parts, "scanning…")
		}
	} else {
		parts = append(parts, kindLabel(tr, n))
	}
	if n.MTime > 0 {
		parts = append(parts, "modified "+time.Unix(0, n.MTime).Format("2006-01-02"))
	}
	if n.Errors > 0 {
		parts = append(parts, fmt.Sprintf("%d errors", n.Errors))
	}
	if m.opts.Diff != nil {
		parts = append(parts, textutil.SignedSize(m.opts.Diff.Delta(id, m.sizeMode))+" vs old snapshot")
	}
	return strings.Join(parts, "  ·  ")
}

func kindLabel(tr *inventory.Tree, n *inventory.Node) string {
	switch n.Kind {
	case inventory.KindFile:
		return n.Cat.String()
	case inventory.KindSymlink:
		return "symlink"
	}
	return n.Kind.String()
}

var spinner = []string{"⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"}

// paintStatus draws the live status bar.
func (m *Model) paintStatus(cv *Canvas, r treemap.Rect, class sizeClass) {
	t := m.theme
	cv.Fill(r, " ", t.status())
	help := " ? help "
	if class == small {
		help = " ? "
	}
	hw := cv.Text(r.X+r.W-textutil.Width(help), r.Y, help, r.W, Style{FG: t.Muted, BG: t.StatusBg})
	avail := r.W - hw - 1
	if m.toast != "" {
		cv.Text(r.X+1, r.Y, m.toast, avail-1, m.toastStyle)
		return
	}
	st := &m.tree.Stats
	root := m.tree.Node(m.tree.Root())
	files := st.Files + st.Symlinks + st.Others
	var parts []struct {
		s  string
		fg Color
	}
	add := func(s string, fg Color) {
		parts = append(parts, struct {
			s  string
			fg Color
		}{s, fg})
	}
	switch {
	case m.scanning:
		frame := spinner[int(time.Now().UnixMilli()/100)%len(spinner)]
		if t.ASCII {
			frame = "*"
		}
		add(frame+" scanning", t.Accent)
	case st.Cancelled:
		add("✗ cancelled", t.Warn)
	case m.snapshot:
		add("◉ snapshot", t.Accent)
	default:
		add("✓ complete", t.OK)
	}
	add(textutil.Count(files)+" files", t.StatusFg)
	add(textutil.Size(root.Total(m.sizeMode)), t.StatusFg)
	if m.scanning {
		el := st.Elapsed()
		p := m.scanner.Progress()
		add(textutil.Count(int64(float64(p.Entries)/max(el.Seconds(), 0.001)))+"/s", t.StatusFg)
		if class != small {
			add(fmt.Sprintf("%d workers", p.Workers), t.Muted)
			add(fmt.Sprintf("queue %s", textutil.Count(p.Pending)), t.Muted)
		}
		add(textutil.Duration(el), t.Muted)
	} else if !m.snapshot && class != small {
		add(textutil.Duration(st.Elapsed()), t.Muted)
	}
	if e := st.TotalErrors(); e > 0 {
		add(fmt.Sprintf("%s errors", textutil.Count(e)), t.Err)
	}
	if st.Excluded > 0 {
		add(fmt.Sprintf("%s excluded", textutil.Count(st.Excluded)), t.Warn)
	}
	if st.Incomplete() && !m.scanning && class != small {
		add("totals incomplete", t.Warn)
	}
	x := r.X + 1
	for i, p := range parts {
		s := p.s
		if i > 0 {
			s = "  " + s
		}
		if x+textutil.Width(s) > r.X+avail {
			break
		}
		st := Style{FG: p.fg, BG: t.StatusBg}
		if i == 0 {
			st.Attr = Bold
		}
		x += cv.Text(x, r.Y, s, avail-x, st)
	}
}
