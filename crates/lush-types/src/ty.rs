//! Internal type representation, schemes, and sealed constraints.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::limits::{MAX_PRINT_DEPTH, MAX_PRINT_NODES};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn fresh_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TvId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RigidId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeDefId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Constraint {
    Eq,
    Neg,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConstraintSet {
    pub eq: bool,
    pub neg: bool,
}

impl ConstraintSet {
    pub fn empty() -> Self {
        Self::default()
    }
    pub fn eq() -> Self {
        Self {
            eq: true,
            neg: false,
        }
    }
    pub fn neg() -> Self {
        Self {
            eq: false,
            neg: true,
        }
    }
    pub fn merge(&mut self, other: &ConstraintSet) {
        self.eq |= other.eq;
        self.neg |= other.neg;
    }
    pub fn is_empty(&self) -> bool {
        !self.eq && !self.neg
    }
    pub fn iter(&self) -> impl Iterator<Item = Constraint> {
        let mut v = Vec::new();
        if self.eq {
            v.push(Constraint::Eq);
        }
        if self.neg {
            v.push(Constraint::Neg);
        }
        v.into_iter()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    /// Unification variable (level-based).
    Var(TvId),
    /// Rigid / skolem variable from an annotation.
    Rigid(RigidId),
    /// Error type: unifies with anything to avoid cascades.
    Error,
    Int,
    Float,
    String,
    Bool,
    Nil,
    BitArray,
    List(Box<Type>),
    Tuple(Vec<Type>),
    Fun {
        params: Vec<Type>,
        ret: Box<Type>,
    },
    /// Nominal application of an ADT or opaque type.
    App {
        def: TypeDefId,
        args: Vec<Type>,
    },
}

impl Type {
    pub fn unit_fun() -> Type {
        Type::Fun {
            params: vec![],
            ret: Box::new(Type::Nil),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scheme {
    pub vars: Vec<TvId>,
    pub constraints: HashMap<TvId, ConstraintSet>,
    pub body: Type,
}

impl Scheme {
    pub fn mono(ty: Type) -> Self {
        Self {
            vars: vec![],
            constraints: HashMap::new(),
            body: ty,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TypeVarInfo {
    pub level: u32,
    pub link: Option<Type>,
    pub constraints: ConstraintSet,
}

#[derive(Clone, Debug)]
pub struct RigidInfo {
    pub name: String,
    pub constraints: ConstraintSet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeDefKind {
    Adt,
    Opaque,
    Alias,
    /// Built-in nominal (Result, Option, Dict, …).
    Builtin,
}

#[derive(Clone, Debug)]
pub struct VariantInfo {
    pub name: String,
    pub fields: Vec<FieldInfo>,
    pub public: bool,
}

#[derive(Clone, Debug)]
pub struct FieldInfo {
    pub label: Option<String>,
    pub ty: Type,
}

#[derive(Clone, Debug)]
pub struct TypeDefInfo {
    pub id: TypeDefId,
    pub module: String,
    pub name: String,
    pub params: Vec<String>,
    pub kind: TypeDefKind,
    pub variants: Vec<VariantInfo>,
    /// Expanded alias body (with param vars as Rigids or placeholders).
    pub alias_body: Option<Type>,
    pub public: bool,
    /// For opaque/ADT: which params require Eq for the type to be Eq.
    pub eq_params: BTreeSet<usize>,
    /// True if the type is Eq regardless of params (e.g. phantom).
    pub always_eq: bool,
    /// True if never Eq (functions inside, etc.) — computed lazily.
    pub never_eq: bool,
}

#[derive(Default)]
pub struct TypeStore {
    pub vars: HashMap<TvId, TypeVarInfo>,
    pub rigids: HashMap<RigidId, RigidInfo>,
    pub defs: HashMap<TypeDefId, TypeDefInfo>,
    pub work: u64,
}

impl TypeStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fresh_var(&mut self, level: u32) -> Type {
        let id = TvId(fresh_id());
        self.vars.insert(
            id,
            TypeVarInfo {
                level,
                link: None,
                constraints: ConstraintSet::empty(),
            },
        );
        Type::Var(id)
    }

    pub fn fresh_rigid(&mut self, name: impl Into<String>) -> Type {
        let id = RigidId(fresh_id());
        self.rigids.insert(
            id,
            RigidInfo {
                name: name.into(),
                constraints: ConstraintSet::empty(),
            },
        );
        Type::Rigid(id)
    }

    pub fn zonk(&mut self, ty: &Type) -> Type {
        self.work = self.work.saturating_add(1);
        match ty {
            Type::Var(id) => {
                if let Some(info) = self.vars.get(id) {
                    if let Some(link) = info.link.clone() {
                        let z = self.zonk(&link);
                        if let Some(info) = self.vars.get_mut(id) {
                            info.link = Some(z.clone());
                        }
                        return z;
                    }
                }
                Type::Var(*id)
            }
            Type::List(t) => Type::List(Box::new(self.zonk(t))),
            Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| self.zonk(t)).collect()),
            Type::Fun { params, ret } => Type::Fun {
                params: params.iter().map(|t| self.zonk(t)).collect(),
                ret: Box::new(self.zonk(ret)),
            },
            Type::App { def, args } => Type::App {
                def: *def,
                args: args.iter().map(|t| self.zonk(t)).collect(),
            },
            other => other.clone(),
        }
    }

    pub fn free_vars(&mut self, ty: &Type, out: &mut BTreeSet<TvId>) {
        let z = self.zonk(ty);
        match z {
            Type::Var(id) => {
                out.insert(id);
            }
            Type::List(t) => self.free_vars(&t, out),
            Type::Tuple(ts) => {
                for t in ts {
                    self.free_vars(&t, out);
                }
            }
            Type::Fun { params, ret } => {
                for p in params {
                    self.free_vars(&p, out);
                }
                self.free_vars(&ret, out);
            }
            Type::App { args, .. } => {
                for a in args {
                    self.free_vars(&a, out);
                }
            }
            _ => {}
        }
    }

    pub fn display(&mut self, ty: &Type) -> String {
        let mut names: HashMap<u64, String> = HashMap::new();
        let mut next = 0u32;
        let mut nodes = 0usize;
        self.display_inner(ty, &mut names, &mut next, 0, &mut nodes)
    }

    fn display_inner(
        &mut self,
        ty: &Type,
        names: &mut HashMap<u64, String>,
        next: &mut u32,
        depth: usize,
        nodes: &mut usize,
    ) -> String {
        *nodes += 1;
        if depth > MAX_PRINT_DEPTH || *nodes > MAX_PRINT_NODES {
            return "…".into();
        }
        let z = self.zonk(ty);
        match z {
            Type::Error => "?".into(),
            Type::Int => "Int".into(),
            Type::Float => "Float".into(),
            Type::String => "String".into(),
            Type::Bool => "Bool".into(),
            Type::Nil => "Nil".into(),
            Type::BitArray => "BitArray".into(),
            Type::Var(id) => self.var_name(id.0, names, next),
            Type::Rigid(id) => {
                if let Some(info) = self.rigids.get(&id) {
                    info.name.clone()
                } else {
                    self.var_name(id.0, names, next)
                }
            }
            Type::List(t) => {
                format!(
                    "List({})",
                    self.display_inner(&t, names, next, depth + 1, nodes)
                )
            }
            Type::Tuple(ts) => {
                let parts: Vec<_> = ts
                    .iter()
                    .map(|t| self.display_inner(t, names, next, depth + 1, nodes))
                    .collect();
                format!("#({})", parts.join(", "))
            }
            Type::Fun { params, ret } => {
                let ps: Vec<_> = params
                    .iter()
                    .map(|t| self.display_inner(t, names, next, depth + 1, nodes))
                    .collect();
                format!(
                    "fn({}) -> {}",
                    ps.join(", "),
                    self.display_inner(&ret, names, next, depth + 1, nodes)
                )
            }
            Type::App { def, args } => {
                let name = self
                    .defs
                    .get(&def)
                    .map(|d| {
                        if d.module.is_empty() || d.module == "prelude" {
                            d.name.clone()
                        } else {
                            // Prefer unqualified when printing locally; qualified form
                            // is filled in by the checker when needed.
                            d.name.clone()
                        }
                    })
                    .unwrap_or_else(|| format!("Type{}", def.0));
                if args.is_empty() {
                    name
                } else {
                    let parts: Vec<_> = args
                        .iter()
                        .map(|t| self.display_inner(t, names, next, depth + 1, nodes))
                        .collect();
                    format!("{name}({})", parts.join(", "))
                }
            }
        }
    }

    fn var_name(&self, id: u64, names: &mut HashMap<u64, String>, next: &mut u32) -> String {
        names
            .entry(id)
            .or_insert_with(|| {
                let n = *next;
                *next += 1;
                tvar_name(n)
            })
            .clone()
    }

    pub fn display_scheme(&mut self, scheme: &Scheme) -> String {
        let mut s = self.display(&scheme.body);
        let mut where_parts = Vec::new();
        // Collect constraints on quantified vars in order.
        for v in &scheme.vars {
            if let Some(c) = scheme.constraints.get(v) {
                let name = {
                    let mut names: HashMap<u64, String> = HashMap::new();
                    let mut next = 0u32;
                    // Force display of just this var to get a stable name via full scheme display.
                    let _ = (&mut names, &mut next);
                    // Use zonked display mapping: re-display body to populate names.
                    let _ = self.display(&scheme.body);
                    format!("{}", v.0) // fallback; improved below
                };
                let _ = name;
                if c.eq {
                    where_parts.push(format!("Eq({})", self.display(&Type::Var(*v))));
                }
                if c.neg {
                    where_parts.push(format!("Neg({})", self.display(&Type::Var(*v))));
                }
            }
        }
        // Also rigid constraints attached during inference may live on the store.
        if !where_parts.is_empty() {
            // Dedup while preserving order
            let mut seen = BTreeSet::new();
            where_parts.retain(|p| seen.insert(p.clone()));
            s.push_str(" where ");
            s.push_str(&where_parts.join(", "));
        }
        s
    }
}

fn tvar_name(n: u32) -> String {
    // a, b, … z, a1, b1, …
    let letter = (b'a' + (n % 26) as u8) as char;
    let cycle = n / 26;
    if cycle == 0 {
        letter.to_string()
    } else {
        format!("{letter}{cycle}")
    }
}

impl fmt::Display for Constraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Constraint::Eq => write!(f, "Eq"),
            Constraint::Neg => write!(f, "Neg"),
        }
    }
}
