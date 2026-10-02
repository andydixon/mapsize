// Package platform hides OS-specific filesystem metadata behind a small
// common API. Files are split by build tag; nothing else in the program
// switches on GOOS.
package platform

import (
	"errors"
	"io"
	"io/fs"
	"os"
	"os/user"
	"strconv"
	"sync"
)

// Meta is the metadata of one directory entry.
type Meta struct {
	Name       string
	Mode       fs.FileMode
	Size       int64
	Alloc      int64
	AllocKnown bool
	MTime      int64 // unix nanoseconds
	BTime      int64 // birth time, 0 if unknown
	UID, GID   uint32
	Nlink      uint32
	Dev, Ino   uint64
	HasIno     bool  // Dev/Ino are meaningful
	Err        error // stat failed; only Name is valid
}

// ChunkFunc receives successive batches of entries. The slice is owned by
// the callee. Returning an error stops the read.
type ChunkFunc func([]Meta) error

// Lstat returns metadata for path without following a final symlink.
func Lstat(path string) (Meta, error) { return statPath(path, false) }

// Stat returns metadata for path, following symlinks.
func Stat(path string) (Meta, error) { return statPath(path, true) }

var (
	nameMu     sync.Mutex
	userNames  = map[uint32]string{}
	groupNames = map[uint32]string{}
)

// UserName resolves a uid to a name (cached; falls back to the number).
func UserName(uid uint32) string {
	return lookupName(userNames, uid, func(s string) (string, error) {
		u, err := user.LookupId(s)
		if err != nil {
			return "", err
		}
		return u.Username, nil
	})
}

// GroupName resolves a gid to a name (cached; falls back to the number).
func GroupName(gid uint32) string {
	return lookupName(groupNames, gid, func(s string) (string, error) {
		g, err := user.LookupGroupId(s)
		if err != nil {
			return "", err
		}
		return g.Name, nil
	})
}

func lookupName(cache map[uint32]string, id uint32, look func(string) (string, error)) string {
	nameMu.Lock()
	defer nameMu.Unlock()
	if n, ok := cache[id]; ok {
		return n
	}
	s := strconv.FormatUint(uint64(id), 10)
	if n, err := look(s); err == nil {
		s = n
	}
	cache[id] = s
	return s
}

// fromFileInfo fills the portable fields of Meta.
func fromFileInfo(fi fs.FileInfo) Meta {
	return Meta{
		Name:  fi.Name(),
		Mode:  fi.Mode(),
		Size:  fi.Size(),
		Alloc: fi.Size(),
		MTime: fi.ModTime().UnixNano(),
		Nlink: 1,
	}
}

// readDirGeneric is the portable fallback: os.ReadDir in chunks plus Lstat
// per entry by path.
func readDirGeneric(path string, chunk int, fn ChunkFunc, stat func(string) (Meta, error)) error {
	f, err := os.Open(path)
	if err != nil {
		return err
	}
	defer f.Close()
	for {
		des, err := f.ReadDir(chunk)
		if len(des) > 0 {
			out := make([]Meta, len(des))
			for i, de := range des {
				m, serr := stat(joinPath(path, de.Name()))
				if serr != nil {
					m = Meta{Err: serr}
				}
				m.Name = de.Name()
				out[i] = m
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

func joinPath(dir, name string) string {
	if len(dir) > 0 && os.IsPathSeparator(dir[len(dir)-1]) {
		return dir + name
	}
	return dir + string(os.PathSeparator) + name
}

// JoinPath joins a directory and an entry name without cleaning.
func JoinPath(dir, name string) string { return joinPath(dir, name) }

func baseName(path string) string {
	i := len(path) - 1
	for i > 0 && os.IsPathSeparator(path[i]) {
		i--
	}
	path = path[:i+1]
	for j := len(path) - 1; j >= 0; j-- {
		if os.IsPathSeparator(path[j]) {
			return path[j+1:]
		}
	}
	return path
}
