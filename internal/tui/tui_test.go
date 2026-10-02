package tui

import (
	"context"
	"fmt"
	"math/rand"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/charmbracelet/colorprofile"
	"github.com/charmbracelet/x/ansi"

	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/scan"
	"github.com/andydixon/mapsize/internal/treemap"
)

const hostile = "evil\x1b]0;pwned\x07\x1b[2J.iso"

func testTree() *inventory.Tree {
	t := inventory.New("/srv", inventory.KindDir)
	add := func(p inventory.NodeID, name string, size int64) inventory.NodeID {
		id := t.Add(p, name, inventory.KindFile)
		n := t.Node(id)
		n.Size, n.Alloc = size, size
		n.Cat = inventory.CategoryFor(inventory.Ext(name), false, false)
		n.MTime = 1_700_000_000_000_000_000
		return id
	}
	dir := func(p inventory.NodeID, name string) inventory.NodeID {
		id := t.Add(p, name, inventory.KindDir)
		t.Node(id).Flags |= inventory.FlagScanned
		return id
	}
	backups := dir(0, "backups")
	for i := range 40 {
		add(backups, fmt.Sprintf("dump-%02d.tar.gz", i), int64(1+i)<<30)
	}
	vms := dir(0, "vms")
	add(vms, "ubuntu.qcow2", 86<<30)
	add(vms, "win.vmdk", 40<<30)
	media := dir(0, "media")
	photos := dir(media, "photos 日本語")
	for i := range 300 {
		add(photos, fmt.Sprintf("img%03d.jpg", i), int64(i+1)<<20)
	}
	add(media, hostile, 9<<30)
	add(media, "👨‍👩‍👧 family.mp4", 12<<30)
	tiny := dir(0, "tiny")
	for i := range 2000 {
		add(tiny, fmt.Sprintf("f%d", i), 100)
	}
	add(0, "README", 10)
	t.Recompute()
	t.Stats.Complete = true
	return t
}

func newTestModel(t *testing.T) *Model {
	t.Helper()
	tr := testTree()
	m := newModel(Options{Start: func(context.Context) (*scan.Scanner, *inventory.Tree, error) { return nil, tr, nil }},
		nil, tr, func() {})
	m.profile = colorprofile.TrueColor
	return m
}

func send(m *Model, msgs ...tea.Msg) {
	for _, msg := range msgs {
		m.Update(msg)
	}
}

func key(s string) tea.KeyPressMsg {
	switch s {
	case "up":
		return tea.KeyPressMsg{Code: tea.KeyUp}
	case "down":
		return tea.KeyPressMsg{Code: tea.KeyDown}
	case "left":
		return tea.KeyPressMsg{Code: tea.KeyLeft}
	case "right":
		return tea.KeyPressMsg{Code: tea.KeyRight}
	case "enter":
		return tea.KeyPressMsg{Code: tea.KeyEnter}
	case "esc":
		return tea.KeyPressMsg{Code: tea.KeyEscape}
	case "space":
		return tea.KeyPressMsg{Code: tea.KeySpace, Text: " "}
	case "backspace":
		return tea.KeyPressMsg{Code: tea.KeyBackspace}
	case "home":
		return tea.KeyPressMsg{Code: tea.KeyHome}
	case "tab":
		return tea.KeyPressMsg{Code: tea.KeyTab}
	}
	r := []rune(s)[0]
	return tea.KeyPressMsg{Code: r, Text: s}
}

func size(w, h int) tea.WindowSizeMsg { return tea.WindowSizeMsg{Width: w, Height: h} }

// checkFrame verifies the rendered frame exactly fills w×h cells and that no
// filename control sequence leaked through.
func checkFrame(t *testing.T, m *Model, w, h int) string {
	t.Helper()
	content := m.View().Content
	if strings.Contains(content, "\x1b]") || strings.Contains(content, "\x07") || strings.Contains(content, "\x1b[2J") {
		t.Fatalf("terminal control sequence from a filename leaked into the frame")
	}
	lines := strings.Split(content, "\n")
	if len(lines) != h {
		t.Fatalf("%dx%d: frame has %d lines", w, h, len(lines))
	}
	for i, l := range lines {
		if got := ansi.StringWidth(l); got != w {
			t.Fatalf("%dx%d: line %d has width %d: %q", w, h, i, got, ansi.Strip(l))
		}
	}
	return ansi.Strip(content)
}

