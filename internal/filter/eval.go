package filter

import (
	"path/filepath"
	"regexp"
	"strings"

	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/platform"
)

// Ctx is the evaluation context for one node. The tree must be read-locked.
type Ctx struct {
	Tree *inventory.Tree
	ID   inventory.NodeID
	Node *inventory.Node
	Now  int64 // unix nanoseconds
	path string
}

func (c *Ctx) str(f Field) string {
	n := c.Node
	switch f {
	case FName:
		return strings.ToLower(n.Name)
	case FPath:
		if c.path == "" {
			c.path = strings.ToLower(c.Tree.Path(c.ID))
		}
		return c.path
	case FExt:
		return c.Tree.ExtName(n)
	case FType:
		return strings.ToLower(n.Kind.String())
	case FCategory:
		if n.IsDir() {
			return "directory"
		}
		return strings.ToLower(n.Cat.String())
	case FOwner:
		return strings.ToLower(platform.UserName(n.UID))
	case FGroup:
		return strings.ToLower(platform.GroupName(n.GID))
	}
	return ""
}

func (c *Ctx) num(f Field) int64 {
	n := c.Node
	switch f {
	case FSize:
		return n.TotSize
	case FAllocated:
		return n.TotAlloc
	case FAge:
		return c.Now - n.MTime
	case FModified:
		return n.MTime
	case FFiles:
		return int64(n.Files)
	}
	return 0
}

type node interface{ eval(*Ctx) bool }

type andNode struct{ l, r node }
type orNode struct{ l, r node }
type notNode struct{ n node }

func (n andNode) eval(c *Ctx) bool { return n.l.eval(c) && n.r.eval(c) }
func (n orNode) eval(c *Ctx) bool  { return n.l.eval(c) || n.r.eval(c) }
func (n notNode) eval(c *Ctx) bool { return !n.n.eval(c) }

type containsNode struct {
	field Field
	sub   string
}

func (n containsNode) eval(c *Ctx) bool { return strings.Contains(c.str(n.field), n.sub) }

type globNode struct {
	field Field
	pat   string
}

func (n globNode) eval(c *Ctx) bool {
	ok, _ := filepath.Match(n.pat, c.str(n.field))
	return ok
}

type reNode struct {
	field Field
	re    *regexp.Regexp
}

func (n reNode) eval(c *Ctx) bool { return n.re.MatchString(c.str(n.field)) }

type eqNode struct {
	field Field
	set   []string
}

func (n eqNode) eval(c *Ctx) bool {
	if n.field == FFlag {
		for _, s := range n.set {
			if c.Node.Flags&flagNames[s] != 0 {
				return true
			}
		}
		return false
	}
	v := c.str(n.field)
	for _, s := range n.set {
		if v == s {
			return true
		}
	}
	return false
}

type numNode struct {
	field Field
	op    string
	vals  []int64
}

func (n numNode) eval(c *Ctx) bool {
	if (n.field == FAge || n.field == FModified) && c.Node.MTime == 0 {
		return false // unknown time never matches
	}
	v := c.num(n.field)
	for _, x := range n.vals {
		var ok bool
		switch n.op {
		case "=":
			ok = v == x
		case "!=":
			ok = v != x
		case ">":
			ok = v > x
		case ">=":
			ok = v >= x
		case "<":
			ok = v < x
		case "<=":
			ok = v <= x
		}
		if ok {
			return true
		}
	}
	return false
}

// Match evaluates the query against one node.
func (q *Query) Match(c *Ctx) bool {
	c.path = ""
	return q.root.eval(c)
}
