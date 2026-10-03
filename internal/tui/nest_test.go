package tui

import (
	"context"
	"fmt"
	"strings"
	"testing"

	"github.com/charmbracelet/colorprofile"
	"github.com/charmbracelet/x/ansi"

	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/scan"
)

func rootTree() *inventory.Tree {
	t := inventory.New("/", inventory.KindDir)
	add := func(p inventory.NodeID, name string, size int64) {
		id := t.Add(p, name, inventory.KindFile)
		n := t.Node(id)
		n.Size, n.Alloc = size, size
		n.Cat = inventory.CategoryFor(inventory.Ext(name), false, false)
	}
	dir := func(p inventory.NodeID, name string) inventory.NodeID {
		id := t.Add(p, name, inventory.KindDir)
		t.Node(id).Flags |= inventory.FlagScanned
		return id
	}
	mnt := dir(0, "mnt")
	raid := dir(mnt, "raid")
	media := dir(raid, "media")
	for _, show := range []string{"X-Files", "Simpsons", "Frasier"} {
		s := dir(media, show)
		for i := 1; i <= 5; i++ {
			se := dir(s, fmt.Sprintf("S%02d", i))
			for e := 1; e <= 20; e++ {
				add(se, fmt.Sprintf("E%02d.mkv", e), int64(e)<<28)
			}
		}
	}
	usr := dir(0, "usr")
	lib := dir(usr, "lib")
	for i := range 50 {
		add(lib, fmt.Sprintf("lib%d.so", i), int64(i+1)<<22)
	}
	v := dir(0, "var")
	add(dir(v, "log"), "syslog", 2<<30)
	t.Recompute()
	t.Stats.Complete = true
	return t
}

// A single-child chain (mnt → raid → media) must not use up the nesting
// budget: the map at / should reach the seasons and their episodes.
func TestNestingReachesDeepMedia(t *testing.T) {
	tr := rootTree()
	m := newModel(Options{Start: func(context.Context) (*scan.Scanner, *inventory.Tree, error) { return nil, tr, nil }}, nil, tr, func() {})
	m.profile = colorprofile.TrueColor
	send(m, size(160, 45))
	if out := ansi.Strip(m.View().Content); !strings.Contains(out, "mnt/raid/media/") || !strings.Contains(out, "S01/") {
		t.Fatalf("chain or seasons missing:\n%s", out)
	}
	files := 0
	for _, nb := range m.tm.nested {
		if nb.id >= 0 && !tr.Node(inventory.NodeID(nb.id)).IsDir() {
			files++
		}
	}
	if files < 50 {
		t.Fatalf("only %d episode blocks drawn", files)
	}
}

// Headless, o shows the selected folder in the map instead of launching a
// file manager.
func TestRevealHeadlessZoomsMap(t *testing.T) {
	t.Setenv("DISPLAY", "")
	t.Setenv("WAYLAND_DISPLAY", "")
	tr := rootTree()
	m := newModel(Options{Start: func(context.Context) (*scan.Scanner, *inventory.Tree, error) { return nil, tr, nil }}, nil, tr, func() {})
	send(m, size(160, 45))
	mnt := m.kidsOf(m.zoom).ids[0]
	m.snapshot = false // reveal is refused for snapshots
	m.sel, m.userSel = int64(mnt), true
	send(m, key("o"))
	if m.zoom != mnt {
		t.Fatalf("zoom = %q, want mnt", tr.Path(m.zoom))
	}
}