func TestResizeTransitionsPreserveState(t *testing.T) {
	m := newTestModel(t)
	send(m, size(120, 40))
	checkFrame(t, m, 120, 40)
	// Zoom into media and select something that is not the default.
	m.tree.RLock()
	media := m.kidsOf(0)
	m.tree.RUnlock()
	var mediaID inventory.NodeID
	for _, id := range media.ids {
		if m.tree.Node(id).Name == "media" {
			mediaID = id
		}
	}
	m.sel, m.userSel = int64(mediaID), true
	send(m, key("space"))
	if m.zoom != mediaID {
		t.Fatalf("zoom = %d want media %d", m.zoom, mediaID)
	}
	checkFrame(t, m, 120, 40)
	send(m, key("right"))
	sel := m.sel
	for _, sz := range [][2]int{{80, 24}, {120, 40}, {300, 80}, {70, 20}, {180, 50}, {40, 10}, {39, 9}, {200, 60}} {
		send(m, size(sz[0], sz[1]))
		checkFrame(t, m, sz[0], sz[1])
		if m.zoom != mediaID {
			t.Fatalf("%v: zoom lost", sz)
		}
		if m.sel != sel && !isGroup(m.sel) {
			t.Fatalf("%v: selection changed from %d to %d", sz, sel, m.sel)
		}
	}
	// The layout must be recomputed for the new geometry, not stretched.
	if m.tm == nil || m.tm.rect.W > 200 || m.tm.rect.H > 60 {
		t.Fatalf("layout not recomputed for final size: %+v", m.tm.rect)
	}
	for _, b := range m.tm.blocks {
		r := m.tm.rect
		if b.X < r.X || b.Y < r.Y || b.X+b.W > r.X+r.W || b.Y+b.H > r.Y+r.H {
			t.Fatalf("block %+v outside treemap %+v", b, r)
		}
	}
}

func TestResizeStorm(t *testing.T) {
	m := newTestModel(t)
	r := rand.New(rand.NewSource(1))
	send(m, size(120, 40))
	m.View()
	send(m, key("right"), key("down"))
	sel := m.sel
	for i := 0; i < 300; i++ {
		send(m, size(20+r.Intn(300), 5+r.Intn(90)))
		if i%7 == 0 {
			m.View() // renders interleave with resizes like the real event loop
		}
	}
	send(m, size(150, 45))
	checkFrame(t, m, 150, 45)
	if m.sel != sel {
		t.Fatalf("selection lost in resize storm")
	}
}

func TestModalSurvivesResize(t *testing.T) {
	m := newTestModel(t)
	send(m, size(120, 40), key("enter"))
	if len(m.modals) != 1 {
		t.Fatal("Enter should open the info modal")
	}
	for _, sz := range [][2]int{{120, 40}, {60, 14}, {41, 10}, {300, 80}, {45, 12}} {
		send(m, size(sz[0], sz[1]))
		out := checkFrame(t, m, sz[0], sz[1])
		if !strings.Contains(out, "ITEM INFORMATION") {
			t.Fatalf("%v: modal title missing:\n%s", sz, out)
		}
		f := m.modals[0].(*modalFrame)
		if f.viewH < 1 || f.viewH > sz[1] {
			t.Fatalf("%v: modal viewport %d", sz, f.viewH)
		}
	}
	// Scrolling in a tiny terminal must stay in range.
	send(m, size(45, 12))
	for range 100 {
		send(m, key("down"))
	}
	checkFrame(t, m, 45, 12)
	f := m.modals[0].(*modalFrame)
	if f.scroll < 0 || f.scroll > f.contentN {
		t.Fatalf("scroll out of range: %d of %d", f.scroll, f.contentN)
	}
	send(m, key("esc"))
	if len(m.modals) != 0 {
		t.Fatal("Esc should close the modal")
	}
}

func TestSpatialNavigationUsesGeometry(t *testing.T) {
	m := newTestModel(t)
	send(m, size(160, 45))
	m.View()
	blocks := m.tm.blocks
	for _, dir := range []string{"right", "down", "left", "up"} {
		before := m.sel
		send(m, key(dir))
		m.View()
		if m.sel == before {
			continue // edge of the map
		}
		var a, b = findBlock(blocks, before), findBlock(blocks, m.sel)
		switch dir {
		case "right":
			if b.X < a.X+a.W {
				t.Fatalf("right moved to a block that is not to the right: %+v -> %+v", a, b)
			}
		case "left":
			if b.X+b.W > a.X {
				t.Fatalf("left moved wrongly: %+v -> %+v", a, b)
			}
		case "down":
			if b.Y < a.Y+a.H {
				t.Fatalf("down moved wrongly: %+v -> %+v", a, b)
			}
		case "up":
			if b.Y+b.H > a.Y {
				t.Fatalf("up moved wrongly: %+v -> %+v", a, b)
			}
		}
	}
}

