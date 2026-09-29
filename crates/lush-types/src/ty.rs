//! Internal type representation, schemes, and sealed constraints.
//!
//! Compound types use [`Rc`] so DAG-shaped types (e.g. let-doubling) share
//! structure; zonk / free_vars / display walk with a pointer-keyed memo.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use lush_syntax::span::Span;

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
    /// Linked type, stored behind [`Rc`] so zonk/occurs/subst follow the same
    /// node and preserve DAG sharing (Mairson-style doubling).
    pub link: Option<Rc<Type>>,
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
    /// Set when a type walk hits [`crate::limits::MAX_DEPTH`]; the checker
    /// emits E1305 once and continues with `Type::Error` rather than aborting.
    pub too_deep: bool,
    /// Best-effort span for the expression that first tripped [`Self::too_deep`].
    pub too_deep_span: Span,
    /// Shared `Rc<Type>` nodes known to contain no free unification variables.
    /// Cleared when the owning scope is popped (pointers must not outlive the `Rc`).
    pub closed: HashSet<*const Type>,
    /// Hint span copied into [`Self::too_deep_span`] when a walk trips the budget.
    pub hint_span: Span,
}

impl TypeStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that a type walk exceeded its budget, remembering `hint_span` once.
    pub fn note_too_deep(&mut self) {
        if !self.too_deep {
            self.too_deep = true;
            self.too_deep_span = self.hint_span;
        }
    }

    /// Ground types and shared nodes previously marked closed (no free `Var`s).
    pub fn rc_is_closed(&self, rc: &Rc<Type>) -> bool {
        match rc.as_ref() {
            Type::Int
            | Type::Float
            | Type::String
            | Type::Bool
            | Type::Nil
            | Type::BitArray
            | Type::Error
            | Type::Rigid(_) => true,
            _ => self.closed.contains(&Rc::as_ptr(rc)),
        }
    }

    /// O(arity) closedness check: primitives, or compounds whose child `Rc`s are closed.
    /// Maintained inductively when locals are bound into [`Self::closed`].
    pub fn type_is_closed(&self, ty: &Type) -> bool {
        match ty {
            Type::Int
            | Type::Float
            | Type::String
            | Type::Bool
            | Type::Nil
            | Type::BitArray
            | Type::Error
            | Type::Rigid(_) => true,
            Type::Var(id) => self
                .vars
                .get(id)
                .and_then(|i| i.link.as_ref())
                .is_some_and(|link| self.rc_is_closed(link)),
            Type::List(t) => self.rc_is_closed(t),
            Type::Tuple(ts) => ts.iter().all(|t| self.rc_is_closed(t)),
            Type::Fun { params, ret } => {
                params.iter().all(|t| self.rc_is_closed(t)) && self.rc_is_closed(ret)
            }
            Type::App { args, .. } => args.iter().all(|t| self.rc_is_closed(t)),
        }
    }

    pub fn mark_closed_rc(&mut self, rc: &Rc<Type>) {
        if self.type_is_closed(rc.as_ref()) {
            self.closed.insert(Rc::as_ptr(rc));
        }
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
        // Only skip the walk for types with no vars *and* no links to follow.
        // A linked `Var` is "closed" via its target but zonk must still unwrap it.
        if !matches!(ty, Type::Var(_)) && self.type_is_closed(ty) {
            self.work = self.work.saturating_add(1);
            return ty.clone();
        }
        let mut memo: HashMap<*const Type, Rc<Type>> = HashMap::new();
        (*self.zonk_rc(&Rc::new(ty.clone()), &mut memo)).clone()
    }

    /// Zonk a shared type node with an explicit stack and pointer-keyed memo so
    /// DAG spines (let-doubling) stay linear and never overflow the native
    /// stack. A single-walk node budget yields E1305 for Mairson-sized trees.
    /// Cross-call pointer caches are unsafe: ephemeral `Rc` roots are freed and
    /// their addresses reused.
    pub fn zonk_rc(
        &mut self,
        ty: &Rc<Type>,
        memo: &mut HashMap<*const Type, Rc<Type>>,
    ) -> Rc<Type> {
        /// Unique nodes visited in one zonk before we report E1305.
        const MAX_ZONK_NODES: usize = 32_768;
        enum Frame {
            Visit(Rc<Type>),
            Build(Rc<Type>),
        }
        let root_ptr = Rc::as_ptr(ty);
        if let Some(z) = memo.get(&root_ptr) {
            return z.clone();
        }
        let mut stack: Vec<Frame> = vec![Frame::Visit(ty.clone())];
        let mut entered: HashSet<*const Type> = HashSet::new();
        while let Some(frame) = stack.pop() {
            match frame {
                Frame::Visit(ty) => {
                    let ptr = Rc::as_ptr(&ty);
                    if memo.contains_key(&ptr) {
                        continue;
                    }
                    if !entered.insert(ptr) {
                        continue;
                    }
                    if entered.len() > MAX_ZONK_NODES {
                        self.note_too_deep();
                        let err = Rc::new(Type::Error);
                        memo.insert(ptr, err.clone());
                        return err;
                    }
                    self.work = self.work.saturating_add(1);
                    stack.push(Frame::Build(ty.clone()));
                    match ty.as_ref() {
                        Type::Var(id) => {
                            if let Some(link) = self.vars.get(id).and_then(|i| i.link.clone()) {
                                stack.push(Frame::Visit(link));
                            }
                        }
                        Type::List(t) => stack.push(Frame::Visit(t.clone())),
                        Type::Tuple(ts) => {
                            for t in ts.iter().rev() {
                                stack.push(Frame::Visit(t.clone()));
                            }
                        }
                        Type::Fun { params, ret } => {
                            stack.push(Frame::Visit(ret.clone()));
                            for p in params.iter().rev() {
                                stack.push(Frame::Visit(p.clone()));
                            }
                        }
                        Type::App { args, .. } => {
                            for a in args.iter().rev() {
                                stack.push(Frame::Visit(a.clone()));
                            }
                        }
                        _ => {}
                    }
                }
                Frame::Build(ty) => {
                    let ptr = Rc::as_ptr(&ty);
                    if memo.contains_key(&ptr) {
                        continue;
                    }
                    let lookup = |memo: &HashMap<*const Type, Rc<Type>>, t: &Rc<Type>| {
                        memo.get(&Rc::as_ptr(t))
                            .cloned()
                            .unwrap_or_else(|| t.clone())
                    };
                    let result = match ty.as_ref() {
                        Type::Var(id) => {
                            if let Some(link) = self.vars.get(id).and_then(|i| i.link.clone()) {
                                let z = lookup(memo, &link);
                                if let Some(info) = self.vars.get_mut(id) {
                                    info.link = Some(z.clone());
                                }
                                // Cache the var node too so later walks hit memo.
                                memo.insert(ptr, z.clone());
                                continue;
                            }
                            ty.clone()
                        }
                        Type::List(t) => {
                            let zt = lookup(memo, t);
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
                                let z = lookup(memo, t);
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
                                let z = lookup(memo, p);
                                if !Rc::ptr_eq(&z, p) {
                                    changed = true;
                                }
                                zs.push(z);
                            }
                            let zr = lookup(memo, ret);
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
                                let z = lookup(memo, a);
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
                    memo.insert(ptr, result);
                }
            }
        }
        memo.get(&root_ptr).cloned().unwrap_or_else(|| ty.clone())
    }

    pub fn free_vars(&mut self, ty: &Type, out: &mut BTreeSet<TvId>) {
        if self.type_is_closed(ty) {
            self.work = self.work.saturating_add(1);
            return;
        }
        let mut visited: HashSet<*const Type> = HashSet::new();
        let mut stack: Vec<Rc<Type>> = vec![Rc::new(ty.clone())];
        while let Some(ty) = stack.pop() {
            let ptr = Rc::as_ptr(&ty);
            if !visited.insert(ptr) {
                continue;
            }
            self.work = self.work.saturating_add(1);
            match ty.as_ref() {
                Type::Var(id) => {
                    if let Some(info) = self.vars.get(id) {
                        if let Some(link) = info.link.clone() {
                            stack.push(link);
                            continue;
                        }
                    }
                    out.insert(*id);
                }
                Type::List(t) => stack.push(t.clone()),
                Type::Tuple(ts) => stack.extend(ts.iter().cloned()),
                Type::Fun { params, ret } => {
                    stack.extend(params.iter().cloned());
                    stack.push(ret.clone());
                }
                Type::App { args, .. } => stack.extend(args.iter().cloned()),
                _ => {}
            }
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
