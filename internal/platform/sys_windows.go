package platform

import (
	"os"
	"syscall"
)

func fillSys(m *Meta, fi os.FileInfo) {
	if d, ok := fi.Sys().(*syscall.Win32FileAttributeData); ok {
		m.BTime = d.CreationTime.Nanoseconds()
	}
}
