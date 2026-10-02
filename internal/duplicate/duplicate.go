// Package duplicate finds files with identical content in stages:
//
//  1. group by exact size (inventory only, no I/O)
//  2. hash three 16 KiB samples (start, middle, end) of each candidate
//  3. full SHA-256 of files whose samples still collide
//
// Only stage 3 establishes identity. Files are never declared duplicates by
// name or size alone. Reading files may update their access times.
package duplicate

import (
	"context"
	"crypto/sha256"
	"io"
	"os"
	"sort"
	"sync"
	"sync/atomic"

	"github.com/andydixon/mapsize/internal/inventory"
)

// Group is a set of files verified to have identical content.
type Group struct {
	Size  int64
	Hash  [32]byte
	Files []inventory.NodeID
}

// Wasted returns the bytes reclaimable by keeping one copy.
func (g *Group) Wasted() int64 { return g.Size * int64(len(g.Files)-1) }

// Progress reports activity.
type Progress struct {
	Stage           string
	Candidates      int64 // files sharing a size with another file
	BytesToHash     int64
	BytesHashed     int64
	Skipped         int64 // unreadable or changed during hashing
	VerifiedGroups  int64
	CandidateGroups int64
}

// Options tunes the search.
type Options struct {
	MinSize int64 // ignore files smaller than this (default 1 byte)
	Workers int   // concurrent readers (default 4)
}

type cand struct {
	id   inventory.NodeID
	path string
	size int64
}

const sampleSize = 16 << 10

// Finder runs a search and exposes live progress.
type Finder struct {
	hashed, toHash, skipped atomic.Int64
	mu                      sync.Mutex
	p                       Progress
}

// Progress returns a copy of the current progress.
func (f *Finder) Progress() Progress {
	f.mu.Lock()
	p := f.p
	f.mu.Unlock()
	p.BytesHashed, p.BytesToHash, p.Skipped = f.hashed.Load(), f.toHash.Load(), f.skipped.Load()
	return p
}

func (f *Finder) stage(s string, fn func(*Progress)) {
	f.mu.Lock()
	f.p.Stage = s
	if fn != nil {
		fn(&f.p)
	}
	f.mu.Unlock()
}

// Find searches t (read-locked internally only while collecting candidates).
func (f *Finder) Find(ctx context.Context, t *inventory.Tree, o Options) ([]Group, error) {
	if o.MinSize < 1 {
		o.MinSize = 1
	}
	if o.Workers < 1 {
		o.Workers = 4
	}
	f.stage("grouping by size", nil)
	bySize := map[int64][]cand{}
	t.RLock()
	t.Walk(t.Root(), func(id inventory.NodeID, n *inventory.Node) bool {
		if n.Kind == inventory.KindFile && n.Flags&inventory.FlagHardlinkDup == 0 && n.Size >= o.MinSize {
			bySize[n.Size] = append(bySize[n.Size], cand{id: id, size: n.Size})
		}
		return true
	})
	var groups [][]cand
	var ncand int64
	for _, g := range bySize {
		if len(g) > 1 {
			for i := range g {
				g[i].path = t.Path(g[i].id)
			}
			groups = append(groups, g)
			ncand += int64(len(g))
		}
	}
	t.RUnlock()
	f.stage("sampling", func(p *Progress) { p.Candidates = ncand; p.CandidateGroups = int64(len(groups)) })

	// Stage 2: sample hashes.
	var sampled [][]cand
	for _, g := range groups {
		if ctx.Err() != nil {
			return nil, ctx.Err()
		}
		if g[0].size <= 3*sampleSize {
			sampled = append(sampled, g) // a sample would be the whole file
			continue
		}
		sampled = append(sampled, f.split(ctx, g, o.Workers, sampleHash)...)
	}

	// Stage 3: full hashes.
	var total int64
	for _, g := range sampled {
		total += g[0].size * int64(len(g))
	}
	f.toHash.Store(total)
	f.stage("hashing", nil)
	var out []Group
	for _, g := range sampled {
		if ctx.Err() != nil {
			return nil, ctx.Err()
		}
		for _, sub := range f.splitHash(ctx, g, o.Workers) {
			grp := Group{Size: sub.size, Hash: sub.hash}
			for _, c := range sub.files {
				grp.Files = append(grp.Files, c.id)
			}
			out = append(out, grp)
		}
	}
	sort.Slice(out, func(i, j int) bool {
		if a, b := out[i].Wasted(), out[j].Wasted(); a != b {
			return a > b
		}
		return out[i].Files[0] < out[j].Files[0]
	})
	f.stage("done", func(p *Progress) { p.VerifiedGroups = int64(len(out)) })
	return out, ctx.Err()
}

