package snapshot

import (
	"sort"

	"github.com/andydixon/mapsize/internal/inventory"
)

// Diff relates two inventories by path. Both trees must not be mutated
// while a Diff is in use (snapshots and finished scans satisfy this).
type Diff struct {
	Old, New *inventory.Tree
	// OldOf maps a New NodeID to the matching Old NodeID, or NoNode if the
	// entry is new.
	OldOf []inventory.NodeID
	// Removed lists the topmost Old nodes with no counterpart in New.
	Removed []inventory.NodeID
	// RemovedUnder sums, per New directory, the sizes of removed children
	// (in both size modes) so growth colouring can account for them.
	removedUnder map[inventory.NodeID][2]int64
}

// Compare matches nodes by name, directory by directory, from the roots.
func Compare(old, new *inventory.Tree) *Diff {
	d := &Diff{Old: old, New: new, OldOf: make([]inventory.NodeID, new.Len()),
		removedUnder: map[inventory.NodeID][2]int64{}}
	for i := range d.OldOf {
		d.OldOf[i] = inventory.NoNode
	}
	type pair struct{ o, n inventory.NodeID }
	stack := []pair{{old.Root(), new.Root()}}
	for len(stack) > 0 {
		p := stack[len(stack)-1]
		stack = stack[:len(stack)-1]
		d.OldOf[p.n] = p.o
		on, nn := old.Node(p.o), new.Node(p.n)
		if !on.IsDir() || !nn.IsDir() {
			continue
		}
		byName := map[string]inventory.NodeID{}
		old.Children(p.o, func(id inventory.NodeID, n *inventory.Node) { byName[n.Name] = id })
		new.Children(p.n, func(id inventory.NodeID, n *inventory.Node) {
			if oid, ok := byName[n.Name]; ok {
				delete(byName, n.Name)
				stack = append(stack, pair{oid, id})
			}
		})
		for _, oid := range byName {
			d.Removed = append(d.Removed, oid)
			o := old.Node(oid)
			r := d.removedUnder[p.n]
			r[0] += o.TotAlloc
			r[1] += o.TotSize
			d.removedUnder[p.n] = r
		}
	}
	sort.Slice(d.Removed, func(i, j int) bool { return d.Removed[i] < d.Removed[j] })
	return d
}

// OldSize returns the matching old size of a New node and whether it existed.
func (d *Diff) OldSize(id inventory.NodeID, m inventory.SizeMode) (int64, bool) {
	o := d.OldOf[id]
	if o == inventory.NoNode {
		return 0, false
	}
	return d.Old.Node(o).Total(m), true
}

// Delta returns new minus old size for a New node (full size if new).
func (d *Diff) Delta(id inventory.NodeID, m inventory.SizeMode) int64 {
	old, _ := d.OldSize(id, m)
	return d.New.Node(id).Total(m) - old
}

// Status classifies a node.
type Status uint8

const (
	Unchanged Status = iota
	Added
	Removed
	Grew
	Shrank
)

func (s Status) String() string {
	return [...]string{"unchanged", "new", "removed", "increased", "decreased"}[s]
}

// StatusOf classifies a New node.
func (d *Diff) StatusOf(id inventory.NodeID, m inventory.SizeMode) Status {
	if d.OldOf[id] == inventory.NoNode {
		return Added
	}
	switch delta := d.Delta(id, m); {
	case delta > 0:
		return Grew
	case delta < 0:
		return Shrank
	}
	return Unchanged
}

// Change is one row in a change report.
type Change struct {
	Path   string
	Status Status
	Delta  int64
	Old    int64
	New    int64
	Node   inventory.NodeID // in New (NoNode for removed)
}

// Changes returns the k most significant changes. A directory is listed
// only if its change is not almost entirely explained by a single child, so
// the report points at /var/lib/postgresql rather than /, /var and /var/lib.
func (d *Diff) Changes(k int, m inventory.SizeMode) []Change {
	var out []Change
	for i := range d.New.Len() {
		id := inventory.NodeID(i)
		delta := d.Delta(id, m)
		if delta == 0 {
			continue
		}
		n := d.New.Node(id)
		if d.OldOf[id] != inventory.NoNode && n.IsDir() && explainedByChild(d, id, delta, m) {
			continue
		}
		if p := n.Parent; p != inventory.NoNode && d.OldOf[p] == inventory.NoNode {
			continue // inside a wholly new directory; the directory is listed
		}
		old, _ := d.OldSize(id, m)
		out = append(out, Change{Path: d.New.Path(id), Status: d.StatusOf(id, m), Delta: delta, Old: old, New: n.Total(m), Node: id})
	}
	for _, oid := range d.Removed {
		o := d.Old.Node(oid)
		out = append(out, Change{Path: d.Old.Path(oid), Status: Removed, Delta: -o.Total(m), Old: o.Total(m), Node: inventory.NoNode})
	}
	sort.Slice(out, func(i, j int) bool {
		a, b := abs(out[i].Delta), abs(out[j].Delta)
		if a != b {
			return a > b
		}
		return out[i].Path < out[j].Path
	})
	if k > 0 && len(out) > k {
		out = out[:k]
	}
	return out
}

func explainedByChild(d *Diff, id inventory.NodeID, delta int64, m inventory.SizeMode) bool {
	explained := false
	d.New.Children(id, func(c inventory.NodeID, _ *inventory.Node) {
		if cd := d.Delta(c, m); (cd > 0) == (delta > 0) && abs(cd)*10 >= abs(delta)*8 {
			explained = true
		}
	})
	if r, ok := d.removedUnder[id]; ok && !explained {
		v := r[0]
		if m == inventory.SizeLogical {
			v = r[1]
		}
		if delta < 0 && v*10 >= abs(delta)*8 {
			explained = true
		}
	}
	return explained
}

func abs(v int64) int64 {
	if v < 0 {
		return -v
	}
	return v
}
