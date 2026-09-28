//! Type checking entry points and inference.
//!
//! This is an early prototype: enough to type-check complete documentation
//! modules and catch basic mismatches. It is not a full Hindley-Milner
//! implementation of `spec.md` §4.3.

use crate::desugar::desugar_module;
use crate::env::Env;
use crate::error::TypeError;
use crate::ty::{apply, free_vars, Scheme, Subst, Type};
use crate::unify::unify;
use lush_syntax::ast::*;
use lush_syntax::{parse_module, Span};
use std::collections::{HashMap, HashSet};

/// Parse and type-check a source module.
pub fn typecheck_source(source: &str) -> Result<(), Vec<TypeError>> {
    let mut module = parse_module(source).map_err(|errors| {
        errors
            .into_iter()
            .map(|e| TypeError::Other {
                span: e.span(),
                message: e.to_string(),
            })
            .collect::<Vec<_>>()
    })?;
    typecheck_module(&mut module)
}

/// Type-check a parsed module (mutates it to desugar `use`).
pub fn typecheck_module(module: &mut Module) -> Result<(), Vec<TypeError>> {
    desugar_module(module);
    let mut checker = Checker::new();
    checker.check_module(module);
    if checker.errors.is_empty() {
        Ok(())
    } else {
        Err(checker.errors)
    }
}

struct Checker {
    env: Env,
    subst: Subst,
    next_var: u32,
    errors: Vec<TypeError>,
    /// Local ADT constructors: name -> scheme.
    ctors: HashMap<String, Scheme>,
    /// Type aliases: name -> (params, body).
    aliases: HashMap<String, (Vec<String>, TypeExpr)>,
    /// Scoped annotation type variables (`a`, `b`, …) → unification vars.
    type_vars: HashMap<String, Type>,
    /// Function/constructor parameter labels for labelled-argument reordering.
    fn_labels: HashMap<String, Vec<Option<String>>>,
    /// Alias expansion stack for cycle detection.
    alias_stack: Vec<String>,
    /// Names of module-level constants (for constant-expression validation).
    const_names: HashSet<String>,
    /// Folded values of module-level constants (dependency order).
    const_vals: HashMap<String, ConstVal>,
}

impl Checker {
    fn new() -> Self {
        Self {
            env: Env::prelude(),
            subst: HashMap::new(),
            next_var: 100,
            errors: Vec::new(),
            ctors: HashMap::new(),
            aliases: HashMap::new(),
            type_vars: HashMap::new(),
            fn_labels: HashMap::new(),
            alias_stack: Vec::new(),
            const_names: HashSet::new(),
            const_vals: HashMap::new(),
        }
    }

    fn fresh(&mut self) -> Type {
        let v = self.next_var;
        self.next_var += 1;
        Type::Var(v)
    }

    fn check_module(&mut self, module: &Module) {
        for import in &module.imports {
            let alias = import
                .alias
                .clone()
                .unwrap_or_else(|| import.path.last().cloned().unwrap_or_else(|| "mod".into()));
            self.env.bind_module_stub(&alias);
            if let Some(items) = &import.items {
                for item in items {
                    let name = item.alias.clone().unwrap_or_else(|| item.name.clone());
                    if item.is_type {
                        self.env.types.insert(name, 0);
                    } else {
                        // Flexible polymorphic value from selective import.
                        let a = self.fresh();
                        let scheme = Scheme {
                            vars: free_vars(&a),
                            body: a,
                        };
                        self.env.extend(name, scheme);
                    }
                }
            }
        }

        // Register aliases before ADT schemes so field types expand correctly.
        for def in &module.definitions {
            if let Definition::Type(t) = def {
                self.env.types.insert(t.name.clone(), t.params.len());
                if let TypeDefBody::Alias(alias) = &t.body {
                    self.aliases
                        .insert(t.name.clone(), (t.params.clone(), alias.clone()));
                }
            }
        }
        for def in &module.definitions {
            if let Definition::Type(t) = def {
                if matches!(t.body, TypeDefBody::Adt(_)) {
                    self.register_adt(t);
                }
            }
        }

        // Pre-bind functions/consts with fresh placeholders (shared with check).
        let mut fn_names = Vec::new();
        for def in &module.definitions {
            if let Definition::Fn(f) = def {
                fn_names.push(f.name.clone());
                let params: Vec<Type> = f.params.iter().map(|_| self.fresh()).collect();
                let ret = self.fresh();
                self.fn_labels.insert(
                    f.name.clone(),
                    f.params.iter().map(effective_param_label).collect(),
                );
                self.env.insert_mono(
                    f.name.clone(),
                    Type::Fn {
                        params,
                        ret: Box::new(ret),
                    },
                );
            }
            if let Definition::Const(c) = def {
                let ty = self.fresh();
                self.env.insert_mono(c.name.clone(), ty);
                self.const_names.insert(c.name.clone());
            }
        }

        self.check_const_cycles_and_fold(module);

        // Check each function against the shared placeholders (mutual recursion),
        // then generalize it before later definitions see it. Remaining unchecked
        // siblings stay monomorphic placeholders until their turn.
        for def in &module.definitions {
            match def {
                Definition::Fn(f) => {
                    self.check_fn(f, false);
                    self.generalize_fn_group(std::slice::from_ref(&f.name));
                }
                Definition::Const(c) => self.check_const(c),
                Definition::Type(_) => {}
            }
        }
        let _ = fn_names;
    }

