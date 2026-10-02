package filter

import (
	"context"
	"time"

	"github.com/andydixon/mapsize/internal/inventory"
)

// Result holds per-node filtered sizes: the bytes of matching content beneath
// each node. A matching directory contributes its whole subtree (once, even
// if descendants match too). Matched marks every node satisfying the query. Nodes added
// after the result was computed (live scan) have no entry and are treated as
// not matching until the filter is re-applied.
type Result struct {
	Query   *Query
	Mode    inventory.SizeMode
	Sizes   []int64 // per NodeID; len = tree size at evaluation time
	Matched []bool  // node itself matched
	Count   int64   // number of matching nodes
	Total   int64
	Elapsed time.Duration
}

// Size returns the filtered size of id (0 if out of range).
func (r *Result) Size(id inventory.NodeID) int64 {
	if int(id) < len(r.Sizes) {
		return r.Sizes[id]
	}
	return 0
}

// IsMatch reports whether id itself matched.
func (r *Result) IsMatch(id inventory.NodeID) bool {
	return int(id) < len(r.Matched) && r.Matched[id]
}

// Apply evaluates q over the whole tree. The tree must be read-locked by the
// caller. It relies on parents having smaller IDs than their children.
func Apply(ctx context.Context, t *inventory.Tree, q *Query, m inventory.SizeMode) (*Result, error) {
	start := time.Now()
	n := t.Len()
	r := &Result{Query: q, Mode: m, Sizes: make([]int64, n), Matched: make([]bool, n)}
	covered := make([]bool, n)
	c := &Ctx{Tree: t, Now: time.Now().UnixNano()}
	for i := range n {
		if i&0xffff == 0 && ctx.Err() != nil {
			return nil, ctx.Err()
		}
		id := inventory.NodeID(i)
		nd := t.Node(id)
		c.ID, c.Node = id, nd
		if i > 0 && q.Match(c) {
			r.Matched[i] = true
			covered[i] = true
			r.Count++
		}
		if i > 0 && covered[nd.Parent] {
			covered[i] = true
		}
		if covered[i] && nd.Flags&inventory.FlagHardlinkDup == 0 {
			r.Sizes[i] = nd.Own(m)
		}
	}
	for i := n - 1; i > 0; i-- {
		r.Sizes[t.Node(inventory.NodeID(i)).Parent] += r.Sizes[i]
	}
	r.Total = r.Sizes[0]
	r.Elapsed = time.Since(start)
	return r, nil
}
