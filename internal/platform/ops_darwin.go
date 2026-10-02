package platform

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"syscall"
)

// TrashName is the user-facing name of the trash.
const TrashName = "Trash"

// Reveal selects the item in Finder.
func Reveal(path string) error {
	if err := checkAbs(path); err != nil {
		return err
	}
	return startDetached("open", "-R", path)
}

// Trash moves path into ~/.Trash (same volume) or the volume's
// .Trashes/<uid> directory. It never copies and never deletes.
func Trash(path string) error {
	if err := checkAbs(path); err != nil {
		return err
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return err
	}
	dirs := []string{filepath.Join(home, ".Trash")}
	if vol := volumeOf(path); vol != "" {
		dirs = append(dirs, filepath.Join(vol, ".Trashes", fmt.Sprint(os.Getuid())))
	}
	base := filepath.Base(path)
	for _, dir := range dirs {
		if err := os.MkdirAll(dir, 0o700); err != nil {
			continue
		}
		for i := 1; i < 10000; i++ {
			name := base
			if i > 1 {
				name = fmt.Sprintf("%s %d", base, i)
			}
			dst := filepath.Join(dir, name)
			if _, err := os.Lstat(dst); err == nil {
				continue
			}
			err := os.Rename(path, dst)
			if errors.Is(err, syscall.EXDEV) {
				break // try the next trash location
			}
			return err
		}
	}
	return errors.New("trash: item is on a volume without a usable trash")
}

func volumeOf(path string) string {
	const prefix = "/Volumes/"
	if len(path) > len(prefix) && path[:len(prefix)] == prefix {
		rest := path[len(prefix):]
		for i := 0; i < len(rest); i++ {
			if rest[i] == '/' {
				return prefix + rest[:i]
			}
		}
		return path
	}
	return ""
}
