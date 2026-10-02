// Package snapshot implements the versioned, checksummed binary inventory
// format and snapshot comparison.
//
// Layout (all integers little-endian / varint):
//
//	magic      8 bytes  brand.SnapshotMagic
//	version    uint16   currently 1
//	flags      uint16   bit 0: body is gzip-compressed (always set by Save)
//	body       gzip stream of:
//	  meta     uvarint length + JSON (root, timestamps, counters, options)
//	  exts     uvarint count, then count × (uvarint length + bytes)
//	  nodes    uvarint count, then per node:
//	             uvarint parentDelta (id-parent; 0 only for the root)
//	             byte kind, byte category, uvarint flags
//	             uvarint nameLen + name
//	             varint size, varint alloc, varint mtime
//	             uvarint mode, uid, gid, nlink, ext
//	  errors   uvarint count, then per record: uvarint node, byte kind,
//	           uvarint len + name, uvarint len + message
//	sha256     32 bytes over the uncompressed body
//
// Nodes are written in ID order; a parent always precedes its children, so a
// reader can rebuild the tree in one pass and malformed input cannot create
// cycles. Aggregates are recomputed on load, never trusted.
package snapshot

import (
	"bufio"
	"bytes"
	"compress/gzip"
	"crypto/sha256"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"strings"
	"time"

	"github.com/andydixon/mapsize/internal/brand"
	"github.com/andydixon/mapsize/internal/inventory"
)

// Version is the current format version.
const Version = 1

// Limits applied when reading untrusted snapshots.
const (
	MaxNodes = 1<<31 - 1
	MaxDepth = 4096    // far beyond any real path; bounds recursion in exporters
	MaxSize  = 1 << 60 // per-node size cap (1 EiB); keeps aggregates from overflowing
	// DefaultMaxBody caps the decompressed body for Load. LoadFile uses a
	// limit proportional to the file size instead (see bodyLimit).
	DefaultMaxBody = 4 << 30
	MaxNameLen     = 1 << 16
	MaxMetaLen     = 16 << 20
	MaxExtLen      = 64
	MaxMsgLen      = 4096
	maxExts        = 1<<16 - 1
	flagGzipped    = 1
)

type meta struct {
	Root           string    `json:"root"`
	Created        time.Time `json:"created"`
	Start          time.Time `json:"start"`
	End            time.Time `json:"end"`
	Complete       bool      `json:"complete"`
	Cancelled      bool      `json:"cancelled"`
	Files          int64     `json:"files"`
	Dirs           int64     `json:"dirs"`
	Symlinks       int64     `json:"symlinks"`
	Others         int64     `json:"others"`
	ErrCounts      []int64   `json:"err_counts"`
	Excluded       int64     `json:"excluded"`
	SkippedMounts  int64     `json:"skipped_mounts"`
	VirtualSkipped int64     `json:"virtual_skipped"`
	LoopsSkipped   int64     `json:"loops_skipped"`
	BrokenLinks    int64     `json:"broken_links"`
	HardlinkDups   int64     `json:"hardlink_dups"`
	Unscanned      int64     `json:"unscanned"`
	Excludes       []string  `json:"excludes"`
	OneFileSystem  bool      `json:"one_file_system"`
	Follow         string    `json:"follow"`
	Workers        int       `json:"workers"`
	Host           string    `json:"host"`
	Generator      string    `json:"generator"`
}

// IsSnapshot reports whether path starts with the snapshot magic.
func IsSnapshot(path string) bool {
	f, err := os.Open(path)
	if err != nil {
		return false
	}
	defer f.Close()
	b := make([]byte, len(brand.SnapshotMagic))
	_, err = io.ReadFull(f, b)
	return err == nil && string(b) == brand.SnapshotMagic
}

type writer struct {
	w   io.Writer
	buf [binary.MaxVarintLen64]byte
	err error
}