func findBlock(bs []treemap.Block, id int64) treemap.Block {
	for _, b := range bs {
		if b.ID == id {
			return b
		}
	}
	return treemap.Block{}
}

func TestZoomBreadcrumbsAndHome(t *testing.T) {
	m := newTestModel(t)
	send(m, size(100, 30))
	m.tree.RLock()
	var media, photos inventory.NodeID
	m.tree.Walk(0, func(id inventory.NodeID, n *inventory.Node) bool {
		switch n.Name {
		case "media":
			media = id
		case "photos 日本語":
			photos = id
		}
		return true
	})
	m.tree.RUnlock()
	m.sel, m.userSel = int64(media), true
	send(m, key("space"))
	m.sel = int64(photos)
	send(m, key("space"))
	if m.zoom != photos {
		t.Fatal("did not zoom into photos")
	}
	out := checkFrame(t, m, 100, 30)
	if !strings.Contains(out, "media › photos 日本語") {
		t.Fatalf("breadcrumbs missing:\n%s", out)
	}
	// Narrow terminal: breadcrumbs truncate from the left, keeping the tail.
	send(m, size(42, 12))
	out = checkFrame(t, m, 42, 12)
	if !strings.Contains(out, "photos") {
		t.Fatalf("breadcrumb tail missing:\n%s", out)
	}
	send(m, key("backspace"))
	if m.zoom != media || m.sel != int64(photos) {
		t.Fatalf("backspace: zoom %d sel %d", m.zoom, m.sel)
	}
	send(m, key("home"))
	if m.zoom != 0 {
		t.Fatal("home did not return to root")
	}
}

func TestFilterTransformsMap(t *testing.T) {
	m := newTestModel(t)
	send(m, size(120, 40), key("/"))
	for _, r := range "ext = gz" {
		send(m, tea.KeyPressMsg{Code: r, Text: string(r)})
	}
	if m.search.err != "" || m.search.query == nil {
		t.Fatalf("query not parsed: %q", m.search.err)
	}
	msg := m.applyFilter()()
	send(m, msg, key("enter"))
	if m.search.result == nil {
		t.Fatal("no filter result")
	}
	out := checkFrame(t, m, 120, 40)
	if strings.Contains(out, "vms/") || !strings.Contains(out, "backups/") {
		t.Fatalf("filter not applied to the map:\n%s", out)
	}
	if !strings.Contains(out, "FILTER") {
		t.Fatal("filter indicator missing")
	}
	send(m, key("esc"))
	if m.search.query != nil || m.search.result != nil {
		t.Fatal("Esc should clear the filter")
	}
	out = checkFrame(t, m, 120, 40)
	if !strings.Contains(out, "vms/") {
		t.Fatalf("map not restored after clearing filter:\n%s", out)
	}
}

func TestHostileNameSanitisedInModal(t *testing.T) {
	m := newTestModel(t)
	send(m, size(120, 40))
	var id inventory.NodeID
	m.tree.Walk(0, func(i inventory.NodeID, n *inventory.Node) bool {
		if n.Name == hostile {
			id = i
		}
		return true
	})
	m.openModal(newInfoModal(m, id))
	out := checkFrame(t, m, 120, 40)
	if !strings.Contains(out, `\x1b]0;pwned\x07`) {
		t.Fatalf("escaped name not shown:\n%s", out)
	}
}

func TestViewsRenderAtManySizes(t *testing.T) {
	m := newTestModel(t)
	for range m.views {
		for _, sz := range [][2]int{{40, 10}, {80, 24}, {150, 45}, {91, 22}} {
			send(m, size(sz[0], sz[1]))
			checkFrame(t, m, sz[0], sz[1])
			send(m, key("down"), key("down"))
			checkFrame(t, m, sz[0], sz[1])
		}
		send(m, key("tab"))
	}
	send(m, key("?"))
	send(m, size(60, 15))
	checkFrame(t, m, 60, 15)
}

