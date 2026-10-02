//go:build darwin || freebsd

package platform

import (
	"errors"
	"io"
	"os"

	"golang.org/x/sys/unix"
)

// ReadDir streams the entries of a directory in chunks, stat-ing each entry
// relative to the directory fd.
func ReadDir(path string, chunk int, fn ChunkFunc) error {
	fd, err := unix.Open(path, unix.O_RDONLY|unix.O_DIRECTORY|unix.O_CLOEXEC, 0)
	if err != nil {
		return &os.PathError{Op: "open", Path: path, Err: err}
	}
	f := os.NewFile(uintptr(fd), path)
	defer f.Close()
	for {
		des, err := f.ReadDir(chunk)
		if len(des) > 0 {
			out := make([]Meta, len(des))
			for i, de := range des {
				var st unix.Stat_t
				if serr := unix.Fstatat(fd, de.Name(), &st, unix.AT_SYMLINK_NOFOLLOW); serr != nil {
					out[i] = Meta{Name: de.Name(), Err: &os.PathError{Op: "stat", Path: de.Name(), Err: serr}}
					continue
				}
				out[i] = fromStat(&st)
				out[i].Name = de.Name()
			}
			if ferr := fn(out); ferr != nil {
				return ferr
			}
		}
		if err != nil {
			if errors.Is(err, io.EOF) {
				return nil
			}
			return err
		}
	}
}

func fromStat(st *unix.Stat_t) Meta {
	return Meta{
		Mode:       modeFromUnix(uint32(st.Mode)),
		Size:       st.Size,
		Alloc:      st.Blocks * 512,
		AllocKnown: true,
		MTime:      st.Mtim.Nano(),
		BTime:      st.Btim.Nano(),
		UID:        st.Uid,
		GID:        st.Gid,
		Nlink:      uint32(st.Nlink),
		Dev:        uint64(st.Dev),
		Ino:        uint64(st.Ino),
		HasIno:     true,
	}
}

func statPath(path string, follow bool) (Meta, error) {
	var st unix.Stat_t
	var err error
	if follow {
		err = unix.Stat(path, &st)
	} else {
		err = unix.Lstat(path, &st)
	}
	if err != nil {
		return Meta{}, &os.PathError{Op: "stat", Path: path, Err: err}
	}
	m := fromStat(&st)
	m.Name = baseName(path)
	return m, nil
}

// IsVirtualFS is Linux-specific; elsewhere nothing is skipped.
func IsVirtualFS(string) bool { return false }
