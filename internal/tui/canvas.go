package tui

import (
	"strconv"
	"strings"

	"github.com/andydixon/mapsize/internal/textutil"
	"github.com/andydixon/mapsize/internal/treemap"
)

// Color is 0 for the terminal default, otherwise 0x01RRGGBB.
type Color uint32

// RGB builds a colour.
func RGB(r, g, b uint8) Color { return Color(1<<24 | uint32(r)<<16 | uint32(g)<<8 | uint32(b)) }

// Hex parses "#rrggbb".
func Hex(s string) (Color, bool) {
	s = strings.TrimPrefix(s, "#")
	if len(s) != 6 {
		return 0, false
	}
	v, err := strconv.ParseUint(s, 16, 32)
	if err != nil {
		return 0, false
	}
	return Color(1<<24 | uint32(v)), true
}

func (c Color) rgb() (r, g, b float64) {
	return float64(c>>16&0xff) / 255, float64(c>>8&0xff) / 255, float64(c&0xff) / 255
}

// Mix blends c towards o by t (0..1). Default colours are returned as-is.
func (c Color) Mix(o Color, t float64) Color {
	if c == 0 || o == 0 {
		return c
	}
	r1, g1, b1 := c.rgb()
	r2, g2, b2 := o.rgb()
	f := func(a, b float64) uint8 { return uint8((a + (b-a)*t) * 255) }
	return RGB(f(r1, r2), f(g1, g2), f(b1, b2))
}

// Luma returns perceived brightness 0..1.
func (c Color) Luma() float64 {
	r, g, b := c.rgb()
	return 0.299*r + 0.587*g + 0.114*b
}

// Attr is a text attribute bit set.
type Attr uint8

const (
	Bold Attr = 1 << iota
	Faint
	Italic
	Underline
	Reverse
)

// Style is a cell style.
type Style struct {
	FG, BG Color
	Attr   Attr
}

type cell struct {
	s  string // "" marks the continuation of a wide cluster
	st Style
}

// Canvas is an in-memory cell grid that every view paints into. It is
// serialised once per frame.
type Canvas struct {
	W, H int
	c    []cell
}

// NewCanvas creates a canvas filled with spaces in style st.
func NewCanvas(w, h int, st Style) *Canvas {
	cv := &Canvas{W: max(w, 0), H: max(h, 0)}
	cv.c = make([]cell, cv.W*cv.H)
	for i := range cv.c {
		cv.c[i] = cell{" ", st}
	}
	return cv
}

func (cv *Canvas) in(x, y int) bool { return x >= 0 && y >= 0 && x < cv.W && y < cv.H }

// put writes one cluster of width w at (x, y), repairing any wide cluster it
// overlaps so the grid never holds half a character.
func (cv *Canvas) put(x, y int, s string, w int, st Style) {
	if !cv.in(x, y) || x+w > cv.W {
		return
	}
	row := y * cv.W
	for i := x; i < x+w; i++ {
		c := &cv.c[row+i]
		if c.s == "" && i > 0 && i == x { // overwriting the tail of a wide cluster
			cv.c[row+i-1].s = " "
		}
		if c.s != "" && textutil.Width(c.s) > 1 && i+1 < cv.W { // overwriting its head
			if n := &cv.c[row+i+1]; n.s == "" {
				n.s = " "
			}
		}
	}
	cv.c[row+x] = cell{s, st}
	for i := 1; i < w; i++ {
		cv.c[row+x+i] = cell{"", st}
	}
}

// Text draws s at (x, y), clipped to maxW cells and the canvas. Control
// characters are replaced as a second line of defence (callers sanitise
// untrusted strings first). It returns the number of cells used.
func (cv *Canvas) Text(x, y int, s string, maxW int, st Style) int {
	if y < 0 || y >= cv.H {
		return 0
	}
	limit := min(x+maxW, cv.W)
	cx := x
	textutil.Clusters(s, func(c string, w int) bool {
		if r := c[0]; r < 0x20 || r == 0x7f || (len(c) > 1 && c[0] == 0xc2 && c[1] < 0xa0) {
			c, w = "?", 1
		}
		if cx+w > limit {
			// A wide cluster that does not fit leaves a blank cell.
			for ; cx < limit; cx++ {
				if cx >= 0 {
					cv.put(cx, y, " ", 1, st)
				}
			}
			return false
		}
		if cx >= 0 {
			cv.put(cx, y, c, w, st)
		}
		cx += w
		return true
	})
	return cx - x
}

// TextCentered draws s centred within [x, x+w).
func (cv *Canvas) TextCentered(x, y, w int, s string, st Style) {
	s = textutil.Truncate(s, w)
	cv.Text(x+(w-textutil.Width(s))/2, y, s, w, st)
}

// TextRight draws s right-aligned ending at x+w.
func (cv *Canvas) TextRight(x, y, w int, s string, st Style) {
	s = textutil.Truncate(s, w)
	cv.Text(x+w-textutil.Width(s), y, s, w, st)
}

// Fill fills r with the cluster ch.
func (cv *Canvas) Fill(r treemap.Rect, ch string, st Style) {
	for y := max(r.Y, 0); y < min(r.Y+r.H, cv.H); y++ {
		for x := max(r.X, 0); x < min(r.X+r.W, cv.W); x++ {
			cv.put(x, y, ch, 1, st)
		}
	}
}

