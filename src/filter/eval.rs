use super::parser::{Field, Query};
use crate::inventory::{Flags, Node, NodeId, Tree};
use crate::platform;
use regex::Regex;
use std::borrow::Cow;

/// The evaluation context for one node. The tree must be read-locked.
pub struct Ctx<'a> {
    pub tree: &'a Tree,
    pub id: NodeId,
    pub node: &'a Node,
    pub now: i64, // unix nanoseconds
    // The lower-cased path of the node once has_path is set; the buffers are
    // reused across nodes so the per-node cost is one copy, not allocations.
    pub(super) path: String,
    pub(super) has_path: bool,
    name: String,
    // dir_paths caches lower-cased directory paths (by NodeId; empty for
    // files) during apply, which visits parents before children, so a node's
    // path is one concatenation.
    pub(super) dir_paths: Option<Vec<String>>,
}

/// Appends name lower-cased, without allocating for ASCII names.
fn push_lower(buf: &mut String, name: &[u8]) {
    if name.is_ascii() {
        buf.extend(name.iter().map(|b| b.to_ascii_lowercase() as char));
    } else {
        buf.push_str(&String::from_utf8_lossy(name).to_lowercase());
    }
}

impl<'a> Ctx<'a> {
    pub fn new(tree: &'a Tree, id: NodeId, now: i64) -> Ctx<'a> {
        Ctx {
            tree,
            id,
            node: tree.node(id),
            now,
            path: String::new(),
            has_path: false,
            name: String::new(),
            dir_paths: None,
        }
    }

    /// Moves the context to another node.
    pub(super) fn reset(&mut self, id: NodeId) {
        self.id = id;
        self.node = self.tree.node(id);
        self.has_path = false;
    }

    fn str(&mut self, f: Field) -> Cow<'_, str> {
        let n = self.node;
        match f {
            Field::Name => {
                self.name.clear();
                push_lower(&mut self.name, &n.name);
                self.name.as_str().into()
            }
            Field::Path => {
                if !self.has_path {
                    self.fill_path();
                }
                self.path.as_str().into()
            }
            Field::Ext => self.tree.ext_name(n).into(),
            Field::Type => n.kind.name().to_lowercase().into(),
            Field::Category if n.is_dir() => "directory".into(),
            Field::Category => n.cat.name().to_lowercase().into(),
            Field::Owner => platform::user_name(n.uid).to_lowercase().into(),
            Field::Group => platform::group_name(n.gid).to_lowercase().into(),
            _ => "".into(),
        }
    }

    fn num(&self, f: Field) -> i64 {
        let n = self.node;
        match f {
            Field::Size => n.tot_size,
            Field::Allocated => n.tot_alloc,
            Field::Age => self.now.saturating_sub(n.mtime),
            Field::Modified => n.mtime,
            Field::Files => n.files as i64,
            _ => 0,
        }
    }

    /// Computes the lower-cased path into self.path.
    pub(super) fn fill_path(&mut self) {
        self.has_path = true;
        self.path.clear();
        if let Some(pp) = self
            .dir_paths
            .as_ref()
            .and_then(|dp| dp.get(self.node.parent as usize))
            .filter(|p| !p.is_empty())
        {
            self.path.push_str(pp);
            if !pp.ends_with('/') {
                self.path.push('/');
            }
            push_lower(&mut self.path, &self.node.name);
            return;
        }
        self.path
            .push_str(&self.tree.path_string(self.id).to_lowercase());
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum NumOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

/// A compiled query expression.
pub(super) enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Contains(Field, String),
    Glob(Field, String),
    Re(Field, Regex),
    Eq(Field, Vec<String>),
    Flag(Flags), // any of these flags set
    Num(Field, NumOp, Vec<i64>),
    False, // malformed bare glob
}

impl Expr {
    fn eval(&self, c: &mut Ctx) -> bool {
        match self {
            Expr::And(l, r) => l.eval(c) && r.eval(c),
            Expr::Or(l, r) => l.eval(c) || r.eval(c),
            Expr::Not(n) => !n.eval(c),
            Expr::Contains(f, sub) => c.str(*f).contains(sub.as_str()),
            Expr::Glob(f, pat) => platform::glob_match(pat.as_bytes(), c.str(*f).as_bytes()),
            Expr::Re(f, re) => re.is_match(&c.str(*f)),
            Expr::Eq(f, set) => {
                let v = c.str(*f);
                set.iter().any(|s| *s == v)
            }
            Expr::Flag(mask) => c.node.flags & mask != 0,
            Expr::Num(f, op, vals) => {
                if matches!(f, Field::Age | Field::Modified) && c.node.mtime == 0 {
                    return false; // unknown time never matches
                }
                let v = c.num(*f);
                vals.iter().any(|&x| match op {
                    NumOp::Eq => v == x,
                    NumOp::Ne => v != x,
                    NumOp::Gt => v > x,
                    NumOp::Ge => v >= x,
                    NumOp::Lt => v < x,
                    NumOp::Le => v <= x,
                })
            }
            Expr::False => false,
        }
    }
}

impl Query {
    /// Evaluates the query against one node.
    pub fn matches(&self, c: &mut Ctx) -> bool {
        self.root.eval(c)
    }
}
