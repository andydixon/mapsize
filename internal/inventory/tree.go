// Package inventory is the in-memory filesystem model: an append-only arena
// of nodes addressed by stable NodeIDs, with incrementally maintained
// directory aggregates.
//
// Concurrency: a Tree has exactly one writer (the scan controller or a
// snapshot loader). Other goroutines must hold RLock while reading node data.
// Nodes are never removed, so a NodeID stays valid for the Tree's lifetime.
package inventory

import (
	"path/filepath"
	"slices"
	"sync"
	"time"
)

// NodeID addresses a node within one Tree. The root is always 0.
type NodeID uint32

// NoNode is the nil NodeID.
const NoNode NodeID = ^NodeID(0)

// Kind is the node type.
type Kind uint8

const (
	KindFile Kind = iota
	KindDir
	KindSymlink
	KindOther // devices, sockets, fifos, reparse points
)

func (k Kind) String() string {
	return [...]string{"File", "Directory", "Symlink", "Special"}[k&3]
}

// Flags record per-node facts.
type Flags uint16

const (
	FlagHardlinkDup  Flags = 1 << iota // additional link to an inode counted elsewhere
	FlagMountPoint                     // directory on a different device than its parent
	FlagVirtualFS                      // virtual filesystem, not descended
	FlagSkippedFS                      // other filesystem, not descended (--one-file-system)
	FlagSparse                         // allocated < logical (sparse or compressed)
	FlagError                          // stat or read failed
	FlagIncomplete                     // directory contents not (fully) read
	FlagScanned                        // directory fully read
	FlagLoop                           // directory already visited via another path
	FlagFollowed                       // reached through a followed symlink
	FlagAllocUnknown                   // platform cannot report allocated size
	FlagBrokenLink                     // symlink target does not exist
	FlagHardlinked                     // regular file with link count > 1
	FlagDeleted                        // moved to trash from the UI
)

// Node is one filesystem object. Sizes are own sizes; Tot* fields are
// aggregates including the node itself and (for directories) everything
// beneath it.
type Node struct {
	Parent, FirstChild, NextSibling NodeID
	dir                             uint32 // index into dir side table (directories only)

	Name string // single path component; the root holds the full root path

	Size, Alloc       int64 // own logical / allocated bytes
	TotSize, TotAlloc int64 // aggregates (hard-link duplicates contribute 0)
	MTime             int64 // unix nanoseconds

	Files, Dirs uint32 // recursive counts beneath (dirs excludes self)
	Errors      uint32 // recursive error count beneath and including self
	Mode        uint32 // fs.FileMode bits
	UID, GID    uint32
	Nlink       uint32

	Ext   uint16
	Flags Flags
	Kind  Kind
	Cat   Category
}

// IsDir reports whether the node is a directory.
func (n *Node) IsDir() bool { return n.Kind == KindDir }

// Delta is an aggregate change applied to a node and all its ancestors.
type Delta struct {
	Size, Alloc         int64
	Files, Dirs, Errors int64
	Cat                 [NumCategories]int64
}

const chunkBits = 16
const chunkSize = 1 << chunkBits

// chunked is a growable array that never moves existing elements.
type chunked[T any] struct {
	c [][]T
	n int
}

func (c *chunked[T]) add(v T) uint32 {
	if c.n&(chunkSize-1) == 0 && c.n>>chunkBits == len(c.c) {
		c.c = append(c.c, make([]T, 0, chunkSize))
	}
	ch := &c.c[c.n>>chunkBits]
	*ch = append(*ch, v)
	c.n++
	return uint32(c.n - 1)
}

func (c *chunked[T]) at(i uint32) *T { return &c.c[i>>chunkBits][i&(chunkSize-1)] }

// Tree is the inventory for one scan root.
type Tree struct {
	sync.RWMutex

	nodes chunked[Node]
	dirs  chunked[[NumCategories]int64]

	exts   []string
	extIdx map[string]uint16

	Stats Stats
}

// New creates a tree whose root has the given full path and kind.
func New(rootPath string, kind Kind) *Tree {
	t := &Tree{exts: []string{""}, extIdx: map[string]uint16{"": 0}}
	t.Stats.Root = rootPath
	t.Stats.Start = time.Now()
	t.Add(NoNode, rootPath, kind)
	return t
}

// Len returns the number of nodes.
func (t *Tree) Len() int { return t.nodes.n }

// Node returns the node for id. The pointer stays valid; reading its fields
// concurrently with the writer requires RLock.
func (t *Tree) Node(id NodeID) *Node { return t.nodes.at(uint32(id)) }

// Root returns the root NodeID (always 0).
func (t *Tree) Root() NodeID { return 0 }

// Add appends a child node and links it under parent. Only the writer may
// call it. It does not touch aggregates; use Propagate.
func (t *Tree) Add(parent NodeID, name string, kind Kind) NodeID {
	n := Node{Parent: parent, FirstChild: NoNode, NextSibling: NoNode, Name: name, Kind: kind}
	if kind == KindDir {
		n.dir = t.dirs.add([NumCategories]int64{})
	} else {
		n.Ext = t.internExt(Ext(name))
	}
	id := NodeID(t.nodes.add(n))
	if parent != NoNode {
		p := t.Node(parent)
		t.Node(id).NextSibling = p.FirstChild
		p.FirstChild = id
	}
	return id
}