func TestLevelOfDetailGroupsTinyFiles(t *testing.T) {
	m := newTestModel(t)
	send(m, size(100, 30))
	m.tree.RLock()
	var tiny inventory.NodeID
	m.tree.Children(0, func(id inventory.NodeID, n *inventory.Node) {
		if n.Name == "tiny" {
			tiny = id
		}
	})
	m.tree.RUnlock()
	m.zoomTo(tiny)
	m.View()
	if len(m.tm.blocks) > 100*30/topMinArea+1 {
		t.Fatalf("%d blocks for 2000 equal files: LOD not applied", len(m.tm.blocks))
	}
	if _, ok := m.tm.groups[groupID(tiny)]; !ok {
		t.Fatal("expected a group block for small items")
	}
}

func TestCtrlCQuitsWhenIdle(t *testing.T) {
	m := newTestModel(t)
	send(m, size(80, 24))
	_, cmd := m.Update(tea.KeyPressMsg{Code: 'c', Mod: tea.ModCtrl})
	if cmd == nil || !m.quit {
		t.Fatal("Ctrl+C should quit when no scan runs")
	}
}

func BenchmarkRenderFrame(b *testing.B) {
	tr := testTree()
	m := newModel(Options{}, nil, tr, func() {})
	m.profile = colorprofile.TrueColor
	send(m, size(200, 60))
	for b.Loop() {
		m.dirty = true
		m.View()
	}
}

func BenchmarkResizeRelayout(b *testing.B) {
	tr := testTree()
	m := newModel(Options{}, nil, tr, func() {})
	i := 0
	for b.Loop() {
		send(m, size(100+i%100, 30+i%30))
		m.View()
		i++
	}
}

func TestMouse(t *testing.T) {
	m := newTestModel(t)
	send(m, size(120, 40))
	m.View()
	// Click the centre of a block other than the selected one.
	var target treemap.Block
	for _, b := range m.tm.blocks {
		if b.ID != m.sel && b.ID >= 0 && m.tree.Node(inventory.NodeID(b.ID)).IsDir() && b.W > 4 {
			target = b
			break
		}
	}
	click := tea.MouseClickMsg{X: target.X + target.W/2, Y: target.Y + target.H/2, Button: tea.MouseLeft}
	send(m, click)
	if m.sel != target.ID {
		t.Fatalf("click selected %d, want %d", m.sel, target.ID)
	}
	m.View()
	send(m, click) // second click within the double-click window
	if m.zoom != inventory.NodeID(target.ID) {
		t.Fatalf("double click should zoom into %d (zoom=%d)", target.ID, m.zoom)
	}
	m.View()
	send(m, tea.MouseWheelMsg{X: 50, Y: 20, Button: tea.MouseWheelDown})
	if m.zoom != 0 {
		t.Fatal("wheel down should zoom out")
	}
	// A modal swallows clicks; clicking outside closes it.
	send(m, key("?"))
	m.View()
	send(m, tea.MouseClickMsg{X: 0, Y: 39, Button: tea.MouseLeft})
	if len(m.modals) != 0 {
		t.Fatal("click outside modal should close it")
	}
}

func TestQuantizers(t *testing.T) {
	if quant16(RGB(10, 12, 8)) != 0 {
		t.Fatal("near-black should map to black")
	}
	if n := quant16(RGB(0xe0, 0x6c, 0x75)); n != 1 && n != 9 {
		t.Fatalf("red-ish mapped to %d", n)
	}
	if n := quant256(RGB(40, 48, 30)); n < 16 {
		t.Fatalf("256 quantizer used palette-dependent index %d", n)
	}
	// A dark, slightly tinted shade must land on the grey ramp or a dark
	// cube entry, not a saturated one like olive (58) or navy (17).
	if n := quant256(RGB(40, 48, 30)); n == 58 || n == 17 || n == 22 {
		t.Fatalf("dark tint mapped to saturated %d", n)
	}
}

func TestColourDepthsRenderSameGeometry(t *testing.T) {
	for _, p := range []colorprofile.Profile{colorprofile.TrueColor, colorprofile.ANSI256, colorprofile.ANSI, colorprofile.ASCII} {
		m := newTestModel(t)
		m.profile = p
		send(m, size(100, 30))
		checkFrame(t, m, 100, 30)
	}
}
