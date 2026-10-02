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
	if err := Trash(victim); err != nil {
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
	if err := Trash(victim); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Lstat(trashed + ".2"); err != nil {
		t.Fatal("collision not handled")
	}
	if err := Trash("relative/path"); err == nil {
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
