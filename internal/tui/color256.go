package tui

import (
	"math"
	"sync"
)

// The stock 256-colour conversion maps dark, slightly tinted colours to
// saturated cube entries (olive, navy), which wrecks the subtle shading the
// treemap relies on. quant256 picks the perceptually nearest entry (redmean
// distance) among the cube and the grey ramp, never the palette-dependent
// first 16 entries.

var (
	cubeLevels = [6]int{0, 95, 135, 175, 215, 255}
	q256Cache  sync.Map // Color -> uint8
)

func redmean(r1, g1, b1, r2, g2, b2 int) int {
	rm := (r1 + r2) / 2
	dr, dg, db := r1-r2, g1-g2, b1-b2
	return ((512+rm)*dr*dr)>>8 + 4*dg*dg + ((767-rm)*db*db)>>8
}

func quant256(c Color) uint8 {
	if v, ok := q256Cache.Load(c); ok {
		return v.(uint8)
	}
	r, g, b := int(c>>16&0xff), int(c>>8&0xff), int(c&0xff)
	best, bestD := 16, math.MaxInt
	for i := 0; i < 216; i++ {
		cr, cg, cb := cubeLevels[i/36], cubeLevels[i/6%6], cubeLevels[i%6]
		if d := redmean(r, g, b, cr, cg, cb); d < bestD {
			best, bestD = 16+i, d
		}
	}
	for i := 0; i < 24; i++ {
		v := 8 + 10*i
		if d := redmean(r, g, b, v, v, v); d < bestD {
			best, bestD = 232+i, d
		}
	}
	q256Cache.Store(c, uint8(best))
	return uint8(best)
}

var q16Cache sync.Map // Color -> uint8

// quant16 maps a colour to a basic colour. Dark shades become black and
// unsaturated colours grey/white, so tints and shadows stay quiet; saturated
// colours keep their hue (bright variant when light), so category colours
// survive. The stock conversion instead turns dark tints into bright
// yellow/cyan.
func quant16(c Color) uint8 {
	if v, ok := q16Cache.Load(c); ok {
		return v.(uint8)
	}
	rf, gf, bf := c.rgb()
	mx, mn := max(rf, gf, bf), min(rf, gf, bf)
	luma := c.Luma()
	var n uint8
	switch {
	case luma < 0.2:
		n = 0
	case mx-mn < 0.12: // grey
		switch {
		case luma < 0.45:
			n = 8
		case luma < 0.8:
			n = 7
		default:
			n = 15
		}
	default:
		// Hue in sixths: red, yellow, green, cyan, blue, magenta.
		var h float64
		switch mx {
		case rf:
			h = math.Mod((gf-bf)/(mx-mn), 6)
		case gf:
			h = (bf-rf)/(mx-mn) + 2
		default:
			h = (rf-gf)/(mx-mn) + 4
		}
		if h < 0 {
			h += 6
		}
		ansi := [6]uint8{1, 3, 2, 6, 4, 5}[int(math.Round(h))%6]
		n = ansi
		if luma > 0.5 {
			n += 8
		}
	}
	q16Cache.Store(c, n)
	return n
}
