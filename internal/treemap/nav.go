package treemap

// Direction is an arrow-key direction.
type Direction int

const (
	Left Direction = iota
	Right
	Up
	Down
)

// Neighbour finds the block a user expects to reach by pressing an arrow
// key from the block with ID sel:
//
//  1. Only blocks entirely beyond the selected block's edge in the
//     direction of travel are candidates.
//  2. Blocks whose span overlaps the selected block's orthogonal span are
//     strongly preferred over all others.
//  3. Then the smallest gap along the direction of travel wins.
//  4. Then the largest overlap with the selected block's span.
//  5. Then the block whose orthogonal span is closest to the selected
//     block's centre line (0 if it contains it).
//  6. Ties break by position (top, then left) and finally ID.
//
// It returns false if there is no block in that direction.
func Neighbour(blocks []Block, sel int64, d Direction) (int64, bool) {
	var s Block
	found := false
	for _, b := range blocks {
		if b.ID == sel {
			s, found = b, true
			break
		}
	}
	if !found {
		return 0, false
	}
	type score struct {
		tier, gap, ovl, off, y, x int
		id                        int64
	}
	less := func(a, b score) bool {
		switch {
		case a.tier != b.tier:
			return a.tier < b.tier
		case a.gap != b.gap:
			return a.gap < b.gap
		case a.ovl != b.ovl:
			return a.ovl > b.ovl
		case a.off != b.off:
			return a.off < b.off
		case a.y != b.y:
			return a.y < b.y
		case a.x != b.x:
			return a.x < b.x
		}
		return a.id < b.id
	}
	var best score
	have := false
	for _, c := range blocks {
		if c.ID == sel {
			continue
		}
		var gap, lo, hi, clo, chi int
		var ok bool
		switch d {
		case Right:
			ok, gap = c.X >= s.X+s.W, c.X-(s.X+s.W)
			lo, hi, clo, chi = s.Y, s.Y+s.H, c.Y, c.Y+c.H
		case Left:
			ok, gap = c.X+c.W <= s.X, s.X-(c.X+c.W)
			lo, hi, clo, chi = s.Y, s.Y+s.H, c.Y, c.Y+c.H
		case Down:
			ok, gap = c.Y >= s.Y+s.H, c.Y-(s.Y+s.H)
			lo, hi, clo, chi = s.X, s.X+s.W, c.X, c.X+c.W
		case Up:
			ok, gap = c.Y+c.H <= s.Y, s.Y-(c.Y+c.H)
			lo, hi, clo, chi = s.X, s.X+s.W, c.X, c.X+c.W
		}
		if !ok {
			continue
		}
		tier, ovl := 1, min(hi, chi)-max(lo, clo)
		if ovl > 0 {
			tier = 0
		} else {
			ovl = 0
		}
		// Distance (doubled, to stay integral) from the selected centre
		// line to the candidate's span; 0 if the span contains it.
		centre := lo + hi
		off := 0
		if centre < 2*clo {
			off = 2*clo - centre
		} else if centre > 2*chi {
			off = centre - 2*chi
		}
		sc := score{tier, gap, ovl, off, c.Y, c.X, c.ID}
		if !have || less(sc, best) {
			best, have = sc, true
		}
	}
	return best.id, have
}
