// Package config loads user settings from a JSON file in the OS-specific
// user configuration directory. It knows nothing about rendering.
package config

import (
	"encoding/json"
	"errors"
	"io/fs"
	"os"
	"path/filepath"

	"github.com/andydixon/mapsize/internal/brand"
)

// Settings are user preferences. Zero values are never used directly; Load
// starts from Defaults and overlays the file.
type Settings struct {
	Workers        string   `json:"workers"`         // conservative|balanced|aggressive|N
	FollowSymlinks string   `json:"follow_symlinks"` // none|same-filesystem|all
	OneFileSystem  bool     `json:"one_file_system"`
	Units          string   `json:"units"`     // iec|si
	SizeMode       string   `json:"size_mode"` // allocated|logical
	Theme          string   `json:"theme"`
	Mouse          bool     `json:"mouse"`
	Excludes       []string `json:"excludes"`
	// ExtensionColors maps an extension (without dot) to "#rrggbb".
	ExtensionColors map[string]string `json:"extension_colors"`
	// CategoryColors maps a category name to "#rrggbb".
	CategoryColors map[string]string `json:"category_colors"`
	// RefreshHz is the UI refresh rate while scanning.
	RefreshHz int `json:"refresh_hz"`
}

// Defaults returns the built-in settings.
func Defaults() Settings {
	return Settings{
		Workers: "balanced", FollowSymlinks: "none", Units: "iec", SizeMode: "allocated",
		Theme: "default", Mouse: true, RefreshHz: 10,
	}
}

// Path returns the config file location.
func Path() (string, error) {
	dir, err := os.UserConfigDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, brand.Name, "config.json"), nil
}

// Load reads the config file if present. A missing file is not an error.
func Load() (Settings, error) {
	s := Defaults()
	p, err := Path()
	if err != nil {
		return s, nil
	}
	b, err := os.ReadFile(p)
	if errors.Is(err, fs.ErrNotExist) {
		return s, nil
	}
	if err != nil {
		return s, err
	}
	if err := json.Unmarshal(b, &s); err != nil {
		return Defaults(), err
	}
	if s.RefreshHz < 1 || s.RefreshHz > 60 {
		s.RefreshHz = 10
	}
	return s, nil
}
