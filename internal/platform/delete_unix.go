//go:build unix

package platform

import (
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"syscall"
)

// Delete permanently removes path (recursively for a directory) if it still
// matches want. The check and the removal both go through an os.Root on the
// parent directory, so a symlink swapped in since the scan cannot redirect
// the removal outside it.
func Delete(path string, want Expect) error {
	if err := checkAbs(path); err != nil {
		return err
	}
	dir, base := filepath.Dir(path), filepath.Base(path)
	root, err := os.OpenRoot(dir)
	if err != nil {
		return err
	}
	defer root.Close()
	fi, err := root.Lstat(base)
	if err != nil {
		return err
	}
	if fi.Sys().(*syscall.Stat_t).Uid != want.UID || fi.Mode()&fs.ModeType != want.Mode&fs.ModeType {
		return errors.New("item changed since the scan (owner or type differs); rescan first")
	}
	return root.RemoveAll(base)
}
