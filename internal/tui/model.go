// Package tui is the interactive terminal interface. All drawing goes to a
// Canvas; Bubble Tea only transports the finished frame.
//
// Threading: Update and View run on Bubble Tea's event goroutine. The scan
// controller mutates the inventory concurrently, so every read of tree data
// happens under tree.RLock (taken once per frame in View, and briefly in key
// handlers). The model itself is only touched on the event goroutine.
package tui

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/colorprofile"

	"github.com/andydixon/mapsize/internal/brand"
	"github.com/andydixon/mapsize/internal/config"
	"github.com/andydixon/mapsize/internal/filter"
	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/scan"
	"github.com/andydixon/mapsize/internal/snapshot"
	"github.com/andydixon/mapsize/internal/textutil"
	"github.com/andydixon/mapsize/internal/treemap"
)

// Options configures the interactive session.
type Options struct {
	ReadOnly bool
	Theme    string
	SizeMode inventory.SizeMode
	Mouse    bool
	Color    string // auto, truecolor, 256, 16, none
	Save     string // save a snapshot here when the scan completes
	Settings config.Settings
	Diff     *snapshot.Diff // compare mode
	// Start begins a scan (or returns a loaded tree with a nil scanner).
	Start func(ctx context.Context) (*scan.Scanner, *inventory.Tree, error)
}

// Messages.
type (
	tickMsg     time.Time
	scanDoneMsg struct{ s *scan.Scanner }
	fatalMsg    struct {
		v     any
		stack []byte
	}
	filterMsg struct {
		gen int
		res *filter.Result
		err error
	}
	filterDebounceMsg struct{ gen int }
)

// Model is the Bubble Tea model.
type Model struct {
	opts  Options
	theme *Theme

	scanner  *scan.Scanner
	tree     *inventory.Tree
	cancel   context.CancelFunc
	scanning bool
	snapshot bool // tree was loaded from a snapshot (not the live filesystem)
	ctrlC    bool // a Ctrl+C already cancelled the scan

	w, h     int
	profile  colorprofile.Profile
	forced   bool // profile forced by --color
	sizeMode inventory.SizeMode

	views []view
	view  int

	zoom inventory.NodeID
	sel  int64 // selected block: NodeID, or groupID(parent) for an LOD group
	// userSel is set once the user has chosen a selection; before that the
	// selection follows the largest block.
	userSel bool
	zoomed  bool // zoom changed during this message (zoomTo set userSel)

	search searchState
	modals []modal

	toast      string
	toastStyle Style
	toastUntil time.Time

	// Frame and data caches.
	dirty      bool
	frame      string
	dataVer    int       // bumped when sizes/filters change; invalidates caches
	lastData   time.Time // last time dataVer advanced during a scan
	kidsCache  map[inventory.NodeID]*kids
	kidsVer    int
	tm         *tmLayout
	hits       []hit // clickable regions from the last frame
	hover      int64
	lastClick  time.Time
	lastClickI int64
	ticking    bool

	pendingZoom string // path to re-zoom after a rescan
	dups        *dupState

	fatal *fatalMsg
	quit  bool
}

// hit is a clickable region recorded while painting.
type hit struct {
	r  treemap.Rect
	fn func(m *Model, double bool) tea.Cmd
}

func groupID(parent inventory.NodeID) int64 { return -1 - int64(parent) }
func isGroup(id int64) bool                 { return id < 0 }
func groupParent(id int64) inventory.NodeID { return inventory.NodeID(-1 - id) }

func newModel(opts Options, s *scan.Scanner, t *inventory.Tree, cancel context.CancelFunc) *Model {
	m := &Model{
		opts: opts, scanner: s, tree: t, cancel: cancel, scanning: s != nil, snapshot: s == nil,
		sizeMode: opts.SizeMode, dirty: true, kidsCache: map[inventory.NodeID]*kids{}, hover: noSel,
		lastClickI: noSel,
	}
	name := opts.Theme
	if name == "" || name == "default" {
		if _, ok := os.LookupEnv("NO_COLOR"); ok {
			name = "mono"
		}
	}
	m.theme = LoadTheme(name, opts.Settings)
	if strings.EqualFold(os.Getenv("TERM"), "linux") || os.Getenv("MAPSIZE_ASCII") != "" {
		m.theme.ASCII = true
	}
	m.views = []view{&mapView{}, &listView{}, &extView{}, &topView{}, &infoView{}}
	if opts.Diff != nil {
		m.views = append(m.views, &changesView{})
	}
	m.zoom = t.Root()
	m.sel = noSel
	return m
}

