package treemap

import (
	"math/rand"
	"testing"
)

func checkPartition(t *testing.T, blocks []Block, b Rect, exact bool) {
	t.Helper()
	grid := make([]int, b.W*b.H)
	for _, bl := range blocks {
		if bl.W <= 0 || bl.H <= 0 {
			t.Fatalf("non-positive block %+v", bl)
		}
		if bl.X < b.X || bl.Y < b.Y || bl.X+bl.W > b.X+b.W || bl.Y+bl.H > b.Y+b.H {
			t.Fatalf("block %+v outside bounds %+v", bl, b)
		}
		for y := bl.Y; y < bl.Y+bl.H; y++ {
			for x := bl.X; x < bl.X+bl.W; x++ {
				i := (y-b.Y)*b.W + (x - b.X)
				if grid[i] != 0 {
					t.Fatalf("overlap at %d,%d", x, y)
				}
				grid[i] = 1
			}
		}
	}
	if exact {
		for i, v := range grid {
			if v == 0 {
				t.Fatalf("gap at %d,%d", b.X+i%b.W, b.Y+i/b.W)
			}
		}
	}
}

func TestSquarifyBasic(t *testing.T) {
	b := Rect{2, 3, 60, 20}
	items := []Item{{1, 6}, {2, 6}, {3, 4}, {4, 3}, {5, 2}, {6, 2}, {7, 1}}
	blocks := Squarify(items, b)
	if len(blocks) != len(items) {
		t.Fatalf("got %d blocks", len(blocks))
	}
	checkPartition(t, blocks, b, true)
	// Areas roughly proportional.
	for _, bl := range blocks {
		want := float64(b.Area()) * items[bl.ID-1].Size / 24
		if got := float64(bl.Area()); got < want*0.6-4 || got > want*1.4+4 {
			t.Errorf("block %d area %v want ~%v", bl.ID, got, want)
		}
	}
	// Deterministic regardless of input order.
	rev := append([]Item(nil), items...)
	for i, j := 0, len(rev)-1; i < j; i, j = i+1, j-1 {
		rev[i], rev[j] = rev[j], rev[i]
	}
	b2 := Squarify(rev, b)
	for i := range blocks {
		if blocks[i] != b2[i] {
			t.Fatal("layout depends on input order")
		}
	}
}

func TestSquarifyEdgeCases(t *testing.T) {
	if Squarify(nil, Rect{0, 0, 10, 10}) != nil {
		t.Fatal("nil items")
	}
	if Squarify([]Item{{1, 0}, {2, -5}}, Rect{0, 0, 10, 10}) != nil {
		t.Fatal("zero sizes should produce nothing")
	}
	for _, b := range []Rect{{0, 0, 0, 0}, {0, 0, 1, 1}, {0, 0, 1, 50}, {0, 0, 50, 1}, {0, 0, -3, 4}} {
		bl := Squarify([]Item{{1, 5}, {2, 3}, {3, 1}}, b)
		if !b.Empty() {
			checkPartition(t, bl, b, true)
		}
	}
	// One enormous item and many tiny ones.
	items := []Item{{0, 1e15}}
	for i := 1; i < 500; i++ {
		items = append(items, Item{int64(i), 1})
	}
	bl := Squarify(items, Rect{0, 0, 80, 24})
	checkPartition(t, bl, Rect{0, 0, 80, 24}, true)
}

func TestResizeRecomputes(t *testing.T) {
	items := []Item{{1, 50}, {2, 30}, {3, 20}}
	for _, b := range []Rect{{0, 0, 80, 24}, {0, 0, 120, 40}, {0, 0, 300, 80}, {0, 0, 70, 20}, {0, 0, 180, 50}} {
		bl := Squarify(items, b)
		checkPartition(t, bl, b, true)
		if len(bl) != 3 {
			t.Fatalf("%v: %d blocks", b, len(bl))
		}
	}
}

func FuzzSquarify(f *testing.F) {
	f.Add(int64(1), 80, 24, 10)
	f.Add(int64(2), 3, 2, 100)
	f.Add(int64(3), 300, 90, 1000)
	f.Fuzz(func(t *testing.T, seed int64, w, h, n int) {
		w, h, n = w%400, h%150, n%3000
		if w < 0 || h < 0 || n < 0 {
			return
		}
		r := rand.New(rand.NewSource(seed))
		items := make([]Item, n)
		for i := range items {
			items[i] = Item{int64(i), float64(r.Int63n(1 << uint(r.Intn(40)+1)))}
		}
		b := Rect{r.Intn(10), r.Intn(10), w, h}
		bl := Squarify(items, b)
		if !b.Empty() && len(bl) > 0 {
			checkPartition(t, bl, b, true)
		}
	})
}

