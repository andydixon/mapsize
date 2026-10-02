package app

import (
	"bytes"
	"encoding/csv"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/andydixon/mapsize/internal/scan"
)

func fixture(t *testing.T) string {
	root := t.TempDir()
	os.MkdirAll(filepath.Join(root, "big"), 0o755)
	os.MkdirAll(filepath.Join(root, "small"), 0o755)
	os.WriteFile(filepath.Join(root, "big", "a.iso"), make([]byte, 300_000), 0o644)
	os.WriteFile(filepath.Join(root, "big", "b.iso"), make([]byte, 300_000), 0o644)
	os.WriteFile(filepath.Join(root, "small", "c.txt"), []byte("hello"), 0o644)
	return root
}

func run(t *testing.T, cfg *Config) string {
	t.Helper()
	var out bytes.Buffer
	cfg.Stdout, cfg.Stderr = &out, &bytes.Buffer{}
	cfg.ScanOpts = scan.Options{Workers: 2}
	if err := Run(cfg); err != nil {
		t.Fatalf("Run: %v", err)
	}
	return out.String()
}

func TestTopAndLargest(t *testing.T) {
	root := fixture(t)
	out := run(t, &Config{Paths: []string{root}, Top: 5, LargestFiles: 2})
	if !strings.Contains(out, filepath.Join(root, "big")) || !strings.Contains(out, "a.iso") {
		t.Fatalf("report missing entries:\n%s", out)
	}
	if strings.Index(out, filepath.Join(root, "big")) > strings.Index(out, filepath.Join(root, "small")) {
		t.Fatalf("not sorted by size:\n%s", out)
	}
}

func TestJSONIsValid(t *testing.T) {
	root := fixture(t)
	out := run(t, &Config{Paths: []string{root}, JSON: true, Depth: -1})
	var v struct {
		Root     string
		Complete bool
		Tree     struct {
			Size     int64
			Children []struct{ Name string }
		}
	}
	if err := json.Unmarshal([]byte(out), &v); err != nil {
		t.Fatalf("invalid JSON: %v\n%s", err, out)
	}
	if !v.Complete || v.Tree.Size < 600_005 || len(v.Tree.Children) != 2 {
		t.Fatalf("unexpected JSON: %+v", v)
	}
}

func TestCSV(t *testing.T) {
	root := fixture(t)
	out := run(t, &Config{Paths: []string{root}, CSV: true, Depth: -1})
	rows, err := csv.NewReader(strings.NewReader(out)).ReadAll()
	if err != nil {
		t.Fatal(err)
	}
	if len(rows) != 1+1+2+3 { // header, root, 2 dirs, 3 files
		t.Fatalf("%d rows:\n%s", len(rows), out)
	}
}

func TestSnapshotSaveLoadCompare(t *testing.T) {
	root := fixture(t)
	dir := t.TempDir()
	a, b := filepath.Join(dir, "a.msz"), filepath.Join(dir, "b.msz")
	run(t, &Config{Paths: []string{root}, NoUI: true, Save: a})
	os.WriteFile(filepath.Join(root, "small", "grown.bin"), make([]byte, 500_000), 0o644)
	os.Remove(filepath.Join(root, "big", "b.iso"))
	run(t, &Config{Paths: []string{root}, NoUI: true, Save: b})

	out := run(t, &Config{Load: a, Top: 3})
	if !strings.Contains(out, "big") {
		t.Fatalf("loaded snapshot report:\n%s", out)
	}
	out = run(t, &Config{Compare: [2]string{a, b}, NoUI: true})
	for _, want := range []string{"new", "grown.bin", "removed", "b.iso"} {
		if !strings.Contains(out, want) {
			t.Fatalf("compare output missing %q:\n%s", want, out)
		}
	}
}

func TestDuplicatesReport(t *testing.T) {
	root := fixture(t)
	out := run(t, &Config{Paths: []string{root}, Duplicates: true})
	if !strings.Contains(out, "1 verified duplicate groups") {
		t.Fatalf("duplicates:\n%s", out)
	}
}
