//go:build !unix

package platform

import "os"

// OpenContent opens a file whose content is about to be read.
func OpenContent(path string) (*os.File, error) { return os.Open(path) }
