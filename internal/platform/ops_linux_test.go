package platform

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestTrashFreedesktop(t *testing.T) {
	data := t.TempDir()
	t.Setenv("XDG_DATA_HOME", data)
	dir := filepath.Join(data, "work")
	os.Mkdir(dir, 0o755)
	victim := filepath.Join(dir, "weird name ;rm -rf ~;")
	if err := os.WriteFile(victim, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := Trash(victim, want(t, victim)); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Lstat(victim); !os.IsNotExist(err) {
		t.Fatal("file still present")
	}
	trashed := filepath.Join(data, "Trash", "files", "weird name ;rm -rf ~;")
	if _, err := os.Lstat(trashed); err != nil {
		t.Fatalf("not in trash: %v", err)
	}
	info, err := os.ReadFile(filepath.Join(data, "Trash", "info", "weird name ;rm -rf ~;.trashinfo"))
	if err != nil || !strings.Contains(string(info), "Path="+strings.ReplaceAll(dir, " ", "%20")) {
		t.Fatalf("bad trashinfo: %q %v", info, err)
	}
	// A second item with the same name gets a distinct trash name.
	os.WriteFile(victim, []byte("y"), 0o644)
	if err := Trash(victim, want(t, victim)); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Lstat(trashed + ".2"); err != nil {
		t.Fatal("collision not handled")
	}
	if err := Trash("relative/path", Expect{}); err == nil {
		t.Fatal("relative paths must be refused")
	}
}

func TestEnsurePrivateDirRejectsSymlink(t *testing.T) {
	base := t.TempDir()
	target := filepath.Join(base, "attacker")
	os.Mkdir(target, 0o700)
	link := filepath.Join(base, ".Trash-1000")
	os.Symlink(target, link)
	if err := ensurePrivateDir(link, false); err == nil {
		t.Fatal("symlinked trash dir accepted")
	}
	open := filepath.Join(base, "open")
	os.Mkdir(open, 0o777)
	os.Chmod(open, 0o777)
	if err := ensurePrivateDir(open, true); err == nil {
		t.Fatal("world-accessible trash dir accepted")
	}
	if err := ensurePrivateDir(filepath.Join(base, "fresh"), true); err != nil {
		t.Fatal(err)
	}
}

func want(t *testing.T, path string) Expect {
	t.Helper()
	m, err := Lstat(path)
	if err != nil {
		t.Fatal(err)
	}
	return Expect{UID: m.UID, Mode: m.Mode}
}

// A parent swapped for a symlink after the scan must not redirect the move
// to whatever the symlink points at.
func TestTrashRefusesSwappedItem(t *testing.T) {
	data := t.TempDir()
	t.Setenv("XDG_DATA_HOME", data)
	dir := filepath.Join(data, "work")
	os.Mkdir(dir, 0o755)
	victim := filepath.Join(dir, "big.iso")
	os.WriteFile(victim, []byte("x"), 0o644)
	scanned := want(t, victim)

	// Item replaced by a symlink to a file elsewhere: type differs.
	other := filepath.Join(data, "precious")
	os.WriteFile(other, []byte("keep"), 0o644)
	os.Remove(victim)
	os.Symlink(other, victim)
	scannedDir := Expect{UID: scanned.UID, Mode: os.ModeDir}
	if err := Trash(victim, scanned); err == nil {
		t.Fatal("symlink accepted where a regular file was scanned")
	}
	if err := Trash(victim, scannedDir); err == nil {
		t.Fatal("type mismatch accepted")
	}
	if _, err := os.Lstat(victim); err != nil {
		t.Fatal("refused item was moved")
	}

	// Owner differs from the scan.
	os.Remove(victim)
	os.WriteFile(victim, []byte("x"), 0o644)
	if err := Trash(victim, Expect{UID: scanned.UID + 1, Mode: scanned.Mode}); err == nil {
		t.Fatal("owner mismatch accepted")
	}
	entries, _ := os.ReadDir(filepath.Join(data, "Trash", "info"))
	if len(entries) != 0 {
		t.Fatalf("refused trash left %d .trashinfo files", len(entries))
	}
	if err := Trash(victim, scanned); err != nil {
		t.Fatal(err)
	}
}
