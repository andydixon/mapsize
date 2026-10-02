package platform

import (
	"errors"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"syscall"
	"time"
)

// TrashName is the user-facing name of the trash.
const TrashName = "trash"

// Reveal opens the item's directory in the desktop file manager.
func Reveal(path string) error {
	if err := checkAbs(path); err != nil {
		return err
	}
	fi, err := os.Lstat(path)
	if err == nil && !fi.IsDir() {
		path = filepath.Dir(path)
	}
	return startDetached("xdg-open", path)
}

// Trash moves path to the freedesktop.org trash: the home trash when on the
// same filesystem, otherwise $topdir/.Trash-$uid on the item's filesystem.
// It never copies across filesystems and never deletes.
func Trash(path string, want Expect) error {
	if err := checkAbs(path); err != nil {
		return err
	}
	st, err := os.Lstat(path)
	if err != nil {
		return err
	}
	dev := st.Sys().(*syscall.Stat_t).Dev
	dataHome := os.Getenv("XDG_DATA_HOME")
	if dataHome == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			return err
		}
		dataHome = filepath.Join(home, ".local", "share")
	}
	home := filepath.Join(dataHome, "Trash")
	var trash, infoPath string
	if hs, err := os.Stat(dataHome); err == nil && hs.Sys().(*syscall.Stat_t).Dev == dev {
		trash, infoPath = home, path
	} else {
		top := mountTop(path, dev)
		trash = filepath.Join(top, ".Trash-"+strconv.Itoa(os.Getuid()))
		rel, err := filepath.Rel(top, path)
		if err != nil {
			return err
		}
		infoPath = rel
	}
	files, info := filepath.Join(trash, "files"), filepath.Join(trash, "info")
	if trash == home {
		if err := os.MkdirAll(trash, 0o700); err != nil {
			return err
		}
	} else if err := ensurePrivateDir(trash, true); err != nil {
		// On a shared filesystem another user could plant a symlink or a
		// world-writable directory here; refuse rather than follow it.
		return fmt.Errorf("trash directory %s is unsafe: %w", trash, err)
	}
	for _, d := range []string{files, info} {
		if err := ensurePrivateDir(d, trash != home); err != nil {
			return fmt.Errorf("trash directory %s is unsafe: %w", d, err)
		}
	}
	base := filepath.Base(path)
	for i := 1; i < 10000; i++ {
		name := base
		if i > 1 {
			name = fmt.Sprintf("%s.%d", base, i)
		}
		dst := filepath.Join(files, name)
		if _, err := os.Lstat(dst); err == nil {
			continue // never overwrite an earlier trashed item (even without .trashinfo)
		}
		ip := filepath.Join(info, name+".trashinfo")
		f, err := os.OpenFile(ip, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
		if errors.Is(err, os.ErrExist) {
			continue
		}
		if err != nil {
			return err
		}
		u := (&url.URL{Path: infoPath}).EscapedPath()
		_, werr := fmt.Fprintf(f, "[Trash Info]\nPath=%s\nDeletionDate=%s\n", u, time.Now().Format("2006-01-02T15:04:05"))
		cerr := f.Close()
		if werr != nil || cerr != nil {
			os.Remove(ip)
			return errors.Join(werr, cerr)
		}
		if err := renameChecked(path, dst, want); err != nil {
			os.Remove(ip)
			return err
		}
		return nil
	}
	return errors.New("trash: too many items with the same name")
}

// mountTop returns the top directory of the filesystem containing path.
func mountTop(path string, dev uint64) string {
	cur := filepath.Dir(path)
	for {
		parent := filepath.Dir(cur)
		if parent == cur {
			return cur
		}
		st, err := os.Stat(parent)
		if err != nil || st.Sys().(*syscall.Stat_t).Dev != dev {
			return cur
		}
		cur = parent
	}
}
