// Package brand isolates the product name so it can be changed in one place.
package brand

const (
	Name          = "mapsize"
	SnapshotExt   = ".msz"
	SnapshotMagic = "MAPSIZE\x00"
)

// Version is stamped at release time via -ldflags -X.
var Version = "0.9.0-dev"
