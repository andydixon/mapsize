// Command treegen creates synthetic directory trees for testing and
// benchmarking mapsize.
package main

import (
	"flag"
	"fmt"
	"math"
	"math/rand"
	"os"
	"path/filepath"
	"time"
)

func main() {
	var (
		out      = flag.String("out", "", "output directory (required, must not exist or be empty)")
		dirs     = flag.Int("directories", 1000, "number of directories")
		files    = flag.Int("files", 10000, "number of files")
		depth    = flag.Int("max-depth", 8, "maximum directory depth")
		sparse   = flag.Bool("sparse", false, "make some large files sparse")
		unicode  = flag.Bool("unicode", false, "use Unicode and unusual (but safe) names")
		hostile  = flag.Bool("hostile", false, "include names with control characters/escape sequences (Unix only)")
		maxSize  = flag.Int64("max-size", 1<<20, "largest regular file in bytes (sizes are log-uniform; content is written)")
		seed     = flag.Int64("seed", 1, "random seed (deterministic output)")
		hotspots = flag.Int("hotspots", 3, "directories that receive a large share of files")
	)
	flag.Parse()
	if *out == "" {
		flag.Usage()
		os.Exit(2)
	}
	if err := os.MkdirAll(*out, 0o755); err != nil {
		fatal(err)
	}
	if ents, _ := os.ReadDir(*out); len(ents) > 0 {
		fatal(fmt.Errorf("%s is not empty", *out))
	}
	r := rand.New(rand.NewSource(*seed))
	start := time.Now()

	type dir struct {
		path  string
		depth int
	}
	all := []dir{{*out, 0}}
	byDepth := [][]int{{0}}
	for len(all) < *dirs+1 {
		// Pick a depth first, then a directory at that depth, so the tree is
		// broad near the top instead of one giant preferential-attachment chain.
		lv := byDepth[r.Intn(len(byDepth))]
		p := all[lv[r.Intn(len(lv))]]
		if p.depth >= *depth {
			continue
		}
		d := dir{filepath.Join(p.path, name(r, len(all), *unicode, *hostile)), p.depth + 1}
		if err := os.Mkdir(d.path, 0o755); err != nil {
			continue // name collision; try again
		}
		all = append(all, d)
		if d.depth == len(byDepth) {
			byDepth = append(byDepth, nil)
		}
		byDepth[d.depth] = append(byDepth[d.depth], len(all)-1)
	}
	hot := make([]int, *hotspots)
	for i := range hot {
		hot[i] = r.Intn(len(all))
	}
	var bytes int64
	buf := make([]byte, *maxSize)
	for i := range buf {
		buf[i] = byte(r.Intn(256)) // incompressible, so allocation is real everywhere
	}
	for i := 0; i < *files; i++ {
		var d dir
		if len(hot) > 0 && r.Intn(3) == 0 {
			d = all[hot[r.Intn(len(hot))]]
		} else {
			d = all[r.Intn(len(all))]
		}
		p := filepath.Join(d.path, name(r, i, *unicode, *hostile)+ext(r))
		// Log-uniform sizes: many tiny files, a few large ones.
		size := int64(math.Exp(r.Float64() * math.Log(float64(*maxSize))))
		if r.Intn(20) == 0 {
			size = 0
		}
		f, err := os.OpenFile(p, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o644)
		if err != nil {
			continue
		}
		if *sparse && r.Intn(200) == 0 {
			// Sparse: a large logical size with almost nothing allocated.
			f.Truncate(int64(1+r.Intn(8)) << 30)
			f.WriteAt([]byte("x"), 0)
		} else if size > 0 {
			if _, err := f.Write(buf[:size]); err != nil {
				fatal(err)
			}
			bytes += size
		}
		f.Close()
		if i%100000 == 0 && i > 0 {
			fmt.Fprintf(os.Stderr, "%d files…\n", i)
		}
	}
	fmt.Fprintf(os.Stderr, "created %d directories, %d files (%d logical bytes) in %s\n",
		len(all)-1, *files, bytes, time.Since(start).Round(time.Millisecond))
}

var words = []string{"data", "cache", "backup", "photos", "src", "build", "logs", "tmp", "vm", "media", "docs", "lib", "node_modules", "archive"}
var exts = []string{".txt", ".log", ".jpg", ".png", ".mp4", ".iso", ".qcow2", ".go", ".c", ".pdf", ".zip", ".tar.gz", ".db", ".so", "", ".json", ".mkv", ".flac"}
var uni = []string{"日本語", "Ünïcödé", "emoji 🎉", "Ελληνικά", "кириллица", "space name", "quote'name", "dash-name"}

func name(r *rand.Rand, i int, unicode, hostile bool) string {
	switch {
	case hostile && r.Intn(50) == 0:
		return fmt.Sprintf("evil\x1b[31m%d\x1b]0;pwned\x07", i)
	case unicode && r.Intn(5) == 0:
		return fmt.Sprintf("%s-%d", uni[r.Intn(len(uni))], i)
	}
	return fmt.Sprintf("%s-%d", words[r.Intn(len(words))], i)
}

func ext(r *rand.Rand) string { return exts[r.Intn(len(exts))] }

func fatal(err error) {
	fmt.Fprintln(os.Stderr, "treegen:", err)
	os.Exit(1)
}
