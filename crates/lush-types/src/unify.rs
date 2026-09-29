//! Unification, occurs check, instantiation, and generalisation.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

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
                if let Type::Var(id2) = t {
                    if id == id2 {
                        self.depth -= 1;
                        return;
                    }
                    self.bind_var(*id, Type::Var(*id2), span, expected_origin);
                } else if matches!(a, Type::Var(_)) {
                    self.bind_var(*id, t.clone(), span, expected_origin);
                } else {
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
        let level = self.store.vars.get(&id).map(|v| v.level).unwrap_or(0);
        let constraints = self
            .store
            .vars
            .get(&id)
            .map(|v| v.constraints.clone())
            .unwrap_or_default();
        self.adjust_level(&ty, level);
        let link = Rc::new(ty);
        if let Type::Var(other) = link.as_ref() {
            if let Some(info) = self.store.vars.get_mut(other) {
                info.constraints.merge(&constraints);
            }
            if let Some(info) = self.store.vars.get_mut(&id) {
                info.link = Some(link);
            }
            return;
        }
        if !constraints.is_empty() {
            self.discharge_constraints(&link, &constraints, span, expected_origin);
        }
        if let Some(info) = self.store.vars.get_mut(&id) {
            info.link = Some(link);
        }
    }

    fn adjust_level(&mut self, ty: &Type, max_level: u32) {
        lower_levels(self.store, ty, max_level);
    }

    /// True when `id` occurs free inside `ty` (infinite type).
    ///
    /// Walks iteratively so Mairson-style types whose DAG spine is deep but
    /// acyclic are not mis-reported as occurs failures. Depth limits belong to
    /// [`Unifier::unify`]'s `E1305`, not to the occurs check.
    pub fn occurs(&mut self, id: TvId, ty: &Type, _depth: usize) -> bool {
        let mut visited: HashSet<*const Type> = HashSet::new();
        let mut var_seen: HashSet<TvId> = HashSet::new();
        let mut stack: Vec<Rc<Type>> = vec![Rc::new(ty.clone())];
        while let Some(ty) = stack.pop() {
            let ptr = Rc::as_ptr(&ty);
            if !visited.insert(ptr) {
                continue;
            }
            self.store.work = self.store.work.saturating_add(1);
            match ty.as_ref() {
                Type::Var(v) => {
                    if *v == id {
                        return true;
                    }
                    if !var_seen.insert(*v) {
                        continue;
                    }
                    if let Some(info) = self.store.vars.get(v) {
                        if let Some(link) = info.link.clone() {
                            stack.push(link);
                        }
                    }
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
        false
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
            let shown = self.store.display(ty);
            self.sink.error(
                codes::E1351_NO_NEG,
                format!("type `{shown}` does not support negation (`Neg`)"),
                span,
                Some("`Neg` is only satisfied by `Int` and `Float`".into()),
            );
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

/// Instantiate a scheme at the current level.
pub fn instantiate(store: &mut TypeStore, scheme: &Scheme, level: u32) -> Type {
    if scheme.vars.is_empty() {
        // Preserve DAG sharing for monomorphic bindings.
        return scheme.body.clone();
    }
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
    let mut memo: HashMap<*const Type, Rc<Type>> = HashMap::new();
    // Same quantified variable may appear under distinct Rc wrappers (e.g. the
    // param slot vs body occurrences). Memoising by TvId keeps instantiate
    // sharing intact for Mairson-style doubling.
    let mut var_memo: HashMap<TvId, Rc<Type>> = HashMap::new();
    (*apply_subst_rc(
        store,
        &Rc::new(ty.clone()),
        subst,
        &mut memo,
        &mut var_memo,
        0,
    ))
    .clone()
}

fn apply_subst_rc(
    store: &mut TypeStore,
    ty: &Rc<Type>,
    subst: &HashMap<TvId, Type>,
    memo: &mut HashMap<*const Type, Rc<Type>>,
    var_memo: &mut HashMap<TvId, Rc<Type>>,
    depth: usize,
) -> Rc<Type> {
    // Bound native recursion; large Mairson spines hit this and become E1305.
    // Let-doubling uses monomorphic schemes and does not instantiate deep DAGs.
    if depth > MAX_DEPTH || memo.len() > 32_768 {
        store.note_too_deep();
        return Rc::new(Type::Error);
    }
    let ptr = Rc::as_ptr(ty);
    if let Some(z) = memo.get(&ptr) {
        return z.clone();
    }
    store.work = store.work.saturating_add(1);
    let result = match ty.as_ref() {
        Type::Var(id) => {
            if let Some(info) = store.vars.get(id) {
                if let Some(link) = info.link.clone() {
                    return apply_subst_rc(store, &link, subst, memo, var_memo, depth + 1);
                }
            }
            if let Some(t) = subst.get(id) {
                if let Some(existing) = var_memo.get(id) {
                    return existing.clone();
                }
                let fresh = Rc::new(t.clone());
                var_memo.insert(*id, fresh.clone());
                fresh
            } else {
                ty.clone()
            }
        }
        Type::List(t) => Rc::new(Type::List(apply_subst_rc(
            store,
            t,
            subst,
            memo,
            var_memo,
            depth + 1,
        ))),
        Type::Tuple(ts) => Rc::new(Type::Tuple(
            ts.iter()
                .map(|t| apply_subst_rc(store, t, subst, memo, var_memo, depth + 1))
                .collect(),
        )),
        Type::Fun { params, ret } => Rc::new(Type::Fun {
            params: params
                .iter()
                .map(|t| apply_subst_rc(store, t, subst, memo, var_memo, depth + 1))
                .collect(),
            ret: apply_subst_rc(store, ret, subst, memo, var_memo, depth + 1),
        }),
        Type::App { def, args } => Rc::new(Type::App {
            def: *def,
            args: args
                .iter()
                .map(|t| apply_subst_rc(store, t, subst, memo, var_memo, depth + 1))
                .collect(),
        }),
        _ => ty.clone(),
    };
    memo.insert(ptr, result.clone());
    result
}

/// Lower every free unification variable in `ty` to at most `max_level`.
///
/// Used after inferring an expansive `let` RHS that was typed one level up:
/// those variables are free in the environment and must not be generalised
/// (§4.3 value restriction).
pub fn lower_levels(store: &mut TypeStore, ty: &Type, max_level: u32) {
    let mut visited: HashSet<*const Type> = HashSet::new();
    let mut stack: Vec<Rc<Type>> = vec![Rc::new(ty.clone())];
    while let Some(ty) = stack.pop() {
        let ptr = Rc::as_ptr(&ty);
        if !visited.insert(ptr) {
            continue;
        }
        match ty.as_ref() {
            Type::Var(id) => {
                if let Some(info) = store.vars.get(id) {
                    if let Some(link) = info.link.clone() {
                        stack.push(link);
                        continue;
                    }
                }
                if let Some(info) = store.vars.get_mut(id) {
                    if info.level > max_level {
                        info.level = max_level;
                    }
                }
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

/// True when `scheme.body` mentions a free unification variable that is not
/// among the quantified `scheme.vars` (after zonking). Used for the module
/// interface escape check and its unit test.
pub fn scheme_has_escaping_vars(store: &mut TypeStore, scheme: &Scheme) -> bool {
    let body = store.zonk(&scheme.body);
    let mut free = BTreeSet::new();
    store.free_vars(&body, &mut free);
    let quantified: BTreeSet<_> = scheme.vars.iter().copied().collect();
    free.iter().any(|id| !quantified.contains(id))
}

/// Generalise type variables with level > current_level.
pub fn generalise(store: &mut TypeStore, ty: &Type, env_level: u32, expansive: bool) -> Scheme {
    // Closed types (no free unification vars) skip zonk/free_vars. Let-doubling
    // builds these inductively from shared ground nodes, so each binding is O(1).
    if store.type_is_closed(ty) {
        store.work = store.work.saturating_add(1);
        return Scheme::mono(ty.clone());
    }
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
    let mut memo: HashMap<*const Type, Rc<Type>> = HashMap::new();
    (*replace_rigids_rc(store, &Rc::new(ty.clone()), subst, &mut memo)).clone()
}

fn replace_rigids_rc(
    store: &mut TypeStore,
    ty: &Rc<Type>,
    subst: &HashMap<RigidId, TvId>,
    memo: &mut HashMap<*const Type, Rc<Type>>,
) -> Rc<Type> {
    let ptr = Rc::as_ptr(ty);
    if let Some(z) = memo.get(&ptr) {
        return z.clone();
    }
    store.work = store.work.saturating_add(1);
    let result = match ty.as_ref() {
        Type::Rigid(id) => {
            if let Some(tv) = subst.get(id) {
                Rc::new(Type::Var(*tv))
            } else {
                ty.clone()
            }
        }
        Type::Var(id) => {
            if let Some(info) = store.vars.get(id) {
                if let Some(link) = info.link.clone() {
                    return replace_rigids_rc(store, &link, subst, memo);
                }
            }
            ty.clone()
        }
        Type::List(t) => Rc::new(Type::List(replace_rigids_rc(store, t, subst, memo))),
        Type::Tuple(ts) => Rc::new(Type::Tuple(
            ts.iter()
                .map(|t| replace_rigids_rc(store, t, subst, memo))
                .collect(),
        )),
        Type::Fun { params, ret } => Rc::new(Type::Fun {
            params: params
                .iter()
                .map(|t| replace_rigids_rc(store, t, subst, memo))
                .collect(),
            ret: replace_rigids_rc(store, ret, subst, memo),
        }),
        Type::App { def, args } => Rc::new(Type::App {
            def: *def,
            args: args
                .iter()
                .map(|t| replace_rigids_rc(store, t, subst, memo))
                .collect(),
        }),
        _ => ty.clone(),
    };
    memo.insert(ptr, result.clone());
    result
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
