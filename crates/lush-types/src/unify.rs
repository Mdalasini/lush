//! Unification, occurs check, instantiation, and generalisation.

use std::collections::{BTreeSet, HashMap};

use lush_syntax::span::Span;

use crate::codes;
use crate::diag::TypeSink;
use crate::limits::MAX_DEPTH;
use crate::ty::{ConstraintSet, RigidId, Scheme, TvId, Type, TypeStore};

#[derive(Clone, Debug)]
pub struct Origin {
    pub span: Span,
    pub label: String,
}

pub struct Unifier<'a> {
    pub store: &'a mut TypeStore,
    pub sink: &'a mut TypeSink,
    pub depth: usize,
}

impl<'a> Unifier<'a> {
    pub fn new(store: &'a mut TypeStore, sink: &'a mut TypeSink) -> Self {
        Self {
            store,
            sink,
            depth: 0,
        }
    }

    pub fn unify(&mut self, a: &Type, b: &Type, span: Span, expected_origin: Option<&Origin>) {
        if self.depth > MAX_DEPTH {
            self.sink.error(
                codes::E1305_TOO_DEEP,
                "type checking exceeded maximum nesting depth",
                span,
                Some("simplify nested types or expressions".into()),
            );
            return;
        }
        self.depth += 1;
        self.store.work = self.store.work.saturating_add(1);
        let a = self.store.zonk(a);
        let b = self.store.zonk(b);
        match (&a, &b) {
            (Type::Error, _) | (_, Type::Error) => {}
            (Type::Var(id), t) | (t, Type::Var(id)) => {
                // Ensure we bind the var side correctly when both are vars.
                if let Type::Var(id2) = t {
                    if id == id2 {
                        self.depth -= 1;
                        return;
                    }
                    // Bind the higher-level var to the lower one (or merge).
                    self.bind_var(*id, Type::Var(*id2), span, expected_origin);
                } else if matches!(a, Type::Var(_)) {
                    self.bind_var(*id, t.clone(), span, expected_origin);
                } else {
                    // b is Var
                    let Type::Var(id) = &b else { unreachable!() };
                    self.bind_var(*id, a.clone(), span, expected_origin);
                }
            }
            (Type::Rigid(i), Type::Rigid(j)) if i == j => {}
            (Type::Rigid(_), _) | (_, Type::Rigid(_)) => {
                let mut d = DiagnosticBuilder::mismatch(self.store, &a, &b, span);
                if let Some(o) = expected_origin {
                    d = d.with_origin(o);
                }
                d.emit(
                    self.sink,
                    codes::E1302_RIGID,
                    "cannot instantiate rigid type variable",
                );
            }
            (Type::Int, Type::Int)
            | (Type::Float, Type::Float)
            | (Type::String, Type::String)
            | (Type::Bool, Type::Bool)
            | (Type::Nil, Type::Nil)
            | (Type::BitArray, Type::BitArray) => {}
            (Type::List(x), Type::List(y)) => self.unify(x, y, span, expected_origin),
            (Type::Tuple(xs), Type::Tuple(ys)) if xs.len() == ys.len() => {
                for (x, y) in xs.iter().zip(ys) {
                    self.unify(x, y, span, expected_origin);
                }
            }
            (
                Type::Fun {
                    params: ps,
                    ret: r1,
                },
                Type::Fun {
                    params: qs,
                    ret: r2,
                },
            ) if ps.len() == qs.len() => {
                for (p, q) in ps.iter().zip(qs) {
                    self.unify(p, q, span, expected_origin);
                }
                self.unify(r1, r2, span, expected_origin);
            }
            (Type::App { def: d1, args: a1 }, Type::App { def: d2, args: a2 })
                if d1 == d2 && a1.len() == a2.len() =>
            {
                for (x, y) in a1.iter().zip(a2) {
                    self.unify(x, y, span, expected_origin);
                }
            }
            _ => {
                let mut d = DiagnosticBuilder::mismatch(self.store, &a, &b, span);
                if let Some(o) = expected_origin {
                    d = d.with_origin(o);
                }
                d.emit(self.sink, codes::E1300_TYPE_MISMATCH, "type mismatch");
            }
        }
        self.depth -= 1;
    }