    /// Reject constant-reference cycles and fold constants in dependency order (§5.7).
    fn check_const_cycles_and_fold(&mut self, module: &Module) {
        let mut defs: Vec<(String, Span, &Expr)> = Vec::new();
        for def in &module.definitions {
            if let Definition::Const(c) = def {
                defs.push((c.name.clone(), c.span, &c.value));
            }
        }
        if defs.is_empty() {
            return;
        }
        let mut deps: HashMap<String, HashSet<String>> = HashMap::new();
        for (name, _, value) in &defs {
            let mut refs = HashSet::new();
            collect_const_refs(value, &self.const_names, &mut refs);
            deps.insert(name.clone(), refs);
        }
        // 0 = unvisited, 1 = visiting, 2 = done
        let mut state: HashMap<String, u8> = HashMap::new();
        let mut stack = Vec::new();
        let mut cyclic = HashSet::new();
        for (name, _, _) in &defs {
            if state.get(name).copied().unwrap_or(0) == 0 {
                self.dfs_const_cycle(name, &deps, &mut state, &mut stack, &mut cyclic);
            }
        }
        for (name, span, _) in &defs {
            if cyclic.contains(name) {
                self.errors.push(TypeError::Other {
                    span: *span,
                    message: format!("constant `{name}` participates in a reference cycle"),
                });
            }
        }

        // Topological fold of non-cyclic constants so cross-constant arithmetic
        // (e.g. `1 / zero`) reports evaluation failures.
        let mut indegree: HashMap<String, usize> = HashMap::new();
        let mut rev: HashMap<String, Vec<String>> = HashMap::new();
        for (name, _, _) in &defs {
            if cyclic.contains(name) {
                continue;
            }
            let deg = deps
                .get(name)
                .map(|refs| refs.iter().filter(|r| !cyclic.contains(*r)).count())
                .unwrap_or(0);
            indegree.insert(name.clone(), deg);
            if let Some(refs) = deps.get(name) {
                for r in refs {
                    if !cyclic.contains(r) {
                        rev.entry(r.clone()).or_default().push(name.clone());
                    }
                }
            }
        }
        let mut queue: Vec<String> = indegree
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(n, _)| n.clone())
            .collect();
        let values: HashMap<String, &Expr> = defs.iter().map(|(n, _, v)| (n.clone(), *v)).collect();
        while let Some(name) = queue.pop() {
            if let Some(value) = values.get(&name) {
                match eval_const_expr(value, &self.const_names, &self.const_vals) {
                    Ok(v) => {
                        self.const_vals.insert(name.clone(), v);
                    }
                    Err(e) => self.errors.push(e),
                }
            }
            if let Some(dependents) = rev.get(&name) {
                for dep in dependents {
                    if let Some(d) = indegree.get_mut(dep) {
                        *d = d.saturating_sub(1);
                        if *d == 0 {
                            queue.push(dep.clone());
                        }
                    }
                }
            }
        }
    }

    fn dfs_const_cycle(
        &self,
        name: &str,
        deps: &HashMap<String, HashSet<String>>,
        state: &mut HashMap<String, u8>,
        stack: &mut Vec<String>,
        cyclic: &mut HashSet<String>,
    ) {
        state.insert(name.to_string(), 1);
        stack.push(name.to_string());
        if let Some(refs) = deps.get(name) {
            for next in refs {
                match state.get(next).copied().unwrap_or(0) {
                    0 => self.dfs_const_cycle(next, deps, state, stack, cyclic),
                    1 => {
                        // Mark the cycle members from `next` through the top of the stack.
                        if let Some(start) = stack.iter().position(|n| n == next) {
                            for n in &stack[start..] {
                                cyclic.insert(n.clone());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        stack.pop();
        state.insert(name.to_string(), 2);
    }

    fn register_adt(&mut self, t: &TypeDef) {
        let TypeDefBody::Adt(variants) = &t.body else {
            return;
        };
        let saved_tvars = self.type_vars.clone();
        self.type_vars.clear();
        let param_vars: Vec<Type> = t
            .params
            .iter()
            .map(|p| {
                let v = self.fresh();
                self.type_vars.insert(p.clone(), v.clone());
                v
            })
            .collect();
        let ret = Type::Named {
            module: None,
            name: t.name.clone(),
            args: param_vars,
        };
        for v in variants {
            if self.ctors.contains_key(&v.name) || self.env.get(&v.name).is_some() {
                self.errors.push(TypeError::Other {
                    span: t.span,
                    message: format!("duplicate constructor `{}`", v.name),
                });
                continue;
            }
            self.fn_labels.insert(
                v.name.clone(),
                v.fields.iter().map(|f| f.label.clone()).collect(),
            );
            let param_tys: Vec<Type> = v.fields.iter().map(|f| self.ast_type(&f.ty)).collect();
            if param_tys.is_empty() {
                let scheme = Scheme {
                    vars: free_vars(&ret),
                    body: ret.clone(),
                };
                self.ctors.insert(v.name.clone(), scheme.clone());
                self.env.insert_scheme(v.name.clone(), scheme);
            } else {
                let body = Type::Fn {
                    params: param_tys,
                    ret: Box::new(ret.clone()),
                };
                let scheme = Scheme {
                    vars: free_vars(&body),
                    body,
                };
                self.ctors.insert(v.name.clone(), scheme.clone());
                self.env.insert_scheme(v.name.clone(), scheme);
            }
        }
        self.type_vars = saved_tvars;
    }

    fn generalize_fn_group(&mut self, names: &[String]) {
        let mut tys = HashMap::new();
        for name in names {
            if let Some(scheme) = self.env.values.remove(name) {
                tys.insert(name.clone(), apply(&self.subst, &scheme.body));
            }
        }
        for (name, ty) in tys {
            let scheme = self.generalize(&ty);
            self.env.insert_scheme(name, scheme);
        }
    }

    fn check_const(&mut self, c: &ConstDef) {
        // Shape/evaluation already handled in `check_const_cycles_and_fold` for
        // dependency-ordered folding; re-check here so consts skipped by the
        // topo pass (e.g. after a dependency failure) still report errors.
        if !self.const_vals.contains_key(&c.name) {
            if let Err(e) = check_const_expr(&c.value, &self.const_names, &self.const_vals) {
                self.errors.push(e);
            }
        }
        let inferred = self.infer_expr(&c.value);
        let expected = self
            .env
            .get(&c.name)
            .cloned()
            .map(|s| s.body)
            .unwrap_or_else(|| self.fresh());
        if let Err(e) = unify(&mut self.subst, &inferred, &expected, c.span) {
            self.errors.push(e);
        }
        if let Some(ann) = &c.ty {
            let saved = self.type_vars.clone();
            self.type_vars.clear();
            let ann_ty = self.ast_type(ann);
            self.type_vars = saved;
            if let Err(e) = unify(&mut self.subst, &inferred, &ann_ty, c.span) {
                self.errors.push(e);
            }
        }
        let final_ty = apply(&self.subst, &inferred);
        self.env.insert_mono(c.name.clone(), final_ty);
    }

    /// Check a function body. When `generalize_now` is false, leave a monomorphic
    /// binding so a recursive group can be generalized together afterward.
    fn check_fn(&mut self, f: &FnDef, generalize_now: bool) {
        let saved_env = self.env.clone();
        let saved_tvars = self.type_vars.clone();
        let saved_labels = self.fn_labels.clone();
        self.type_vars.clear();

        let placeholder = self
            .env
            .get(&f.name)
            .cloned()
            .map(|s| apply(&self.subst, &s.body));

        let (param_tys, ret_ty) = match placeholder {
            Some(Type::Fn { params, ret }) => (params, *ret),
            _ => {
                let params: Vec<Type> = f.params.iter().map(|_| self.fresh()).collect();
                (params, self.fresh())
            }
        };

        self.fn_labels.insert(
            f.name.clone(),
            f.params.iter().map(effective_param_label).collect(),
        );

        for (p, ty) in f.params.iter().zip(param_tys.iter()) {
            if let Some(ann) = &p.ty {
                let ann_ty = self.ast_type(ann);
                if let Err(e) = unify(&mut self.subst, ty, &ann_ty, p.span) {
                    self.errors.push(e);
                }
            }
            self.env.insert_mono(p.name.clone(), apply(&self.subst, ty));
        }

        let body_ty = self.infer_block_scoped(&f.body);
        if let Err(e) = unify(&mut self.subst, &body_ty, &ret_ty, f.span) {
            self.errors.push(e);
        }
        if let Some(ann) = &f.return_type {
            let ann_ty = self.ast_type(ann);
            if let Err(e) = unify(&mut self.subst, &ret_ty, &ann_ty, f.span) {
                self.errors.push(e);
            }
        }

        let fn_ty = apply(
            &self.subst,
            &Type::Fn {
                params: param_tys,
                ret: Box::new(ret_ty),
            },
        );
        self.env = saved_env;
        self.type_vars = saved_tvars;
        self.fn_labels = saved_labels;
        self.fn_labels.insert(
            f.name.clone(),
            f.params.iter().map(effective_param_label).collect(),
        );
        if generalize_now {
            self.env.values.remove(&f.name);
            let scheme = self.generalize(&fn_ty);
            self.env.insert_scheme(f.name.clone(), scheme);
        } else {
            self.env.insert_mono(f.name.clone(), fn_ty);
        }
    }

    fn generalize(&self, ty: &Type) -> Scheme {
        let applied = apply(&self.subst, ty);
        let mut env_vars = HashSet::new();
        for scheme in self.env.values.values() {
            for v in free_vars(&apply(&self.subst, &scheme.body)) {
                // Quantified vars in the scheme are not free in the env.
                if !scheme.vars.contains(&v) {
                    env_vars.insert(v);
                }
            }
            for v in &scheme.vars {
                // Instantiations may have replaced these; track applied body only.
                let _ = v;
            }
        }
        let vars: Vec<u32> = free_vars(&applied)
            .into_iter()
            .filter(|v| !env_vars.contains(v))
            .collect();
        Scheme {
            vars,
            body: applied,
        }
    }

    fn bind_pattern(&mut self, pattern: &Pattern, ty: &Type) {
        let ty = apply(&self.subst, ty);
        match &pattern.kind {
            PatternKind::Var(name) => {
                self.env.insert_mono(name.clone(), ty);
            }
            PatternKind::As {
                pattern: inner,
                name,
            } => {
                self.bind_pattern(inner, &ty);
                self.env.insert_mono(name.clone(), apply(&self.subst, &ty));
            }
            PatternKind::Discard => {}
            PatternKind::Int(_) => {
                if let Err(e) = unify(&mut self.subst, &ty, &Type::int(), pattern.span) {
                    self.errors.push(e);
                }
            }
            PatternKind::Float(_) => {
                if let Err(e) = unify(&mut self.subst, &ty, &Type::float(), pattern.span) {
                    self.errors.push(e);
                }
            }
            PatternKind::String(_) => {
                if let Err(e) = unify(&mut self.subst, &ty, &Type::string(), pattern.span) {
                    self.errors.push(e);
                }
            }
            PatternKind::StringPrefix { rest, .. } => {
                if let Err(e) = unify(&mut self.subst, &ty, &Type::string(), pattern.span) {
                    self.errors.push(e);
                }
                self.bind_pattern(rest, &Type::string());
            }
            PatternKind::BitArray(_) => {
                if let Err(e) = unify(&mut self.subst, &ty, &Type::bit_array(), pattern.span) {
                    self.errors.push(e);
                }
            }
            PatternKind::Constructor {
                module,
                name,
                fields,
                with_spread,
            } => {
                if module.is_some() {
                    // Qualified constructor from an imported module stub.
                    for f in fields {
                        let ft = self.fresh();
                        self.bind_pattern(&f.pattern, &ft);
                    }
                } else {
                    let ctor_ty = if let Some(scheme) = self.ctors.get(name).cloned() {
                        self.instantiate(&scheme)
                    } else if let Some(scheme) = self.env.get(name).cloned() {
                        self.instantiate(&scheme)
                    } else {
                        self.errors.push(TypeError::Unbound {
                            span: pattern.span,
                            name: name.clone(),
                        });
                        let field_tys: Vec<Type> = fields.iter().map(|_| self.fresh()).collect();
                        if field_tys.is_empty() && !*with_spread {
                            ty.clone()
                        } else {
                            Type::Fn {
                                params: field_tys,
                                ret: Box::new(ty.clone()),
                            }
                        }
                    };
                    let ctor_ty = apply(&self.subst, &ctor_ty);
                    match ctor_ty {
                        Type::Fn { params, ret } => {
                            if let Err(e) = unify(&mut self.subst, &ret, &ty, pattern.span) {
                                self.errors.push(e);
                            }
                            let labels = self.fn_labels.get(name).cloned();
                            match align_pattern_fields(
                                fields,
                                labels.as_deref(),
                                params.len(),
                                *with_spread,
                                pattern.span,
                            ) {
                                Ok(ordered) => {
                                    for (slot, ft) in ordered.iter().zip(params.iter()) {
                                        if let Some(f) = slot {
                                            self.bind_pattern(&f.pattern, ft);
                                        }
                                    }
                                }
                                Err(e) => self.errors.push(e),
                            }
                        }
                        other => {
                            if fields.is_empty() {
                                if let Err(e) = unify(&mut self.subst, &other, &ty, pattern.span) {
                                    self.errors.push(e);
                                }
                            } else {
                                self.errors.push(TypeError::Other {
                                    span: pattern.span,
                                    message: format!("constructor `{name}` does not take fields"),
                                });
                            }
                        }
                    }
                }
            }
            PatternKind::Tuple(elems) => {
                if let Type::Tuple(tys) = &ty {
                    for (p, t) in elems.iter().zip(tys.iter()) {
                        self.bind_pattern(p, t);
                    }
                } else {
                    let elem_tys: Vec<Type> = elems.iter().map(|_| self.fresh()).collect();
                    let expected = Type::Tuple(elem_tys.clone());
                    if let Err(e) = unify(&mut self.subst, &ty, &expected, pattern.span) {
                        self.errors.push(e);
                    }
                    for (p, t) in elems.iter().zip(elem_tys.iter()) {
                        self.bind_pattern(p, t);
                    }
                }
            }
            PatternKind::List { items, rest } => {
                let elem = self.fresh();
                if let Err(e) = unify(
                    &mut self.subst,
                    &ty,
                    &Type::list(elem.clone()),
                    pattern.span,
                ) {
                    self.errors.push(e);
                }
                for p in items {
                    self.bind_pattern(p, &elem);
                }
                if let Some(r) = rest {
                    self.bind_pattern(r, &Type::list(elem));
                }
            }
        }
    }

    fn infer_expr(&mut self, expr: &Expr) -> Type {
        match &expr.kind {
            ExprKind::Int(lit) => {
                if let Err(e) = validate_int_literal(lit, expr.span) {
                    self.errors.push(e);
                }
                Type::int()
            }
            ExprKind::Float(lit) => {
                if let Err(e) = validate_float_literal(lit, expr.span) {
                    self.errors.push(e);
                }
                Type::float()
            }
            ExprKind::String(_) => Type::string(),
            ExprKind::Ident(name) => self.lookup_value(name, expr.span),
            ExprKind::Constructor(name) => self.lookup_value(name, expr.span),
            ExprKind::Field { base, field, .. } => {
                let inferred = self.infer_expr(base);
                let base_ty = apply(&self.subst, &inferred);
                if let Type::Named { name, .. } = &base_ty {
                    if name.starts_with("Module_") {
                        // Imported module member: flexible stub.
                        let _ = field;
                        return self.fresh();
                    }
                }
                let _ = field;
                self.fresh()
            }
            ExprKind::Call { callee, args } => self.infer_call(callee, args, expr.span),
            ExprKind::Binary { left, op, right } => {
                let lt = self.infer_expr(left);
                let rt = self.infer_expr(right);
                self.infer_binop(*op, lt, rt, expr.span)
            }
            ExprKind::Unary { op, expr: inner } => {
                match op {
                    UnaryOp::Neg => {
                        // Spec §5.7: a directly negated integer literal may have magnitude
                        // 2^63 and denotes MIN_INT; the same positive literal is invalid.
                        if let ExprKind::Int(lit) = &inner.kind {
                            let cleaned: String = lit.chars().filter(|c| *c != '_').collect();
                            if cleaned == "9223372036854775808" {
                                return Type::int();
                            }
                        }
                        let t = self.infer_expr(inner);
                        if let Err(e) = unify(&mut self.subst, &t, &Type::int(), expr.span) {
                            if unify(&mut self.subst, &t, &Type::float(), expr.span).is_err() {
                                self.errors.push(e);
                            }
                            Type::float()
                        } else {
                            Type::int()
                        }
                    }
                    UnaryOp::Not => {
                        let t = self.infer_expr(inner);
                        if let Err(e) = unify(&mut self.subst, &t, &Type::bool(), expr.span) {
                            self.errors.push(e);
                        }
                        Type::bool()
                    }
                }
            }
            ExprKind::Pipe { left, right } => {
                // Infer left once; never rebuild a Call that re-infers it.
                let left_ty = self.infer_expr(left);
                match &right.kind {
                    ExprKind::Call { callee, args } => {
                        let labels = match &callee.kind {
                            ExprKind::Ident(name) | ExprKind::Constructor(name) => {
                                self.fn_labels.get(name).cloned()
                            }
                            ExprKind::Field { field, .. } => self.fn_labels.get(field).cloned(),
                            _ => None,
                        };
                        let ordered = match reorder_args(args, labels.as_deref()) {
                            Ok(o) => o,
                            Err(e) => {
                                self.errors.push(e);
                                args.to_vec()
                            }
                        };
                        let has_hole = ordered.iter().any(|a| matches!(a.value, ArgValue::Hole));
                        let mut arg_tys = Vec::new();
                        if !has_hole {
                            // Pipe supplies the first parameter; reject an explicit
                            // argument that uses that parameter's label.
                            if let Some(Some(first_label)) =
                                labels.as_deref().and_then(|ls| ls.first())
                            {
                                if let Some(arg) = ordered
                                    .iter()
                                    .find(|a| a.label.as_ref() == Some(first_label))
                                {
                                    self.errors.push(TypeError::Other {
                                        span: arg.span,
                                        message: format!(
                                            "duplicate argument label `{first_label}`"
                                        ),
                                    });
                                }
                            }
                            arg_tys.push(left_ty.clone());
                        }
                        for arg in &ordered {
                            match &arg.value {
                                ArgValue::Hole => arg_tys.push(left_ty.clone()),
                                ArgValue::Expr(e) => arg_tys.push(self.infer_expr(e)),
                            }
                        }
                        let callee_ty = self.infer_expr(callee);
                        let ret = self.fresh();
                        let expected = Type::Fn {
                            params: arg_tys,
                            ret: Box::new(ret.clone()),
                        };
                        if let Err(e) = unify(&mut self.subst, &callee_ty, &expected, expr.span) {
                            self.errors.push(e);
                        }
                        ret
                    }
                    _ => {
                        let callee_ty = self.infer_expr(right);
                        let ret = self.fresh();
                        let expected = Type::Fn {
                            params: vec![left_ty],
                            ret: Box::new(ret.clone()),
                        };
                        if let Err(e) = unify(&mut self.subst, &callee_ty, &expected, expr.span) {
                            self.errors.push(e);
                        }
                        ret
                    }
                }
            }
            ExprKind::Fn {
                params,
                return_type,
                body,
            } => {
                let saved = self.env.clone();
                let saved_tvars = self.type_vars.clone();
                self.type_vars.clear();
                let mut pts = Vec::new();
                for p in params {
                    let ty =
                        p.ty.as_ref()
                            .map(|t| self.ast_type(t))
                            .unwrap_or_else(|| self.fresh());
                    self.env.insert_mono(p.name.clone(), ty.clone());
                    pts.push(ty);
                }
                let body_ty = self.infer_block_scoped(body);
                if let Some(ret) = return_type {
                    let r = self.ast_type(ret);
                    if let Err(e) = unify(&mut self.subst, &body_ty, &r, expr.span) {
                        self.errors.push(e);
                    }
                }
                self.env = saved;
                self.type_vars = saved_tvars;
                Type::Fn {
                    params: pts,
                    ret: Box::new(body_ty),
                }
            }
            ExprKind::Case { subjects, clauses } => {
                let subject_tys: Vec<Type> = subjects.iter().map(|s| self.infer_expr(s)).collect();
                let result = self.fresh();
                for clause in clauses {
                    let saved = self.env.clone();
                    // Each alternative is checked in isolation; only names bound by
                    // every alternative are visible in the clause body.
                    let mut common: Option<HashMap<String, Type>> = None;
                    for row in &clause.patterns {
                        self.env = saved.clone();
                        if row.patterns.len() != subject_tys.len() {
                            self.errors.push(TypeError::Other {
                                span: clause.span,
                                message: "case pattern count must match subject count".into(),
                            });
                            continue;
                        }
                        let mut introduced = Vec::new();
                        for (p, st) in row.patterns.iter().zip(subject_tys.iter()) {
                            pattern_bound_names(p, &mut introduced);
                            self.bind_pattern(p, st);
                        }
                        let mut bound = HashMap::new();
                        for name in introduced {
                            if let Some(scheme) = self.env.get(&name) {
                                bound.insert(name, apply(&self.subst, &scheme.body));
                            }
                        }
                        match &mut common {
                            None => common = Some(bound),
                            Some(shared) => {
                                shared.retain(|name, ty| {
                                    if let Some(other) = bound.get(name) {
                                        if let Err(e) =
                                            unify(&mut self.subst, ty, other, clause.span)
                                        {
                                            self.errors.push(e);
                                        }
                                        *ty = apply(&self.subst, ty);
                                        true
                                    } else {
                                        false
                                    }
                                });
                            }
                        }
                    }
                    self.env = saved.clone();
                    if let Some(shared) = common {
                        for (name, ty) in shared {
                            self.env.insert_mono(name, ty);
                        }
                    }
                    if let Some(g) = &clause.guard {
                        let gt = self.infer_expr(g);
                        if let Err(e) = unify(&mut self.subst, &gt, &Type::bool(), g.span) {
                            self.errors.push(e);
                        }
                    }
                    let bt = self.infer_expr(&clause.body);
                    if let Err(e) = unify(&mut self.subst, &bt, &result, clause.span) {
                        self.errors.push(e);
                    }
                    self.env = saved;
                    // Note: exhaustiveness is intentionally deferred in this prototype.
                }
                result
            }
            ExprKind::Todo { .. } | ExprKind::Panic { .. } => self.fresh(),
            ExprKind::Assert { condition, .. } => {
                let t = self.infer_expr(condition);
                if let Err(e) = unify(&mut self.subst, &t, &Type::bool(), expr.span) {
                    self.errors.push(e);
                }
                Type::nil()
            }
            ExprKind::Echo { value } => self.infer_expr(value),
            ExprKind::Group(inner) => self.infer_expr(inner),
            ExprKind::Block(b) => {
                let saved = self.env.clone();
                let saved_labels = self.fn_labels.clone();
                let t = self.infer_block_scoped(b);
                self.env = saved;
                self.fn_labels = saved_labels;
                t
            }
            ExprKind::List { items, spread } => {
                let elem = self.fresh();
                for item in items {
                    let t = self.infer_expr(item);
                    if let Err(e) = unify(&mut self.subst, &t, &elem, item.span) {
                        self.errors.push(e);
                    }
                }
                if let Some(s) = spread {
                    let st = self.infer_expr(s);
                    if let Err(e) = unify(&mut self.subst, &st, &Type::list(elem.clone()), s.span) {
                        self.errors.push(e);
                    }
                }
                Type::list(elem)
            }
            ExprKind::Tuple(elems) => {
                Type::Tuple(elems.iter().map(|e| self.infer_expr(e)).collect())
            }
            ExprKind::BitArray(segs) => {
                for seg in segs {
                    match &seg.value {
                        BitSegmentValue::Expr(e) => {
                            let _ = self.infer_expr(e);
                        }
                        BitSegmentValue::Pattern(p) => {
                            let t = self.fresh();
                            self.bind_pattern(p, &t);
                        }
                    }
                }
                Type::bit_array()
            }
            ExprKind::RecordUpdate { base, fields, .. } => {
                let base_ty = self.infer_expr(base);
                for (_, v) in fields {
                    let _ = self.infer_expr(v);
                }
                base_ty
            }
        }
    }

    fn infer_call(&mut self, callee: &Expr, args: &[Arg], span: Span) -> Type {
        let callee_ty = self.infer_expr(callee);
        let labels = match &callee.kind {
            ExprKind::Ident(name) | ExprKind::Constructor(name) => {
                self.fn_labels.get(name).cloned()
            }
            ExprKind::Field { field, .. } => self.fn_labels.get(field).cloned(),
            _ => None,
        };
        let ordered = match reorder_args(args, labels.as_deref()) {
            Ok(o) => o,
            Err(e) => {
                self.errors.push(e);
                args.to_vec()
            }
        };

        let mut arg_tys = Vec::new();
        let mut hole_ty = None;
        for arg in &ordered {
            match &arg.value {
                ArgValue::Hole => {
                    let t = self.fresh();
                    hole_ty = Some(t.clone());
                    arg_tys.push(t);
                }
                ArgValue::Expr(e) => arg_tys.push(self.infer_expr(e)),
            }
        }
        let ret = self.fresh();
        let expected = Type::Fn {
            params: arg_tys,
            ret: Box::new(ret.clone()),
        };
        // Always go through unify so occurs-check is never bypassed.
        if let Err(e) = unify(&mut self.subst, &callee_ty, &expected, span) {
            self.errors.push(e);
        }
        if let Some(h) = hole_ty {
            Type::Fn {
                params: vec![h],
                ret: Box::new(ret),
            }
        } else {
            ret
        }
    }

    /// Infer a block; `let`/`fn` bindings stay in scope for later statements.
    /// The result type is the type of the final statement (expression), or `Nil`.
    fn infer_block_scoped(&mut self, block: &Block) -> Type {
        if block.statements.is_empty() {
            return Type::nil();
        }
        let last_idx = block.statements.len() - 1;
        let mut result = Type::nil();
        let mut i = 0;
        while i < block.statements.len() {
            match &block.statements[i] {
                Statement::Fn(_) => {
                    let start = i;
                    while i < block.statements.len()
                        && matches!(block.statements[i], Statement::Fn(_))
                    {
                        i += 1;
                    }
                    let mut names = Vec::new();
                    for stmt in &block.statements[start..i] {
                        if let Statement::Fn(f) = stmt {
                            names.push(f.name.clone());
                            let params: Vec<Type> = f.params.iter().map(|_| self.fresh()).collect();
                            let ret = self.fresh();
                            self.fn_labels.insert(
                                f.name.clone(),
                                f.params.iter().map(effective_param_label).collect(),
                            );
                            self.env.insert_mono(
                                f.name.clone(),
                                Type::Fn {
                                    params,
                                    ret: Box::new(ret),
                                },
                            );
                        }
                    }
                    for stmt in &block.statements[start..i] {
                        if let Statement::Fn(f) = stmt {
                            self.check_fn(f, false);
                        }
                    }
                    self.generalize_fn_group(&names);
                    if i - 1 == last_idx {
                        result = Type::nil();
                    }
                }
                Statement::Let(l) => {
                    let value_ty = self.infer_expr(&l.value);
                    if let Some(ann) = &l.ty {
                        let ann_ty = self.ast_type(ann);
                        if let Err(e) = unify(&mut self.subst, &value_ty, &ann_ty, l.span) {
                            self.errors.push(e);
                        }
                    }
                    if !l.is_assert && !is_irrefutable(&l.pattern) {
                        self.errors.push(TypeError::Other {
                            span: l.pattern.span,
                            message: "refutable pattern requires `let assert`".into(),
                        });
                    }
                    if let Some(msg) = &l.message {
                        let mt = self.infer_expr(msg);
                        if let Err(e) = unify(&mut self.subst, &mt, &Type::string(), msg.span) {
                            self.errors.push(e);
                        }
                    }
                    self.bind_pattern(&l.pattern, &value_ty);
                    if i == last_idx {
                        result = Type::nil();
                    }
                    i += 1;
                }
                Statement::Expr(e) => {
                    let t = self.infer_expr(e);
                    if i == last_idx {
                        result = t;
                    }
                    i += 1;
                }
                Statement::Use(_) => {
                    if i == last_idx {
                        result = Type::nil();
                    }
                    i += 1;
                }
            }
        }
        result
    }

    fn infer_binop(&mut self, op: BinOp, left: Type, right: Type, span: Span) -> Type {
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                if let Err(e) = unify(&mut self.subst, &left, &Type::int(), span) {
                    self.errors.push(e);
                }
                if let Err(e) = unify(&mut self.subst, &right, &Type::int(), span) {
                    self.errors.push(e);
                }
                Type::int()
            }
            BinOp::AddFloat | BinOp::SubFloat | BinOp::MulFloat | BinOp::DivFloat => {
                if let Err(e) = unify(&mut self.subst, &left, &Type::float(), span) {
                    self.errors.push(e);
                }
                if let Err(e) = unify(&mut self.subst, &right, &Type::float(), span) {
                    self.errors.push(e);
                }
                Type::float()
            }
            BinOp::Concat => {
                if let Err(e) = unify(&mut self.subst, &left, &Type::string(), span) {
                    self.errors.push(e);
                }
                if let Err(e) = unify(&mut self.subst, &right, &Type::string(), span) {
                    self.errors.push(e);
                }
                Type::string()
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                if let Err(e) = unify(&mut self.subst, &left, &Type::int(), span) {
                    self.errors.push(e);
                }
                if let Err(e) = unify(&mut self.subst, &right, &Type::int(), span) {
                    self.errors.push(e);
                }
                Type::bool()
            }
            BinOp::LtFloat | BinOp::LeFloat | BinOp::GtFloat | BinOp::GeFloat => {
                if let Err(e) = unify(&mut self.subst, &left, &Type::float(), span) {
                    self.errors.push(e);
                }
                if let Err(e) = unify(&mut self.subst, &right, &Type::float(), span) {
                    self.errors.push(e);
                }
                Type::bool()
            }
            BinOp::Eq | BinOp::NotEq => {
                if let Err(e) = unify(&mut self.subst, &left, &right, span) {
                    self.errors.push(e);
                }
                Type::bool()
            }
            BinOp::And | BinOp::Or => {
                if let Err(e) = unify(&mut self.subst, &left, &Type::bool(), span) {
                    self.errors.push(e);
                }
                if let Err(e) = unify(&mut self.subst, &right, &Type::bool(), span) {
                    self.errors.push(e);
                }
                Type::bool()
            }
        }
    }

    fn lookup_value(&mut self, name: &str, span: Span) -> Type {
        if let Some(scheme) = self.env.get(name).cloned() {
            return self.instantiate(&scheme);
        }
        if let Some(scheme) = self.ctors.get(name).cloned() {
            return self.instantiate(&scheme);
        }
        // Do not invent a binding — that created cyclic types for `x(x)`.
        self.errors.push(TypeError::Unbound {
            span,
            name: name.into(),
        });
        self.fresh()
    }

    fn instantiate(&mut self, scheme: &Scheme) -> Type {
        let mut map = HashMap::new();
        for v in &scheme.vars {
            map.insert(*v, self.fresh());
        }
        apply_map(&map, &scheme.body)
    }

    fn expand_alias(&mut self, name: &str, args: &[TypeExpr], span: Span) -> Option<Type> {
        let (params, body) = self.aliases.get(name)?.clone();
        if params.len() != args.len() {
            return None;
        }
        if self.alias_stack.iter().any(|n| n == name) {
            self.errors.push(TypeError::Other {
                span,
                message: format!("cyclic type alias `{name}`"),
            });
            return Some(Type::Named {
                module: None,
                name: name.into(),
                args: args.iter().map(|a| self.ast_type(a)).collect(),
            });
        }
        self.alias_stack.push(name.into());
        let saved = self.type_vars.clone();
        for (p, a) in params.iter().zip(args.iter()) {
            let at = self.ast_type(a);
            self.type_vars.insert(p.clone(), at);
        }
        let expanded = self.ast_type(&body);
        self.type_vars = saved;
        self.alias_stack.pop();
        Some(expanded)
    }

    fn ast_type(&mut self, ty: &TypeExpr) -> Type {
        match &ty.kind {
            TypeKind::Var(name) => {
                if let Some(t) = self.type_vars.get(name) {
                    return t.clone();
                }
                // Lowercase names are type variables; uppercase are nominal types.
                if name.starts_with(|c: char| c.is_ascii_lowercase()) {
                    let v = self.fresh();
                    self.type_vars.insert(name.clone(), v.clone());
                    v
                } else if let Some(expanded) = self.expand_alias(name, &[], ty.span) {
                    expanded
                } else {
                    Type::Named {
                        module: None,
                        name: name.clone(),
                        args: vec![],
                    }
                }
            }
            TypeKind::Named { module, name, args } => {
                if module.is_none() {
                    if let Some(expanded) = self.expand_alias(name, args, ty.span) {
                        return expanded;
                    }
                }
                Type::Named {
                    module: module.clone(),
                    name: name.clone(),
                    args: args.iter().map(|a| self.ast_type(a)).collect(),
                }
            }
            TypeKind::Fn { params, ret } => Type::Fn {
                params: params.iter().map(|p| self.ast_type(p)).collect(),
                ret: Box::new(self.ast_type(ret)),
            },
            TypeKind::Tuple(elems) => Type::Tuple(elems.iter().map(|e| self.ast_type(e)).collect()),
        }
    }
}

/// External label for a parameter: explicit `label name` or the binding name.
fn effective_param_label(p: &Param) -> Option<String> {
    p.label.clone().or_else(|| Some(p.name.clone()))
}

/// Names introduced by a pattern (including those that shadow outer bindings).
fn pattern_bound_names(pattern: &Pattern, out: &mut Vec<String>) {
    match &pattern.kind {
        PatternKind::Var(name) => out.push(name.clone()),
        PatternKind::As { pattern, name } => {
            pattern_bound_names(pattern, out);
            out.push(name.clone());
        }
        PatternKind::Constructor { fields, .. } => {
            for f in fields {
                pattern_bound_names(&f.pattern, out);
            }
        }
        PatternKind::Tuple(elems) => {
            for p in elems {
                pattern_bound_names(p, out);
            }
        }
        PatternKind::List { items, rest } => {
            for p in items {
                pattern_bound_names(p, out);
            }
            if let Some(r) = rest {
                pattern_bound_names(r, out);
            }
        }
        PatternKind::StringPrefix { rest, .. } => pattern_bound_names(rest, out),
        PatternKind::BitArray(segs) => {
            for seg in segs {
                if let BitSegmentValue::Pattern(p) = &seg.value {
                    pattern_bound_names(p, out);
                }
            }
        }
        PatternKind::Discard
        | PatternKind::Int(_)
        | PatternKind::Float(_)
        | PatternKind::String(_) => {}
    }
}

fn is_irrefutable(pattern: &Pattern) -> bool {
    match &pattern.kind {
        PatternKind::Var(_) | PatternKind::Discard => true,
        PatternKind::As { pattern, .. } => is_irrefutable(pattern),
        PatternKind::Tuple(elems) => elems.iter().all(is_irrefutable),
        _ => false,
    }
}

fn validate_int_literal(lit: &str, span: Span) -> Result<(), TypeError> {
    parse_int_literal(lit)
        .map(|_| ())
        .ok_or_else(|| TypeError::Other {
            span,
            message: format!("integer literal `{lit}` out of Int range"),
        })
}

fn validate_float_literal(lit: &str, span: Span) -> Result<(), TypeError> {
    let cleaned = lit.replace('_', "");
    match cleaned.parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(()),
        _ => Err(TypeError::Other {
            span,
            message: format!("float literal `{lit}` is not a finite binary64 value"),
        }),
    }
}

/// Collect identifiers in a constant expression that refer to other constants.
fn collect_const_refs(expr: &Expr, const_names: &HashSet<String>, out: &mut HashSet<String>) {
    match &expr.kind {
        ExprKind::Ident(name) => {
            if const_names.contains(name) {
                out.insert(name.clone());
            }
        }
        ExprKind::Group(inner)
        | ExprKind::Unary { expr: inner, .. }
        | ExprKind::Echo { value: inner } => collect_const_refs(inner, const_names, out),
        ExprKind::Binary { left, right, .. } => {
            collect_const_refs(left, const_names, out);
            collect_const_refs(right, const_names, out);
        }
        ExprKind::Tuple(elems) => {
            for e in elems {
                collect_const_refs(e, const_names, out);
            }
        }
        ExprKind::List { items, spread } => {
            for e in items {
                collect_const_refs(e, const_names, out);
            }
            if let Some(s) = spread {
                collect_const_refs(s, const_names, out);
            }
        }
        ExprKind::Call { callee, args } => {
            collect_const_refs(callee, const_names, out);
            for a in args {
                if let ArgValue::Expr(e) = &a.value {
                    collect_const_refs(e, const_names, out);
                }
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Constructor(_)
        | ExprKind::Fn { .. }
        | ExprKind::Pipe { .. }
        | ExprKind::Case { .. }
        | ExprKind::Block(_)
        | ExprKind::Todo { .. }
        | ExprKind::Panic { .. }
        | ExprKind::Assert { .. }
        | ExprKind::Field { .. }
        | ExprKind::RecordUpdate { .. }
        | ExprKind::BitArray(_) => {}
    }
}

/// Folded constant value used for evaluation-failure checks (`spec.md` §5.7).
#[derive(Debug, Clone)]
enum ConstVal {
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
    /// Well-formed but not folded further in this prototype.
    Opaque,
}

/// Constant-expression check (`spec.md` §5.7 / constants).
/// Validates shape and evaluates foldable arithmetic so division-by-zero and
/// overflow are compile errors.
fn check_const_expr(
    expr: &Expr,
    const_names: &HashSet<String>,
    const_env: &HashMap<String, ConstVal>,
) -> Result<(), TypeError> {
    eval_const_expr(expr, const_names, const_env).map(|_| ())
}

fn eval_const_expr(
    expr: &Expr,
    const_names: &HashSet<String>,
    const_env: &HashMap<String, ConstVal>,
) -> Result<ConstVal, TypeError> {
    match &expr.kind {
        ExprKind::Int(lit) => {
            validate_int_literal(lit, expr.span)?;
            Ok(ConstVal::Int(
                parse_int_literal(lit).expect("validated int"),
            ))
        }
        ExprKind::Float(lit) => {
            validate_float_literal(lit, expr.span)?;
            let cleaned = lit.replace('_', "");
            Ok(ConstVal::Float(
                cleaned.parse::<f64>().expect("validated float"),
            ))
        }
        ExprKind::String(s) => Ok(ConstVal::String(s.clone())),
        ExprKind::Ident(name) => {
            if let Some(v) = const_env.get(name) {
                Ok(v.clone())
            } else if const_names.contains(name) {
                Ok(ConstVal::Opaque)
            } else {
                Err(TypeError::Other {
                    span: expr.span,
                    message: format!("`{name}` is not a constant reference"),
                })
            }
        }
        ExprKind::Constructor(_) => Ok(ConstVal::Opaque),
        ExprKind::Group(inner) => eval_const_expr(inner, const_names, const_env),
        ExprKind::Echo { .. } => Err(TypeError::Other {
            span: expr.span,
            message: "echo is not allowed in a constant".into(),
        }),
        ExprKind::Unary { op, expr: inner } => match op {
            UnaryOp::Neg => {
                // Direct `-9223372036854775808` denotes MIN_INT (§5.7); the positive
                // magnitude is not a valid standalone Int literal.
                if let ExprKind::Int(lit) = &inner.kind {
                    let cleaned: String = lit.chars().filter(|c| *c != '_').collect();
                    if cleaned == "9223372036854775808" {
                        return Ok(ConstVal::Int(i64::MIN));
                    }
                }
                match eval_const_expr(inner, const_names, const_env)? {
                    ConstVal::Int(i) => {
                        i.checked_neg()
                            .map(ConstVal::Int)
                            .ok_or_else(|| TypeError::Other {
                                span: expr.span,
                                message: "integer overflow in constant".into(),
                            })
                    }
                    ConstVal::Float(f) => {
                        let r = -f;
                        if r.is_finite() {
                            Ok(ConstVal::Float(r))
                        } else {
                            Err(TypeError::Other {
                                span: expr.span,
                                message: "float overflow in constant".into(),
                            })
                        }
                    }
                    ConstVal::Opaque => Ok(ConstVal::Opaque),
                    _ => Err(TypeError::Other {
                        span: expr.span,
                        message: "invalid operand for negation in constant".into(),
                    }),
                }
            }
            UnaryOp::Not => match eval_const_expr(inner, const_names, const_env)? {
                ConstVal::Bool(b) => Ok(ConstVal::Bool(!b)),
                ConstVal::Opaque => Ok(ConstVal::Opaque),
                _ => Err(TypeError::Other {
                    span: expr.span,
                    message: "invalid operand for `!` in constant".into(),
                }),
            },
        },
        ExprKind::Binary { left, op, right } => {
            let l = eval_const_expr(left, const_names, const_env)?;
            let r = eval_const_expr(right, const_names, const_env)?;
            eval_const_binop(*op, l, r, expr.span)
        }
        ExprKind::Tuple(elems) => {
            for e in elems {
                eval_const_expr(e, const_names, const_env)?;
            }
            Ok(ConstVal::Opaque)
        }
        ExprKind::List { items, spread } => {
            for e in items {
                eval_const_expr(e, const_names, const_env)?;
            }
            if let Some(s) = spread {
                eval_const_expr(s, const_names, const_env)?;
            }
            Ok(ConstVal::Opaque)
        }
        ExprKind::Call { callee, args } => {
            // ADT construction only; no general function calls in constants.
            if !matches!(callee.kind, ExprKind::Constructor(_)) {
                return Err(TypeError::Other {
                    span: expr.span,
                    message: "function calls are not allowed in constants".into(),
                });
            }
            for a in args {
                if let ArgValue::Expr(e) = &a.value {
                    eval_const_expr(e, const_names, const_env)?;
                }
            }
            Ok(ConstVal::Opaque)
        }
        ExprKind::Fn { .. }
        | ExprKind::Pipe { .. }
        | ExprKind::Case { .. }
        | ExprKind::Block(_)
        | ExprKind::Todo { .. }
        | ExprKind::Panic { .. }
        | ExprKind::Assert { .. }
        | ExprKind::Field { .. }
        | ExprKind::RecordUpdate { .. }
        | ExprKind::BitArray(_) => Err(TypeError::Other {
            span: expr.span,
            message: "expression is not allowed in a constant".into(),
        }),
    }
}

fn eval_const_binop(
    op: BinOp,
    left: ConstVal,
    right: ConstVal,
    span: Span,
) -> Result<ConstVal, TypeError> {
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => match (left, right) {
            (ConstVal::Int(a), ConstVal::Int(b)) => {
                let result = match op {
                    BinOp::Add => a.checked_add(b),
                    BinOp::Sub => a.checked_sub(b),
                    BinOp::Mul => a.checked_mul(b),
                    BinOp::Div => {
                        if b == 0 || (a == i64::MIN && b == -1) {
                            return Err(TypeError::Other {
                                span,
                                message: "integer division error in constant".into(),
                            });
                        }
                        Some(a / b)
                    }
                    BinOp::Rem => {
                        if b == 0 || (a == i64::MIN && b == -1) {
                            return Err(TypeError::Other {
                                span,
                                message: "integer remainder error in constant".into(),
                            });
                        }
                        Some(a % b)
                    }
                    _ => unreachable!(),
                };
                result.map(ConstVal::Int).ok_or_else(|| TypeError::Other {
                    span,
                    message: "integer overflow in constant".into(),
                })
            }
            (ConstVal::Opaque, _) | (_, ConstVal::Opaque) => Ok(ConstVal::Opaque),
            _ => Err(TypeError::Other {
                span,
                message: "invalid operands for integer operator in constant".into(),
            }),
        },
        BinOp::AddFloat | BinOp::SubFloat | BinOp::MulFloat | BinOp::DivFloat => {
            match (left, right) {
                (ConstVal::Float(a), ConstVal::Float(b)) => {
                    if matches!(op, BinOp::DivFloat) && b == 0.0 {
                        return Err(TypeError::Other {
                            span,
                            message: "float division by zero in constant".into(),
                        });
                    }
                    let r = match op {
                        BinOp::AddFloat => a + b,
                        BinOp::SubFloat => a - b,
                        BinOp::MulFloat => a * b,
                        BinOp::DivFloat => a / b,
                        _ => unreachable!(),
                    };
                    if r.is_finite() {
                        Ok(ConstVal::Float(r))
                    } else {
                        Err(TypeError::Other {
                            span,
                            message: "float overflow in constant".into(),
                        })
                    }
                }
                (ConstVal::Opaque, _) | (_, ConstVal::Opaque) => Ok(ConstVal::Opaque),
                _ => Err(TypeError::Other {
                    span,
                    message: "invalid operands for float operator in constant".into(),
                }),
            }
        }
        BinOp::Concat => match (left, right) {
            (ConstVal::String(a), ConstVal::String(b)) => Ok(ConstVal::String(a + &b)),
            (ConstVal::Opaque, _) | (_, ConstVal::Opaque) => Ok(ConstVal::Opaque),
            _ => Err(TypeError::Other {
                span,
                message: "invalid operands for `<>` in constant".into(),
            }),
        },
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => match (left, right) {
            (ConstVal::Int(a), ConstVal::Int(b)) => Ok(ConstVal::Bool(match op {
                BinOp::Lt => a < b,
                BinOp::Le => a <= b,
                BinOp::Gt => a > b,
                BinOp::Ge => a >= b,
                _ => unreachable!(),
            })),
            (ConstVal::Opaque, _) | (_, ConstVal::Opaque) => Ok(ConstVal::Opaque),
            _ => Err(TypeError::Other {
                span,
                message: "invalid operands for integer comparison in constant".into(),
            }),
        },
        BinOp::LtFloat | BinOp::LeFloat | BinOp::GtFloat | BinOp::GeFloat => match (left, right) {
            (ConstVal::Float(a), ConstVal::Float(b)) => Ok(ConstVal::Bool(match op {
                BinOp::LtFloat => a < b,
                BinOp::LeFloat => a <= b,
                BinOp::GtFloat => a > b,
                BinOp::GeFloat => a >= b,
                _ => unreachable!(),
            })),
            (ConstVal::Opaque, _) | (_, ConstVal::Opaque) => Ok(ConstVal::Opaque),
            _ => Err(TypeError::Other {
                span,
                message: "invalid operands for float comparison in constant".into(),
            }),
        },
        BinOp::Eq | BinOp::NotEq => match (left, right) {
            (ConstVal::Int(a), ConstVal::Int(b)) => Ok(ConstVal::Bool(if op == BinOp::Eq {
                a == b
            } else {
                a != b
            })),
            (ConstVal::Float(a), ConstVal::Float(b)) => Ok(ConstVal::Bool(if op == BinOp::Eq {
                a == b
            } else {
                a != b
            })),
            (ConstVal::String(a), ConstVal::String(b)) => Ok(ConstVal::Bool(if op == BinOp::Eq {
                a == b
            } else {
                a != b
            })),
            (ConstVal::Bool(a), ConstVal::Bool(b)) => Ok(ConstVal::Bool(if op == BinOp::Eq {
                a == b
            } else {
                a != b
            })),
            (ConstVal::Opaque, _) | (_, ConstVal::Opaque) => Ok(ConstVal::Opaque),
            _ => Err(TypeError::Other {
                span,
                message: "invalid operands for equality in constant".into(),
            }),
        },
        BinOp::And | BinOp::Or => match (left, right) {
            (ConstVal::Bool(a), ConstVal::Bool(b)) => Ok(ConstVal::Bool(if op == BinOp::And {
                a && b
            } else {
                a || b
            })),
            (ConstVal::Opaque, _) | (_, ConstVal::Opaque) => Ok(ConstVal::Opaque),
            _ => Err(TypeError::Other {
                span,
                message: "invalid operands for boolean operator in constant".into(),
            }),
        },
    }
}

fn parse_int_literal(lit: &str) -> Option<i64> {
    let cleaned: String = lit.chars().filter(|c| *c != '_').collect();
    if let Some(rest) = cleaned
        .strip_prefix("0x")
        .or_else(|| cleaned.strip_prefix("0X"))
    {
        i64::from_str_radix(rest, 16).ok()
    } else if let Some(rest) = cleaned
        .strip_prefix("0o")
        .or_else(|| cleaned.strip_prefix("0O"))
    {
        i64::from_str_radix(rest, 8).ok()
    } else if let Some(rest) = cleaned
        .strip_prefix("0b")
        .or_else(|| cleaned.strip_prefix("0B"))
    {
        i64::from_str_radix(rest, 2).ok()
    } else {
        cleaned.parse::<i64>().ok()
    }
}

/// Align constructor pattern fields to parameter order.
/// When `with_spread` is set, omitted fields remain `None` instead of erroring.
fn align_pattern_fields(
    fields: &[PatternField],
    labels: Option<&[Option<String>]>,
    param_count: usize,
    with_spread: bool,
    span: Span,
) -> Result<Vec<Option<PatternField>>, TypeError> {
    let labelled = labels.is_some_and(|ls| ls.iter().any(|l| l.is_some()))
        && fields.iter().any(|f| f.label.is_some());
    if !labelled {
        if fields.len() > param_count || (!with_spread && fields.len() != param_count) {
            return Err(TypeError::Other {
                span,
                message: "constructor pattern field count mismatch".into(),
            });
        }
        let mut slots: Vec<Option<PatternField>> = fields.iter().cloned().map(Some).collect();
        slots.resize(param_count, None);
        return Ok(slots);
    }
    let labels = labels.expect("labelled branch");
    let mut slots: Vec<Option<PatternField>> = vec![None; param_count.max(labels.len())];
    let mut unlabelled = Vec::new();
    for field in fields {
        if let Some(label) = &field.label {
            let Some(idx) = labels.iter().position(|l| l.as_ref() == Some(label)) else {
                return Err(TypeError::Other {
                    span: field.span,
                    message: format!("unknown field label `{label}`"),
                });
            };
            if idx >= slots.len() {
                return Err(TypeError::Other {
                    span: field.span,
                    message: format!("unknown field label `{label}`"),
                });
            }
            if slots[idx].is_some() {
                return Err(TypeError::Other {
                    span: field.span,
                    message: format!("duplicate field label `{label}`"),
                });
            }
            slots[idx] = Some(field.clone());
        } else {
            unlabelled.push(field.clone());
        }
    }
    let mut ui = 0;
    for slot in &mut slots {
        if slot.is_none() {
            if let Some(f) = unlabelled.get(ui) {
                *slot = Some(f.clone());
                ui += 1;
            }
        }
    }
    if ui < unlabelled.len() {
        return Err(TypeError::Other {
            span,
            message: "too many fields in constructor pattern".into(),
        });
    }
    if !with_spread {
        if slots.len() != param_count || slots.iter().any(|s| s.is_none()) {
            return Err(TypeError::Other {
                span,
                message: "constructor pattern field count mismatch".into(),
            });
        }
    } else if slots.len() > param_count {
        slots.truncate(param_count);
    } else {
        slots.resize(param_count, None);
    }
    Ok(slots)
}

/// Reorder labelled arguments to match declaration label order.
/// Unlabelled args keep relative order and fill remaining slots left-to-right.
fn reorder_args(args: &[Arg], labels: Option<&[Option<String>]>) -> Result<Vec<Arg>, TypeError> {
    let Some(labels) = labels else {
        return Ok(args.to_vec());
    };
    if labels.iter().all(|l| l.is_none()) || args.iter().all(|a| a.label.is_none()) {
        return Ok(args.to_vec());
    }

    let mut slots: Vec<Option<Arg>> = vec![None; labels.len()];
    let mut unlabelled = Vec::new();

    for arg in args {
        if let Some(label) = &arg.label {
            let Some(idx) = labels.iter().position(|l| l.as_ref() == Some(label)) else {
                return Err(TypeError::Other {
                    span: arg.span,
                    message: format!("unknown argument label `{label}`"),
                });
            };
            if slots[idx].is_some() {
                return Err(TypeError::Other {
                    span: arg.span,
                    message: format!("duplicate argument label `{label}`"),
                });
            }
            slots[idx] = Some(arg.clone());
        } else {
            unlabelled.push(arg.clone());
        }
    }

    let mut ui = 0;
    for slot in &mut slots {
        if slot.is_none() {
            if let Some(a) = unlabelled.get(ui) {
                *slot = Some(a.clone());
                ui += 1;
            }
        }
    }
    if ui < unlabelled.len() {
        let span = unlabelled[ui].span;
        return Err(TypeError::Other {
            span,
            message: "too many arguments for labelled call".into(),
        });
    }

    Ok(slots.into_iter().flatten().collect())
}

fn apply_map(map: &HashMap<u32, Type>, ty: &Type) -> Type {
    match ty {
        Type::Var(v) => map.get(v).cloned().unwrap_or(Type::Var(*v)),
        Type::Named { module, name, args } => Type::Named {
            module: module.clone(),
            name: name.clone(),
            args: args.iter().map(|a| apply_map(map, a)).collect(),
        },
        Type::Fn { params, ret } => Type::Fn {
            params: params.iter().map(|p| apply_map(map, p)).collect(),
            ret: Box::new(apply_map(map, ret)),
        },
        Type::Tuple(elems) => Type::Tuple(elems.iter().map(|e| apply_map(map, e)).collect()),
    }
}
