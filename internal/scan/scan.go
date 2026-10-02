// Package scan implements the bounded concurrent filesystem scanner.
//
// Architecture (see ARCHITECTURE.md): a single controller goroutine owns the
// inventory.Tree and a FIFO of pending directories. A fixed pool of workers
// performs directory I/O and sends result chunks back. The controller's
// select loop offers jobs and accepts results at the same time, so bounded
// channels can never deadlock: workers never enqueue work themselves.
package scan

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/platform"
)

// FollowMode controls symlink traversal.
type FollowMode int

const (
	FollowNone FollowMode = iota
	FollowSameFS
	FollowAll
)

func (f FollowMode) String() string {
	return [...]string{"none", "same-filesystem", "all"}[f]
}

// ParseFollow parses a --follow-symlinks value.
func ParseFollow(s string) (FollowMode, error) {
	switch s {
	case "none", "":
		return FollowNone, nil
	case "same-filesystem", "same-fs":
		return FollowSameFS, nil
	case "all":
		return FollowAll, nil
	}
	return 0, fmt.Errorf("invalid --follow-symlinks %q (want none, same-filesystem or all)", s)
}

// Workers resolves a concurrency mode name or number to a worker count.
func Workers(mode string) (int, error) {
	cpu := runtime.NumCPU()
	switch mode {
	case "conservative":
		return 2, nil
	case "balanced", "":
		return min(8, max(2, 2*cpu)), nil
	case "aggressive":
		return min(64, max(4, 8*cpu)), nil
	}
	var n int
	if _, err := fmt.Sscan(mode, &n); err != nil || n < 1 || n > 1024 {
		return 0, fmt.Errorf("invalid workers %q (conservative, balanced, aggressive or 1-1024)", mode)
	}
	return n, nil
}

// Options configures a scan.
type Options struct {
	Workers       int
	Follow        FollowMode
	OneFileSystem bool
	Excludes      []string

	// Tuning knobs, mostly for tests. Zero means default.
	ChunkSize  int // entries per result chunk
	JobBuffer  int // capacity of the job channel
	ResultBuf  int // capacity of the result channel
	ApplyBatch int // max results applied per write-lock hold
}

func (o *Options) defaults() {
	if o.Workers <= 0 {
		o.Workers, _ = Workers("balanced")
	}
	if o.ChunkSize <= 0 {
		o.ChunkSize = 1024
	}
	if o.JobBuffer <= 0 {
		o.JobBuffer = o.Workers * 2
	}
	if o.ResultBuf <= 0 {
		o.ResultBuf = o.Workers * 4
	}
	if o.ApplyBatch <= 0 {
		o.ApplyBatch = 64
	}
}

// Progress is a cheap, lock-free view of scan activity.
type Progress struct {
	Entries  int64
	Pending  int64 // directories discovered but not yet dispatched
	Inflight int64 // directories being read
	Workers  int
}

// Scanner runs one scan. Tree is safe to read under Tree.RLock while the
// scan runs.
type Scanner struct {
	Tree *inventory.Tree
	opts Options

	entries, pending, inflight atomic.Int64
	done                       chan struct{}
}

type job struct {
	id   inventory.NodeID
	path string
	dev  uint64
	// hints derived once per directory
	inCache, inLog bool
}

type result struct {
	job     *job
	entries []platform.Meta
	final   bool
	err     error
}

type inodeKey struct{ dev, ino uint64 }

