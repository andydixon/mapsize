//go:build !linux && !darwin && !freebsd

package platform

import "os"

// ReadDir streams the entries of a directory using the portable os API.
// Allocated size, inode identity and link counts are not available here.
func ReadDir(path string, chunk int, fn ChunkFunc) error {
	return readDirGeneric(path, chunk, fn, func(p string) (Meta, error) { return statPath(p, false) })
}

func statPath(path string, follow bool) (Meta, error) {
	var fi os.FileInfo
	var err error
	if follow {
		fi, err = os.Stat(path)
	} else {
		fi, err = os.Lstat(path)
	}
	if err != nil {
		return Meta{}, err
	}
	m := fromFileInfo(fi)
	m.Name = baseName(path)
	fillSys(&m, fi)
	return m, nil
}

// IsVirtualFS is Linux-specific; elsewhere nothing is skipped.
func IsVirtualFS(string) bool { return false }
