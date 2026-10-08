//! Writes inventories as JSON, CSV or plain-text reports. Callers hold the
//! tree's read lock (or own the tree exclusively).

use crate::brand;
use crate::inventory::{self as inv, Flags, Kind, NodeId, SizeMode, Tree, NO_NODE};
use crate::textutil;
use std::io::{self, Write};
use std::time::UNIX_EPOCH;

const FLAG_NAMES: [(Flags, &str); 12] = [
    (inv::FLAG_HARDLINK_DUP, "hardlink-duplicate"),
    (inv::FLAG_MOUNT_POINT, "mount-point"),
    (inv::FLAG_VIRTUAL_FS, "virtual-fs-skipped"),
    (inv::FLAG_SKIPPED_FS, "other-fs-skipped"),
    (inv::FLAG_SPARSE, "sparse"),
    (inv::FLAG_ERROR, "error"),
    (inv::FLAG_INCOMPLETE, "incomplete"),
    (inv::FLAG_LOOP, "loop-skipped"),
    (inv::FLAG_FOLLOWED, "followed-symlink"),
    (inv::FLAG_ALLOC_UNKNOWN, "allocated-unknown"),
    (inv::FLAG_BROKEN_LINK, "broken-symlink"),
    (inv::FLAG_HARDLINKED, "hardlinked"),
];

/// Human-readable names for a node's flags.
pub fn flag_list(f: Flags) -> Vec<&'static str> {
    FLAG_NAMES
        .iter()
        .filter(|(b, _)| f & b != 0)
        .map(|(_, n)| *n)
        .collect()
}

fn kind_name(k: Kind) -> &'static str {
    ["file", "dir", "symlink", "other"][k as usize]
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

/// Writes the tree beneath root as nested JSON, down to max_depth levels
/// (negative means unlimited).
pub fn json(w: &mut dyn Write, t: &Tree, root: NodeId, max_depth: i64) -> io::Result<()> {
    let mut bw = io::BufWriter::new(w);
    let st = &t.stats;
    let errors: serde_json::Map<String, serde_json::Value> = st
        .err_counts
        .iter()
        .enumerate()
        .filter(|(_, &c)| c > 0)
        .map(|(k, &c)| (inv::ErrKind::from_u8(k as u8).name().to_string(), c.into()))
        .collect();
    let started = st
        .start
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64);
    let head = serde_json::json!({
        "generator": format!("{} {}", brand::NAME, brand::VERSION),
        "root": t.path_string(root),
        "complete": !st.incomplete(),
        "cancelled": st.cancelled,
        "started": textutil::rfc3339_utc(started),
        "elapsed_s": st.elapsed().as_secs_f64(),
        "errors": errors,
        "excluded": st.excluded,
        "excludes": st.excludes,
        "hardlink_duplicates": st.hardlink_dups,
    });
    let hb = serde_json::to_string(&head)?;
    bw.write_all(&hb.as_bytes()[..hb.len() - 1])?;
    bw.write_all(b",\"tree\":")?;
    write_node(&mut bw, t, root, max_depth)?;
    bw.write_all(b"}\n")?;
    bw.flush()
}

/// Streams one node; children are written recursively so memory stays
/// proportional to depth, not tree size.
fn write_node(bw: &mut impl Write, t: &Tree, id: NodeId, depth: i64) -> io::Result<()> {
    let n = t.node(id);
    write!(
        bw,
        "{{\"name\":{},\"type\":\"{}\",\"size\":{},\"allocated\":{}",
        json_str(&n.name_str()),
        kind_name(n.kind),
        n.tot_size,
        n.tot_alloc
    )?;
    for (k, v) in [("files", n.files), ("dirs", n.dirs), ("errors", n.errors)] {
        if v != 0 {
            write!(bw, ",\"{k}\":{v}")?;
        }
    }
    if n.mtime != 0 {
        write!(bw, ",\"modified\":\"{}\"", textutil::rfc3339_utc(n.mtime))?;
    }
    let flags = flag_list(n.flags);
    if !flags.is_empty() {
        write!(bw, ",\"flags\":{}", serde_json::to_string(&flags)?)?;
    }
    if n.first_child == NO_NODE || depth == 0 {
        return bw.write_all(b"}");
    }
    bw.write_all(b",\"children\":[")?;
    for (i, c) in t
        .sorted_children(id, SizeMode::Allocated)
        .into_iter()
        .enumerate()
    {
        if i > 0 {
            bw.write_all(b",")?;
        }
        write_node(bw, t, c, depth - 1)?;
    }
    bw.write_all(b"]}")
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) || s.starts_with(' ') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Writes one row per node beneath root (depth-limited). Paths are
/// sanitised so the output is safe to print to a terminal.
pub fn csv(w: &mut dyn Write, t: &Tree, root: NodeId, max_depth: i64) -> io::Result<()> {
    let mut bw = io::BufWriter::new(w);
    bw.write_all(b"path,type,size,allocated,files,dirs,errors,modified,flags\n")?;
    fn rec(bw: &mut impl Write, t: &Tree, id: NodeId, path: &[u8], depth: i64) -> io::Result<()> {
        let n = t.node(id);
        let modified = if n.mtime != 0 {
            textutil::rfc3339_utc(n.mtime)
        } else {
            String::new()
        };
        writeln!(
            bw,
            "{},{},{},{},{},{},{},{},{}",
            csv_field(&textutil::sanitize_bytes(path)),
            kind_name(n.kind),
            n.tot_size,
            n.tot_alloc,
            n.files,
            n.dirs,
            n.errors,
            modified,
            flag_list(n.flags).join("|")
        )?;
        if depth == 0 {
            return Ok(());
        }
        for c in t.sorted_children(id, SizeMode::Allocated) {
            rec(
                bw,
                t,
                c,
                &crate::platform::join_path(path, &t.node(c).name),
                depth - 1,
            )?;
        }
        Ok(())
    }
    rec(&mut bw, t, root, &t.path_bytes(root), max_depth)?;
    bw.flush()
}

