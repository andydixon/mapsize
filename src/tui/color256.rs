//! The stock 256-colour conversion maps dark, slightly tinted colours to
//! saturated cube entries (olive, navy), which wrecks the subtle shading the
//! treemap relies on. quant256 picks the perceptually nearest entry (redmean
//! distance) among the cube and the grey ramp, never the palette-dependent
//! first 16 entries.

use super::canvas::Color;
use std::cell::RefCell;
use std::collections::HashMap;

const CUBE_LEVELS: [i32; 6] = [0, 95, 135, 175, 215, 255];

thread_local! {
    static Q256: RefCell<HashMap<u32, u8>> = RefCell::new(HashMap::new());
    static Q16: RefCell<HashMap<u32, u8>> = RefCell::new(HashMap::new());
}

fn redmean(r1: i32, g1: i32, b1: i32, r2: i32, g2: i32, b2: i32) -> i32 {
    let rm = (r1 + r2) / 2;
    let (dr, dg, db) = (r1 - r2, g1 - g2, b1 - b2);
    (((512 + rm) * dr * dr) >> 8) + 4 * dg * dg + (((767 - rm) * db * db) >> 8)
}

pub(crate) fn quant256(c: Color) -> u8 {
    if let Some(v) = Q256.with(|q| q.borrow().get(&c.0).copied()) {
        return v;
    }
    let (r, g, b) = (c.r() as i32, c.g() as i32, c.b() as i32);
    let (mut best, mut best_d) = (16, i32::MAX);
    for i in 0..216 {
        let (cr, cg, cb) = (
            CUBE_LEVELS[i / 36],
            CUBE_LEVELS[i / 6 % 6],
            CUBE_LEVELS[i % 6],
        );
        let d = redmean(r, g, b, cr, cg, cb);
        if d < best_d {
            (best, best_d) = (16 + i, d);
        }
    }
    for i in 0..24 {
        let v = 8 + 10 * i as i32;
        let d = redmean(r, g, b, v, v, v);
        if d < best_d {
            (best, best_d) = (232 + i, d);
        }
    }
    Q256.with(|q| q.borrow_mut().insert(c.0, best as u8));
    best as u8
}

/// Maps a colour to a basic colour. Dark shades become black and
/// unsaturated colours grey/white, so tints and shadows stay quiet;
/// saturated colours keep their hue (bright variant when light), so category
/// colours survive. The stock conversion instead turns dark tints into
/// bright yellow/cyan.
pub(crate) fn quant16(c: Color) -> u8 {
    if let Some(v) = Q16.with(|q| q.borrow().get(&c.0).copied()) {
        return v;
    }
    let (rf, gf, bf) = c.comps();
    let (mx, mn) = (rf.max(gf).max(bf), rf.min(gf).min(bf));
    let luma = c.luma();
    let n = if luma < 0.2 {
        0
    } else if mx - mn < 0.12 {
        // grey
        if luma < 0.45 {
            8
        } else if luma < 0.8 {
            7
        } else {
            15
        }
    } else {
        // Hue in sixths: red, yellow, green, cyan, blue, magenta.
        let mut h = if mx == rf {
            ((gf - bf) / (mx - mn)) % 6.0
        } else if mx == gf {
            (bf - rf) / (mx - mn) + 2.0
        } else {
            (rf - gf) / (mx - mn) + 4.0
        };
        if h < 0.0 {
            h += 6.0
        }
        let ansi = [1u8, 3, 2, 6, 4, 5][h.round() as usize % 6];
        if luma > 0.5 {
            ansi + 8
        } else {
            ansi
        }
    };
    Q16.with(|q| q.borrow_mut().insert(c.0, n));
    n
}
