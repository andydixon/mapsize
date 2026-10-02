// Package export writes inventories as JSON, CSV or plain-text reports.
// Callers must hold the tree's read lock (or own the tree exclusively).
package export

import (
	"bufio"
	"encoding/csv"
	"encoding/json"
	"fmt"
	"io"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/andydixon/mapsize/internal/brand"
	"github.com/andydixon/mapsize/internal/inventory"
	"github.com/andydixon/mapsize/internal/textutil"
)

type jsonNode struct {
	Name      string      `json:"name"`
	Type      string      `json:"type"`
	Size      int64       `json:"size"`
	Allocated int64       `json:"allocated"`
	Files     uint32      `json:"files,omitempty"`
	Dirs      uint32      `json:"dirs,omitempty"`
	Errors    uint32      `json:"errors,omitempty"`
	Modified  string      `json:"modified,omitempty"`
	Flags     []string    `json:"flags,omitempty"`
	Children  []*jsonNode `json:"children,omitempty"`
}

var flagNames = []struct {
	f inventory.Flags
	n string
}{
	{inventory.FlagHardlinkDup, "hardlink-duplicate"}, {inventory.FlagMountPoint, "mount-point"},
	{inventory.FlagVirtualFS, "virtual-fs-skipped"}, {inventory.FlagSkippedFS, "other-fs-skipped"},
	{inventory.FlagSparse, "sparse"}, {inventory.FlagError, "error"}, {inventory.FlagIncomplete, "incomplete"},
	{inventory.FlagLoop, "loop-skipped"}, {inventory.FlagFollowed, "followed-symlink"},
	{inventory.FlagAllocUnknown, "allocated-unknown"}, {inventory.FlagBrokenLink, "broken-symlink"},
	{inventory.FlagHardlinked, "hardlinked"},
}

// FlagList returns human-readable names for a node's flags.
func FlagList(f inventory.Flags) []string {
	var out []string
	for _, fn := range flagNames {
		if f&fn.f != 0 {
			out = append(out, fn.n)
		}
	}
	return out
}

// JSON writes the tree beneath root as nested JSON, down to maxDepth levels
// (maxDepth < 0 means unlimited).
func JSON(w io.Writer, t *inventory.Tree, root inventory.NodeID, maxDepth int) error {
	bw := bufio.NewWriter(w)
	st := &t.Stats
	head := map[string]any{
		"generator":           brand.Name + " " + brand.Version,
		"root":                t.Path(root),
		"complete":            !st.Incomplete(),
		"cancelled":           st.Cancelled,
		"started":             st.Start.Format(time.RFC3339),
		"elapsed_s":           st.Elapsed().Seconds(),
		"errors":              errorCounts(st),
		"excluded":            st.Excluded,
		"excludes":            st.Excludes,
		"hardlink_duplicates": st.HardlinkDups,
	}
	hb, err := json.Marshal(head)
	if err != nil {
		return err
	}
	bw.Write(hb[:len(hb)-1])
	bw.WriteString(`,"tree":`)
	if err := writeNode(bw, t, root, maxDepth); err != nil {
		return err
	}
	bw.WriteString("}\n")
	return bw.Flush()
}

func errorCounts(st *inventory.Stats) map[string]int64 {
	m := map[string]int64{}
	for k, c := range st.ErrCounts {
		if c > 0 {
			m[inventory.ErrKind(k).String()] = c
		}
	}
	return m
}

// writeNode streams one node; children are written recursively so memory
// stays proportional to depth, not tree size.
func writeNode(bw *bufio.Writer, t *inventory.Tree, id inventory.NodeID, depth int) error {
	n := t.Node(id)
	jn := jsonNode{Name: n.Name, Type: kindName(n.Kind), Size: n.TotSize, Allocated: n.TotAlloc,
		Files: n.Files, Dirs: n.Dirs, Errors: n.Errors, Flags: FlagList(n.Flags)}
	if n.MTime != 0 {
		jn.Modified = time.Unix(0, n.MTime).UTC().Format(time.RFC3339)
	}
	b, err := json.Marshal(jn)
	if err != nil {
		return err
	}
	if n.FirstChild == inventory.NoNode || depth == 0 {
		_, err = bw.Write(b)
		return err
	}
	bw.Write(b[:len(b)-1])
	bw.WriteString(`,"children":[`)
	first := true
	for _, c := range t.SortedChildren(id, inventory.SizeAllocated) {
		if !first {
			bw.WriteByte(',')
		}
		first = false
		if err := writeNode(bw, t, c, depth-1); err != nil {
			return err
		}
	}
	_, err = bw.WriteString("]}")
	return err
}

