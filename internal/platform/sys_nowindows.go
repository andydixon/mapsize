//go:build !windows && !linux && !darwin && !freebsd

package platform

import "os"

func fillSys(*Meta, os.FileInfo) {}
