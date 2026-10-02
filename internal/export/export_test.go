package export

import (
	"bytes"
	"encoding/json"
	"strings"
	"testing"

	"github.com/andydixon/mapsize/internal/inventory"
)

const hostile = "evil\x1b]0;pwned\x07\x1b[2J"

func tree() *inventory.Tree {
	t := inventory.New("/r", inventory.KindDir)
	d := t.Add(0, "d", inventory.KindDir)
	f := t.Add(d, hostile, inventory.KindFile)
	t.Node(f).Size, t.Node(f).Alloc = 10, 4096
	t.Recompute()
	t.Stats.Complete = true
	return t
}

func noControl(t *testing.T, s string) {
	t.Helper()
	if strings.ContainsAny(s, "\x1b\x07") {
		t.Fatalf("raw control characters in output: %q", s)
	}
}

func TestOutputsAreTerminalSafe(t *testing.T) {
	tr := tree()
	var b bytes.Buffer
	if err := CSV(&b, tr, 0, -1); err != nil {
		t.Fatal(err)
	}
	noControl(t, b.String())
	if !strings.Contains(b.String(), `evil\x1b]0;pwned\x07`) {
		t.Fatalf("CSV should show the escaped name: %s", b.String())
	}

	b.Reset()
	Table(&b, tr, []inventory.NodeID{2}, inventory.SizeAllocated)
	Summary(&b, tr)
	noControl(t, b.String())

	b.Reset()
	if err := JSON(&b, tr, 0, -1); err != nil {
		t.Fatal(err)
	}
	noControl(t, b.String())
	var v struct {
		Tree struct {
			Children []struct {
				Children []struct{ Name string }
			}
		}
	}
	if err := json.Unmarshal(b.Bytes(), &v); err != nil {
		t.Fatal(err)
	}
	// JSON keeps the exact name (escaped by the encoder, decoded back).
	if got := v.Tree.Children[0].Children[0].Name; got != hostile {
		t.Fatalf("JSON name %q", got)
	}
}

func TestJSONDepthLimit(t *testing.T) {
	var b bytes.Buffer
	JSON(&b, tree(), 0, 1)
	if strings.Contains(b.String(), "evil") {
		t.Fatal("depth 1 should not include grandchildren")
	}
}
