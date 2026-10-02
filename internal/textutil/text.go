// Package textutil holds display helpers: sanitising untrusted strings for
// the terminal, width-aware truncation, and human-readable units.
package textutil

import (
	"fmt"
	"strings"
	"unicode/utf8"

	"github.com/charmbracelet/x/ansi"
	"github.com/rivo/uniseg"
)

// Sanitize makes an untrusted string (a filename, snapshot data) safe to
// write to a terminal. Control characters (C0, DEL, C1) and bidi overrides are
// replaced by visible escapes; invalid UTF-8 becomes U+FFFD. The original
// string should be kept for filesystem operations.
func Sanitize(s string) string {
	clean := true
	for i := 0; i < len(s); i++ {
		if c := s[i]; c < 0x20 || c >= 0x7f {
			clean = false
			break
		}
	}
	if clean {
		return s
	}
	var b strings.Builder
	b.Grow(len(s) + 8)
	for i := 0; i < len(s); {
		r, size := utf8.DecodeRuneInString(s[i:])
		if r == utf8.RuneError && size <= 1 {
			b.WriteRune(utf8.RuneError)
			i++
			continue
		}
		i += size
		switch {
		case r < 0x20 || r == 0x7f || (r >= 0x80 && r <= 0x9f):
			fmt.Fprintf(&b, `\x%02x`, r)
		case isBidiControl(r):
			fmt.Fprintf(&b, `\u%04x`, r)
		default:
			b.WriteRune(r)
		}
	}
	return b.String()
}

func isBidiControl(r rune) bool {
	return (r >= 0x202a && r <= 0x202e) || (r >= 0x2066 && r <= 0x2069) ||
		r == 0x200e || r == 0x200f || r == 0x061c
}

// Clusters calls fn for each grapheme cluster of s with its display width.
// Clusters whose width terminals disagree on (per-rune wcwidth versus
// grapheme width, e.g. emoji ZWJ sequences) are replaced by U+FFFD with
// width 1, and zero-width clusters are dropped, so that measuring and
// drawing always agree with the terminal. fn returns false to stop.
func Clusters(s string, fn func(c string, w int) bool) {
	state := -1
	for s != "" {
		var c string
		var w int
		c, s, w, state = uniseg.FirstGraphemeClusterInString(s, state)
		if w == 0 {
			continue
		}
		if len(c) > 1 && ansi.WcWidth.StringWidth(c) != w {
			c, w = "\uFFFD", 1
		}
		if !fn(c, w) {
			return
		}
	}
}

// Width returns the display width of s in terminal cells.
func Width(s string) int {
	n := 0
	for i := 0; i < len(s); i++ {
		if s[i] < 0x20 || s[i] >= 0x7f {
			Clusters(s, func(_ string, w int) bool { n += w; return true })
			return n
		}
	}
	return len(s)
}

// Truncate clips s to at most w cells, appending "…" if anything was cut.
func Truncate(s string, w int) string {
	if w <= 0 {
		return ""
	}
	if Width(s) <= w {
		return s
	}
	return clip(s, w-1) + "…"
}

// TruncateLeft clips from the left, prefixing "…" (used for paths and
// breadcrumbs where the tail is the informative part).
func TruncateLeft(s string, w int) string {
	if w <= 0 {
		return ""
	}
	total := Width(s)
	if total <= w {
		return s
	}
	skip := total - (w - 1)
	var b strings.Builder
	Clusters(s, func(c string, cw int) bool {
		if skip > 0 {
			skip -= cw
			if skip < 0 {
				// A wide cluster overshot by one cell; pad to keep width exact.
				b.WriteString(strings.Repeat(" ", -skip))
			}
			return true
		}
		b.WriteString(c)
		return true
	})
	return "…" + b.String()
}

// clip returns the longest prefix of s that fits in w cells.
func clip(s string, w int) string {
	var b strings.Builder
	used := 0
	Clusters(s, func(c string, cw int) bool {
		if used+cw > w {
			return false
		}
		used += cw
		b.WriteString(c)
		return true
	})
	return b.String()
}

// PadRight pads s with spaces to exactly w cells (truncating if longer).
func PadRight(s string, w int) string {
	s = Truncate(s, w)
	if d := w - Width(s); d > 0 {
		s += strings.Repeat(" ", d)
	}
	return s
}

// PadLeft right-aligns s in w cells.
func PadLeft(s string, w int) string {
	s = Truncate(s, w)
	if d := w - Width(s); d > 0 {
		s = strings.Repeat(" ", d) + s
	}
	return s
}
