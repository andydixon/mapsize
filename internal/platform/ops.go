package platform

import (
	"errors"
	"os"
	"os/exec"
	"path/filepath"
)

// startDetached runs a helper program with an argument vector (never a
// shell), detached from the terminal so it cannot disturb the TUI.
func startDetached(name string, args ...string) error {
	cmd := exec.Command(name, args...)
	cmd.Stdin, cmd.Stdout, cmd.Stderr = nil, nil, nil
	if err := cmd.Start(); err != nil {
		return err
	}
	go cmd.Wait()
	return nil
}

// checkAbs guards helper invocations: an absolute path can never be parsed
// as an option by the helper program.
func checkAbs(path string) error {
	if !filepath.IsAbs(path) {
		return errors.New("refusing relative path")
	}
	if _, err := os.Lstat(path); err != nil {
		return err
	}
	return nil
}