    fn bind_var(&mut self, id: TvId, ty: Type, span: Span, expected_origin: Option<&Origin>) {
        if self.occurs(id, &ty, 0) {
            self.sink.error(
                codes::E1301_OCCURS,
                "infinite type (occurs check failed)",
                span,
                Some("a type variable would contain itself".into()),
            );
            return;
        }
        // Level adjustment + constraint merge
        let level = self.store.vars.get(&id).map(|v| v.level).unwrap_or(0);
        let constraints = self
            .store
            .vars
            .get(&id)
            .map(|v| v.constraints.clone())
            .unwrap_or_default();
        self.adjust_level(&ty, level);
        // If tying two vars, merge constraints onto the representative.
        if let Type::Var(other) = &ty {
            if let Some(info) = self.store.vars.get_mut(other) {
                info.constraints.merge(&constraints);
            }
            if let Some(info) = self.store.vars.get_mut(&id) {
                info.link = Some(ty);
            }
            return;
        }
        // Concrete type: discharge constraints
        if !constraints.is_empty() {
            self.discharge_constraints(&ty, &constraints, span, expected_origin);
        }
        if let Some(info) = self.store.vars.get_mut(&id) {
            info.link = Some(ty);
        }
    }

    fn adjust_level(&mut self, ty: &Type, max_level: u32) {
        let z = self.store.zonk(ty);
        match z {
            Type::Var(id) => {
                if let Some(info) = self.store.vars.get_mut(&id) {
                    if info.level > max_level {
                        info.level = max_level;
                    }
                }
            }
            Type::List(t) => self.adjust_level(&t, max_level),
            Type::Tuple(ts) => {
                for t in ts {
                    self.adjust_level(&t, max_level);
                }
            }
            Type::Fun { params, ret } => {
                for p in params {
                    self.adjust_level(&p, max_level);
                }
                self.adjust_level(&ret, max_level);
            }
            Type::App { args, .. } => {
                for a in args {
                    self.adjust_level(&a, max_level);
                }
            }
            _ => {}
        }
    }

    pub fn occurs(&mut self, id: TvId, ty: &Type, depth: usize) -> bool {
        if depth > MAX_DEPTH {
            return true;
        }
        self.store.work = self.store.work.saturating_add(1);
        let z = self.store.zonk(ty);
        match z {
            Type::Var(v) => v == id,
            Type::List(t) => self.occurs(id, &t, depth + 1),
            Type::Tuple(ts) => ts.iter().any(|t| self.occurs(id, t, depth + 1)),
            Type::Fun { params, ret } => {
                params.iter().any(|t| self.occurs(id, t, depth + 1))
                    || self.occurs(id, &ret, depth + 1)
            }
            Type::App { args, .. } => args.iter().any(|t| self.occurs(id, t, depth + 1)),
            _ => false,
        }
    }

    fn discharge_constraints(
        &mut self,
        ty: &Type,
        c: &ConstraintSet,
        span: Span,
        _origin: Option<&Origin>,
    ) {
        if c.eq && !crate::eq_capability::has_eq(self.store, ty) {
            let shown = self.store.display(ty);
            self.sink.error(
                codes::E1350_NO_EQ,
                format!("type `{shown}` does not support equality (`Eq`)"),
                span,
                Some("functions, selectors, and tasks are not Eq; structural types need Eq components".into()),
            );
        }
        if c.neg && !matches!(ty, Type::Int | Type::Float | Type::Var(_) | Type::Error) {
            // Still a var? leave constraint. Concrete non-numeric:
            if !matches!(ty, Type::Var(_)) {
                let shown = self.store.display(ty);
                self.sink.error(
                    codes::E1351_NO_NEG,
                    format!("type `{shown}` does not support negation (`Neg`)"),
                    span,
                    Some("`Neg` is only satisfied by `Int` and `Float`".into()),
                );
            }
        }
        if c.neg {
            match ty {
                Type::Int | Type::Float | Type::Error | Type::Var(_) => {}
                _ => {}
            }
        }
    }
}

struct DiagnosticBuilder {
    expected: String,
    actual: String,
    span: Span,
    origin: Option<Origin>,
}

impl DiagnosticBuilder {
    fn mismatch(store: &mut TypeStore, a: &Type, b: &Type, span: Span) -> Self {
        Self {
            expected: store.display(a),
            actual: store.display(b),
            span,
            origin: None,
        }
    }
    fn with_origin(mut self, o: &Origin) -> Self {
        self.origin = Some(o.clone());
        self
    }
    fn emit(self, sink: &mut TypeSink, code: &str, head: &str) {
        let msg = format!(
            "{head}: expected `{}`, found `{}`",
            self.expected, self.actual
        );
        let mut d = lush_syntax::diagnostic::Diagnostic::error(
            code,
            msg,
            self.span,
            Some("check annotations and earlier uses of this value".into()),
            lush_syntax::diagnostic::DiagnosticKind::Type,
        );
        if let Some(o) = self.origin {
            d = d.with_secondary(o.span, o.label);
        }
        sink.push(d);
    }
}