type hashed struct {
	c   cand
	sum [32]byte
	ok  bool
}

// hashAll hashes every candidate with a bounded worker pool.
func (f *Finder) hashAll(ctx context.Context, g []cand, workers int, fn func(*Finder, cand) ([32]byte, bool)) []hashed {
	res := make([]hashed, len(g))
	var wg sync.WaitGroup
	sem := make(chan struct{}, workers)
	for i, c := range g {
		if ctx.Err() != nil {
			break
		}
		sem <- struct{}{}
		wg.Add(1)
		go func() {
			defer wg.Done()
			defer func() { <-sem }()
			sum, ok := fn(f, c)
			if !ok {
				f.skipped.Add(1)
			}
			res[i] = hashed{c, sum, ok}
		}()
	}
	wg.Wait()
	return res
}

func (f *Finder) split(ctx context.Context, g []cand, workers int, fn func(*Finder, cand) ([32]byte, bool)) [][]cand {
	by := map[[32]byte][]cand{}
	for _, h := range f.hashAll(ctx, g, workers, fn) {
		if h.ok {
			by[h.sum] = append(by[h.sum], h.c)
		}
	}
	var out [][]cand
	for _, v := range by {
		if len(v) > 1 {
			out = append(out, v)
		}
	}
	return out
}

type fullGroup struct {
	size  int64
	hash  [32]byte
	files []cand
}

func (f *Finder) splitHash(ctx context.Context, g []cand, workers int) []fullGroup {
	by := map[[32]byte][]cand{}
	for _, h := range f.hashAll(ctx, g, workers, fullHash) {
		if h.ok {
			by[h.sum] = append(by[h.sum], h.c)
		}
	}
	var out []fullGroup
	for sum, v := range by {
		if len(v) > 1 {
			sort.Slice(v, func(i, j int) bool { return v[i].id < v[j].id })
			out = append(out, fullGroup{size: v[0].size, hash: sum, files: v})
		}
	}
	return out
}

// open opens a candidate and checks it still has the inventoried size.
func open(c cand) (*os.File, bool) {
	fh, err := os.Open(c.path)
	if err != nil {
		return nil, false
	}
	fi, err := fh.Stat()
	if err != nil || !fi.Mode().IsRegular() || fi.Size() != c.size {
		fh.Close()
		return nil, false
	}
	return fh, true
}

func sampleHash(_ *Finder, c cand) ([32]byte, bool) {
	fh, ok := open(c)
	if !ok {
		return [32]byte{}, false
	}
	defer fh.Close()
	h := sha256.New()
	buf := make([]byte, sampleSize)
	for _, off := range []int64{0, c.size/2 - sampleSize/2, c.size - sampleSize} {
		if _, err := fh.ReadAt(buf, off); err != nil && err != io.EOF {
			return [32]byte{}, false
		}
		h.Write(buf)
	}
	var s [32]byte
	h.Sum(s[:0])
	return s, true
}

func fullHash(f *Finder, c cand) ([32]byte, bool) {
	fh, ok := open(c)
	if !ok {
		return [32]byte{}, false
	}
	defer fh.Close()
	h := sha256.New()
	n, err := io.Copy(h, &countingReader{fh, &f.hashed})
	if err != nil || n != c.size {
		return [32]byte{}, false
	}
	var s [32]byte
	h.Sum(s[:0])
	return s, true
}

type countingReader struct {
	r io.Reader
	n *atomic.Int64
}

func (c *countingReader) Read(p []byte) (int, error) {
	n, err := c.r.Read(p)
	c.n.Add(int64(n))
	return n, err
}
