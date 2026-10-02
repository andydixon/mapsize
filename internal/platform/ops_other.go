//go:build !linux && !darwin && !windows

package platform

import "errors"

// TrashName is the user-facing name of the trash.
const TrashName = "trash"

// Reveal is unsupported on this platform.
func Reveal(string) error { return errors.New("not supported on this platform") }

// Trash is unsupported on this platform.
func Trash(string) error { return errors.New("trash not supported on this platform") }
