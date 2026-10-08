//! Colour themes. Nothing else in the TUI hard-codes colours.

use super::canvas::{rgb, BoxChars, Color, Style, BOX_ASCII, BOX_ASCII_SEL, BOX_DOUBLE, BOX_LIGHT};
use crate::config::Settings;
use crate::inventory::{parse_category, NUM_CATEGORIES};
use std::collections::HashMap;

/// Every colour the UI uses.
#[derive(Clone, Default)]
pub(crate) struct Theme {
    pub name: &'static str,
    pub mono: bool,  // no colour: rely on attributes and glyphs
    pub ascii: bool, // no box-drawing glyphs

    pub bg: Color,
    pub fg: Color,
    pub muted: Color,
    pub faint: Color,
    pub accent: Color,
    pub sel: Color,
    pub header_bg: Color,
    pub header_fg: Color,
    pub status_bg: Color,
    pub status_fg: Color,
    pub panel_bg: Color,
    pub warn: Color,
    pub err: Color,
    pub ok: Color,
    pub group: Color,
    pub empty_dir: Color,
    pub cat: [Color; NUM_CATEGORIES],
    pub diff_grow: Color,
    pub diff_shrink: Color,
    pub diff_new: Color,
    pub diff_same: Color,
    pub ext: Option<HashMap<String, Color>>,
}

/// Lists built-in themes (sorted).
pub(crate) fn theme_names() -> Vec<&'static str> {
    vec!["dark", "default", "high-contrast", "mono"]
}

fn default_theme() -> Theme {
    Theme {
        name: "default",
        bg: rgb(0x16, 0x18, 0x1d),
        fg: rgb(0xdc, 0xdf, 0xe4),
        muted: rgb(0x8a, 0x91, 0x9c),
        faint: rgb(0x4a, 0x50, 0x5a),
        accent: rgb(0x61, 0xaf, 0xef),
        sel: rgb(0xff, 0xd8, 0x66),
        header_bg: rgb(0x21, 0x25, 0x2b),
        header_fg: rgb(0xdc, 0xdf, 0xe4),
        status_bg: rgb(0x2c, 0x31, 0x3a),
        status_fg: rgb(0xc8, 0xcc, 0xd4),
        panel_bg: rgb(0x1c, 0x1f, 0x25),
        warn: rgb(0xe5, 0xc0, 0x7b),
        err: rgb(0xe0, 0x6c, 0x75),
        ok: rgb(0x98, 0xc3, 0x79),
        group: rgb(0x5c, 0x63, 0x70),
        empty_dir: rgb(0x4b, 0x52, 0x63),
        // Other, Image, Video, Audio, Archive, DiskImage, Database, Code,
        // Document, Executable, Log, Cache
        cat: [
            rgb(0x7f, 0x8c, 0x9d),
            rgb(0xe5, 0xc0, 0x7b),
            rgb(0xe0, 0x6c, 0x75),
            rgb(0xd1, 0x9a, 0x66),
            rgb(0xc6, 0x78, 0xdd),
            rgb(0x8e, 0x7c, 0xf0),
            rgb(0x56, 0xb6, 0xc2),
            rgb(0x98, 0xc3, 0x79),
            rgb(0x61, 0xaf, 0xef),
            rgb(0xbe, 0x50, 0x46),
            rgb(0xa3, 0xa8, 0x6a),
            rgb(0x6b, 0x77, 0x88),
        ],
        diff_grow: rgb(0xf0, 0x5a, 0x5a),
        diff_shrink: rgb(0x5a, 0xd0, 0x7a),
        diff_new: rgb(0x4f, 0xa8, 0xff),
        diff_same: rgb(0x50, 0x56, 0x60),
        ..Theme::default()
    }
}

fn dark_theme() -> Theme {
    let mut t = default_theme();
    t.name = "dark";
    (t.bg, t.panel_bg) = (rgb(0, 0, 0), rgb(0x0c, 0x0c, 0x0e));
    (t.header_bg, t.status_bg) = (rgb(0x12, 0x12, 0x16), rgb(0x1a, 0x1a, 0x20));
    for c in t.cat.iter_mut() {
        *c = c.mix(rgb(0, 0, 0), 0.2);
    }
    t
}

fn high_contrast_theme() -> Theme {
    let mut t = default_theme();
    t.name = "high-contrast";
    (t.bg, t.fg, t.muted) = (rgb(0, 0, 0), rgb(0xff, 0xff, 0xff), rgb(0xd0, 0xd0, 0xd0));
    (t.header_bg, t.status_bg, t.panel_bg) = (rgb(0, 0, 0), rgb(0x20, 0x20, 0x20), rgb(0, 0, 0));
    (t.sel, t.accent) = (rgb(0xff, 0xff, 0x00), rgb(0x00, 0xff, 0xff));
    t.cat = [
        rgb(0xb0, 0xb0, 0xb0),
        rgb(0xff, 0xd7, 0x00),
        rgb(0xff, 0x30, 0x30),
        rgb(0xff, 0x8c, 0x00),
        rgb(0xff, 0x00, 0xff),
        rgb(0x9b, 0x7b, 0xff),
        rgb(0x00, 0xe5, 0xff),
        rgb(0x00, 0xff, 0x00),
        rgb(0x40, 0x9c, 0xff),
        rgb(0xff, 0x55, 0x55),
        rgb(0xc8, 0xff, 0x00),
        rgb(0x90, 0x90, 0xa0),
    ];
    t
}

fn mono_theme() -> Theme {
    Theme {
        name: "mono",
        mono: true,
        ..Theme::default()
    }
}

/// Returns a built-in theme with user overrides applied.
pub(crate) fn load_theme(name: &str, s: &Settings) -> Theme {
    let mut t = match name.to_lowercase().as_str() {
        "dark" => dark_theme(),
        "high-contrast" => high_contrast_theme(),
        "mono" => mono_theme(),
        _ => default_theme(),
    };
    if t.mono {
        return t;
    }
    for (k, v) in &s.category_colors {
        if let (Some(c), Some(col)) = (parse_category(k), Color::hex(v)) {
            t.cat[c as usize] = col;
        }
    }
    for (k, v) in &s.extension_colors {
        if let Some(col) = Color::hex(v) {
            let key = k.strip_prefix('.').unwrap_or(k).to_lowercase();
            t.ext.get_or_insert_with(HashMap::new).insert(key, col);
        }
    }
    t
}

impl Theme {
    pub fn base(&self) -> Style {
        Style::new(self.fg, self.bg)
    }
    pub fn muted(&self) -> Style {
        Style::new(self.muted, self.bg)
    }
    pub fn header(&self) -> Style {
        Style::new(self.header_fg, self.header_bg)
    }
    pub fn status(&self) -> Style {
        Style::new(self.status_fg, self.status_bg)
    }
    pub fn panel(&self) -> Style {
        Style::new(self.fg, self.panel_bg)
    }

    pub fn box_chars(&self) -> BoxChars {
        if self.ascii {
            BOX_ASCII
        } else {
            BOX_LIGHT
        }
    }

    pub fn sel_box(&self) -> BoxChars {
        if self.ascii {
            BOX_ASCII_SEL
        } else {
            BOX_DOUBLE
        }
    }

    /// Picks a readable text colour for a background.
    pub fn text_on(&self, bg: Color) -> Color {
        if bg.is_default() {
            return self.fg;
        }
        if bg.luma() > 0.55 {
            return rgb(0x10, 0x12, 0x16);
        }
        rgb(0xf2, 0xf4, 0xf7)
    }
}