func (w *writer) bytes(b []byte) {
	if w.err == nil {
		_, w.err = w.w.Write(b)
	}
}
func (w *writer) uvarint(v uint64) { w.bytes(w.buf[:binary.PutUvarint(w.buf[:], v)]) }
func (w *writer) varint(v int64)   { w.bytes(w.buf[:binary.PutVarint(w.buf[:], v)]) }
func (w *writer) str(s string)     { w.uvarint(uint64(len(s))); w.bytes([]byte(s)) }

// Save writes t to w. The caller must hold t's read lock.
func Save(out io.Writer, t *inventory.Tree) error {
	bw := bufio.NewWriterSize(out, 1<<20)
	hdr := make([]byte, 0, 12)
	hdr = append(hdr, brand.SnapshotMagic...)
	hdr = binary.LittleEndian.AppendUint16(hdr, Version)
	hdr = binary.LittleEndian.AppendUint16(hdr, flagGzipped)
	bw.Write(hdr)

	gz, _ := gzip.NewWriterLevel(bw, gzip.BestSpeed)
	h := sha256.New()
	w := &writer{w: io.MultiWriter(gz, h)}

	st := &t.Stats
	host, _ := os.Hostname()
	m := meta{Root: st.Root, Created: time.Now().UTC(), Start: st.Start, End: st.End, Complete: st.Complete,
		Cancelled: st.Cancelled, Files: st.Files, Dirs: st.Dirs, Symlinks: st.Symlinks, Others: st.Others,
		ErrCounts: st.ErrCounts[:], Excluded: st.Excluded, SkippedMounts: st.SkippedMounts,
		VirtualSkipped: st.VirtualSkipped, LoopsSkipped: st.LoopsSkipped, BrokenLinks: st.BrokenLinks,
		HardlinkDups: st.HardlinkDups, Unscanned: st.Unscanned, Excludes: st.Excludes,
		OneFileSystem: st.OneFileSystem, Follow: st.Follow, Workers: st.Workers, Host: host,
		Generator: brand.Name + " " + brand.Version}
	mb, err := json.Marshal(m)
	if err != nil {
		return err
	}
	w.uvarint(uint64(len(mb)))
	w.bytes(mb)

	w.uvarint(uint64(t.ExtCount()))
	for i := range t.ExtCount() {
		w.str(t.ExtByIndex(i))
	}
	w.uvarint(uint64(t.Len()))
	for i := range t.Len() {
		id := inventory.NodeID(i)
		n := t.Node(id)
		if i == 0 {
			w.uvarint(0)
		} else {
			w.uvarint(uint64(id - n.Parent))
		}
		w.bytes([]byte{byte(n.Kind), byte(n.Cat)})
		w.uvarint(uint64(n.Flags))
		w.str(n.Name)
		w.varint(n.Size)
		w.varint(n.Alloc)
		w.varint(n.MTime)
		w.uvarint(uint64(n.Mode))
		w.uvarint(uint64(n.UID))
		w.uvarint(uint64(n.GID))
		w.uvarint(uint64(n.Nlink))
		w.uvarint(uint64(n.Ext))
	}
	w.uvarint(uint64(len(st.Errors)))
	for _, e := range st.Errors {
		w.uvarint(uint64(e.Node))
		w.bytes([]byte{byte(e.Kind)})
		w.str(e.Name)
		w.str(e.Msg)
	}
	if w.err != nil {
		return w.err
	}
	if err := gz.Close(); err != nil {
		return err
	}
	bw.Write(h.Sum(nil))
	return bw.Flush()
}

// SaveFile writes a snapshot atomically (temp file + rename).
func SaveFile(path string, t *inventory.Tree) error {
	tmp, err := os.CreateTemp(dirOf(path), ".mapsize-*.tmp")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name())
	if err := Save(tmp, t); err != nil {
		tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	return os.Rename(tmp.Name(), path)
}

