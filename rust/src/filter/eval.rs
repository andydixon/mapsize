use super::parser::{Field, Query};
use crate::inventory::{Flags, Node, NodeId, Tree};
use crate::platform;
use regex::Regex;
use std::borrow::Cow;
use std::collections::HashMap;

/// The evaluation context for one node. The tree must be read-locked.
pub struct Ctx<'a> {
    pub tree: &'a Tree,
    pub id: NodeId,
    pub node: &'a Node,
    pub now: i64, // unix nanoseconds
    pub(super) path: Option<String>,
    // dir_paths caches lower-cased directory paths during apply, which visits
    // parents before children, so a node's path is one concatenation.
    pub(super) dir_paths: Option<HashMap<NodeId, String>>,
}

impl<'a> Ctx<'a> {
    pub fn new(tree: &'a Tree, id: NodeId, now: i64) -> Ctx<'a> {
        Ctx { tree, id, node: tree.node(id), now, path: None, dir_paths: None }
    }

    fn str(&mut self, f: Field) -> Cow<'_, str> {
        let n = self.node;
        match f {
            Field::Name => n.name_str().to_lowercase().into(),
            Field::Path => {
                if self.path.is_none() {
                    self.path = Some(self.lower_path());
                }
                self.path.as_deref().unwrap().into()
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

    pub(super) fn lower_path(&self) -> String {
        if let Some(dp) = &self.dir_paths {
            if let Some(p) = dp.get(&self.id) {
                return p.clone();
            }
            if let Some(pp) = dp.get(&self.node.parent) {
                let sep = if pp.ends_with('/') { "" } else { "/" };
                return format!("{pp}{sep}{}", self.node.name_str().to_lowercase());
            }
        }
        self.tree.path_string(self.id).to_lowercase()
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