const noSel int64 = 1 << 62

// Init starts ticking and waits for scan completion.
func (m *Model) Init() tea.Cmd {
	cmds := []tea.Cmd{tea.RequestWindowSize}
	if m.scanning {
		cmds = append(cmds, waitScan(m.scanner), m.startTicking())
	}
	return tea.Batch(cmds...)
}

func waitScan(s *scan.Scanner) tea.Cmd {
	return func() tea.Msg { <-s.Done(); return scanDoneMsg{s} }
}

func (m *Model) refreshInterval() time.Duration {
	hz := m.opts.Settings.RefreshHz
	if hz <= 0 {
		hz = 10
	}
	return time.Second / time.Duration(hz)
}

// startTicking returns a tick command unless one is already pending.
func (m *Model) startTicking() tea.Cmd {
	if m.ticking {
		return nil
	}
	m.ticking = true
	return tea.Tick(m.refreshInterval(), func(t time.Time) tea.Msg { return tickMsg(t) })
}

func (m *Model) needsTicks() bool {
	return m.scanning || time.Now().Before(m.toastUntil) || (m.dups != nil && m.dups.running)
}

// notify shows a transient status message.
func (m *Model) notify(s string, st Style) tea.Cmd {
	m.toast, m.toastStyle, m.toastUntil = s, st, time.Now().Add(4*time.Second)
	m.dirty = true
	return m.startTicking()
}

func (m *Model) info(s string) tea.Cmd {
	return m.notify(s, Style{FG: m.theme.Fg, BG: m.theme.StatusBg})
}
func (m *Model) warn(s string) tea.Cmd {
	return m.notify(s, Style{FG: m.theme.Warn, BG: m.theme.StatusBg, Attr: Bold})
}

// invalidate marks size-dependent caches stale.
func (m *Model) invalidate() {
	m.dataVer++
	m.dirty = true
}

// Update handles one message.
func (m *Model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	cmd := m.update(msg)
	if m.quit {
		return m, tea.Quit
	}
	return m, cmd
}

func (m *Model) update(msg tea.Msg) tea.Cmd {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		// Only record the size; layout is recomputed lazily in View, so a
		// storm of resize events costs at most one layout per frame.
		if msg.Width != m.w || msg.Height != m.h {
			m.w, m.h = msg.Width, msg.Height
			m.dirty = true
		}
	case tickMsg:
		m.ticking = false
		if m.scanning {
			m.dirty = true
			if time.Since(m.lastData) >= 400*time.Millisecond {
				m.invalidate()
				m.lastData = time.Now()
			}
			m.resolvePendingZoom()
		}
		if m.toast != "" && time.Now().After(m.toastUntil) {
			m.toast = ""
			m.dirty = true
		}
		var cmds []tea.Cmd
		if m.scanning && m.search.query != nil && time.Since(m.search.appliedAt) > time.Second {
			cmds = append(cmds, m.applyFilter())
		}
		if m.dups != nil && m.dups.running {
			m.dirty = true
		}
		if m.needsTicks() {
			cmds = append(cmds, m.startTicking())
		}
		return tea.Batch(cmds...)
	case scanDoneMsg:
		if msg.s != m.scanner {
			return nil // a superseded scan
		}
		return m.scanFinished()
	case filterMsg:
		return m.filterResult(msg)
	case filterDebounceMsg:
		if msg.gen == m.search.gen {
			return m.applyFilter()
		}
	case dupProgressMsg, dupDoneMsg:
		return m.dupMsg(msg)
	case trashDoneMsg:
		return m.trashDone(msg)
	case saveDoneMsg:
		if msg.err != nil {
			return m.warn("Snapshot save failed: " + textutil.Sanitize(msg.err.Error()))
		}
		return m.info("Saved snapshot " + textutil.Sanitize(msg.path))
	case fatalMsg:
		m.fatal = &msg
		m.quit = true
	case tea.KeyPressMsg:
		m.dirty = true
		prev := m.sel
		m.zoomed = false
		cmd := m.handleKey(msg)
		m.noteSelection(prev)
		return cmd
	case tea.MouseClickMsg:
		prev := m.sel
		m.zoomed = false
		cmd := m.mouseClick(msg.Mouse())
		m.noteSelection(prev)
		return cmd
	case tea.MouseWheelMsg:
		return m.mouseWheel(msg.Mouse())
	case tea.MouseMotionMsg:
		m.mouseMotion(msg.Mouse())
	case tea.ColorProfileMsg:
		if !m.forced {
			m.profile = msg.Profile
		}
		m.dirty = true
	}
	return nil
}