func dirOf(p string) string {
	i := strings.LastIndexAny(p, `/\`)
	if i < 0 {
		return "."
	}
	if i == 0 {
		return p[:1]
	}
	return p[:i]
}

type reader struct {
	r   *bufio.Reader
	err error
}

func (r *reader) fail(format string, a ...any) {
	if r.err == nil {
		r.err = fmt.Errorf("snapshot: "+format, a...)
	}
}

func (r *reader) byte1() byte {
	if r.err != nil {
		return 0
	}
	b, err := r.r.ReadByte()
	if err != nil {
		r.fail("truncated: %v", err)
	}
	return b
}

func (r *reader) uvarint(max uint64, what string) uint64 {
	if r.err != nil {
		return 0
	}
	v, err := readUvarint(r.r)
	if err != nil {
		r.fail("bad %s: %v", what, err)
		return 0
	}
	if v > max {
		r.fail("%s %d exceeds limit %d", what, v, max)
		return 0
	}
	return v
}

func (r *reader) varint(what string) int64 {
	if r.err != nil {
		return 0
	}
	ux, err := readUvarint(r.r)
	if err != nil {
		r.fail("bad %s: %v", what, err)
	}
	v := int64(ux >> 1) // zig-zag, as encoding/binary.PutVarint
	if ux&1 != 0 {
		v = ^v
	}
	return v
}

var errOverflow = errors.New("varint overflows 64 bits")

// readUvarint is encoding/binary.ReadUvarint specialised to *bufio.Reader
// (no interface dispatch per byte; this is the loader's hot loop).
func readUvarint(br *bufio.Reader) (uint64, error) {
	var x uint64
	var s uint
	for i := 0; i < binary.MaxVarintLen64; i++ {
		b, err := br.ReadByte()
		if err != nil {
			if i > 0 && err == io.EOF {
				err = io.ErrUnexpectedEOF
			}
			return x, err
		}
		if b < 0x80 {
			if i == binary.MaxVarintLen64-1 && b > 1 {
				return x, errOverflow
			}
			return x | uint64(b)<<s, nil
		}
		x |= uint64(b&0x7f) << s
		s += 7
	}
	return x, errOverflow
}

func (r *reader) bytesN(n uint64) []byte {
	if r.err != nil {
		return nil
	}
	b := make([]byte, n)
	if _, err := io.ReadFull(r.r, b); err != nil {
		r.fail("truncated: %v", err)
		return nil
	}
	return b
}

func (r *reader) str(max uint64, what string) string {
	return string(r.bytesN(r.uvarint(max, what+" length")))
}

// limitedReader fails (rather than silently truncating) past n bytes, so a
// decompression bomb is reported as such.
type limitedReader struct {
	r io.Reader
	n int64
}

func (l *limitedReader) Read(p []byte) (int, error) {
	if l.n <= 0 {
		return 0, errors.New("snapshot: decompressed size exceeds limit (possible decompression bomb)")
	}
	if int64(len(p)) > l.n {
		p = p[:l.n]
	}
	n, err := l.r.Read(p)
	l.n -= int64(n)
	return n, err
}

// bodyLimit allows decompressed bodies up to 64× the compressed file size
// (real snapshots compress ~6×), with a 64 MiB floor for tiny files.
func bodyLimit(fileSize int64) int64 { return max(64<<20, fileSize*64) }

// nodeLimit caps the node count for a file of fileSize bytes. Real
// snapshots spend ~15 compressed bytes per node; a crafted one describes a
// node in a fraction of a byte, so without this a 1 MB file can demand
// gigabytes of memory while staying within bodyLimit.
func nodeLimit(fileSize int64) uint64 { return uint64(max(1<<20, fileSize)) }

// Load reads and validates a snapshot, allowing up to DefaultMaxBody bytes
// of decompressed data.
func Load(in io.Reader) (*inventory.Tree, error) { return LoadLimit(in, DefaultMaxBody) }

// LoadLimit is Load with an explicit decompressed-size limit.
func LoadLimit(in io.Reader, maxBody int64) (*inventory.Tree, error) {
	return load(in, maxBody, MaxNodes)
}

func load(in io.Reader, maxBody int64, maxNodes uint64) (*inventory.Tree, error) {
	br := bufio.NewReaderSize(in, 1<<20)
	hdr := make([]byte, len(brand.SnapshotMagic)+4)
	if _, err := io.ReadFull(br, hdr); err != nil {
		return nil, errors.New("snapshot: file too short")
	}
	if string(hdr[:8]) != brand.SnapshotMagic {
		return nil, errors.New("snapshot: not a " + brand.Name + " snapshot")
	}
	if v := binary.LittleEndian.Uint16(hdr[8:]); v != Version {
		return nil, fmt.Errorf("snapshot: unsupported version %d (this build reads %d)", v, Version)
	}
	if binary.LittleEndian.Uint16(hdr[10:])&flagGzipped == 0 {
		return nil, errors.New("snapshot: uncompressed bodies are not supported")
	}
	gz, err := gzip.NewReader(br)
	if err != nil {
		return nil, fmt.Errorf("snapshot: %w", err)
	}
	gz.Multistream(false)
	// Decompression and hashing run on their own goroutine, overlapping
	// with parsing. The pipe's close orders the hash and br accesses: the
	// parser only sees EOF after the copier has finished with both.
	h := sha256.New()
	pr, pw := io.Pipe()
	defer pr.Close() // unblocks the copier if we bail out early
	go func() {
		_, err := io.CopyBuffer(pw, io.TeeReader(&limitedReader{gz, maxBody}, h), make([]byte, 256<<10))
		pw.CloseWithError(err)
	}()
	r := &reader{r: bufio.NewReaderSize(pr, 1<<16)}

	var m meta
	if mb := r.bytesN(r.uvarint(MaxMetaLen, "metadata length")); r.err == nil {
		if err := json.Unmarshal(mb, &m); err != nil {
			return nil, fmt.Errorf("snapshot: metadata: %w", err)
		}
	}
	if r.err != nil {
		return nil, r.err
	}
	t := inventory.New(m.Root, inventory.KindDir)
	nExt := r.uvarint(maxExts, "extension count")
	exts := make([]string, 0, min(nExt, 1024))
	for range nExt {
		exts = append(exts, r.str(MaxExtLen, "extension"))
	}
	if r.err != nil {
		return nil, r.err
	}
	extIndex := t.ExtIndexer()
	nNodes := r.uvarint(maxNodes, "node count")
	depth := []uint16{0} // per node, for the depth limit; parents precede children
	if r.err != nil {
		return nil, r.err
	}
	if nNodes == 0 {
		return nil, errors.New("snapshot: no root node")
	}
	for i := uint64(0); i < nNodes && r.err == nil; i++ {
		pd := r.uvarint(MaxNodes, "parent")
		kind := inventory.Kind(r.byte1())
		cat := inventory.Category(r.byte1())
		flags := inventory.Flags(r.uvarint(1<<16-1, "flags"))
		name := r.str(MaxNameLen, "name")
		size, alloc, mtime := r.varint("size"), r.varint("alloc"), r.varint("mtime")
		mode := r.uvarint(1<<32-1, "mode")
		uid := r.uvarint(1<<32-1, "uid")
		gid := r.uvarint(1<<32-1, "gid")
		nlink := r.uvarint(1<<32-1, "nlink")
		ext := r.uvarint(nExt, "ext")
		if r.err != nil {
			break
		}
		if kind > inventory.KindOther || cat >= inventory.NumCategories {
			r.fail("node %d: invalid kind/category", i)
			break
		}
		if size < 0 || alloc < 0 || size > MaxSize || alloc > MaxSize {
			r.fail("node %d: size out of range", i)
			break
		}
		var id inventory.NodeID
		if i == 0 {
			if pd != 0 || kind != inventory.KindDir && nNodes > 1 {
				r.fail("invalid root")
				break
			}
			id = 0
			rn := t.Node(0)
			rn.Kind = kind
		} else {
			if pd == 0 || pd > i {
				r.fail("node %d: invalid parent reference", i)
				break
			}
			parent := inventory.NodeID(i - pd)
			if t.Node(parent).Kind != inventory.KindDir {
				r.fail("node %d: parent is not a directory", i)
				break
			}
			if !validName(name) {
				r.fail("node %d: invalid name %q", i, name)
				break
			}
			if d := depth[parent] + 1; d > MaxDepth {
				r.fail("node %d: deeper than %d levels", i, MaxDepth)
				break
			}
			id = t.Add(parent, name, kind)
			depth = append(depth, depth[parent]+1)
		}
		n := t.Node(id)
		n.Cat, n.Flags = cat, flags
		n.Size, n.Alloc, n.MTime = size, alloc, mtime
		n.Mode, n.UID, n.GID, n.Nlink = uint32(mode), uint32(uid), uint32(gid), uint32(nlink)
		if kind != inventory.KindDir && ext < uint64(len(exts)) {
			n.Ext = extIndex(exts[ext])
		}
	}
	nErr := r.uvarint(inventory.MaxErrorRecords, "error count")
	for range nErr {
		node := r.uvarint(nNodes-1, "error node")
		kind := inventory.ErrKind(r.byte1())
		name := r.str(MaxNameLen, "error name")
		msg := r.str(MaxMsgLen, "error message")
		if r.err != nil {
			break
		}
		if kind >= inventory.NumErrKinds {
			kind = inventory.ErrOther
		}
		t.Stats.Errors = append(t.Stats.Errors, inventory.ErrorRecord{Node: inventory.NodeID(node), Kind: kind, Name: name, Msg: msg})
	}
	if r.err != nil {
		return nil, r.err
	}
	// The body must end here, then the checksum follows the gzip stream.
	if _, err := r.r.ReadByte(); err != io.EOF {
		return nil, errors.New("snapshot: trailing data in body")
	}
	sum := make([]byte, sha256.Size)
	if _, err := io.ReadFull(br, sum); err != nil {
		return nil, errors.New("snapshot: missing checksum")
	}
	if !bytes.Equal(sum, h.Sum(nil)) {
		return nil, errors.New("snapshot: checksum mismatch (file corrupt)")
	}

	t.Recompute()
	st := &t.Stats
	st.Start, st.End, st.Complete, st.Cancelled = m.Start, m.End, true, m.Cancelled
	st.Files, st.Dirs, st.Symlinks, st.Others = m.Files, m.Dirs, m.Symlinks, m.Others
	copy(st.ErrCounts[:], m.ErrCounts)
	st.Excluded, st.SkippedMounts, st.VirtualSkipped = m.Excluded, m.SkippedMounts, m.VirtualSkipped
	st.LoopsSkipped, st.BrokenLinks, st.HardlinkDups, st.Unscanned = m.LoopsSkipped, m.BrokenLinks, m.HardlinkDups, m.Unscanned
	st.Excludes, st.OneFileSystem, st.Follow, st.Workers = m.Excludes, m.OneFileSystem, m.Follow, m.Workers
	st.FromSnapshot = fmt.Sprintf("%s (%s, %s)", m.Created.Local().Format("2006-01-02 15:04"), m.Host, m.Generator)
	return t, nil
}

func validName(s string) bool {
	return s != "" && s != "." && s != ".." && !strings.ContainsAny(s, "/\x00")
}

// LoadFile opens and loads a snapshot file.
func LoadFile(path string) (*inventory.Tree, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	limit, nodes := int64(DefaultMaxBody), uint64(MaxNodes)
	if fi, err := f.Stat(); err == nil {
		limit, nodes = bodyLimit(fi.Size()), nodeLimit(fi.Size())
	}
	return load(f, limit, nodes)
}
