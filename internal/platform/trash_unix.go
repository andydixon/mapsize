//go:build linux || darwin

package platform

import (
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"syscall"

	"golang.org/x/sys/unix"
)

// renameChecked moves path to dst if it still matches want. The parent
// directory is opened once and both the check and the rename are relative to
// that descriptor, so a parent swapped for a symlink since the scan (or
// between check and rename) cannot redirect the move to another user's file.
func renameChecked(path, dst string, want Expect) error {
	dir, base := filepath.Dir(path), filepath.Base(path)
	dfd, err := unix.Open(dir, unix.O_RDONLY|unix.O_DIRECTORY|unix.O_CLOEXEC, 0)
	if err != nil {
		return &os.PathError{Op: "open", Path: dir, Err: err}
	}
	defer unix.Close(dfd)
	var st unix.Stat_t
	if err := unix.Fstatat(dfd, base, &st, unix.AT_SYMLINK_NOFOLLOW); err != nil {
		return &os.PathError{Op: "stat", Path: path, Err: err}
	}
	if st.Uid != want.UID || modeFromUnix(uint32(st.Mode))&fs.ModeType != want.Mode&fs.ModeType {
		return errors.New("item changed since the scan (owner or type differs); rescan first")
	}
	return unix.Renameat(dfd, base, unix.AT_FDCWD, dst)
}

// ensurePrivateDir creates dir (mode 0700) if missing; an existing entry must
// be a real directory (not a symlink) owned by us. With strict (trash on a
// shared filesystem) it must also be inaccessible to others, as the
// freedesktop.org spec requires; home trash directories made by desktop
// environments are sometimes 0755, which is harmless there.
func ensurePrivateDir(dir string, strict bool) error {
	if err := os.Mkdir(dir, 0o700); err != nil && !errors.Is(err, os.ErrExist) {
		return err
	}
	fi, err := os.Lstat(dir)
	if err != nil {
		return err
	}
	st := fi.Sys().(*syscall.Stat_t)
	switch {
	case !fi.IsDir():
		return errors.New("not a directory (symlink?)")
	case int(st.Uid) != os.Getuid():
		return errors.New("owned by another user")
	case strict && fi.Mode().Perm()&0o077 != 0:
		return errors.New("accessible to other users")
	}
	return nil
}
