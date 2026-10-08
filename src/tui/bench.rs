//! Benchmark hooks: the model driven with no terminal, rendering in
//! true colour.

use super::model::{Key, Model, Msg, Options, Profile};
use crate::cancel::Cancel;
use crate::config::Settings;
use crate::inventory::{Category, Kind, NodeId, SizeMode, Tree};
use std::sync::{Arc, RwLock};

/// A model with no terminal. Every method that renders returns the frame
/// length so the work cannot be optimised away.
pub struct BenchModel {
    m: Model,
}

pub(crate) fn opts_for(tree: Arc<RwLock<Tree>>) -> Options {
    Options {
        read_only: false,
        theme: String::new(),
        size_mode: SizeMode::Allocated,
        mouse: false,
        color: String::new(),
        save: String::new(),
        settings: Settings::default(),
        start: Arc::new(move |_| Ok((None, tree.clone()))),
        diff: None,
    }
}

impl BenchModel {
    pub fn new(tree: Arc<RwLock<Tree>>, w: u16, h: u16) -> BenchModel {
        let mut m = Model::new(opts_for(tree.clone()), None, tree, Cancel::new());
        m.profile = Profile::TrueColor;
        m.update(Msg::Size(w as i32, h as i32));
        BenchModel { m }
    }

    /// Invalidates the data caches and renders: sorting children, LOD
    /// selection, layout and paint.
    pub fn cold_frame(&mut self) -> usize {
        self.m.invalidate();
        self.m.view().len()
    }

    /// Resizes and renders (child lists stay cached).
    pub fn resize(&mut self, w: u16, h: u16) -> usize {
        self.m.update(Msg::Size(w as i32, h as i32));
        self.m.view().len()
    }

    /// Sends a key ("right", "down", "left", "up", …) and renders.
    pub fn key(&mut self, k: &str) -> usize {
        self.m.update(Msg::Key(Key::from_name(k)));
        self.m.view().len()
    }

    /// Makes the root's child with this name the zoom root.
    pub fn zoom_to_child_named(&mut self, name: &[u8]) {
        let t = self.m.tree.clone();
        let g = t.read().unwrap();
        if let Some((id, _)) = g.children(0).find(|(_, n)| &*n.name == name) {
            self.m.zoom = id;
        }
    }

    /// Sets the query and runs the filter synchronously.
    pub fn apply_filter(&mut self, q: &str) {
        self.m.search.ed.set(q);
        let _ = self.m.query_changed();
        for msg in self.m.apply_filter().run_sync() {
            self.m.update(msg);
        }
    }
}

/// A small deterministic PRNG (splitmix64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    /// Uniform in (0, 1].
    fn unit(&mut self) -> f64 {
        ((self.next() >> 11) + 1) as f64 / (1u64 << 53) as f64
    }
    fn exp(&mut self) -> f64 {
        -self.unit().ln()
    }
    fn intn(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// ~1M nodes in memory: 200 top-level dirs with 40 subdirectories of 100
/// files each, plus one flat directory with 200k files.
pub fn big_tree() -> Tree {
    let mut r = Rng(1);
    let mut t = Tree::new(b"/big", Kind::Dir);
    let mut file = |t: &mut Tree, p: NodeId, i: usize| {
        let id = t.add(p, format!("f{i}.dat").as_bytes(), Kind::File);
        let n = t.node_mut(id);
        n.size = (r.exp() * 1e6) as i64;
        n.alloc = n.size;
        n.cat = Category::from_u8(r.intn(crate::inventory::NUM_CATEGORIES as u64) as u8);
    };
    let flat = t.add(0, b"flat", Kind::Dir);
    for i in 0..200_000 {
        file(&mut t, flat, i);
    }
    for d in 0..200 {
        let top = t.add(0, format!("dir{d}").as_bytes(), Kind::Dir);
        for s in 0..40 {
            let sub = t.add(top, format!("sub{s}").as_bytes(), Kind::Dir);
            for f in 0..100 {
                file(&mut t, sub, f);
            }
        }
    }
    t.recompute();
    t
}
