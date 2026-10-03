//go:build !linux && !darwin && !windows

package platform

import (
	"errors"
	"os"
)

// TrashName is the user-facing name of the trash.
const TrashName = "trash"

// Reveal is unsupported on this platform.
func Reveal(string) error { return errors.New("not supported on this platform") }

// Trash is unsupported on this platform.
func Trash(string, Expect) error { return errors.New("trash not supported on this platform") }

// Headless reports whether there is no graphical session (no X or Wayland
// display), so there is no file manager to open and no desktop trash anyone
// will empty.
func Headless() bool {
	return os.Getenv("DISPLAY") == "" && os.Getenv("WAYLAND_DISPLAY") == ""
}