/// Instantiate a scheme at the current level, returning the body and a map of
/// fresh vars (for constraint origin tracking).
pub fn instantiate(store: &mut TypeStore, scheme: &Scheme, level: u32) -> Type {
    let mut subst: HashMap<TvId, Type> = HashMap::new();
    for v in &scheme.vars {
        let fresh = store.fresh_var(level);
        if let (Type::Var(fid), Some(c)) = (&fresh, scheme.constraints.get(v)) {
            if let Some(info) = store.vars.get_mut(fid) {
                info.constraints.merge(c);
            }
        }
        subst.insert(*v, fresh);
    }
    apply_subst(store, &scheme.body, &subst)
}

fn apply_subst(store: &mut TypeStore, ty: &Type, subst: &HashMap<TvId, Type>) -> Type {
    let z = store.zonk(ty);
    match z {
        Type::Var(id) => subst.get(&id).cloned().unwrap_or(Type::Var(id)),
        Type::List(t) => Type::List(Box::new(apply_subst(store, &t, subst))),
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| apply_subst(store, t, subst)).collect()),
        Type::Fun { params, ret } => Type::Fun {
            params: params
                .iter()
                .map(|t| apply_subst(store, t, subst))
                .collect(),
            ret: Box::new(apply_subst(store, &ret, subst)),
        },
        Type::App { def, args } => Type::App {
            def,
            args: args.iter().map(|t| apply_subst(store, t, subst)).collect(),
        },
        other => other,
    }
}

/// Generalise type variables with level > current_level that are not free in the environment.
pub fn generalise(store: &mut TypeStore, ty: &Type, env_level: u32, expansive: bool) -> Scheme {
    let z = store.zonk(ty);
    if expansive {
        return Scheme::mono(z);
    }
    let mut free = BTreeSet::new();
    store.free_vars(&z, &mut free);
    let mut vars = Vec::new();
    let mut constraints = HashMap::new();
    for id in free {
        let level = store.vars.get(&id).map(|v| v.level).unwrap_or(0);
        if level > env_level {
            vars.push(id);
            if let Some(info) = store.vars.get(&id) {
                if !info.constraints.is_empty() {
                    constraints.insert(id, info.constraints.clone());
                }
            }
        }
    }
    vars.sort();
    Scheme {
        vars,
        constraints,
        body: z,
    }
}

/// Replace rigid variables with quantified unification vars for the scheme.
pub fn generalise_rigids(store: &mut TypeStore, ty: &Type, rigids: &[(RigidId, String)]) -> Scheme {
    let mut subst: HashMap<RigidId, TvId> = HashMap::new();
    let mut vars = Vec::new();
    let mut constraints = HashMap::new();
    for (rid, _name) in rigids {
        let tv = match store.fresh_var(0) {
            Type::Var(id) => id,
            _ => unreachable!(),
        };
        // Transfer rigid constraints
        if let Some(info) = store.rigids.get(rid) {
            if !info.constraints.is_empty() {
                if let Some(vi) = store.vars.get_mut(&tv) {
                    vi.constraints.merge(&info.constraints);
                }
                constraints.insert(tv, info.constraints.clone());
            }
        }
        subst.insert(*rid, tv);
        vars.push(tv);
    }
    let body = replace_rigids(store, ty, &subst);
    Scheme {
        vars,
        constraints,
        body,
    }
}

fn replace_rigids(store: &mut TypeStore, ty: &Type, subst: &HashMap<RigidId, TvId>) -> Type {
    let z = store.zonk(ty);
    match z {
        Type::Rigid(id) => {
            if let Some(tv) = subst.get(&id) {
                Type::Var(*tv)
            } else {
                Type::Rigid(id)
            }
        }
        Type::List(t) => Type::List(Box::new(replace_rigids(store, &t, subst))),
        Type::Tuple(ts) => {
            Type::Tuple(ts.iter().map(|t| replace_rigids(store, t, subst)).collect())
        }
        Type::Fun { params, ret } => Type::Fun {
            params: params
                .iter()
                .map(|t| replace_rigids(store, t, subst))
                .collect(),
            ret: Box::new(replace_rigids(store, &ret, subst)),
        },
        Type::App { def, args } => Type::App {
            def,
            args: args
                .iter()
                .map(|t| replace_rigids(store, t, subst))
                .collect(),
        },
        other => other,
    }
}

/// Attach a constraint to a type (var or rigid).
pub fn add_constraint(store: &mut TypeStore, ty: &Type, c: ConstraintSet) {
    let z = store.zonk(ty);
    match z {
        Type::Var(id) => {
            if let Some(info) = store.vars.get_mut(&id) {
                info.constraints.merge(&c);
            }
        }
        Type::Rigid(id) => {
            if let Some(info) = store.rigids.get_mut(&id) {
                info.constraints.merge(&c);
            }
        }
        _ => {}
    }
}
