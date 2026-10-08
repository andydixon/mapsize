//! The search/filter query language:
//!
//! ```text
//! query   := or
//! or      := and { OR and }
//! and     := unary { [AND] unary }        (juxtaposition means AND)
//! unary   := NOT unary | "(" query ")" | cmp | word
//! cmp     := field op value
//! op      := = | != | > | >= | < | <= | contains | matches | in | ~
//! value   := word | "quoted string" | "(" value { "," value } ")"
//! ```
//!
//! A bare word matches the name: substring (case-insensitive), or a glob if it
//! contains * ? or [. Parsing is independent of the UI and fully testable.

mod apply;
mod eval;
mod lexer;
mod parser;

pub use apply::{apply, FilterResult};
pub use eval::Ctx;
pub use parser::{parse, parse_age, parse_size, Field, Query, FIELDS};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancel::Cancel;
    use crate::inventory::{category_for, ext, Kind, NodeId, SizeMode, Tree};
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const HOUR: Duration = Duration::from_secs(3600);
    const DAY: Duration = Duration::from_secs(86400);

    fn tree() -> Tree {
        let mut t = Tree::new(b"/srv", Kind::Dir);
        let vms = t.add(0, b"vms", Kind::Dir);
        let cache = t.add(0, b"cache", Kind::Dir);
        let mut add = |p: NodeId, name: &str, size: i64, age: Duration| {
            let id = t.add(p, name.as_bytes(), Kind::File);
            let n = t.node_mut(id);
            n.size = size;
            n.alloc = size;
            n.mtime = (SystemTime::now() - age).duration_since(UNIX_EPOCH).unwrap().as_nanos() as i64;
            n.cat = category_for(&ext(name.as_bytes()), false, false);
        };
        add(vms, "ubuntu.qcow2", 80 << 30, 400 * DAY);
        add(vms, "win.vmdk", 40 << 30, HOUR);
        add(vms, "notes.txt", 1000, HOUR);
        add(cache, "blob.iso", 700 << 20, 10 * DAY);
        add(cache, "x.tmp", 10, HOUR);
        t.recompute();
        t
    }

    fn names(tr: &Tree, q: &str) -> HashSet<String> {
        let pq = parse(q, SystemTime::now()).unwrap_or_else(|e| panic!("parse({q:?}): {e}"));
        let r = apply(&Cancel::new(), tr, Arc::new(pq), SizeMode::Logical).unwrap();
        (0..tr.len() as NodeId).filter(|&i| r.is_match(i)).map(|i| tr.node(i).name_str().into_owned()).collect()
    }

    #[test]
    fn queries() {
        let tr = tree();
        let cases: &[(&str, &[&str])] = &[
            ("ubuntu", &["ubuntu.qcow2"]),
            ("*.iso", &["blob.iso"]),
            ("size > 1GB", &["vms", "ubuntu.qcow2", "win.vmdk"]),
            ("size > 1GB AND type = file", &["ubuntu.qcow2", "win.vmdk"]),
            ("ext = qcow2", &["ubuntu.qcow2"]),
            ("size > 500MB AND ext IN (iso,qcow2,vmdk)", &["ubuntu.qcow2", "win.vmdk", "blob.iso"]),
            ("path contains cache", &["cache", "blob.iso", "x.tmp"]),
            ("age > 365d", &["ubuntu.qcow2"]),
            ("NOT type = dir AND size < 1k", &["notes.txt", "x.tmp"]),
            ("(ext = iso OR ext = tmp) type=file", &["blob.iso", "x.tmp"]),
            ("category = vm", &["ubuntu.qcow2", "win.vmdk", "blob.iso"]),
            ("name matches '^w.*k$'", &["win.vmdk"]),
            ("ext != qcow2 && type = file && size > 1GB", &["win.vmdk"]),
            ("name = 'notes.txt'", &["notes.txt"]),
        ];
        for (q, want) in cases {
            let want: HashSet<String> = want.iter().map(|s| s.to_string()).collect();
            assert_eq!(names(&tr, q), want, "{q:?}");
        }
    }

    #[test]
    fn filtered_sizes_no_double_count() {
        let tr = tree();
        let q = parse("name = vms OR ext = qcow2", SystemTime::now()).unwrap();
        let r = apply(&Cancel::new(), &tr, Arc::new(q), SizeMode::Logical).unwrap();
        assert_eq!(r.total, tr.node(1).tot_size, "matching dir must not double count matching child");
    }

    #[test]
    fn parse_errors() {
        for q in [
            "", "size >", "size > banana", "(ext = iso", "ext = iso)", "size contains 5",
            "name matches '['", "type = blob", "'unterminated", "a & b", "category = nope",
        ] {
            assert!(parse(q, SystemTime::now()).is_err(), "parse({q:?}) should fail");
        }
    }

    #[test]
    fn dates_globs_and_non_ascii() {
        let tr = tree();
        assert_eq!(names(&tr, "modified < 2000-01-01"), HashSet::new());
        assert_eq!(names(&tr, "modified > '2000-01-01 9:30'").len(), 5);
        assert_eq!(names(&tr, "modified > 2000-01-01T00:00:00.5+01:00").len(), 5);
        assert_eq!(names(&tr, "modified > 30d").len(), 4);
        for bad in ["modified > 2024-02-30", "modified > 2024-01-31T10:00", "ext = 'a['"] {
            assert!(parse(bad, SystemTime::now()).is_ok() == bad.ends_with("10:00"), "{bad}");
        }
        assert!(names(&tr, "[").is_empty()); // malformed bare glob matches nothing
        assert!(names(&tr, "voilà\u{a0}x").is_empty()); // Go's lexer hangs on byte 0xA0
    }

    #[test]
    fn parse_size_units() {
        for (input, want) in [("1GB", 1_000_000_000), ("1GiB", 1 << 30), ("1.5k", 1536), ("500MB", 500_000_000), ("10", 10)] {
            assert_eq!(parse_size(input), Ok(want), "{input}");
        }
    }
}
