package tui

import (
	"sort"
	"strings"

	"github.com/andydixon/mapsize/internal/config"
	"github.com/andydixon/mapsize/internal/inventory"
)

// Theme holds every colour the UI uses. Nothing else in the TUI hard-codes
// colours.
type Theme struct {
	Name  string
	Mono  bool // no colour: rely on attributes and glyphs
	ASCII bool // no box-drawing glyphs

	Bg, Fg, Muted, Faint Color
	Accent, Sel          Color
	HeaderBg, HeaderFg   Color
	StatusBg, StatusFg   Color
	PanelBg              Color
	Warn, Err, OK        Color
	Group                Color
	EmptyDir             Color
	Cat                  [inventory.NumCategories]Color
	DiffGrow, DiffShrink Color
	DiffNew, DiffSame    Color
	Ext                  map[string]Color
}

var themes = map[string]func() *Theme{
	"default":       defaultTheme,
	"dark":          darkTheme,
	"high-contrast": highContrastTheme,
	"mono":          monoTheme,
}

// ThemeNames lists built-in themes.
func ThemeNames() []string {
	var out []string
	for k := range themes {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

func defaultTheme() *Theme {
	return &Theme{
		Name: "default",
		Bg:   RGB(0x16, 0x18, 0x1d), Fg: RGB(0xdc, 0xdf, 0xe4), Muted: RGB(0x8a, 0x91, 0x9c), Faint: RGB(0x4a, 0x50, 0x5a),
		Accent: RGB(0x61, 0xaf, 0xef), Sel: RGB(0xff, 0xd8, 0x66),
		HeaderBg: RGB(0x21, 0x25, 0x2b), HeaderFg: RGB(0xdc, 0xdf, 0xe4),
		StatusBg: RGB(0x2c, 0x31, 0x3a), StatusFg: RGB(0xc8, 0xcc, 0xd4),
		PanelBg: RGB(0x1c, 0x1f, 0x25),
		Warn:    RGB(0xe5, 0xc0, 0x7b), Err: RGB(0xe0, 0x6c, 0x75), OK: RGB(0x98, 0xc3, 0x79),
		Group: RGB(0x5c, 0x63, 0x70), EmptyDir: RGB(0x4b, 0x52, 0x63),
		Cat: [inventory.NumCategories]Color{
			inventory.CatOther:      RGB(0x7f, 0x8c, 0x9d),
			inventory.CatImage:      RGB(0xe5, 0xc0, 0x7b),
			inventory.CatVideo:      RGB(0xe0, 0x6c, 0x75),
			inventory.CatAudio:      RGB(0xd1, 0x9a, 0x66),
			inventory.CatArchive:    RGB(0xc6, 0x78, 0xdd),
			inventory.CatDiskImage:  RGB(0x8e, 0x7c, 0xf0),
			inventory.CatDatabase:   RGB(0x56, 0xb6, 0xc2),
			inventory.CatCode:       RGB(0x98, 0xc3, 0x79),
			inventory.CatDocument:   RGB(0x61, 0xaf, 0xef),
			inventory.CatExecutable: RGB(0xbe, 0x50, 0x46),
			inventory.CatLog:        RGB(0xa3, 0xa8, 0x6a),
			inventory.CatCache:      RGB(0x6b, 0x77, 0x88),
		},
		DiffGrow: RGB(0xf0, 0x5a, 0x5a), DiffShrink: RGB(0x5a, 0xd0, 0x7a),
		DiffNew: RGB(0x4f, 0xa8, 0xff), DiffSame: RGB(0x50, 0x56, 0x60),
	}
}

func darkTheme() *Theme {
	t := defaultTheme()
	t.Name = "dark"
	t.Bg, t.PanelBg = RGB(0, 0, 0), RGB(0x0c, 0x0c, 0x0e)
	t.HeaderBg, t.StatusBg = RGB(0x12, 0x12, 0x16), RGB(0x1a, 0x1a, 0x20)
	for i, c := range t.Cat {
		t.Cat[i] = c.Mix(RGB(0, 0, 0), 0.2)
	}
	return t
}

func highContrastTheme() *Theme {
	t := defaultTheme()
	t.Name = "high-contrast"
	t.Bg, t.Fg, t.Muted = RGB(0, 0, 0), RGB(0xff, 0xff, 0xff), RGB(0xd0, 0xd0, 0xd0)
	t.HeaderBg, t.StatusBg, t.PanelBg = RGB(0, 0, 0), RGB(0x20, 0x20, 0x20), RGB(0, 0, 0)
	t.Sel, t.Accent = RGB(0xff, 0xff, 0x00), RGB(0x00, 0xff, 0xff)
	t.Cat = [inventory.NumCategories]Color{
		RGB(0xb0, 0xb0, 0xb0), RGB(0xff, 0xd7, 0x00), RGB(0xff, 0x30, 0x30), RGB(0xff, 0x8c, 0x00),
		RGB(0xff, 0x00, 0xff), RGB(0x9b, 0x7b, 0xff), RGB(0x00, 0xe5, 0xff), RGB(0x00, 0xff, 0x00),
		RGB(0x40, 0x9c, 0xff), RGB(0xff, 0x55, 0x55), RGB(0xc8, 0xff, 0x00), RGB(0x90, 0x90, 0xa0),
	}
	return t
}

func monoTheme() *Theme {
	return &Theme{Name: "mono", Mono: true}
}

// LoadTheme returns a built-in theme with user overrides applied.
func LoadTheme(name string, s config.Settings) *Theme {
	f, ok := themes[strings.ToLower(name)]
	if !ok {
		f = defaultTheme
	}
	t := f()
	if t.Mono {
		return t
	}
	for k, v := range s.CategoryColors {
		if c, ok := inventory.ParseCategory(k); ok {
			if col, ok := Hex(v); ok {
				t.Cat[c] = col
			}
		}
	}
	for k, v := range s.ExtensionColors {
		if col, ok := Hex(v); ok {
			if t.Ext == nil {
				t.Ext = map[string]Color{}
			}
			t.Ext[strings.ToLower(strings.TrimPrefix(k, "."))] = col
		}
	}
	return t
}

// Styles derived from the theme.
func (t *Theme) base() Style   { return Style{FG: t.Fg, BG: t.Bg} }
func (t *Theme) muted() Style  { return Style{FG: t.Muted, BG: t.Bg} }
func (t *Theme) header() Style { return Style{FG: t.HeaderFg, BG: t.HeaderBg} }
func (t *Theme) status() Style { return Style{FG: t.StatusFg, BG: t.StatusBg} }
func (t *Theme) panel() Style  { return Style{FG: t.Fg, BG: t.PanelBg} }

func (t *Theme) box() BoxChars {
	if t.ASCII {
		return BoxASCII
	}
	return BoxLight
}

func (t *Theme) selBox() BoxChars {
	if t.ASCII {
		return BoxASCIISel
	}
	return BoxDouble
}

// textOn picks a readable text colour for a background.
func (t *Theme) textOn(bg Color) Color {
	if bg == 0 {
		return t.Fg
	}
	if bg.Luma() > 0.55 {
		return RGB(0x10, 0x12, 0x16)
	}
	return RGB(0xf2, 0xf4, 0xf7)
}
