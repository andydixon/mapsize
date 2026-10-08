//! Pure geometry: squarified layout on a cell grid, level-of-detail
//! selection and spatial neighbour search. No knowledge of filesystems or
//! terminals.

mod nav;

pub use nav::{neighbour, Direction};

/// An integer cell rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    /// Whether the rectangle has no cells.
    pub fn empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    /// w*h.
    pub fn area(&self) -> i32 {
        self.w.max(0) * self.h.max(0)
    }

    /// Whether the cell (x, y) is inside.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    /// Shrinks by n cells on each side.
    pub fn inset(&self, n: i32) -> Rect {
        Rect {
            x: self.x + n,
            y: self.y + n,
            w: (self.w - 2 * n).max(0),
            h: (self.h - 2 * n).max(0),
        }
    }
}

/// Something to lay out. Size must be > 0 to receive space.
#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub id: i64,
    pub size: f64,
}

/// A laid-out item.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Block {
    pub id: i64,
    pub rect: Rect,
}

/// Lays items out inside bounds using the squarified treemap algorithm
/// (Bruls, Huizing, van Wijk). Items with non-positive or non-finite size are
/// ignored. The output partitions bounds exactly: rectangle edges are snapped
/// to the grid by rounding shared float edges, so there are no gaps and no
/// overlaps. Items that round to zero width or height are omitted. The result
/// is deterministic for a given input set regardless of input order.
pub fn squarify(items: &[Item], bounds: Rect) -> Vec<Block> {
    if bounds.empty() {
        return Vec::new();
    }
    let mut its: Vec<Item> = items
        .iter()
        .copied()
        .filter(|it| it.size > 0.0 && it.size.is_finite())
        .collect();
    let total: f64 = its.iter().map(|it| it.size).sum();
    if its.is_empty() || total <= 0.0 {
        return Vec::new();
    }
    its.sort_by(|a, b| b.size.total_cmp(&a.size).then(a.id.cmp(&b.id)));
    let scale = bounds.w as f64 * bounds.h as f64 / total;
    let areas: Vec<f64> = its.iter().map(|it| it.size * scale).collect();
    let frs = layout(
        &areas,
        bounds.x as f64,
        bounds.y as f64,
        bounds.w as f64,
        bounds.h as f64,
    );
    let mut out = Vec::with_capacity(its.len());
    for (it, f) in its.iter().zip(frs) {
        let (x0, y0) = (f[0].round() as i32, f[1].round() as i32);
        let x1 = ((f[0] + f[2]).round() as i32).min(bounds.x + bounds.w);
        let y1 = ((f[1] + f[3]).round() as i32).min(bounds.y + bounds.h);
        if x1 > x0 && y1 > y0 {
            out.push(Block {
                id: it.id,
                rect: Rect {
                    x: x0,
                    y: y0,
                    w: x1 - x0,
                    h: y1 - y0,
                },
            });
        }
    }
    out
}

/// The worst aspect ratio of a row with the given sum, largest and smallest
/// areas laid along a side of length side.
fn worst(sum: f64, largest: f64, smallest: f64, side: f64) -> f64 {
    let (s2, w2) = (sum * sum, side * side);
    (w2 * largest / s2).max(s2 / (w2 * smallest))
}

/// Float rectangles [x, y, w, h] for areas sorted descending.
fn layout(areas: &[f64], mut x: f64, mut y: f64, mut w: f64, mut h: f64) -> Vec<[f64; 4]> {
    let n = areas.len();
    let mut out = vec![[0.0; 4]; n];
    let mut i = 0;
    while i < n {
        let short = w.min(h);
        let mut j = i + 1;
        let mut sum = areas[i];
        let mut cur = worst(sum, areas[i], areas[i], short);
        while j < n {
            let s2 = sum + areas[j];
            let nw = worst(s2, areas[i], areas[j], short);
            if nw > cur {
                break;
            }
            cur = nw;
            sum = s2;
            j += 1;
        }
        let last = j == n;
        if w >= h {
            let mut thick = sum / h;
            if last || thick > w {
                thick = w;
            }
            let mut yy = y;
            for k in i..j {
                let hh = if k == j - 1 {
                    y + h - yy
                } else {
                    areas[k] / sum * h
                };
                out[k] = [x, yy, thick, hh];
                yy += hh;
            }
            x += thick;
            w -= thick;
        } else {
            let mut thick = sum / w;
            if last || thick > h {
                thick = h;
            }
            let mut xx = x;
            for k in i..j {
                let ww = if k == j - 1 {
                    x + w - xx
                } else {
                    areas[k] / sum * w
                };
                out[k] = [xx, y, ww, thick];
                xx += ww;
            }
            y += thick;
            h -= thick;
        }
        i = j;
    }
    out
}

/// How many leading items of a size-sorted (descending) list deserve their
/// own block when area cells represent total: items are kept while each
/// would get at least min_area cells and fewer than max_items have been
/// kept. The remainder should be merged into one group block.
pub fn visible(sizes: &[f64], total: f64, area: i32, min_area: i32, max_items: usize) -> usize {
    if total <= 0.0 || area <= 0 {
        return 0;
    }
    let cells = |s: f64| s / total * area as f64;
    let mut k = 0;
    while k < sizes.len() && k < max_items {
        if cells(sizes[k]) < min_area as f64 {
            break;
        }
        k += 1;
    }
    // Grouping a single leftover item gains nothing; show it directly if it
    // gets at least one cell.
    if k + 1 == sizes.len() && k < max_items && cells(sizes[k]) >= 1.0 {
        k += 1;
    }
    k
}

