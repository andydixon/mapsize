//go:build unix

package platform

import (
	"os"
	"syscall"
)

// OpenContent opens a file whose content is about to be read. It does not
// follow a final symlink, and O_NONBLOCK keeps a FIFO swapped in since the
// scan from blocking open(2) forever; callers still check the file type.
func OpenContent(path string) (*os.File, error) {
	return os.OpenFile(path, os.O_RDONLY|syscall.O_NOFOLLOW|syscall.O_NONBLOCK, 0)
}