/// Writes "size  %  files  path" rows for the given nodes.
pub fn table(w: &mut dyn Write, t: &Tree, ids: &[NodeId], m: SizeMode) -> io::Result<()> {
    let total = t.node(t.root()).total(m);
    writeln!(w, "{:>12} {:>6} {:>12}  PATH", "SIZE", "%", "FILES")?;
    for &id in ids {
        let n = t.node(id);
        let files = if n.is_dir() {
            textutil::count(n.files as i64)
        } else {
            String::new()
        };
        writeln!(
            w,
            "{:>12} {:>6} {:>12}  {}",
            textutil::size(n.total(m)),
            textutil::percent(n.total(m), total),
            files,
            textutil::sanitize_bytes(&t.path_bytes(id))
        )?;
    }
    Ok(())
}

/// Writes the scan summary, making incompleteness explicit.
pub fn summary(w: &mut dyn Write, t: &Tree) -> io::Result<()> {
    let st = &t.stats;
    let r = t.node(t.root());
    writeln!(w, "Root            {}", textutil::sanitize_bytes(&st.root))?;
    writeln!(
        w,
        "Files           {}",
        textutil::count(st.files + st.symlinks + st.others)
    )?;
    writeln!(w, "Directories     {}", textutil::count(st.dirs))?;
    writeln!(
        w,
        "Logical size    {} ({} bytes)",
        textutil::size(r.tot_size),
        r.tot_size
    )?;
    writeln!(
        w,
        "Allocated size  {} ({} bytes)",
        textutil::size(r.tot_alloc),
        r.tot_alloc
    )?;
    writeln!(w, "Elapsed         {}", textutil::go_duration(st.elapsed()))?;
    for (k, &c) in st.err_counts.iter().enumerate() {
        if c > 0 {
            writeln!(
                w,
                "{:<15} {}",
                inv::ErrKind::from_u8(k as u8).name(),
                textutil::count(c)
            )?;
        }
    }
    if st.excluded > 0 {
        writeln!(
            w,
            "Excluded        {} entries ({})",
            textutil::count(st.excluded),
            textutil::sanitize(&st.excludes.join(", "))
        )?;
    }
    if st.skipped_mounts > 0 {
        writeln!(w, "Other FS skipped {}", textutil::count(st.skipped_mounts))?;
    }
    if st.virtual_skipped > 0 {
        writeln!(
            w,
            "Virtual FS skipped {}",
            textutil::count(st.virtual_skipped)
        )?;
    }
    if st.hardlink_dups > 0 {
        writeln!(
            w,
            "Hard-link dups  {} (counted once)",
            textutil::count(st.hardlink_dups)
        )?;
    }
    if st.incomplete() {
        writeln!(
            w,
            "WARNING: totals are incomplete — some data could not be inspected."
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hostile_tree() -> Tree {
        let mut t = Tree::new(b"/r", Kind::Dir);
        let d = t.add(0, b"esc\x1b[2Jdir", Kind::Dir);
        let f = t.add(d, b"nl\nname\x07.txt", Kind::File);
        t.node_mut(f).size = 10;
        t.node_mut(f).alloc = 4096;
        t.stats.complete = true;
        t.recompute();
        t
    }

    #[test]
    fn outputs_are_terminal_safe() {
        let t = hostile_tree();
        for out in [
            {
                let mut b = Vec::new();
                json(&mut b, &t, 0, -1).unwrap();
                b
            },
            {
                let mut b = Vec::new();
                csv(&mut b, &t, 0, -1).unwrap();
                b
            },
            {
                let mut b = Vec::new();
                table(&mut b, &t, &[1, 2], SizeMode::Allocated).unwrap();
                b
            },
        ] {
            assert!(
                !out.iter().any(|&c| c == 0x1b || c == 0x07),
                "{}",
                String::from_utf8_lossy(&out)
            );
        }
        let mut b = Vec::new();
        json(&mut b, &t, 0, -1).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(v["tree"]["children"][0]["children"][0]["size"], 10);
    }

    #[test]
    fn json_depth_limit() {
        let t = hostile_tree();
        let mut b = Vec::new();
        json(&mut b, &t, 0, 1).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
        assert!(v["tree"]["children"][0].get("children").is_none());
    }
}
