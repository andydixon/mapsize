package inventory

import (
	"errors"
	"io/fs"
	"syscall"
	"time"
)

// ErrKind classifies scan errors for reporting.
type ErrKind uint8

const (
	ErrPermission ErrKind = iota
	ErrVanished
	ErrIO
	ErrLoop
	ErrOther
	NumErrKinds
)

func (k ErrKind) String() string {
	return [...]string{"Permission denied", "Vanished during scan", "I/O error", "Symlink loop", "Other error"}[k]
}

// Classify maps an OS error to an ErrKind.
func Classify(err error) ErrKind {
	switch {
	case errors.Is(err, fs.ErrPermission):
		return ErrPermission
	case errors.Is(err, fs.ErrNotExist):
		return ErrVanished
	case errors.Is(err, syscall.ELOOP):
		return ErrLoop
	case errors.Is(err, syscall.EIO):
		return ErrIO
	}
	return ErrOther
}

// ErrorRecord is one recorded scan error.
type ErrorRecord struct {
	Node NodeID // node the error is attached to (the directory for listing failures)
	Name string // entry name when no node was created (e.g. vanished)
	Kind ErrKind
	Msg  string
}

// MaxErrorRecords caps the detailed error list; counts are always exact.
const MaxErrorRecords = 100_000

// Stats describes a scan as a whole.
type Stats struct {
	Root       string
	Start, End time.Time
	Complete   bool // scan finished (possibly with errors)
	Cancelled  bool

	Files, Dirs, Symlinks, Others int64

	ErrCounts      [NumErrKinds]int64
	Errors         []ErrorRecord
	Excluded       int64
	SkippedMounts  int64
	VirtualSkipped int64
	LoopsSkipped   int64
	BrokenLinks    int64
	HardlinkDups   int64
	Unscanned      int64 // directories never read because the scan was cancelled

	Excludes      []string
	OneFileSystem bool
	Follow        string
	Workers       int
	FromSnapshot  string
}

// TotalErrors sums all error kinds.
func (s *Stats) TotalErrors() int64 {
	var t int64
	for _, c := range s.ErrCounts {
		t += c
	}
	return t
}

// AddError records an error (the detailed list is capped).
func (s *Stats) AddError(r ErrorRecord) {
	s.ErrCounts[r.Kind]++
	if len(s.Errors) < MaxErrorRecords {
		s.Errors = append(s.Errors, r)
	}
}

// Elapsed is the scan duration so far (or total once finished).
func (s *Stats) Elapsed() time.Duration {
	if !s.End.IsZero() {
		return s.End.Sub(s.Start)
	}
	return time.Since(s.Start)
}

// Incomplete reports whether totals may under-count real usage.
func (s *Stats) Incomplete() bool {
	return !s.Complete || s.Cancelled || s.TotalErrors() > 0 || s.Unscanned > 0
}
