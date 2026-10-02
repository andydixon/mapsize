// Package treemap is pure geometry: squarified layout on a cell grid,
// level-of-detail selection and spatial neighbour search. It has no
// knowledge of filesystems or terminals.
package treemap

import (
	"math"
	"sort"
)

// Rect is an integer cell rectangle.
type Rect struct{ X, Y, W, H int }

// Empty reports whether r has no cells.
func (r Rect) Empty() bool { return r.W <= 0 || r.H <= 0 }

// Area returns W*H.
func (r Rect) Area() int { return max(r.W, 0) * max(r.H, 0) }

// Contains reports whether the cell (x, y) is inside r.
func (r Rect) Contains(x, y int) bool { return x >= r.X && x < r.X+r.W && y >= r.Y && y < r.Y+r.H }

// Inset shrinks r by n cells on each side.
func (r Rect) Inset(n int) Rect {
	r = Rect{r.X + n, r.Y + n, r.W - 2*n, r.H - 2*n}
	if r.W < 0 {
		r.W = 0
	}
	if r.H < 0 {
		r.H = 0
	}
	return r
}

// Item is something to lay out. Size must be > 0 to receive space.
type Item struct {
	ID   int64
	Size float64
}

// Block is a laid-out item.
type Block struct {
	ID int64
	Rect
}

// Squarify lays items out inside bounds using the squarified treemap
// algorithm (Bruls, Huizing, van Wijk). Items with non-positive size are
// ignored. The output partitions bounds exactly: rectangle edges are snapped
// to the grid by rounding shared float edges, so there are no gaps and no
// overlaps. Items that round to zero width or height are omitted. The result
// is deterministic for a given input set regardless of input order.
func Squarify(items []Item, bounds Rect) []Block {
	if bounds.Empty() {
		return nil
	}
	its := make([]Item, 0, len(items))
	total := 0.0
	for _, it := range items {
		if it.Size > 0 && !math.IsInf(it.Size, 0) && !math.IsNaN(it.Size) {
			its = append(its, it)
			total += it.Size
		}
	}
	if len(its) == 0 || total <= 0 {
		return nil
	}
	sort.SliceStable(its, func(i, j int) bool {
		if its[i].Size != its[j].Size {
			return its[i].Size > its[j].Size
		}
		return its[i].ID < its[j].ID
	})
	scale := float64(bounds.W) * float64(bounds.H) / total
	areas := make([]float64, len(its))
	for i, it := range its {
		areas[i] = it.Size * scale
	}
	frs := layout(areas, float64(bounds.X), float64(bounds.Y), float64(bounds.W), float64(bounds.H))
	out := make([]Block, 0, len(its))
	for i, f := range frs {
		x0, y0 := int(math.Round(f[0])), int(math.Round(f[1]))
		x1, y1 := int(math.Round(f[0]+f[2])), int(math.Round(f[1]+f[3]))
		x1 = min(x1, bounds.X+bounds.W)
		y1 = min(y1, bounds.Y+bounds.H)
		if x1 > x0 && y1 > y0 {
			out = append(out, Block{ID: its[i].ID, Rect: Rect{x0, y0, x1 - x0, y1 - y0}})
		}
	}
	return out
}

// worst returns the worst aspect ratio of a row with the given sum, largest
// and smallest areas laid along a side of length side.
func worst(sum, largest, smallest, side float64) float64 {
	s2, w2 := sum*sum, side*side
	return math.Max(w2*largest/s2, s2/(w2*smallest))
}

// layout returns float rectangles [x, y, w, h] for areas sorted descending.
func layout(areas []float64, x, y, w, h float64) [][4]float64 {
	out := make([][4]float64, len(areas))
	n := len(areas)
	for i := 0; i < n; {
		short := math.Min(w, h)
		j := i + 1
		sum := areas[i]
		cur := worst(sum, areas[i], areas[i], short)
		for j < n {
			s2 := sum + areas[j]
			if nw := worst(s2, areas[i], areas[j], short); nw > cur {
				break
			} else {
				cur = nw
			}
			sum = s2
			j++
		}
		last := j == n
		if w >= h {
			thick := sum / h
			if last || thick > w {
				thick = w
			}
			yy := y
			for k := i; k < j; k++ {
				hh := areas[k] / sum * h
				if k == j-1 {
					hh = y + h - yy
				}
				out[k] = [4]float64{x, yy, thick, hh}
				yy += hh
			}
			x += thick
			w -= thick
		} else {
			thick := sum / w
			if last || thick > h {
				thick = h
			}
			xx := x
			for k := i; k < j; k++ {
				ww := areas[k] / sum * w
				if k == j-1 {
					ww = x + w - xx
				}
				out[k] = [4]float64{xx, y, ww, thick}
				xx += ww
			}
			y += thick
			h -= thick
		}
		i = j
	}
	return out
}

// Visible decides how many leading items of a size-sorted (descending)
// list deserve their own block when area cells represent total: items are
// kept while each would get at least minArea cells and fewer than maxItems
// have been kept. The remainder should be merged into one group block.
func Visible(sizes []float64, total float64, area, minArea, maxItems int) int {
	if total <= 0 || area <= 0 {
		return 0
	}
	k := 0
	for k < len(sizes) && k < maxItems {
		if sizes[k]/total*float64(area) < float64(minArea) {
			break
		}
		k++
	}
	// Grouping a single leftover item gains nothing; show it directly if it
	// gets at least one cell.
	if k == len(sizes)-1 && k < maxItems && sizes[k]/total*float64(area) >= 1 {
		k++
	}
	return k
}

// At returns the block containing the cell (x, y).
func At(blocks []Block, x, y int) (int64, bool) {
	for _, b := range blocks {
		if b.Contains(x, y) {
			return b.ID, true
		}
	}
	return 0, false
}