func (t *Tree) internExt(e string) uint16 {
	if i, ok := t.extIdx[e]; ok {
		return i
	}
	if len(t.exts) >= 1<<16-1 {
		return 0
	}
	t.exts = append(t.exts, e)
	t.extIdx[e] = uint16(len(t.exts) - 1)
	return uint16(len(t.exts) - 1)
}

// ExtName returns the extension string for a node ("" if none).
func (t *Tree) ExtName(n *Node) string { return t.exts[n.Ext] }

// ExtCount returns the number of interned extensions (including "").
func (t *Tree) ExtCount() int { return len(t.exts) }

// ExtByIndex returns an interned extension.
func (t *Tree) ExtByIndex(i int) string { return t.exts[i] }

// CatSizes returns the per-category allocated bytes beneath a directory.
func (t *Tree) CatSizes(id NodeID) *[NumCategories]int64 {
	n := t.Node(id)
	if n.Kind != KindDir {
		return nil
	}
	return t.dirs.at(n.dir)
}

// Propagate adds d to id and every ancestor. Cost is O(depth); callers batch
// many entries into one Delta.
func (t *Tree) Propagate(id NodeID, d Delta) {
	for id != NoNode {
		n := t.Node(id)
		n.TotSize += d.Size
		n.TotAlloc += d.Alloc
		n.Files = addU32(n.Files, d.Files)
		n.Dirs = addU32(n.Dirs, d.Dirs)
		n.Errors = addU32(n.Errors, d.Errors)
		if n.Kind == KindDir {
			cs := t.dirs.at(n.dir)
			for i, v := range d.Cat {
				cs[i] += v
			}
		}
		id = n.Parent
	}
}

func addU32(a uint32, d int64) uint32 {
	v := int64(a) + d
	if v < 0 {
		return 0
	}
	if v > 1<<32-1 {
		return 1<<32 - 1
	}
	return uint32(v)
}

// Children calls fn for each child of id (in reverse insertion order).
func (t *Tree) Children(id NodeID, fn func(NodeID, *Node)) {
	for c := t.Node(id).FirstChild; c != NoNode; {
		n := t.Node(c)
		fn(c, n)
		c = n.NextSibling
	}
}

// ChildCount returns the number of direct children.
func (t *Tree) ChildCount(id NodeID) int {
	k := 0
	for c := t.Node(id).FirstChild; c != NoNode; c = t.Node(c).NextSibling {
		k++
	}
	return k
}

// Path reconstructs the full path of id.
func (t *Tree) Path(id NodeID) string {
	var parts []string
	for id != NoNode {
		n := t.Node(id)
		parts = append(parts, n.Name)
		id = n.Parent
	}
	slices.Reverse(parts)
	return filepath.Join(parts...)
}

// Ancestors returns the chain root..id inclusive.
func (t *Tree) Ancestors(id NodeID) []NodeID {
	var out []NodeID
	for id != NoNode {
		out = append(out, id)
		id = t.Node(id).Parent
	}
	slices.Reverse(out)
	return out
}

// IsAncestor reports whether a is id or an ancestor of id.
func (t *Tree) IsAncestor(a, id NodeID) bool {
	for id != NoNode {
		if id == a {
			return true
		}
		id = t.Node(id).Parent
	}
	return false
}

// Depth returns the number of ancestors of id.
func (t *Tree) Depth(id NodeID) int {
	d := 0
	for id = t.Node(id).Parent; id != NoNode; id = t.Node(id).Parent {
		d++
	}
	return d
}

// Recompute rebuilds all aggregates bottom-up. It relies on the invariant
// that a parent's ID is smaller than its children's, which both the scanner
// and the snapshot loader guarantee. Used after loading a snapshot.
func (t *Tree) Recompute() {
	for i := 0; i < t.Len(); i++ {
		n := t.Node(NodeID(i))
		n.TotSize, n.TotAlloc, n.Files, n.Dirs = 0, 0, 0, 0
		n.Errors = 0
		if n.Flags&FlagError != 0 {
			n.Errors = 1
		}
		if n.Flags&FlagHardlinkDup == 0 {
			n.TotSize, n.TotAlloc = n.Size, n.Alloc
		}
		if n.Kind == KindDir {
			*t.dirs.at(n.dir) = [NumCategories]int64{}
		}
	}
	for i := t.Len() - 1; i > 0; i-- {
		n := t.Node(NodeID(i))
		p := t.Node(n.Parent)
		p.TotSize += n.TotSize
		p.TotAlloc += n.TotAlloc
		p.Errors += n.Errors
		cs := t.dirs.at(p.dir)
		if n.Kind == KindDir {
			p.Files += n.Files
			p.Dirs += n.Dirs + 1
			for c, v := range t.dirs.at(n.dir) {
				cs[c] += v
			}
		} else {
			p.Files++
			if n.Flags&FlagHardlinkDup == 0 {
				cs[n.Cat] += n.Alloc
			}
		}
	}
}

// ExtIndexer returns a function interning extension strings; used by
// loaders that rebuild a tree from external data.
func (t *Tree) ExtIndexer() func(string) uint16 { return t.internExt }
