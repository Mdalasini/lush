//! Type checking entry points and inference.

use crate::desugar::desugar_module;
use crate::env::Env;
use crate::error::TypeError;
use crate::ty::{apply, free_vars, Scheme, Subst, Type};
use crate::unify::unify;
use lush_syntax::ast::*;
use lush_syntax::{parse_module, Span};
use std::collections::HashMap;

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
    /// Local ADT constructors: name -> scheme
    ctors: HashMap<String, Scheme>,
}

impl Checker {
    fn new() -> Self {
        Self {
            env: Env::prelude(),
            subst: HashMap::new(),
            next_var: 100,
            errors: Vec::new(),
            ctors: HashMap::new(),
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

        for def in &module.definitions {
            if let Definition::Type(t) = def {
                self.register_type_def(t);
            }
        }

        // Pre-bind functions for mutual recursion (monomorphic placeholders).
        for def in &module.definitions {
            if let Definition::Fn(f) = def {
                let ret = f
                    .return_type
                    .as_ref()
                    .map(|t| self.ast_type(t))
                    .unwrap_or_else(|| self.fresh());
                let params: Vec<Type> = f
                    .params
                    .iter()
                    .map(|p| {
                        p.ty.as_ref()
                            .map(|t| self.ast_type(t))
                            .unwrap_or_else(|| self.fresh())
                    })
                    .collect();
                self.env.insert_mono(
                    f.name.clone(),
                    Type::Fn {
                        params,
                        ret: Box::new(ret),
                    },
                );
            }
            if let Definition::Const(c) = def {
                let ty =
                    c.ty.as_ref()
                        .map(|t| self.ast_type(t))
                        .unwrap_or_else(|| self.fresh());
                self.env.insert_mono(c.name.clone(), ty);
            }
        }

        for def in &module.definitions {
            match def {
                Definition::Fn(f) => self.check_fn(f),
                Definition::Const(c) => {
                    let inferred = self.infer_expr(&c.value);
                    if let Some(ann) = &c.ty {
                        let ann_ty = self.ast_type(ann);
                        let _ = unify(&mut self.subst, &inferred, &ann_ty, c.span);
                    }
                }
                Definition::Type(_) => {}
            }
        }
    }

    fn register_type_def(&mut self, t: &TypeDef) {
        self.env.types.insert(t.name.clone(), t.params.len());
        if let TypeDefBody::Adt(variants) = &t.body {
            for v in variants {
                let param_tys: Vec<Type> = v.fields.iter().map(|f| self.ast_type(&f.ty)).collect();
                let ret = Type::Named {
                    module: None,
                    name: t.name.clone(),
                    args: t.params.iter().map(|_| self.fresh()).collect(),
                };
                if param_tys.is_empty() {
                    self.ctors.insert(v.name.clone(), Scheme::mono(ret.clone()));
                    self.env.insert_mono(v.name.clone(), ret);
                } else {
                    let scheme = Scheme::mono(Type::Fn {
                        params: param_tys,
                        ret: Box::new(ret),
                    });
                    self.ctors.insert(v.name.clone(), scheme.clone());
                    self.env.insert_scheme(v.name.clone(), scheme);
                }
            }
        }
    }

    fn check_fn(&mut self, f: &FnDef) {
        let saved = self.env.clone();
        let mut param_tys = Vec::new();
        for p in &f.params {
            let ty =
                p.ty.as_ref()
                    .map(|t| self.ast_type(t))
                    .unwrap_or_else(|| self.fresh());
            self.env.insert_mono(p.name.clone(), ty.clone());
            param_tys.push(ty);
        }
        let body_ty = self.infer_block_scoped(&f.body);
        if let Some(ret) = &f.return_type {
            let ret_ty = self.ast_type(ret);
            if let Err(e) = unify(&mut self.subst, &body_ty, &ret_ty, f.span) {
                self.errors.push(e);
            }
        }
        let fn_ty = Type::Fn {
            params: param_tys,
            ret: Box::new(body_ty),
        };
        self.env = saved;
        self.env
            .insert_mono(f.name.clone(), apply(&self.subst, &fn_ty));
    }

    fn bind_pattern(&mut self, pattern: &Pattern, ty: &Type) {
        match &pattern.kind {
            PatternKind::Var(name) | PatternKind::As { name, .. } => {
                self.env.insert_mono(name.clone(), apply(&self.subst, ty));
            }
            PatternKind::Discard => {}
            PatternKind::Constructor { fields, .. } => {
                for f in fields {
                    let field_ty = self.fresh();
                    self.bind_pattern(&f.pattern, &field_ty);
                }
            }
            PatternKind::Tuple(elems) => {
                if let Type::Tuple(tys) = apply(&self.subst, ty) {
                    for (p, t) in elems.iter().zip(tys.iter()) {
                        self.bind_pattern(p, t);
                    }
                } else {
                    for p in elems {
                        let t = self.fresh();
                        self.bind_pattern(p, &t);
                    }
                }
            }
            PatternKind::List { items, rest } => {
                let elem = self.fresh();
                let _ = unify(&mut self.subst, ty, &Type::list(elem.clone()), pattern.span);
                for p in items {
                    self.bind_pattern(p, &elem);
                }
                if let Some(r) = rest {
                    self.bind_pattern(r, &Type::list(elem));
                }
            }
            PatternKind::StringPrefix { rest, .. } => {
                self.bind_pattern(rest, &Type::string());
            }
            PatternKind::BitArray(_) => {}
            PatternKind::Int(_) | PatternKind::Float(_) | PatternKind::String(_) => {}
        }
    }

    fn infer_expr(&mut self, expr: &Expr) -> Type {
        match &expr.kind {
            ExprKind::Int(_) => Type::int(),
            ExprKind::Float(_) => Type::float(),
            ExprKind::String(_) => Type::string(),
            ExprKind::Ident(name) => self.lookup_value(name, expr.span),
            ExprKind::Constructor(name) => self.lookup_value(name, expr.span),
            ExprKind::Field { base, field, .. } => {
                let inferred = self.infer_expr(base);
                let base_ty = apply(&self.subst, &inferred);
                if let Type::Named { name, .. } = &base_ty {
                    if name.starts_with("Module_") {
                        // Flexible member.
                        return self.fresh();
                    }
                }
                // Record field: fresh for minimal checker.
                let _ = field;
                self.fresh()
            }
            ExprKind::Call { callee, args } => {
                let callee_ty = self.infer_expr(callee);
                let mut arg_tys = Vec::new();
                let mut hole_ty = None;
                for arg in args {
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
                // Flexible: if callee is a free var (module stub member), bind it.
                if let Err(e) = unify(&mut self.subst, &callee_ty, &expected, expr.span) {
                    // Soft-fail for stub flexibility: ignore arity mismatches from stubs.
                    if !matches!(apply(&self.subst, &callee_ty), Type::Var(_)) {
                        self.errors.push(e);
                    } else {
                        self.subst.insert(
                            match apply(&self.subst, &callee_ty) {
                                Type::Var(v) => v,
                                _ => unreachable!(),
                            },
                            expected,
                        );
                    }
                }
                if let Some(h) = hole_ty {
                    // Capture: return fn(h) -> ret
                    Type::Fn {
                        params: vec![h],
                        ret: Box::new(ret),
                    }
                } else {
                    ret
                }
            }
            ExprKind::Binary { left, op, right } => {
                let lt = self.infer_expr(left);
                let rt = self.infer_expr(right);
                self.infer_binop(*op, lt, rt, expr.span)
            }
            ExprKind::Unary { op, expr: inner } => {
                let t = self.infer_expr(inner);
                match op {
                    UnaryOp::Neg => {
                        // Int or Float
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
                        if let Err(e) = unify(&mut self.subst, &t, &Type::bool(), expr.span) {
                            self.errors.push(e);
                        }
                        Type::bool()
                    }
                }
            }
            ExprKind::Pipe { left, right } => {
                let left_ty = self.infer_expr(left);
                // Desugar pipe typing: right should accept left as first arg.
                match &right.kind {
                    ExprKind::Call { callee, args } => {
                        let mut new_args = vec![Arg {
                            label: None,
                            value: ArgValue::Expr((**left).clone()),
                            span: left.span,
                        }];
                        // If hole present, replace; else insert first.
                        let mut replaced = false;
                        for a in args {
                            if matches!(a.value, ArgValue::Hole) && !replaced {
                                new_args.push(Arg {
                                    label: a.label.clone(),
                                    value: ArgValue::Expr((**left).clone()),
                                    span: a.span,
                                });
                                replaced = true;
                            } else if !matches!(a.value, ArgValue::Hole) {
                                new_args.push(a.clone());
                            }
                        }
                        if replaced {
                            // left already used as hole fill — don't also prepend
                            new_args.remove(0);
                        }
                        let call = Expr {
                            span: expr.span,
                            kind: ExprKind::Call {
                                callee: callee.clone(),
                                args: new_args,
                            },
                        };
                        let _ = left_ty;
                        self.infer_expr(&call)
                    }
                    _ => {
                        // f |> g  means g(f) when g is a function ref
                        let call = Expr {
                            span: expr.span,
                            kind: ExprKind::Call {
                                callee: right.clone(),
                                args: vec![Arg {
                                    label: None,
                                    value: ArgValue::Expr((**left).clone()),
                                    span: left.span,
                                }],
                            },
                        };
                        self.infer_expr(&call)
                    }
                }
            }
            ExprKind::Fn {
                params,
                return_type,
                body,
            } => {
                let saved = self.env.clone();
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
                    for row in &clause.patterns {
                        for (p, st) in row.patterns.iter().zip(subject_tys.iter()) {
                            self.bind_pattern(p, st);
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
            ExprKind::Block(b) => self.infer_block_scoped(b),
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
            ExprKind::BitArray(_) => Type::bit_array(),
            ExprKind::RecordUpdate { base, fields, .. } => {
                let base_ty = self.infer_expr(base);
                for (_, v) in fields {
                    let _ = self.infer_expr(v);
                }
                base_ty
            }
        }
    }

    /// Infer a block while keeping let bindings in the current env (for fn bodies).
    fn infer_block_scoped(&mut self, block: &Block) -> Type {
        let mut last = Type::nil();
        let mut saw_expr = false;
        for stmt in &block.statements {
            match stmt {
                Statement::Fn(f) => self.check_fn(f),
                Statement::Let(l) => {
                    let value_ty = self.infer_expr(&l.value);
                    if let Some(ann) = &l.ty {
                        let ann_ty = self.ast_type(ann);
                        if let Err(e) = unify(&mut self.subst, &value_ty, &ann_ty, l.span) {
                            self.errors.push(e);
                        }
                    }
                    self.bind_pattern(&l.pattern, &value_ty);
                }
                Statement::Expr(e) => {
                    last = self.infer_expr(e);
                    saw_expr = true;
                }
                Statement::Use(_) => {}
            }
        }
        if saw_expr {
            last
        } else {
            Type::nil()
        }
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
        // Unknown: flexible fresh type (allows incomplete stubs while docs evolve).
        let ty = self.fresh();
        self.env.insert_mono(name, ty.clone());
        let _ = span;
        ty
    }

    fn instantiate(&mut self, scheme: &Scheme) -> Type {
        let mut map = HashMap::new();
        for v in &scheme.vars {
            map.insert(*v, self.fresh());
        }
        apply_map(&map, &scheme.body)
    }

    fn ast_type(&mut self, ty: &TypeExpr) -> Type {
        match &ty.kind {
            TypeKind::Var(name) => Type::Named {
                module: None,
                name: name.clone(),
                args: vec![],
            },
            TypeKind::Named { module, name, args } => Type::Named {
                module: module.clone(),
                name: name.clone(),
                args: args.iter().map(|a| self.ast_type(a)).collect(),
            },
            TypeKind::Fn { params, ret } => Type::Fn {
                params: params.iter().map(|p| self.ast_type(p)).collect(),
                ret: Box::new(self.ast_type(ret)),
            },
            TypeKind::Tuple(elems) => Type::Tuple(elems.iter().map(|e| self.ast_type(e)).collect()),
        }
    }
}

fn apply_map(map: &HashMap<u32, Type>, ty: &Type) -> Type {
    match ty {
        Type::Var(v) => map.get(v).cloned().unwrap_or_else(|| Type::Var(*v)),
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
