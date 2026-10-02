package platform

import (
	"errors"
	"io"
	"os"
	"sync/atomic"

	"golang.org/x/sys/unix"
)

// noStatx is set if the kernel lacks statx (pre-4.11); we fall back to fstatat.
var noStatx atomic.Bool

const statxMask = unix.STATX_BASIC_STATS | unix.STATX_BTIME

// ReadDir streams the entries of a directory in chunks, stat-ing each entry
// relative to the directory fd (no repeated path resolution).
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
				m, serr := statAt(fd, de.Name(), unix.AT_SYMLINK_NOFOLLOW)
				if serr != nil {
					m = Meta{Err: &os.PathError{Op: "stat", Path: de.Name(), Err: serr}}
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

func statAt(dirfd int, name string, flags int) (Meta, error) {
	if !noStatx.Load() {
		var st unix.Statx_t
		err := unix.Statx(dirfd, name, flags|unix.AT_STATX_DONT_SYNC, statxMask, &st)
		if err == nil {
			return fromStatx(&st), nil
		}
		if !errors.Is(err, unix.ENOSYS) {
			return Meta{}, err
		}
		noStatx.Store(true)
	}
	var st unix.Stat_t
	if err := unix.Fstatat(dirfd, name, &st, flags); err != nil {
		return Meta{}, err
	}
	return Meta{
		Mode: modeFromUnix(st.Mode), Size: st.Size, Alloc: st.Blocks * 512, AllocKnown: true,
		MTime: st.Mtim.Nano(), UID: st.Uid, GID: st.Gid, Nlink: uint32(st.Nlink),
		Dev: st.Dev, Ino: st.Ino, HasIno: true,
	}, nil
}

func fromStatx(st *unix.Statx_t) Meta {
	m := Meta{
		Mode:       modeFromUnix(uint32(st.Mode)),
		Size:       int64(st.Size),
		Alloc:      int64(st.Blocks) * 512,
		AllocKnown: true,
		MTime:      st.Mtime.Sec*1e9 + int64(st.Mtime.Nsec),
		UID:        st.Uid,
		GID:        st.Gid,
		Nlink:      st.Nlink,
		Dev:        unix.Mkdev(st.Dev_major, st.Dev_minor),
		Ino:        st.Ino,
		HasIno:     true,
	}
	if st.Mask&unix.STATX_BTIME != 0 {
		m.BTime = st.Btime.Sec*1e9 + int64(st.Btime.Nsec)
	}
	return m
}

func statPath(path string, follow bool) (Meta, error) {
	flags := unix.AT_SYMLINK_NOFOLLOW
	if follow {
		flags = 0
	}
	m, err := statAt(unix.AT_FDCWD, path, flags)
	if err != nil {
		return Meta{}, &os.PathError{Op: "stat", Path: path, Err: err}
	}
	m.Name = baseName(path)
	return m, nil
}

// Linux pseudo-filesystems whose contents are not disk usage.
var virtualFS = map[int64]bool{
	0x9fa0:     true, // proc
	0x62656572: true, // sysfs
	0x1cd1:     true, // devpts
	0x27e0eb:   true, // cgroup
	0x63677270: true, // cgroup2
	0x64626720: true, // debugfs
	0x74726163: true, // tracefs
	0x73636673: true, // securityfs
	0x6165676c: true, // pstore
	0xcafe4a11: true, // bpf
	0x62656570: true, // configfs
	0x65735543: true, // fusectl
	0x19800202: true, // mqueue
	0xde5e81e4: true, // efivarfs
	0x42494e4d: true, // binfmt_misc
	0xf97cff8c: true, // selinuxfs
	0x958458f6: true, // hugetlbfs
	0x6e736673: true, // nsfs
	0x50495045: true, // pipefs
}

// IsVirtualFS reports whether path is the root of a pseudo filesystem
// (proc, sysfs, cgroup, …) that should not be scanned as disk usage.
func IsVirtualFS(path string) bool {
	var st unix.Statfs_t
	if err := unix.Statfs(path, &st); err != nil {
		return false
	}
	return virtualFS[int64(st.Type)]
}
