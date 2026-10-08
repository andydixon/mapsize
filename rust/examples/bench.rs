//! In-process benchmarks mirroring the Go `go test -bench` suite
//! (internal/tui/bench_test.go, internal/treemap/treemap_test.go):
//!
//!     cargo run --release --example bench
//!
//! Each case runs for ~2 s after a warm-up and reports the mean time per op.

use mapsize::treemap::{self, Direction, Item, Rect};
use mapsize::tui::bench::{big_tree, BenchModel};
use std::hint::black_box;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

fn bench(name: &str, mut f: impl FnMut(u64)) {
    f(0);
    let budget = Duration::from_secs(2);
    let start = Instant::now();
    let mut n = 0u64;
    while start.elapsed() < budget {
        f(n);
        n += 1;
    }
    let per = start.elapsed() / n as u32;
    println!("{name:<40} {n:>8} ops  {per:>12.2?}/op");
}

fn main() {
    let items: Vec<Item> = (0..300).map(|i| Item { id: i, size: (1 + i * i) as f64 }).collect();
    let bounds = Rect { x: 0, y: 0, w: 200, h: 60 };
    bench("treemap/Squarify(300)", |_| {
        black_box(treemap::squarify(black_box(&items), bounds));
    });
    let blocks = treemap::squarify(&items, bounds);
    bench("treemap/Neighbour(300)", |_| {
        black_box(treemap::neighbour(black_box(&blocks), 150, Direction::Right));
    });

    let t0 = Instant::now();
    let tree = Arc::new(RwLock::new(big_tree()));
    println!("big tree: {} nodes built in {:.2?}", tree.read().unwrap().len(), t0.elapsed());

    let mut m = BenchModel::new(tree.clone(), 200, 60);
    bench("tui/BigColdFrame", |_| m.cold_frame());
    let mut m = BenchModel::new(tree.clone(), 200, 60);
    m.cold_frame();
    bench("tui/BigResize", |i| m.resize(150 + (i % 100) as u16, 40 + (i % 30) as u16));
    let mut m = BenchModel::new(tree.clone(), 200, 60);
    m.zoom_to_child_named(b"flat");
    bench("tui/BigFlatDirCold", |_| m.cold_frame());
    let mut m = BenchModel::new(tree.clone(), 200, 60);
    m.cold_frame();
    let keys = ["right", "down", "left", "up"];
    bench("tui/BigNavigate", |i| m.key(keys[i as usize % 4]));
    for q in ["size > 1MB", "*.dat AND size > 500k", "path contains sub3"] {
        let mut m = BenchModel::new(tree.clone(), 200, 60);
        bench(&format!("tui/BigFilter/{q}"), |_| m.apply_filter(q));
    }
}
