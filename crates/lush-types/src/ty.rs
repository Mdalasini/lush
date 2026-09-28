//! Internal type representation, schemes, and sealed constraints.
//!
//! Compound types use [`Rc`] so DAG-shaped types (e.g. let-doubling) share
//! structure; zonk / free_vars / display walk with a pointer-keyed memo.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::rc::Rc;
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
    List(Rc<Type>),
    Tuple(Vec<Rc<Type>>),
    Fun {
        params: Vec<Rc<Type>>,
        ret: Rc<Type>,
    },
    /// Nominal application of an ADT or opaque type.
    App {
        def: TypeDefId,
        args: Vec<Rc<Type>>,
    },
}

impl Type {
    pub fn unit_fun() -> Type {
        Type::Fun {
            params: vec![],
            ret: Rc::new(Type::Nil),
        }
    }

    pub fn list(elem: Type) -> Type {
        Type::List(Rc::new(elem))
    }

    pub fn tuple(elems: Vec<Type>) -> Type {
        Type::Tuple(elems.into_iter().map(Rc::new).collect())
    }

    pub fn tuple_shared(elems: Vec<Rc<Type>>) -> Type {
        Type::Tuple(elems)
    }

    pub fn fun(params: Vec<Type>, ret: Type) -> Type {
        Type::Fun {
            params: params.into_iter().map(Rc::new).collect(),
            ret: Rc::new(ret),
        }
    }

    pub fn app(def: TypeDefId, args: Vec<Type>) -> Type {
        Type::App {
            def,
            args: args.into_iter().map(Rc::new).collect(),
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
        let mut memo: HashMap<*const Type, Rc<Type>> = HashMap::new();
        (*self.zonk_rc(&Rc::new(ty.clone()), &mut memo)).clone()
    }

    /// Zonk a shared type node, memoising by `Rc` pointer so DAG walks are linear.
    pub fn zonk_rc(
        &mut self,
        ty: &Rc<Type>,
        memo: &mut HashMap<*const Type, Rc<Type>>,
    ) -> Rc<Type> {
        let ptr = Rc::as_ptr(ty);
        if let Some(z) = memo.get(&ptr) {
            return z.clone();
        }
        let result = match ty.as_ref() {
            Type::Var(id) => {
                if let Some(info) = self.vars.get(id) {
                    if let Some(link) = info.link.clone() {
                        let z = self.zonk_rc(&Rc::new(link), memo);
                        if let Some(info) = self.vars.get_mut(id) {
                            info.link = Some((*z).clone());
                        }
                        // Don't insert the Var node's ptr → linked type; callers see the link.
                        return z;
                    }
                }
                ty.clone()
            }
            Type::List(t) => {
                let zt = self.zonk_rc(t, memo);
                if Rc::ptr_eq(&zt, t) {
                    ty.clone()
                } else {
                    Rc::new(Type::List(zt))
                }
            }
            Type::Tuple(ts) => {
                let mut changed = false;
                let mut zs = Vec::with_capacity(ts.len());
                for t in ts {
                    let z = self.zonk_rc(t, memo);
                    if !Rc::ptr_eq(&z, t) {
                        changed = true;
                    }
                    zs.push(z);
                }
                if changed {
                    Rc::new(Type::Tuple(zs))
                } else {
                    ty.clone()
                }
            }
            Type::Fun { params, ret } => {
                let mut changed = false;
                let mut zs = Vec::with_capacity(params.len());
                for p in params {
                    let z = self.zonk_rc(p, memo);
                    if !Rc::ptr_eq(&z, p) {
                        changed = true;
                    }
                    zs.push(z);
                }
                let zr = self.zonk_rc(ret, memo);
                if !Rc::ptr_eq(&zr, ret) {
                    changed = true;
                }
                if changed {
                    Rc::new(Type::Fun {
                        params: zs,
                        ret: zr,
                    })
                } else {
                    ty.clone()
                }
            }
            Type::App { def, args } => {
                let mut changed = false;
                let mut zs = Vec::with_capacity(args.len());
                for a in args {
                    let z = self.zonk_rc(a, memo);
                    if !Rc::ptr_eq(&z, a) {
                        changed = true;
                    }
                    zs.push(z);
                }
                if changed {
                    Rc::new(Type::App {
                        def: *def,
                        args: zs,
                    })
                } else {
                    ty.clone()
                }
            }
            _ => ty.clone(),
        };
        memo.insert(ptr, result.clone());
        result
    }

    pub fn free_vars(&mut self, ty: &Type, out: &mut BTreeSet<TvId>) {
        let mut visited: HashMap<*const Type, ()> = HashMap::new();
        self.free_vars_rc(&Rc::new(ty.clone()), out, &mut visited);
    }

    fn free_vars_rc(
        &mut self,
        ty: &Rc<Type>,
        out: &mut BTreeSet<TvId>,
        visited: &mut HashMap<*const Type, ()>,
    ) {
        let ptr = Rc::as_ptr(ty);
        if visited.contains_key(&ptr) {
            return;
        }
        visited.insert(ptr, ());
        match ty.as_ref() {
            Type::Var(id) => {
                if let Some(info) = self.vars.get(id) {
                    if let Some(link) = info.link.clone() {
                        self.free_vars_rc(&Rc::new(link), out, visited);
                        return;
                    }
                }
                out.insert(*id);
            }
            Type::List(t) => self.free_vars_rc(t, out, visited),
            Type::Tuple(ts) => {
                for t in ts {
                    self.free_vars_rc(t, out, visited);
                }
            }
            Type::Fun { params, ret } => {
                for p in params {
                    self.free_vars_rc(p, out, visited);
                }
                self.free_vars_rc(ret, out, visited);
            }
            Type::App { args, .. } => {
                for a in args {
                    self.free_vars_rc(a, out, visited);
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
