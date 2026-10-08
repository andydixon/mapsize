//! An in-memory cell grid that every view paints into; serialised once per
//! frame with minimal SGR changes.

use super::color256::{quant16, quant256};
use crate::textutil;
use crate::treemap::Rect;
use std::fmt::Write;

/// 0 for the terminal default, otherwise 0x01RRGGBB.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub(crate) struct Color(pub u32);

/// Builds a colour.
pub(crate) const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color(1 << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32)
}

impl Color {
    /// Parses "#rrggbb".
    pub fn hex(s: &str) -> Option<Color> {
        let s = s.strip_prefix('#').unwrap_or(s);
        if s.len() != 6 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(s, 16).ok().map(|v| Color(1 << 24 | v))
    }

    pub fn is_default(self) -> bool {
        self.0 == 0
    }

    pub fn r(self) -> u8 {
        (self.0 >> 16) as u8
    }
    pub fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }
    pub fn b(self) -> u8 {
        self.0 as u8
    }

    pub fn comps(self) -> (f64, f64, f64) {
        (self.r() as f64 / 255.0, self.g() as f64 / 255.0, self.b() as f64 / 255.0)
    }

    /// Blends c towards o by t (0..1). Default colours are returned as-is.
    pub fn mix(self, o: Color, t: f64) -> Color {
        if self.0 == 0 || o.0 == 0 {
            return self;
        }
        let (r1, g1, b1) = self.comps();
        let (r2, g2, b2) = o.comps();
        let f = |a: f64, b: f64| ((a + (b - a) * t) * 255.0) as u8;
        rgb(f(r1, r2), f(g1, g2), f(b1, b2))
    }

    /// Perceived brightness 0..1.
    pub fn luma(self) -> f64 {
        let (r, g, b) = self.comps();
        0.299 * r + 0.587 * g + 0.114 * b
    }
}

/// Text attribute bit set.
pub(crate) type Attr = u8;
pub(crate) const BOLD: Attr = 1;
pub(crate) const FAINT: Attr = 2;
pub(crate) const ITALIC: Attr = 4;
pub(crate) const UNDERLINE: Attr = 8;
pub(crate) const REVERSE: Attr = 16;

/// A cell style.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(crate) struct Style {
    pub fg: Color,
    pub bg: Color,
    pub attr: Attr,
}

impl Style {
    pub const fn new(fg: Color, bg: Color) -> Style {
        Style { fg, bg, attr: 0 }
    }
    pub const fn with(self, a: Attr) -> Style {
        Style { attr: self.attr | a, ..self }
    }
}

const INLINE: usize = 15;
const HEAP: u8 = 255;

/// One grid cell. The cluster is stored inline (n bytes) or, if longer,
/// in the canvas side table (n == HEAP, index in the first four bytes).
/// n == 0 marks the continuation of a wide cluster.
#[derive(Clone, Copy)]
struct Cell {
    g: [u8; INLINE],
    n: u8,
    w: u8,
    st: Style,
}

impl Cell {
    fn space(st: Style) -> Cell {
        let mut g = [0; INLINE];
        g[0] = b' ';
        Cell { g, n: 1, w: 1, st }
    }
    fn is_cont(&self) -> bool {
        self.n == 0
    }
    fn set_space(&mut self) {
        self.g[0] = b' ';
        self.n = 1;
        self.w = 1;
    }
}

/// Box glyph sets.
#[derive(Clone, Copy)]
pub(crate) struct BoxChars {
    pub h: &'static str,
    pub v: &'static str,
    pub tl: &'static str,
    pub tr: &'static str,
    pub bl: &'static str,
    pub br: &'static str,
}

pub(crate) const BOX_LIGHT: BoxChars = BoxChars { h: "─", v: "│", tl: "┌", tr: "┐", bl: "└", br: "┘" };
pub(crate) const BOX_ROUND: BoxChars = BoxChars { h: "─", v: "│", tl: "╭", tr: "╮", bl: "╰", br: "╯" };
pub(crate) const BOX_DOUBLE: BoxChars = BoxChars { h: "═", v: "║", tl: "╔", tr: "╗", bl: "╚", br: "╝" };
pub(crate) const BOX_ASCII: BoxChars = BoxChars { h: "-", v: "|", tl: "+", tr: "+", bl: "+", br: "+" };
pub(crate) const BOX_ASCII_SEL: BoxChars = BoxChars { h: "=", v: "#", tl: "#", tr: "#", bl: "#", br: "#" };

