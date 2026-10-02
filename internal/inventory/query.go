package inventory

import (
	"container/heap"
	"sort"
)

// SizeMode selects which size drives layout and sorting.
type SizeMode uint8

const (
	SizeAllocated SizeMode = iota // disk usage (default)
	SizeLogical                   // apparent size
)

func (m SizeMode) String() string {
	if m == SizeLogical {
		return "logical"
	}
	return "allocated"
}

// Total returns the node's aggregate size in the given mode.
func (n *Node) Total(m SizeMode) int64 {
	if m == SizeLogical {
		return n.TotSize
	}
	return n.TotAlloc
}

// Own returns the node's own size in the given mode.
func (n *Node) Own(m SizeMode) int64 {
	if m == SizeLogical {
		return n.Size
	}
	return n.Alloc
}

type ranked struct {
	id  NodeID
	key int64
}

type minHeap []ranked

func (h minHeap) Len() int { return len(h) }
func (h minHeap) Less(i, j int) bool {
	if h[i].key != h[j].key {
		return h[i].key < h[j].key
	}
	return h[i].id > h[j].id
}
func (h minHeap) Swap(i, j int) { h[i], h[j] = h[j], h[i] }
func (h *minHeap) Push(x any)   { *h = append(*h, x.(ranked)) }
func (h *minHeap) Pop() any {
	old := *h
	x := old[len(old)-1]
	*h = old[:len(old)-1]
	return x
}

// TopK keeps the k entries with the largest keys; O(n log k).
type TopK struct {
	k int
	h minHeap
}

// NewTopK creates a collector for the k largest keys.
func NewTopK(k int) *TopK { return &TopK{k: k} }

// Offer considers one candidate.
func (t *TopK) Offer(id NodeID, key int64) {
	if t.k <= 0 {
		return
	}
	if len(t.h) < t.k {
		heap.Push(&t.h, ranked{id, key})
		return
	}
	if r := t.h[0]; key > r.key || key == r.key && id < r.id {
		t.h[0] = ranked{id, key}
		heap.Fix(&t.h, 0)
	}
}

// Sorted returns the collected IDs, largest key first (ties by ID).
func (t *TopK) Sorted() []NodeID {
	rs := append([]ranked(nil), t.h...)
	sort.Slice(rs, func(i, j int) bool {
		if rs[i].key != rs[j].key {
			return rs[i].key > rs[j].key
		}
		return rs[i].id < rs[j].id
	})
	out := make([]NodeID, len(rs))
	for i, r := range rs {
		out[i] = r.id
	}
	return out
}

// Walk visits id and all its descendants depth-first (iteratively).
func (t *Tree) Walk(id NodeID, fn func(NodeID, *Node) bool) {
	stack := []NodeID{id}
	for len(stack) > 0 {
		cur := stack[len(stack)-1]
		stack = stack[:len(stack)-1]
		n := t.Node(cur)
		if !fn(cur, n) {
			continue
		}
		for c := n.FirstChild; c != NoNode; c = t.Node(c).NextSibling {
			stack = append(stack, c)
		}
	}
}

// Top returns the k nodes beneath (and including) root with the largest key,
// considering only nodes accepted by keep.
func (t *Tree) Top(root NodeID, k int, keep func(*Node) bool, key func(*Node) int64) []NodeID {
	tk := NewTopK(k)
	t.Walk(root, func(id NodeID, n *Node) bool {
		if keep(n) {
			tk.Offer(id, key(n))
		}
		return true
	})
	return tk.Sorted()
}

// SortedChildren returns the children of id ordered by size descending.
func (t *Tree) SortedChildren(id NodeID, m SizeMode) []NodeID {
	var out []NodeID
	t.Children(id, func(c NodeID, _ *Node) { out = append(out, c) })
	sort.Slice(out, func(i, j int) bool {
		a, b := t.Node(out[i]).Total(m), t.Node(out[j]).Total(m)
		if a != b {
			return a > b
		}
		return out[i] < out[j]
	})
	return out
}

// ExtStat summarises one extension.
type ExtStat struct {
	Ext   string
	Cat   Category
	Count int64
	Size  int64 // logical
	Alloc int64
}

// ExtStats aggregates file statistics by extension beneath root.
func (t *Tree) ExtStats(root NodeID) []ExtStat {
	stats := make([]ExtStat, t.ExtCount())
	t.Walk(root, func(_ NodeID, n *Node) bool {
		if n.Kind == KindFile && n.Flags&FlagHardlinkDup == 0 {
			s := &stats[n.Ext]
			s.Count++
			s.Size += n.Size
			s.Alloc += n.Alloc
			s.Cat = n.Cat
		}
		return true
	})
	out := stats[:0]
	for i, s := range stats {
		if s.Count > 0 {
			s.Ext = t.exts[i]
			s.Cat = CategoryFor(s.Ext, false, false)
			out = append(out, s)
		}
	}
	return out
}
