package textutil

import (
	"fmt"
	"strconv"
	"time"
)

// SI selects decimal units (kB, MB, …). The default is IEC (KiB, MiB, …).
// It is set once at start-up, before any goroutines format sizes.
var SI bool

var (
	iecUnits = []string{"B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"}
	siUnits  = []string{"B", "kB", "MB", "GB", "TB", "PB", "EB"}
)

func scaled(n int64) (float64, string) {
	base, units := 1024.0, iecUnits
	if SI {
		base, units = 1000.0, siUnits
	}
	neg := n < 0
	v := float64(n)
	if neg {
		v = -v
	}
	i := 0
	for v >= base && i < len(units)-1 {
		v /= base
		i++
	}
	if neg {
		v = -v
	}
	return v, units[i]
}

func num(v float64, unit string) string {
	a := v
	if a < 0 {
		a = -a
	}
	switch {
	case unit == "B":
		return strconv.FormatFloat(v, 'f', 0, 64)
	case a < 10:
		return strconv.FormatFloat(v, 'f', 2, 64)
	case a < 100:
		return strconv.FormatFloat(v, 'f', 1, 64)
	default:
		return strconv.FormatFloat(v, 'f', 0, 64)
	}
}

// Size formats a byte count, e.g. "86.4 GiB".
func Size(n int64) string {
	v, u := scaled(n)
	return num(v, u) + " " + u
}

// SizeCompact formats a byte count in at most ~7 cells, e.g. "86GiB".
func SizeCompact(n int64) string {
	v, u := scaled(n)
	a := v
	if a < 0 {
		a = -a
	}
	if u != "B" && a < 10 {
		return strconv.FormatFloat(v, 'f', 1, 64) + u
	}
	return strconv.FormatFloat(v, 'f', 0, 64) + u
}

// SignedSize formats a delta with an explicit sign.
func SignedSize(n int64) string {
	if n > 0 {
		return "+" + Size(n)
	}
	if n < 0 {
		return "-" + Size(-n)
	}
	return "±0 B"
}

// Count formats an integer with thousands separators.
func Count(n int64) string {
	s := strconv.FormatInt(n, 10)
	neg := false
	if n < 0 {
		neg, s = true, s[1:]
	}
	out := make([]byte, 0, len(s)+len(s)/3)
	for i := range len(s) {
		if i > 0 && (len(s)-i)%3 == 0 {
			out = append(out, ',')
		}
		out = append(out, s[i])
	}
	if neg {
		return "-" + string(out)
	}
	return string(out)
}

// Duration formats an elapsed time as mm:ss or h:mm:ss.
func Duration(d time.Duration) string {
	s := int64(d.Seconds())
	if s >= 3600 {
		return fmt.Sprintf("%d:%02d:%02d", s/3600, s/60%60, s%60)
	}
	return fmt.Sprintf("%02d:%02d", s/60, s%60)
}

// Percent formats part/whole as a percentage.
func Percent(part, whole int64) string {
	if whole <= 0 {
		return "–"
	}
	p := float64(part) * 100 / float64(whole)
	if p < 10 {
		return strconv.FormatFloat(p, 'f', 1, 64) + "%"
	}
	return strconv.FormatFloat(p, 'f', 0, 64) + "%"
}
