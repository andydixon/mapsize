package tui

import (
	"fmt"
	"os"
	"path/filepath"
	"time"

	tea "charm.land/bubbletea/v2"

	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/platform"
	"github.com/andydixon/mapsize/internal/textutil"
	"github.com/andydixon/mapsize/internal/treemap"
)

// view is one of the Tab-cycled main views.
type view interface {
	name() string
	paint(m *Model, cv *Canvas, r treemap.Rect)
	key(m *Model, k string) (bool, tea.Cmd)
	wheel(m *Model, up bool) tea.Cmd
}

func keyName(k tea.KeyPressMsg) string {
	switch s := k.String(); s {
	case " ":
		return "space"
	default:
		return s
	}
}

// handleKey dispatches a key press. It holds the tree read lock for the
// whole handler; helpers it calls must not lock again (a recursive RLock
// can deadlock against a waiting writer).
func (m *Model) handleKey(msg tea.KeyPressMsg) tea.Cmd {
	k := keyName(msg)
	if k == "ctrl+c" {
		return m.ctrlCPressed()
	}
	t := m.tree
	t.RLock()
	defer t.RUnlock()
	m.ensureSelection()
	if len(m.modals) > 0 {
		return m.modalKey(msg)
	}
	if m.search.editing {
		return m.searchKey(msg)
	}
	handled, cmd := m.views[m.view].key(m, k)
	if handled {
		return cmd
	}
	switch k {
	case "q":
		m.quit = true
	case "?", "f1":
		m.openModal(newHelpModal(m))
	case "/":
		m.search.begin()
	case "esc":
		if m.search.query != nil {
			m.clearFilter()
		} else if m.zoom != m.tree.Root() {
			m.zoomOut()
		}
	case "tab":
		m.setView((m.view + 1) % len(m.views))
	case "shift+tab":
		m.setView((m.view + len(m.views) - 1) % len(m.views))
	case "1", "2", "3", "4", "5", "6", "7":
		m.setView(int(k[0] - '1'))
	case "enter":
		return m.inspectSelection()
	case "space", "right":
		return m.zoomIntoSelection()
	case "backspace", "left":
		m.zoomOut()
	case "home":
		m.zoomTo(m.tree.Root())
	case "a":
		if m.sizeMode == inventory.SizeAllocated {
			m.sizeMode = inventory.SizeLogical
		} else {
			m.sizeMode = inventory.SizeAllocated
		}
		m.invalidate()
		var cmds []tea.Cmd
		if m.search.query != nil {
			cmds = append(cmds, m.applyFilter())
		}
		cmds = append(cmds, m.info("Sizing by "+m.sizeMode.String()+" size"))
		return tea.Batch(cmds...)
	case "r":
		return m.rescan()
	case "s":
		m.openModal(newSaveModal(m))
	case "e":
		m.setViewByName("Scan")
		if v, ok := m.views[m.view].(*infoView); ok {
			v.focusErrors(m)
		}
	case "x":
		m.setViewByName("Types")
	case "g":
		m.setViewByName("Top")
	case "D":
		return m.startDuplicates()
	case "c":
		return m.copyPath()
	case "o":
		return m.reveal()
	case "d", "delete":
		return m.requestTrash()
	case "T":
		names := ThemeNames()
		cur := 0
		for i, n := range names {
			if n == m.theme.Name {
				cur = i
			}
		}
		ascii := m.theme.ASCII
		m.theme = LoadTheme(names[(cur+1)%len(names)], m.opts.Settings)
		m.theme.ASCII = ascii
		return m.info("Theme: " + m.theme.Name)
	}
	return nil
}

func (m *Model) setViewByName(n string) {
	for i, v := range m.views {
		if v.name() == n {
			m.setView(i)
		}
	}
}

func (m *Model) inspectSelection() tea.Cmd {
	if isGroup(m.sel) {
		if g, ok := m.groupInfo(m.sel); ok {
			m.openModal(newGroupModal(m, g))
		}
		return nil
	}
	if id := m.selectedNode(); id != inventory.NoNode {
		m.openModal(newInfoModal(m, id))
	}
	return nil
}

func (m *Model) zoomIntoSelection() tea.Cmd {
	if isGroup(m.sel) {
		// Groups cannot be zoomed: show their members in the list view.
		if g, ok := m.groupInfo(m.sel); ok && len(g.ids) > 0 {
			m.sel = int64(g.ids[0])
			m.setViewByName("List")
		}
		return nil
	}
	id := m.selectedNode()
	if id == inventory.NoNode {
		return nil
	}
	if !m.tree.Node(id).IsDir() {
		return m.info("Not a directory — Enter shows details")
	}
	if len(m.kidsOf(id).ids) == 0 {
		return m.info("Directory is empty")
	}
	m.zoomTo(id)
	return nil
}

func (m *Model) zoomOut() {
	if p := m.tree.Node(m.zoom).Parent; p != inventory.NoNode {
		m.zoomTo(p)
	}
}

// jumpTo zooms to a node's parent directory and selects it.
func (m *Model) jumpTo(id inventory.NodeID) {
	p := m.tree.Node(id).Parent
	if p == inventory.NoNode {
		m.zoomTo(id)
		return
	}
	m.zoomTo(p)
	m.sel = int64(id)
	m.setViewByName("Map")
}

func (m *Model) selectedPath() (string, bool) {
	id := m.selectedNode()
	if id == inventory.NoNode {
		return "", false
	}
	return m.tree.Path(id), true
}

func (m *Model) copyPath() tea.Cmd {
	p, ok := m.selectedPath()
	if !ok {
		return nil
	}
	return tea.Batch(tea.SetClipboard(p), m.info("Copied path to clipboard (OSC 52)"))
}

func (m *Model) reveal() tea.Cmd {
	if m.snapshot {
		return m.warn("Snapshot — the files may not exist on this machine")
	}
	p, ok := m.selectedPath()
	if !ok {
		return nil
	}
	if err := platform.Reveal(p); err != nil {
		return m.warn("Open failed: " + textutil.Sanitize(err.Error()))
	}
	return m.info("Opened in file manager")
}

func (m *Model) saveSnapshot(path string) tea.Cmd {
	t := m.tree
	return func() tea.Msg {
		t.RLock()
		err := saveFile(path, t)
		t.RUnlock()
		return saveDoneMsg{path, err}
	}
}

type saveDoneMsg struct {
	path string
	err  error
}

func defaultSnapshotName(root string) string {
	base := filepath.Base(root)
	if base == string(filepath.Separator) || base == "." || base == "" {
		base = "root"
	}
	wd, _ := os.Getwd()
	return filepath.Join(wd, fmt.Sprintf("%s-%s.msz", base, time.Now().Format("20060102-1504")))
}