func kindName(k inventory.Kind) string {
	return [...]string{"file", "dir", "symlink", "other"}[k&3]
}

// CSV writes one row per node beneath root (depth-limited). Paths are
// sanitised so the output is safe to print to a terminal.
func CSV(w io.Writer, t *inventory.Tree, root inventory.NodeID, maxDepth int) error {
	cw := csv.NewWriter(w)
	cw.Write([]string{"path", "type", "size", "allocated", "files", "dirs", "errors", "modified", "flags"})
	var rec func(id inventory.NodeID, path string, depth int) error
	rec = func(id inventory.NodeID, path string, depth int) error {
		n := t.Node(id)
		mod := ""
		if n.MTime != 0 {
			mod = time.Unix(0, n.MTime).UTC().Format(time.RFC3339)
		}
		flags := ""
		for i, f := range FlagList(n.Flags) {
			if i > 0 {
				flags += "|"
			}
			flags += f
		}
		if err := cw.Write([]string{textutil.Sanitize(path), kindName(n.Kind),
			strconv.FormatInt(n.TotSize, 10), strconv.FormatInt(n.TotAlloc, 10),
			strconv.FormatUint(uint64(n.Files), 10), strconv.FormatUint(uint64(n.Dirs), 10),
			strconv.FormatUint(uint64(n.Errors), 10), mod, flags}); err != nil {
			return err
		}
		if depth == 0 {
			return nil
		}
		for _, c := range t.SortedChildren(id, inventory.SizeAllocated) {
			if err := rec(c, filepath.Join(path, t.Node(c).Name), depth-1); err != nil {
				return err
			}
		}
		return nil
	}
	if err := rec(root, t.Path(root), maxDepth); err != nil {
		return err
	}
	cw.Flush()
	return cw.Error()
}

// Table writes "size  %  files  path" rows for the given nodes.
func Table(w io.Writer, t *inventory.Tree, ids []inventory.NodeID, m inventory.SizeMode) {
	total := t.Node(t.Root()).Total(m)
	fmt.Fprintf(w, "%12s %6s %12s  %s\n", "SIZE", "%", "FILES", "PATH")
	for _, id := range ids {
		n := t.Node(id)
		files := ""
		if n.IsDir() {
			files = textutil.Count(int64(n.Files))
		}
		fmt.Fprintf(w, "%12s %6s %12s  %s\n", textutil.Size(n.Total(m)), textutil.Percent(n.Total(m), total),
			files, textutil.Sanitize(t.Path(id)))
	}
}

// Summary writes the scan summary, making incompleteness explicit.
func Summary(w io.Writer, t *inventory.Tree) {
	st := &t.Stats
	r := t.Node(t.Root())
	fmt.Fprintf(w, "Root            %s\n", textutil.Sanitize(st.Root))
	fmt.Fprintf(w, "Files           %s\n", textutil.Count(st.Files+st.Symlinks+st.Others))
	fmt.Fprintf(w, "Directories     %s\n", textutil.Count(st.Dirs))
	fmt.Fprintf(w, "Logical size    %s (%d bytes)\n", textutil.Size(r.TotSize), r.TotSize)
	fmt.Fprintf(w, "Allocated size  %s (%d bytes)\n", textutil.Size(r.TotAlloc), r.TotAlloc)
	fmt.Fprintf(w, "Elapsed         %s\n", st.Elapsed().Round(time.Millisecond))
	for k, c := range st.ErrCounts {
		if c > 0 {
			fmt.Fprintf(w, "%-15s %s\n", inventory.ErrKind(k).String(), textutil.Count(c))
		}
	}
	if st.Excluded > 0 {
		fmt.Fprintf(w, "Excluded        %s entries (%s)\n", textutil.Count(st.Excluded), textutil.Sanitize(strings.Join(st.Excludes, ", ")))
	}
	if st.SkippedMounts > 0 {
		fmt.Fprintf(w, "Other FS skipped %s\n", textutil.Count(st.SkippedMounts))
	}
	if st.VirtualSkipped > 0 {
		fmt.Fprintf(w, "Virtual FS skipped %s\n", textutil.Count(st.VirtualSkipped))
	}
	if st.HardlinkDups > 0 {
		fmt.Fprintf(w, "Hard-link dups  %s (counted once)\n", textutil.Count(st.HardlinkDups))
	}
	if st.Incomplete() {
		fmt.Fprintln(w, "WARNING: totals are incomplete — some data could not be inspected.")
	}
}
