use super::Block;

/// An arrow-key direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Finds the block a user expects to reach by pressing an arrow key from the
/// block with id sel:
///
///  1. Only blocks entirely beyond the selected block's edge in the
///     direction of travel are candidates.
///  2. Blocks whose span overlaps the selected block's orthogonal span are
///     strongly preferred over all others.
///  3. Then the smallest gap along the direction of travel wins.
///  4. Then the largest overlap with the selected block's span.
///  5. Then the block whose orthogonal span is closest to the selected
///     block's centre line (0 if it contains it).
///  6. Ties break by position (top, then left) and finally id.
///
/// None if there is no block in that direction.
pub fn neighbour(blocks: &[Block], sel: i64, d: Direction) -> Option<i64> {
    let s = blocks.iter().find(|b| b.id == sel)?.rect;
    // Lexicographic score, smaller is better; overlap is negated so larger wins.
    let mut best: Option<(i32, i32, i32, i32, i32, i32, i64)> = None;
    for b in blocks {
        if b.id == sel {
            continue;
        }
        let c = b.rect;
        let (ok, gap, lo, hi, clo, chi) = match d {
            Direction::Right => (c.x >= s.x + s.w, c.x - (s.x + s.w), s.y, s.y + s.h, c.y, c.y + c.h),
            Direction::Left => (c.x + c.w <= s.x, s.x - (c.x + c.w), s.y, s.y + s.h, c.y, c.y + c.h),
            Direction::Down => (c.y >= s.y + s.h, c.y - (s.y + s.h), s.x, s.x + s.w, c.x, c.x + c.w),
            Direction::Up => (c.y + c.h <= s.y, s.y - (c.y + c.h), s.x, s.x + s.w, c.x, c.x + c.w),
        };
        if !ok {
            continue;
        }
        let ovl = hi.min(chi) - lo.max(clo);
        let (tier, ovl) = if ovl > 0 { (0, ovl) } else { (1, 0) };
        // Distance (doubled, to stay integral) from the selected centre line
        // to the candidate's span; 0 if the span contains it.
        let centre = lo + hi;
        let off = if centre < 2 * clo {
            2 * clo - centre
        } else if centre > 2 * chi {
            centre - 2 * chi
        } else {
            0
        };
        let sc = (tier, gap, -ovl, off, c.y, c.x, b.id);
        if best.is_none_or(|b| sc < b) {
            best = Some(sc);
        }
    }
    best.map(|b| b.6)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{it, r, Rng};
    use super::super::*;
    use super::Direction::*;

    fn blk(id: i64, x: i32, y: i32, w: i32, h: i32) -> Block {
        Block { id, rect: r(x, y, w, h) }
    }

    // Layout:
    //
    //   ┌───────┬───────┐
    //   │   A   │   B   │
    //   ├───┬───┴───────┤
    //   │ C │     D     │
    //   └───┴───────────┘
    #[test]
    fn neighbour_basic() {
        let a = blk(1, 0, 0, 8, 4);
        let b = blk(2, 8, 0, 8, 4);
        let c = blk(3, 0, 4, 4, 4);
        let d = blk(4, 4, 4, 12, 4);
        let bs = [d, c, b, a]; // order must not matter
        let cases = [
            (1, Right, Some(2)), (2, Left, Some(1)), (1, Down, Some(3)), (2, Down, Some(4)),
            (3, Right, Some(4)), (4, Left, Some(3)), (3, Up, Some(1)), (4, Up, Some(2)),
            (1, Left, None), (1, Up, None), (4, Right, None), (4, Down, None),
        ];
        for (from, dir, want) in cases {
            assert_eq!(neighbour(&bs, from, dir), want, "from {from} dir {dir:?}");
        }
    }

    #[test]
    fn neighbour_prefers_adjacent_over_array_order() {
        // S is on the left; R is directly right; F is far right and lower but
        // appears first in the slice.
        let s = blk(1, 0, 0, 10, 5);
        let rr = blk(2, 10, 0, 10, 5);
        let f = blk(3, 10, 20, 30, 5);
        assert_eq!(neighbour(&[f, s, rr], 1, Right), Some(2));
        // Tall block on the left with three stacked blocks to the right: pick
        // the one at the centre line.
        let t = blk(10, 0, 0, 10, 9);
        let r1 = blk(11, 10, 0, 10, 3);
        let r2 = blk(12, 10, 3, 10, 3);
        let r3 = blk(13, 10, 6, 10, 3);
        assert_eq!(neighbour(&[r3, r1, r2, t], 10, Right), Some(12), "centre preference");
        assert_eq!(neighbour(&[r3, r1, r2, t], 13, Left), Some(10), "back left");
    }

    #[test]
    fn neighbour_random_layouts_always_adjacent() {
        let mut rng = Rng::new(7);
        for iter in 0..200 {
            let n = 2 + rng.intn(30) as i64;
            let items: Vec<Item> = (0..n).map(|i| it(i, (1 + rng.intn(1000)) as f64)).collect();
            let bl = squarify(&items, r(0, 0, 120, 40));
            for s in &bl {
                for d in [Left, Right, Up, Down] {
                    let Some(id) = neighbour(&bl, s.id, d) else { continue };
                    let c = bl.iter().find(|b| b.id == id).unwrap().rect;
                    let s = s.rect;
                    // In a partition, if any block touches the edge in direction d
                    // with overlapping span, the chosen one must touch too.
                    let touch = match d {
                        Right => c.x == s.x + s.w,
                        Left => c.x + c.w == s.x,
                        Down => c.y == s.y + s.h,
                        Up => c.y + c.h == s.y,
                    };
                    assert!(touch, "iter {iter}: from {s:?} dir {d:?} chose non-adjacent {c:?}");
                }
            }
        }
    }
}