// Start stats root, creates the tree and begins scanning in the background.
func Start(ctx context.Context, root string, opts Options) (*Scanner, error) {
	opts.defaults()
	// Path-style exclusions are matched against absolute paths.
	excl := make([]string, len(opts.Excludes))
	for i, p := range opts.Excludes {
		excl[i] = p
		if strings.ContainsAny(p, `/\`) {
			if a, err := filepath.Abs(p); err == nil {
				excl[i] = a
			}
		}
	}
	opts.Excludes = excl
	abs, err := filepath.Abs(root)
	if err != nil {
		return nil, err
	}
	m, err := platform.Stat(abs)
	if err != nil {
		return nil, err
	}
	kind := kindOf(m)
	t := inventory.New(abs, kind)
	t.Stats.Workers = opts.Workers
	t.Stats.Excludes = opts.Excludes
	t.Stats.OneFileSystem = opts.OneFileSystem
	t.Stats.Follow = opts.Follow.String()
	r := t.Node(t.Root())
	setMeta(r, m)
	r.TotSize, r.TotAlloc = m.Size, m.Alloc
	s := &Scanner{Tree: t, opts: opts, done: make(chan struct{})}
	if kind != inventory.KindDir {
		t.Stats.Files = 1
		t.Stats.Complete = true
		t.Stats.End = time.Now()
		close(s.done)
		return s, nil
	}
	t.Stats.Dirs = 1
	c := &controller{s: s, t: t, opts: opts, links: map[inodeKey]struct{}{}}
	// Loop detection needs inode identity; without it (Windows and other
	// portable-fallback platforms) symlinks are not followed at all.
	if opts.Follow != FollowNone && !m.HasIno {
		c.opts.Follow = FollowNone
		t.Stats.Follow = "none (no inode identity on this platform)"
	}
	if c.opts.Follow != FollowNone {
		c.visited = map[inodeKey]struct{}{{m.Dev, m.Ino}: {}}
	}
	c.rootDev = m.Dev
	c.pending = []pendingDir{{id: t.Root(), dev: m.Dev}}
	s.pending.Store(1)
	go c.run(ctx)
	return s, nil
}

// Done is closed when the scan has finished or been cancelled and drained.
func (s *Scanner) Done() <-chan struct{} { return s.done }

// Progress returns live counters without locking the tree.
func (s *Scanner) Progress() Progress {
	return Progress{Entries: s.entries.Load(), Pending: s.pending.Load(), Inflight: s.inflight.Load(), Workers: s.opts.Workers}
}

// maxEntriesPerLock bounds how long the controller holds the write lock, so
// the UI's read lock is never starved for more than a few milliseconds.
const maxEntriesPerLock = 8192

type pendingDir struct {
	id  inventory.NodeID
	dev uint64
}

type controller struct {
	s       *Scanner
	t       *inventory.Tree
	opts    Options
	rootDev uint64
	pending []pendingDir // FIFO; head index advances, compacted occasionally
	head    int
	links   map[inodeKey]struct{}
	visited map[inodeKey]struct{} // only when following symlinks
}

func (c *controller) run(ctx context.Context) {
	defer guard()
	defer close(c.s.done)
	jobs := make(chan *job, c.opts.JobBuffer)
	results := make(chan result, c.opts.ResultBuf)
	var wg sync.WaitGroup
	for range c.opts.Workers {
		wg.Add(1)
		go func() {
			defer wg.Done()
			defer guard()
			worker(ctx, jobs, results, c.opts.ChunkSize)
		}()
	}

	inflight := 0
	ctxDone := ctx.Done()
	cancelled := false
	var next *job
	batch := make([]result, 0, c.opts.ApplyBatch)
	for {
		if next == nil && !cancelled && c.head < len(c.pending) {
			next = c.makeJob(c.pending[c.head])
		}
		if inflight == 0 && next == nil {
			break
		}
		var jobCh chan<- *job
		if next != nil {
			jobCh = jobs
		}
		select {
		case jobCh <- next:
			next = nil
			c.head++
			if c.head > 4096 && c.head*2 > len(c.pending) {
				c.pending = append(c.pending[:0], c.pending[c.head:]...)
				c.head = 0
			}
			inflight++
			c.s.inflight.Store(int64(inflight))
			c.s.pending.Store(int64(len(c.pending) - c.head))
		case r := <-results:
			batch = append(batch[:0], r)
			n := len(r.entries)
		drain:
			for len(batch) < cap(batch) && n < maxEntriesPerLock {
				select {
				case r := <-results:
					batch = append(batch, r)
					n += len(r.entries)
				default:
					break drain
				}
			}
			c.t.Lock()
			for i := range batch {
				if c.apply(&batch[i]) {
					inflight--
				}
				batch[i] = result{}
			}
			c.t.Unlock()
			c.s.inflight.Store(int64(inflight))
			c.s.pending.Store(int64(len(c.pending) - c.head))
		case <-ctxDone:
			cancelled = true
			ctxDone = nil
			next = nil
		}
	}
	close(jobs)
	wg.Wait()

	c.t.Lock()
	st := &c.t.Stats
	if cancelled {
		st.Cancelled = true
		st.Unscanned += int64(len(c.pending) - c.head)
		for _, p := range c.pending[c.head:] {
			c.t.Node(p.id).Flags |= inventory.FlagIncomplete
		}
	}
	st.Complete = true
	st.End = time.Now()
	c.t.Unlock()
	c.s.pending.Store(0)
	c.s.inflight.Store(0)
}

func (c *controller) makeJob(p pendingDir) *job {
	path := c.t.Path(p.id)
	j := &job{id: p.id, path: path, dev: p.dev}
	j.inCache, j.inLog = inventory.PathHints(path)
	return j
}

func worker(ctx context.Context, jobs <-chan *job, results chan<- result, chunk int) {
	for j := range jobs {
		var err error
		if err = ctx.Err(); err == nil {
			err = platform.ReadDir(j.path, chunk, func(ms []platform.Meta) error {
				if err := ctx.Err(); err != nil {
					return err
				}
				results <- result{job: j, entries: ms}
				return nil
			})
		}
		results <- result{job: j, final: true, err: err}
	}
}

func kindOf(m platform.Meta) inventory.Kind {
	switch {
	case m.Mode.IsDir():
		return inventory.KindDir
	case m.Mode.IsRegular():
		return inventory.KindFile
	case m.Mode&os.ModeSymlink != 0:
		return inventory.KindSymlink
	}
	return inventory.KindOther
}

func setMeta(n *inventory.Node, m platform.Meta) {
	n.Size, n.Alloc = m.Size, m.Alloc
	n.MTime = m.MTime
	n.Mode = uint32(m.Mode)
	n.UID, n.GID = m.UID, m.GID
	n.Nlink = m.Nlink
	if !m.AllocKnown {
		n.Flags |= inventory.FlagAllocUnknown
	}
}

// apply merges one result into the tree. It reports whether the result
// completed its directory. Called with the tree write-locked.
func (c *controller) apply(r *result) bool {
	t := c.t
	j := r.job
	dir := j.id
	var d inventory.Delta
	c.s.entries.Add(int64(len(r.entries)))
	for _, m := range r.entries {
		c.addEntry(j, m, &d)
	}
	if r.final {
		dn := t.Node(dir)
		switch {
		case r.err == nil:
			dn.Flags |= inventory.FlagScanned
		case errors.Is(r.err, context.Canceled) || errors.Is(r.err, context.DeadlineExceeded):
			dn.Flags |= inventory.FlagIncomplete
			t.Stats.Unscanned++
		default:
			slog.Debug("read directory failed", "path", j.path, "err", r.err)
			dn.Flags |= inventory.FlagError | inventory.FlagIncomplete
			t.Stats.AddError(inventory.ErrorRecord{Node: dir, Kind: inventory.Classify(r.err), Msg: errMsg(r.err)})
			d.Errors++
		}
	}
	if d != (inventory.Delta{}) {
		t.Propagate(dir, d)
	}
	return r.final
}

func errMsg(err error) string {
	var pe *os.PathError
	if errors.As(err, &pe) {
		return pe.Op + ": " + pe.Err.Error()
	}
	return err.Error()
}

func (c *controller) excluded(path, name string) bool {
	for _, p := range c.opts.Excludes {
		if strings.ContainsAny(p, `/\`) {
			pp := filepath.Clean(p)
			if path == pp || strings.HasPrefix(path, pp+string(os.PathSeparator)) {
				return true
			}
			if ok, _ := filepath.Match(pp, path); ok {
				return true
			}
		} else if ok, _ := filepath.Match(p, name); ok {
			return true
		}
	}
	return false
}

func (c *controller) addEntry(j *job, m platform.Meta, d *inventory.Delta) {
	t := c.t
	st := &t.Stats
	if m.Err != nil {
		k := inventory.Classify(m.Err)
		if k == inventory.ErrVanished {
			st.AddError(inventory.ErrorRecord{Node: j.id, Name: m.Name, Kind: k, Msg: "vanished between listing and stat"})
			d.Errors++
			return
		}
		id := t.Add(j.id, m.Name, inventory.KindOther)
		n := t.Node(id)
		n.Flags |= inventory.FlagError
		n.Errors = 1
		st.AddError(inventory.ErrorRecord{Node: id, Kind: k, Msg: errMsg(m.Err)})
		d.Errors++
		d.Files++
		st.Others++
		return
	}
	var path string
	if len(c.opts.Excludes) > 0 {
		path = platform.JoinPath(j.path, m.Name)
		if c.excluded(path, m.Name) {
			st.Excluded++
			return
		}
	}
	kind := kindOf(m)
	var target platform.Meta
	followed, broken := false, false
	if kind == inventory.KindSymlink {
		if path == "" {
			path = platform.JoinPath(j.path, m.Name)
		}
		tm, err := platform.Stat(path)
		switch {
		case err != nil:
			st.BrokenLinks++
			broken = true
		case tm.Mode.IsDir() && tm.HasIno && (c.opts.Follow == FollowAll || c.opts.Follow == FollowSameFS && tm.Dev == j.dev):
			target, followed = tm, true
			kind = inventory.KindDir
		}
	}

	id := t.Add(j.id, m.Name, kind)
	n := t.Node(id)
	setMeta(n, m)
	if broken {
		n.Flags |= inventory.FlagBrokenLink
	}

	switch kind {
	case inventory.KindDir:
		st.Dirs++
		d.Dirs++
		d.Size += n.Size
		d.Alloc += n.Alloc
		n.TotSize, n.TotAlloc = n.Size, n.Alloc
		dev := m.Dev
		if followed {
			n.Flags |= inventory.FlagFollowed
			dev = target.Dev
		}
		if c.visited != nil {
			key := inodeKey{m.Dev, m.Ino}
			if followed {
				key = inodeKey{target.Dev, target.Ino}
			}
			if _, seen := c.visited[key]; seen {
				n.Flags |= inventory.FlagLoop
				st.LoopsSkipped++
				return
			}
			c.visited[key] = struct{}{}
		}
		if m.HasIno && dev != j.dev {
			n.Flags |= inventory.FlagMountPoint
			if c.opts.OneFileSystem {
				n.Flags |= inventory.FlagSkippedFS
				st.SkippedMounts++
				return
			}
			if path == "" {
				path = platform.JoinPath(j.path, m.Name)
			}
			if platform.IsVirtualFS(path) {
				n.Flags |= inventory.FlagVirtualFS
				st.VirtualSkipped++
				return
			}
		}
		c.pending = append(c.pending, pendingDir{id: id, dev: dev})
	case inventory.KindFile:
		st.Files++
		d.Files++
		if m.Nlink > 1 && m.HasIno {
			n.Flags |= inventory.FlagHardlinked
			key := inodeKey{m.Dev, m.Ino}
			if _, seen := c.links[key]; seen {
				n.Flags |= inventory.FlagHardlinkDup
				st.HardlinkDups++
				return
			}
			c.links[key] = struct{}{}
		}
		if m.AllocKnown && m.Alloc < m.Size && m.Size-m.Alloc >= 4096 {
			n.Flags |= inventory.FlagSparse
		}
		n.Cat = inventory.CategoryFor(t.ExtName(n), j.inCache, j.inLog)
		n.TotSize, n.TotAlloc = n.Size, n.Alloc
		d.Size += n.Size
		d.Alloc += n.Alloc
		d.Cat[n.Cat] += n.Alloc
	default:
		if kind == inventory.KindSymlink {
			st.Symlinks++
		} else {
			st.Others++
		}
		d.Files++
		n.TotSize, n.TotAlloc = n.Size, n.Alloc
		d.Size += n.Size
		d.Alloc += n.Alloc
	}
}
