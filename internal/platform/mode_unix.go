//go:build unix

package platform

import (
	"io/fs"
	"syscall"
)

func modeFromUnix(m uint32) fs.FileMode {
	mode := fs.FileMode(m & 0o777)
	switch m & syscall.S_IFMT {
	case syscall.S_IFDIR:
		mode |= fs.ModeDir
	case syscall.S_IFLNK:
		mode |= fs.ModeSymlink
	case syscall.S_IFIFO:
		mode |= fs.ModeNamedPipe
	case syscall.S_IFSOCK:
		mode |= fs.ModeSocket
	case syscall.S_IFCHR:
		mode |= fs.ModeDevice | fs.ModeCharDevice
	case syscall.S_IFBLK:
		mode |= fs.ModeDevice
	}
	if m&syscall.S_ISUID != 0 {
		mode |= fs.ModeSetuid
	}
	if m&syscall.S_ISGID != 0 {
		mode |= fs.ModeSetgid
	}
	if m&syscall.S_ISVTX != 0 {
		mode |= fs.ModeSticky
	}
	return mode
}