func TestVisible(t *testing.T) {
	sizes := []float64{100, 50, 10, 1, 1, 1}
	if k := Visible(sizes, 163, 1630, 20, 100); k != 3 {
		t.Fatalf("k=%d", k)
	}
	if k := Visible(sizes, 163, 1630, 20, 2); k != 2 {
		t.Fatalf("maxItems k=%d", k)
	}
	if k := Visible([]float64{10, 1}, 11, 1100, 200, 10); k != 2 {
		t.Fatalf("single leftover k=%d", k)
	}
}

// Layout:
//
//	┌───────┬───────┐
//	│   A   │   B   │
//	├───┬───┴───────┤
//	│ C │     D     │
//	└───┴───────────┘
func TestNeighbour(t *testing.T) {
	A := Block{1, Rect{0, 0, 8, 4}}
	B := Block{2, Rect{8, 0, 8, 4}}
	C := Block{3, Rect{0, 4, 4, 4}}
	D := Block{4, Rect{4, 4, 12, 4}}
	bs := []Block{D, C, B, A} // order must not matter
	cases := []struct {
		from int64
		d    Direction
		want int64
		ok   bool
	}{
		{1, Right, 2, true}, {2, Left, 1, true}, {1, Down, 3, true}, {2, Down, 4, true},
		{3, Right, 4, true}, {4, Left, 3, true}, {3, Up, 1, true}, {4, Up, 2, true},
		{1, Left, 0, false}, {1, Up, 0, false}, {4, Right, 0, false}, {4, Down, 0, false},
	}
	for _, c := range cases {
		got, ok := Neighbour(bs, c.from, c.d)
		if ok != c.ok || got != c.want {
			t.Errorf("from %d dir %d: got %d,%v want %d,%v", c.from, c.d, got, ok, c.want, c.ok)
		}
	}
}

func TestNeighbourPrefersAdjacentOverArrayOrder(t *testing.T) {
	// S is on the left; R is directly right; F is far right and lower but
	// appears first in the slice.
	S := Block{1, Rect{0, 0, 10, 5}}
	R := Block{2, Rect{10, 0, 10, 5}}
	F := Block{3, Rect{10, 20, 30, 5}}
	if got, _ := Neighbour([]Block{F, S, R}, 1, Right); got != 2 {
		t.Fatalf("got %d", got)
	}
	// Tall block on the left with three stacked blocks to the right: pick
	// the one at the centre line.
	T := Block{10, Rect{0, 0, 10, 9}}
	r1 := Block{11, Rect{10, 0, 10, 3}}
	r2 := Block{12, Rect{10, 3, 10, 3}}
	r3 := Block{13, Rect{10, 6, 10, 3}}
	if got, _ := Neighbour([]Block{r3, r1, r2, T}, 10, Right); got != 12 {
		t.Fatalf("centre preference: got %d", got)
	}
	if got, _ := Neighbour([]Block{r3, r1, r2, T}, 13, Left); got != 10 {
		t.Fatalf("back left: got %d", got)
	}
}

func TestNeighbourRandomLayoutsAlwaysAdjacent(t *testing.T) {
	r := rand.New(rand.NewSource(7))
	for iter := 0; iter < 200; iter++ {
		items := make([]Item, 2+r.Intn(30))
		for i := range items {
			items[i] = Item{int64(i), float64(1 + r.Intn(1000))}
		}
		bl := Squarify(items, Rect{0, 0, 120, 40})
		for _, s := range bl {
			for _, d := range []Direction{Left, Right, Up, Down} {
				id, ok := Neighbour(bl, s.ID, d)
				if !ok {
					continue
				}
				var c Block
				for _, b := range bl {
					if b.ID == id {
						c = b
					}
				}
				// In a partition, if any block touches the edge in direction d
				// with overlapping span, the chosen one must touch too.
				var touch bool
				switch d {
				case Right:
					touch = c.X == s.X+s.W
				case Left:
					touch = c.X+c.W == s.X
				case Down:
					touch = c.Y == s.Y+s.H
				case Up:
					touch = c.Y+c.H == s.Y
				}
				if !touch {
					t.Fatalf("iter %d: from %+v dir %d chose non-adjacent %+v", iter, s, d, c)
				}
			}
		}
	}
}

func BenchmarkSquarify(b *testing.B) {
	items := make([]Item, 300)
	for i := range items {
		items[i] = Item{int64(i), float64(1 + i*i)}
	}
	for b.Loop() {
		Squarify(items, Rect{0, 0, 200, 60})
	}
}

func BenchmarkNeighbour(b *testing.B) {
	items := make([]Item, 300)
	for i := range items {
		items[i] = Item{int64(i), float64(1 + i*i)}
	}
	bl := Squarify(items, Rect{0, 0, 200, 60})
	for b.Loop() {
		Neighbour(bl, 150, Right)
	}
}
