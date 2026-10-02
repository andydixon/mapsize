package tui

import (
	"fmt"
	"math/rand"
	"testing"

	"github.com/charmbracelet/colorprofile"

	"github.com/andydixon/mapsize/internal/inventory"
)

// bigTree builds ~1M nodes in memory: 200 top-level dirs with nested
// subdirectories, plus one flat directory with 200k files.
func bigTree() *inventory.Tree {
	r := rand.New(rand.NewSource(1))
	t := inventory.New("/big", inventory.KindDir)
	file := func(p inventory.NodeID, i int) {
		id := t.Add(p, fmt.Sprintf("f%d.dat", i), inventory.KindFile)
		n := t.Node(id)
		n.Size = int64(r.ExpFloat64() * 1e6)
		n.Alloc = n.Size
		n.Cat = inventory.Category(r.Intn(int(inventory.NumCategories)))
	}
	flat := t.Add(0, "flat", inventory.KindDir)
	for i := range 200_000 {
		file(flat, i)
	}
	for d := range 200 {
		top := t.Add(0, fmt.Sprintf("dir%d", d), inventory.KindDir)
		for s := range 40 {
			sub := t.Add(top, fmt.Sprintf("sub%d", s), inventory.KindDir)
			for f := range 100 {
				file(sub, f)
			}
		}
	}
	t.Recompute()
	return t
}

var bigTreeCache *inventory.Tree

func benchModel(b *testing.B) *Model {
	if bigTreeCache == nil {
		bigTreeCache = bigTree()
	}
	m := newModel(Options{}, nil, bigTreeCache, func() {})
	m.profile = colorprofile.TrueColor
	send(m, size(200, 60))
	return m
}

// Cold frame: sorting children, LOD selection, layout and paint.
func BenchmarkBigColdFrame(b *testing.B) {
	m := benchModel(b)
	b.ReportMetric(float64(m.tree.Len()), "nodes")
	for b.Loop() {
		m.invalidate()
		m.View()
	}
}

// Resize: child lists cached; layout and paint rerun for the new size.
func BenchmarkBigResize(b *testing.B) {
	m := benchModel(b)
	m.View()
	i := 0
	for b.Loop() {
		send(m, size(150+i%100, 40+i%30))
		m.View()
		i++
	}
}

// Zoomed into a 200k-entry directory (cold child sort each time).
func BenchmarkBigFlatDirCold(b *testing.B) {
	m := benchModel(b)
	m.tree.Children(0, func(id inventory.NodeID, n *inventory.Node) {
		if n.Name == "flat" {
			m.zoom = id
		}
	})
	for b.Loop() {
		m.invalidate()
		m.View()
	}
}

// Arrow-key navigation including the re-render it triggers.
func BenchmarkBigNavigate(b *testing.B) {
	m := benchModel(b)
	m.View()
	keys := []string{"right", "down", "left", "up"}
	i := 0
	for b.Loop() {
		send(m, key(keys[i%4]))
		m.View()
		i++
	}
}

func BenchmarkBigFilter(b *testing.B) {
	m := benchModel(b)
	for _, q := range []string{"size > 1MB", "*.dat AND size > 500k", "path contains sub3"} {
		b.Run(q, func(b *testing.B) {
			m.search.ed.set(q)
			m.queryChanged()
			for b.Loop() {
				m.applyFilter()()
			}
		})
	}
}