/// Selects how colours are serialised.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Depth {
    True,
    C256,
    C16,
    None, // attributes only
}

/// An in-memory cell grid.
pub(crate) struct Canvas {
    pub w: i32,
    pub h: i32,
    c: Vec<Cell>,
    big: Vec<String>,
}

impl Canvas {
    /// Creates a canvas filled with spaces in style st.
    pub fn new(w: i32, h: i32, st: Style) -> Canvas {
        let (w, h) = (w.max(0), h.max(0));
        Canvas { w, h, c: vec![Cell::space(st); (w * h) as usize], big: Vec::new() }
    }

    fn inside(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && x < self.w && y < self.h
    }

    fn cell_str<'a>(&'a self, c: &'a Cell) -> &'a str {
        if c.n == HEAP {
            let i = u32::from_le_bytes([c.g[0], c.g[1], c.g[2], c.g[3]]) as usize;
            return &self.big[i];
        }
        // Only whole UTF-8 clusters are ever stored.
        std::str::from_utf8(&c.g[..c.n as usize]).unwrap_or("?")
    }

    fn make(&mut self, s: &str, w: i32, st: Style) -> Cell {
        let mut g = [0; INLINE];
        let n = if s.len() <= INLINE {
            g[..s.len()].copy_from_slice(s.as_bytes());
            s.len() as u8
        } else {
            g[..4].copy_from_slice(&(self.big.len() as u32).to_le_bytes());
            self.big.push(s.to_string());
            HEAP
        };
        Cell { g, n, w: w as u8, st }
    }

    /// Writes one cluster of width w at (x, y), repairing any wide cluster
    /// it overlaps so the grid never holds half a character.
    fn put(&mut self, x: i32, y: i32, s: &str, w: i32, st: Style) {
        if !self.inside(x, y) || x + w > self.w {
            return;
        }
        let row = (y * self.w) as usize;
        let (xu, wu, cw) = (x as usize, w as usize, self.w as usize);
        for i in xu..xu + wu {
            if self.c[row + i].is_cont() && i > 0 && i == xu {
                // overwriting the tail of a wide cluster
                self.c[row + i - 1].set_space();
            }
            let c = self.c[row + i];
            if !c.is_cont() && c.w > 1 && i + 1 < cw && self.c[row + i + 1].is_cont() {
                // overwriting its head
                self.c[row + i + 1].set_space();
            }
        }
        self.c[row + xu] = self.make(s, w, st);
        for i in 1..wu {
            self.c[row + xu + i] = Cell { g: [0; INLINE], n: 0, w: 0, st };
        }
    }

    /// Draws s at (x, y), clipped to max_w cells and the canvas. Control
    /// characters are replaced as a second line of defence (callers sanitise
    /// untrusted strings first). It returns the number of cells used.
    pub fn text(&mut self, x: i32, y: i32, s: &str, max_w: i32, st: Style) -> i32 {
        if y < 0 || y >= self.h {
            return 0;
        }
        let limit = (x + max_w).min(self.w);
        let mut cx = x;
        textutil::clusters(s, |c, w| {
            let (mut c, mut w) = (c, w as i32);
            let b = c.as_bytes();
            if b[0] < 0x20 || b[0] == 0x7f || (b.len() > 1 && b[0] == 0xc2 && b[1] < 0xa0) {
                (c, w) = ("?", 1);
            }
            if cx + w > limit {
                // A wide cluster that does not fit leaves a blank cell.
                while cx < limit {
                    if cx >= 0 {
                        self.put(cx, y, " ", 1, st);
                    }
                    cx += 1;
                }
                return false;
            }
            if cx >= 0 {
                self.put(cx, y, c, w, st);
            }
            cx += w;
            true
        });
        cx - x
    }

    /// Draws s centred within [x, x+w).
    pub fn text_centered(&mut self, x: i32, y: i32, w: i32, s: &str, st: Style) {
        let s = textutil::truncate(s, w.max(0) as usize);
        self.text(x + (w - textutil::width(&s) as i32) / 2, y, &s, w, st);
    }