// noteSelection records that the user chose a selection explicitly.
func (m *Model) noteSelection(prev int64) {
	if !m.zoomed && m.sel != prev && m.sel != noSel {
		m.userSel = true
	}
}

func (m *Model) scanFinished() tea.Cmd {
	m.scanning = false
	m.invalidate()
	m.resolvePendingZoom()
	t := m.tree
	t.RLock()
	st := t.Stats
	r := t.Node(t.Root())
	total := r.Total(m.sizeMode)
	files := st.Files + st.Symlinks + st.Others
	errs := st.TotalErrors()
	el := st.Elapsed()
	t.RUnlock()
	var cmds []tea.Cmd
	rate := float64(files+st.Dirs) / max(el.Seconds(), 0.001)
	msg := fmt.Sprintf("✓ Scan complete  %s  ·  %s files  ·  %s dirs  ·  %s  ·  %s/s",
		textutil.Duration(el), textutil.Count(files), textutil.Count(st.Dirs), textutil.Size(total), textutil.Count(int64(rate)))
	if st.Cancelled {
		msg = "✗ Scan cancelled — totals are incomplete"
	}
	if errs > 0 {
		msg += fmt.Sprintf("  ·  %s errors (e)", textutil.Count(errs))
	}
	if st.Cancelled || errs > 0 {
		cmds = append(cmds, m.warn(msg))
	} else {
		cmds = append(cmds, m.notify(msg, Style{FG: m.theme.OK, BG: m.theme.StatusBg, Attr: Bold}))
	}
	if m.opts.Save != "" && !st.Cancelled {
		cmds = append(cmds, m.saveSnapshot(m.opts.Save))
	}
	if m.search.query != nil {
		cmds = append(cmds, m.applyFilter())
	}
	return tea.Batch(cmds...)
}

// ctrlCPressed: first press cancels a running scan, second quits.
func (m *Model) ctrlCPressed() tea.Cmd {
	if m.scanning && !m.ctrlC {
		m.ctrlC = true
		m.cancel()
		return m.warn("Cancelling scan… press Ctrl+C again to quit")
	}
	m.quit = true
	return nil
}

func (m *Model) rescan() tea.Cmd {
	if m.snapshot {
		return m.warn("Snapshot loaded — nothing to rescan")
	}
	// Called from handleKey, which holds the old tree's read lock.
	m.pendingZoom = m.tree.Path(m.zoom)
	m.cancel()
	ctx, cancel := context.WithCancel(context.Background())
	s, t, err := m.opts.Start(ctx)
	if err != nil {
		cancel()
		return m.warn("Rescan failed: " + textutil.Sanitize(err.Error()))
	}
	m.scanner, m.tree, m.cancel, m.scanning, m.ctrlC = s, t, cancel, s != nil, false
	m.zoom, m.sel = t.Root(), noSel
	m.kidsCache = map[inventory.NodeID]*kids{}
	m.dups = nil
	m.search.result = nil
	m.invalidate()
	return tea.Batch(waitScan(s), m.startTicking(), m.info("Rescanning…"))
}

// resolvePendingZoom re-zooms into the remembered directory once the new
// scan has discovered it.
func (m *Model) resolvePendingZoom() {
	if m.pendingZoom == "" {
		return
	}
	m.tree.RLock()
	defer m.tree.RUnlock()
	root := m.tree.Path(m.tree.Root())
	rel, err := filepath.Rel(root, m.pendingZoom)
	if err != nil || strings.HasPrefix(rel, "..") {
		m.pendingZoom = ""
		return
	}
	id := m.tree.Root()
	if rel != "." {
		for _, part := range strings.Split(rel, string(filepath.Separator)) {
			next := inventory.NoNode
			m.tree.Children(id, func(c inventory.NodeID, n *inventory.Node) {
				if n.Name == part {
					next = c
				}
			})
			if next == inventory.NoNode {
				return // not discovered yet
			}
			id = next
		}
	}
	m.zoom = id
	m.pendingZoom = ""
	m.dirty = true
}

// View renders the frame. Expensive work happens only when something
// changed; otherwise the cached frame is returned.
func (m *Model) View() tea.View {
	if m.dirty || m.frame == "" {
		m.frame = m.render()
		m.dirty = false
	}
	v := tea.NewView(m.frame)
	v.AltScreen = true
	if m.opts.Mouse {
		v.MouseMode = tea.MouseModeAllMotion
	}
	v.WindowTitle = brand.Name
	return v
}