// Restyle applies fn to the style of every cell in r.
func (cv *Canvas) Restyle(r treemap.Rect, fn func(Style) Style) {
	for y := max(r.Y, 0); y < min(r.Y+r.H, cv.H); y++ {
		for x := max(r.X, 0); x < min(r.X+r.W, cv.W); x++ {
			c := &cv.c[y*cv.W+x]
			c.st = fn(c.st)
		}
	}
}

// Box glyph sets.
type BoxChars struct{ H, V, TL, TR, BL, BR string }

var (
	BoxLight    = BoxChars{"─", "│", "┌", "┐", "└", "┘"}
	BoxRound    = BoxChars{"─", "│", "╭", "╮", "╰", "╯"}
	BoxDouble   = BoxChars{"═", "║", "╔", "╗", "╚", "╝"}
	BoxHeavy    = BoxChars{"━", "┃", "┏", "┓", "┗", "┛"}
	BoxASCII    = BoxChars{"-", "|", "+", "+", "+", "+"}
	BoxASCIISel = BoxChars{"=", "#", "#", "#", "#", "#"}
)

// Box draws a border around r (which must be at least 2×2).
func (cv *Canvas) Box(r treemap.Rect, b BoxChars, st Style) {
	if r.W < 2 || r.H < 2 {
		return
	}
	x1, y1 := r.X+r.W-1, r.Y+r.H-1
	for x := r.X + 1; x < x1; x++ {
		cv.put(x, r.Y, b.H, 1, st)
		cv.put(x, y1, b.H, 1, st)
	}
	for y := r.Y + 1; y < y1; y++ {
		cv.put(r.X, y, b.V, 1, st)
		cv.put(x1, y, b.V, 1, st)
	}
	cv.put(r.X, r.Y, b.TL, 1, st)
	cv.put(x1, r.Y, b.TR, 1, st)
	cv.put(r.X, y1, b.BL, 1, st)
	cv.put(x1, y1, b.BR, 1, st)
}

// HLine draws a horizontal rule.
func (cv *Canvas) HLine(x, y, w int, ch string, st Style) {
	for i := x; i < x+w; i++ {
		cv.put(i, y, ch, 1, st)
	}
}

// Cell returns the text and style at (x, y) (for tests).
func (cv *Canvas) Cell(x, y int) (string, Style) {
	if !cv.in(x, y) {
		return "", Style{}
	}
	c := cv.c[y*cv.W+x]
	return c.s, c.st
}

// Line returns the plain text of row y (for tests).
func (cv *Canvas) Line(y int) string {
	var b strings.Builder
	for x := 0; x < cv.W; x++ {
		b.WriteString(cv.c[y*cv.W+x].s)
	}
	return b.String()
}

// colorDepth selects how colours are serialised.
type colorDepth uint8

const (
	depthTrue colorDepth = iota
	depth256
	depth16
)

func sgr(b *strings.Builder, st Style, depth colorDepth) {
	b.WriteString("\x1b[0")
	if st.Attr&Bold != 0 {
		b.WriteString(";1")
	}
	if st.Attr&Faint != 0 {
		b.WriteString(";2")
	}
	if st.Attr&Italic != 0 {
		b.WriteString(";3")
	}
	if st.Attr&Underline != 0 {
		b.WriteString(";4")
	}
	if st.Attr&Reverse != 0 {
		b.WriteString(";7")
	}
	col := func(c Color, base string) {
		if c == 0 {
			return
		}
		switch depth {
		case depth256:
			b.WriteString(";" + base + ";5;")
			b.WriteString(strconv.Itoa(int(quant256(c))))
			return
		case depth16:
			n := int(quant16(c))
			code := 30 + n
			if n >= 8 {
				code = 90 + n - 8
			}
			if base == "48" {
				code += 10
			}
			b.WriteString(";" + strconv.Itoa(code))
			return
		}
		b.WriteString(";" + base + ";2;")
		b.WriteString(strconv.Itoa(int(c >> 16 & 0xff)))
		b.WriteByte(';')
		b.WriteString(strconv.Itoa(int(c >> 8 & 0xff)))
		b.WriteByte(';')
		b.WriteString(strconv.Itoa(int(c & 0xff)))
	}
	col(st.FG, "38")
	col(st.BG, "48")
	b.WriteByte('m')
}

// String serialises the canvas as lines of text with SGR styling and
// 24-bit colours.
func (cv *Canvas) String() string { return cv.Render(depthTrue) }

// Render serialises the canvas. For 256 and 16 colours it quantizes with
// its own perceptual mapping (see color256.go); Bubble Tea strips colour
// entirely for colourless terminals.
func (cv *Canvas) Render(depth colorDepth) string {
	var b strings.Builder
	b.Grow(cv.W * cv.H * 4)
	for y := 0; y < cv.H; y++ {
		if y > 0 {
			b.WriteByte('\n')
		}
		var cur Style
		first := true
		for x := 0; x < cv.W; x++ {
			c := cv.c[y*cv.W+x]
			if c.s == "" {
				continue
			}
			if first || c.st != cur {
				sgr(&b, c.st, depth)
				cur, first = c.st, false
			}
			b.WriteString(c.s)
		}
		b.WriteString("\x1b[0m")
	}
	return b.String()
}