    /// Draws s right-aligned ending at x+w.
    pub fn text_right(&mut self, x: i32, y: i32, w: i32, s: &str, st: Style) {
        let s = textutil::truncate(s, w.max(0) as usize);
        self.text(x + w - textutil::width(&s) as i32, y, &s, w, st);
    }

    /// Fills r with the single-cell cluster ch.
    pub fn fill(&mut self, r: Rect, ch: &str, st: Style) {
        for y in r.y.max(0)..(r.y + r.h).min(self.h) {
            for x in r.x.max(0)..(r.x + r.w).min(self.w) {
                self.put(x, y, ch, 1, st);
            }
        }
    }

    /// Applies f to the style of every cell in r.
    pub fn restyle(&mut self, r: Rect, f: impl Fn(Style) -> Style) {
        for y in r.y.max(0)..(r.y + r.h).min(self.h) {
            for x in r.x.max(0)..(r.x + r.w).min(self.w) {
                let c = &mut self.c[(y * self.w + x) as usize];
                c.st = f(c.st);
            }
        }
    }

    /// Draws a border around r (which must be at least 2×2).
    pub fn draw_box(&mut self, r: Rect, b: BoxChars, st: Style) {
        if r.w < 2 || r.h < 2 {
            return;
        }
        let (x1, y1) = (r.x + r.w - 1, r.y + r.h - 1);
        for x in r.x + 1..x1 {
            self.put(x, r.y, b.h, 1, st);
            self.put(x, y1, b.h, 1, st);
        }
        for y in r.y + 1..y1 {
            self.put(r.x, y, b.v, 1, st);
            self.put(x1, y, b.v, 1, st);
        }
        self.put(r.x, r.y, b.tl, 1, st);
        self.put(x1, r.y, b.tr, 1, st);
        self.put(r.x, y1, b.bl, 1, st);
        self.put(x1, y1, b.br, 1, st);
    }

    /// Draws a horizontal rule.
    pub fn hline(&mut self, x: i32, y: i32, w: i32, ch: &str, st: Style) {
        for i in x..x + w {
            self.put(i, y, ch, 1, st);
        }
    }

    /// Serialises the canvas as lines of text with SGR styling. For 256 and
    /// 16 colours it quantizes with its own perceptual mapping (see
    /// color256.rs); Depth::None keeps attributes only.
    pub fn render(&self, depth: Depth) -> String {
        let mut b = String::with_capacity((self.w * self.h * 4) as usize);
        for y in 0..self.h {
            if y > 0 {
                b.push('\n');
            }
            let mut cur = Style::default();
            let mut first = true;
            for x in 0..self.w {
                let c = &self.c[(y * self.w + x) as usize];
                if c.is_cont() {
                    continue;
                }
                if first || c.st != cur {
                    sgr(&mut b, c.st, depth);
                    cur = c.st;
                    first = false;
                }
                b.push_str(self.cell_str(c));
            }
            b.push_str("\x1b[0m");
        }
        b
    }
}

fn sgr(b: &mut String, st: Style, depth: Depth) {
    b.push_str("\x1b[0");
    for (bit, code) in [(BOLD, ";1"), (FAINT, ";2"), (ITALIC, ";3"), (UNDERLINE, ";4"), (REVERSE, ";7")] {
        if st.attr & bit != 0 {
            b.push_str(code);
        }
    }
    let col = |b: &mut String, c: Color, bg: bool| {
        if c.is_default() {
            return;
        }
        let _ = match depth {
            Depth::None => Ok(()),
            Depth::C256 => write!(b, ";{};5;{}", if bg { 48 } else { 38 }, quant256(c)),
            Depth::C16 => {
                let n = quant16(c) as u32;
                let mut code = if n >= 8 { 90 + n - 8 } else { 30 + n };
                if bg {
                    code += 10;
                }
                write!(b, ";{code}")
            }
            Depth::True => write!(b, ";{};2;{};{};{}", if bg { 48 } else { 38 }, c.r(), c.g(), c.b()),
        };
    };
    col(b, st.fg, false);
    col(b, st.bg, true);
    b.push('m');
}