/// The block containing the cell (x, y).
pub fn at(blocks: &[Block], x: i32, y: i32) -> Option<i64> {
    blocks.iter().find(|b| b.rect.contains(x, y)).map(|b| b.id)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn r(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn it(id: i64, size: f64) -> Item {
        Item { id, size }
    }

    /// Small deterministic PRNG for property checks (splitmix64).
    pub struct Rng(u64);
    impl Rng {
        pub fn new(seed: u64) -> Rng {
            Rng(seed)
        }
        pub fn intn(&mut self, n: u64) -> u64 {
            self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
            (z ^ (z >> 31)) % n
        }
    }

    fn check_partition(blocks: &[Block], b: Rect, exact: bool) {
        let mut grid = vec![false; (b.w * b.h) as usize];
        for bl in blocks {
            let br = bl.rect;
            assert!(br.w > 0 && br.h > 0, "non-positive block {bl:?}");
            assert!(
                br.x >= b.x && br.y >= b.y && br.x + br.w <= b.x + b.w && br.y + br.h <= b.y + b.h,
                "block {bl:?} outside bounds {b:?}"
            );
            for y in br.y..br.y + br.h {
                for x in br.x..br.x + br.w {
                    let i = ((y - b.y) * b.w + (x - b.x)) as usize;
                    assert!(!grid[i], "overlap at {x},{y}");
                    grid[i] = true;
                }
            }
        }
        if exact {
            if let Some(i) = grid.iter().position(|v| !v) {
                panic!("gap at {},{}", b.x + i as i32 % b.w, b.y + i as i32 / b.w);
            }
        }
    }

    #[test]
    fn squarify_basic() {
        let b = r(2, 3, 60, 20);
        let items = [
            it(1, 6.0),
            it(2, 6.0),
            it(3, 4.0),
            it(4, 3.0),
            it(5, 2.0),
            it(6, 2.0),
            it(7, 1.0),
        ];
        let blocks = squarify(&items, b);
        assert_eq!(blocks.len(), items.len());
        check_partition(&blocks, b, true);
        // Areas roughly proportional.
        for bl in &blocks {
            let want = b.area() as f64 * items[(bl.id - 1) as usize].size / 24.0;
            let got = bl.rect.area() as f64;
            assert!(
                got >= want * 0.6 - 4.0 && got <= want * 1.4 + 4.0,
                "block {} area {got} want ~{want}",
                bl.id
            );
        }
        // Deterministic regardless of input order.
        let mut rev = items.to_vec();
        rev.reverse();
        assert_eq!(blocks, squarify(&rev, b), "layout depends on input order");
    }

    #[test]
    fn squarify_edge_cases() {
        assert!(squarify(&[], r(0, 0, 10, 10)).is_empty());
        assert!(
            squarify(&[it(1, 0.0), it(2, -5.0)], r(0, 0, 10, 10)).is_empty(),
            "zero sizes should produce nothing"
        );
        assert!(squarify(&[it(1, f64::NAN), it(2, f64::INFINITY)], r(0, 0, 10, 10)).is_empty());
        for b in [
            r(0, 0, 0, 0),
            r(0, 0, 1, 1),
            r(0, 0, 1, 50),
            r(0, 0, 50, 1),
            r(0, 0, -3, 4),
        ] {
            let bl = squarify(&[it(1, 5.0), it(2, 3.0), it(3, 1.0)], b);
            if !b.empty() {
                check_partition(&bl, b, true);
            }
        }
        // One enormous item and many tiny ones.
        let mut items = vec![it(0, 1e15)];
        items.extend((1..500).map(|i| it(i, 1.0)));
        check_partition(&squarify(&items, r(0, 0, 80, 24)), r(0, 0, 80, 24), true);
    }

    #[test]
    fn resize_recomputes() {
        let items = [it(1, 50.0), it(2, 30.0), it(3, 20.0)];
        for b in [
            r(0, 0, 80, 24),
            r(0, 0, 120, 40),
            r(0, 0, 300, 80),
            r(0, 0, 70, 20),
            r(0, 0, 180, 50),
        ] {
            let bl = squarify(&items, b);
            check_partition(&bl, b, true);
            assert_eq!(bl.len(), 3, "{b:?}");
        }
    }

    /// Squarify invariants over random partitions, from fixed seeds.
    #[test]
    fn squarify_random_partitions() {
        let mut rng = Rng::new(1);
        for _ in 0..300 {
            let (w, h, n) = (
                rng.intn(400) as i32,
                rng.intn(150) as i32,
                rng.intn(3000) as usize,
            );
            let items: Vec<Item> = (0..n)
                .map(|i| {
                    let bits = rng.intn(40) + 1;
                    it(i as i64, rng.intn(1 << bits) as f64)
                })
                .collect();
            let b = r(rng.intn(10) as i32, rng.intn(10) as i32, w, h);
            let bl = squarify(&items, b);
            if !b.empty() && !bl.is_empty() {
                check_partition(&bl, b, true);
            }
        }
    }

    #[test]
    fn visible_counts() {
        let sizes = [100.0, 50.0, 10.0, 1.0, 1.0, 1.0];
        assert_eq!(visible(&sizes, 163.0, 1630, 20, 100), 3);
        assert_eq!(visible(&sizes, 163.0, 1630, 20, 2), 2, "max_items");
        assert_eq!(
            visible(&[10.0, 1.0], 11.0, 1100, 200, 10),
            2,
            "single leftover"
        );
    }

    #[test]
    fn at_finds_block() {
        let bl = [
            Block {
                id: 1,
                rect: r(0, 0, 5, 5),
            },
            Block {
                id: 2,
                rect: r(5, 0, 5, 5),
            },
        ];
        assert_eq!(at(&bl, 6, 2), Some(2));
        assert_eq!(at(&bl, 10, 2), None);
    }
}
