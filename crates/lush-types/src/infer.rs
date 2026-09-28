//! Hindley–Milner inference with value restriction and sealed constraints.

use std::collections::{BTreeMap, HashMap, HashSet};

use lush_syntax::ast::*;
use lush_syntax::span::Span;
use lush_syntax::token::IntBase;

use crate::codes;
use crate::const_eval::{self, ConstEnv, ConstValue};
use crate::diag::TypeSink;
use crate::eq_capability;
use crate::exhaust::ExhaustChecker;
use crate::interface::{ExportedType, ExportedValue, ModuleInterface};
use crate::resolve::{ResolvedModule, ValueInfo};
use crate::ty::{
    ConstraintSet, FieldInfo, Scheme, Type, TypeDefId, TypeDefInfo, TypeDefKind, TypeStore,
    VariantInfo,
};
use crate::unify::{self, Origin, Unifier};

pub struct InferCtx<'a> {
    pub store: &'a mut TypeStore,
    pub sink: &'a mut TypeSink,
    pub resolved: &'a mut ResolvedModule,
    pub level: u32,
    pub env: Vec<HashMap<String, Scheme>>,
    /// Names currently being defined in an SCC (monomorphic).
    pub scc: HashSet<String>,
    pub used_values: HashSet<String>,
    pub used_types: HashSet<String>,
    /// Local bindings eligible for unused-variable warnings: (name, span, underscore_suppressed)
    pub local_bindings: Vec<(String, Span, bool)>,
    pub const_env: ConstEnv,
    pub entry_main: bool,
    pub module_path: String,
    /// Tracks whether we're inside the defining module for opaque access.
    pub defining_opaques: HashSet<TypeDefId>,
    /// Already-checked dependency interfaces (for qualified `module.value` lookup).
    pub deps: &'a BTreeMap<String, ModuleInterface>,
    /// When set, argument unification failures are reported as polymorphic recursion.
    pub scc_rec_call: bool,
}

impl<'a> InferCtx<'a> {
    pub fn push_scope(&mut self) {
        self.env.push(HashMap::new());
    }
    pub fn pop_scope(&mut self) {
        self.env.pop();
    }
    pub fn define_local(&mut self, name: String, scheme: Scheme) {
        if let Some(scope) = self.env.last_mut() {
            scope.insert(name, scheme);
        }
    }
    pub fn lookup_value(
        &mut self,
        name: &str,
    ) -> Option<(Scheme, Vec<Option<String>>, Option<TypeDefId>)> {
        for scope in self.env.iter().rev() {
            if let Some(s) = scope.get(name) {
                self.used_values.insert(name.to_string());
                // Prefer label metadata from the module resolution table when present.
                if let Some(v) = self.resolved.values.get(name) {
                    return Some((s.clone(), v.labels.clone(), v.constructor_of));
                }
                return Some((s.clone(), vec![], None));
            }
        }
        if let Some(v) = self.resolved.values.get(name) {
            if v.from_module.starts_with("module:") {
                return None; // module alias, not a value
            }
            self.used_values.insert(name.to_string());
            self.resolved.imports_used.insert(name.to_string());
            return Some((v.scheme.clone(), v.labels.clone(), v.constructor_of));
        }
        None
    }

    pub fn module_path_for_alias(&self, alias: &str) -> Option<String> {
        self.resolved
            .values
            .get(alias)
            .and_then(|v| v.from_module.strip_prefix("module:").map(|s| s.to_string()))
    }
}

pub fn infer_module(
    path: &str,
    module: &Module,
    resolved: &mut ResolvedModule,
    store: &mut TypeStore,
    sink: &mut TypeSink,
    deps: &BTreeMap<String, ModuleInterface>,
    entry: bool,
) -> ModuleInterface {
    // Elaborate type definitions
    elaborate_types(path, module, resolved, store, sink);

    let mut ctx = InferCtx {
        store,
        sink,
        resolved,
        level: 0,
        env: vec![HashMap::new()],
        scc: HashSet::new(),
        used_values: HashSet::new(),
        used_types: HashSet::new(),
        local_bindings: Vec::new(),
        const_env: ConstEnv::new(),
        entry_main: entry,
        module_path: path.to_string(),
        defining_opaques: HashSet::new(),
        deps,
        scc_rec_call: false,
    };
    for (name, t) in ctx.resolved.types.clone() {
        if t.from_module == path {
            if let Some(info) = ctx.store.defs.get(&t.def) {
                if info.kind == TypeDefKind::Opaque {
                    ctx.defining_opaques.insert(t.def);
                }
            }
            let _ = name;
        }
    }

    // Constants (may reference later ones — pending map enables forward refs + cycles)
    let consts: Vec<&ConstDef> = module
        .items
        .iter()
        .filter_map(|i| match i {
            ModuleItem::Const(c) => Some(c),
            _ => None,
        })
        .collect();
    for c in &consts {
        ctx.const_env
            .pending
            .insert(c.name.text.clone(), c.value.clone());
    }
    for c in &consts {
        if ctx.const_env.values.contains_key(&c.name.text) {
            continue; // already forced via forward ref
        }
        ctx.const_env.visiting.push(c.name.text.clone());
        let expr = ctx
            .const_env
            .pending
            .remove(&c.name.text)
            .unwrap_or_else(|| c.value.clone());
        if let Some(val) = const_eval::eval_const_expr(&expr, &mut ctx.const_env, ctx.sink) {
            let ty = const_value_type(&val);
            if let Some(ann) = &c.ty {
                let ann_ty = translate_type_expr(&mut ctx, ann, &HashMap::new());
                let mut u = Unifier::new(ctx.store, ctx.sink);
                u.unify(
                    &ann_ty,
                    &ty,
                    c.span,
                    Some(&Origin {
                        span: ann.span,
                        label: "constant annotation".into(),
                    }),
                );
                // If mismatch, also emit E1405
                if !types_compat(ctx.store, &ann_ty, &ty) {
                    ctx.sink.error(
                        codes::E1405_CONST_ANNOT,
                        "constant annotation does not match value type",
                        c.span,
                        None,
                    );
                }
            }
            ctx.const_env.values.insert(c.name.text.clone(), val);
            let scheme = Scheme::mono(ty.clone());
            if let Some(v) = ctx.resolved.values.get_mut(&c.name.text) {
                v.scheme = scheme.clone();
            }
            ctx.const_env.types.insert(c.name.text.clone(), ty);
        }
        ctx.const_env.visiting.pop();
    }

    // Functions: dependency SCCs
    let fns: Vec<&FnDef> = module
        .items
        .iter()
        .filter_map(|i| match i {
            ModuleItem::Fn(f) => Some(f),
            _ => None,
        })
        .collect();
    let deps = fn_deps(&fns);
    let sccs = strong_components(&deps);
    for scc in sccs {
        infer_fn_scc(&mut ctx, &fns, &scc);
    }

    // Entry main check
    if entry {
        match ctx.resolved.values.get("main") {
            Some(v) if v.public => {
                let body = ctx.store.zonk(&v.scheme.body);
                let ok = matches!(
                    body,
                    Type::Fun { ref params, ref ret } if params.is_empty() && matches!(ctx.store.zonk(ret), Type::Nil)
                );
                if !ok {
                    ctx.sink.error(
                        codes::E1215_MAIN_SIG,
                        "`main` must have type `fn() -> Nil`",
                        v.span,
                        None,
                    );
                }
            }
            _ => {
                ctx.sink.error(
                    codes::E1215_MAIN_SIG,
                    "entry module requires `pub fn main() -> Nil`",
                    Span::default(),
                    None,
                );
            }
        }
    }

    // Warnings: unused imports, unused private, unused values handled during infer
    warn_unused(&mut ctx, module);

    build_interface(path, &ctx)
}

fn types_compat(store: &mut TypeStore, a: &Type, b: &Type) -> bool {
    let a = store.zonk(a);
    let b = store.zonk(b);
    matches!(
        (&a, &b),
        (Type::Error, _)
            | (_, Type::Error)
            | (Type::Int, Type::Int)
            | (Type::Float, Type::Float)
            | (Type::String, Type::String)
            | (Type::Bool, Type::Bool)
            | (Type::Nil, Type::Nil)
            | (Type::BitArray, Type::BitArray)
    )
}

fn const_value_type(v: &ConstValue) -> Type {
    match v {
        ConstValue::Int(_) => Type::Int,
        ConstValue::Float(_) => Type::Float,
        ConstValue::String(_) => Type::String,
        ConstValue::Bool(_) => Type::Bool,
        ConstValue::Nil => Type::Nil,
        ConstValue::Tuple(xs) => Type::Tuple(xs.iter().map(const_value_type).collect()),
        ConstValue::List(xs) => {
            let elem = xs.first().map(const_value_type).unwrap_or(Type::Error);
            Type::List(Box::new(elem))
        }
        ConstValue::Adt { .. } => Type::Error, // nominal filled elsewhere
    }
}

fn elaborate_types(
    path: &str,
    module: &Module,
    resolved: &mut ResolvedModule,
    store: &mut TypeStore,
    sink: &mut TypeSink,
) {
    for item in &module.items {
        let ModuleItem::Type(td) = item else { continue };
        let Some(ti) = resolved.types.get(&td.name.text).cloned() else {
            continue;
        };
        let mut tvar_rigids = HashMap::new();
        for p in &td.tvars {
            tvar_rigids.insert(p.text.clone(), store.fresh_rigid(p.text.clone()));
        }
        match &td.body {
            TypeDefBody::Alias(te) => {
                if td.opaque {
                    continue; // already errored
                }
                let body = translate_type_expr_raw(store, sink, resolved, te, &tvar_rigids, path);
                if let Some(info) = store.defs.get_mut(&ti.def) {
                    info.alias_body = Some(body);
                    info.kind = TypeDefKind::Alias;
                }
            }
            TypeDefBody::Adt(variants) => {
                let mut vins = Vec::new();
                let mut seen_ctors = HashSet::new();
                for v in variants {
                    if !seen_ctors.insert(v.name.text.clone()) {
                        sink.error(
                            codes::E1005_DUPLICATE_CTOR,
                            format!("duplicate constructor `{}`", v.name.text),
                            v.span,
                            None,
                        );
                    }
                    let mut fields = Vec::new();
                    let mut seen_labels = HashSet::new();
                    if let Some(fs) = &v.fields {
                        for f in fs {
                            if let Some(lab) = &f.label {
                                if !seen_labels.insert(lab.text.clone()) {
                                    sink.error(
                                        codes::E1203_DUP_FIELD,
                                        format!("duplicate field label `{}`", lab.text),
                                        lab.span,
                                        None,
                                    );
                                }
                            }
                            let ty = translate_type_expr_raw(
                                store,
                                sink,
                                resolved,
                                &f.ty,
                                &tvar_rigids,
                                path,
                            );
                            // Check undeclared tvars — handled in translate
                            fields.push(FieldInfo {
                                label: f.label.as_ref().map(|l| l.text.clone()),
                                ty,
                            });
                        }
                    }
                    vins.push(VariantInfo {
                        name: v.name.text.clone(),
                        fields: fields.clone(),
                        public: td.public && !td.opaque,
                    });
                    // Register constructor in value namespace
                    let app_args: Vec<Type> = td
                        .tvars
                        .iter()
                        .filter_map(|p| tvar_rigids.get(&p.text).cloned())
                        .collect();
                    let ret = Type::App {
                        def: ti.def,
                        args: app_args,
                    };
                    let param_tys: Vec<Type> = fields.iter().map(|f| f.ty.clone()).collect();
                    let body_ty = if param_tys.is_empty() {
                        ret
                    } else {
                        Type::Fun {
                            params: param_tys.clone(),
                            ret: Box::new(ret),
                        }
                    };
                    let pairs: Vec<_> = tvar_rigids
                        .values()
                        .filter_map(|t| match t {
                            Type::Rigid(id) => {
                                let n = store
                                    .rigids
                                    .get(id)
                                    .map(|r| r.name.clone())
                                    .unwrap_or_default();
                                Some((*id, n))
                            }
                            _ => None,
                        })
                        .collect();
                    let scheme = unify::generalise_rigids(store, &body_ty, &pairs);
                    let labels: Vec<_> = fields.iter().map(|f| f.label.clone()).collect();
                    let pub_ctor = td.public && !td.opaque;
                    if let Some(prev) = resolved.values.insert(
                        v.name.text.clone(),
                        ValueInfo {
                            scheme,
                            public: pub_ctor,
                            from_module: path.to_string(),
                            labels,
                            constructor_of: Some(ti.def),
                            is_const: param_tys.is_empty(),
                            span: v.name.span,
                        },
                    ) {
                        if prev.from_module == path {
                            sink.error(
                                codes::E1005_DUPLICATE_CTOR,
                                format!("duplicate constructor `{}`", v.name.text),
                                v.span,
                                None,
                            );
                        }
                    }
                }
                if let Some(info) = store.defs.get_mut(&ti.def) {
                    info.variants = vins;
                    info.kind = if td.opaque {
                        TypeDefKind::Opaque
                    } else {
                        TypeDefKind::Adt
                    };
                }
                let (eq_params, always_eq, never_eq) =
                    eq_capability::compute_eq_params(store, ti.def);
                if let Some(info) = store.defs.get_mut(&ti.def) {
                    info.eq_params = eq_params;
                    info.always_eq = always_eq;
                    info.never_eq = never_eq;
                }
            }
        }
    }
    // Alias cycle detection
    detect_alias_cycles(resolved, store, sink);
}

fn detect_alias_cycles(resolved: &ResolvedModule, store: &TypeStore, sink: &mut TypeSink) {
    for (name, ti) in &resolved.types {
        let mut seen = HashSet::new();
        let mut cur = ti.def;
        loop {
            if !seen.insert(cur) {
                sink.error(
                    codes::E1008_ALIAS_CYCLE,
                    format!("type alias cycle involving `{name}`"),
                    ti.span,
                    None,
                );
                break;
            }
            let Some(info) = store.defs.get(&cur) else {
                break;
            };
            if info.kind != TypeDefKind::Alias {
                break;
            }
            let Some(body) = &info.alias_body else { break };
            match body {
                Type::App { def, .. } => cur = *def,
                _ => break,
            }
        }
    }
}

fn translate_type_expr_raw(
    store: &mut TypeStore,
    sink: &mut TypeSink,
    resolved: &ResolvedModule,
    te: &TypeExpr,
    rigids: &HashMap<String, Type>,
    _path: &str,
) -> Type {
    match &te.kind {
        TypeKind::Var(n) => {
            if let Some(t) = rigids.get(&n.text) {
                t.clone()
            } else {
                sink.error(
                    codes::E1201_UNDECLARED_TVAR,
                    format!("undeclared type variable `{}`", n.text),
                    n.span,
                    None,
                );
                Type::Error
            }
        }
        TypeKind::Named { name, args } => {
            let (tname, module_qual) = match name {
                TypeName::Unqualified(u) => (u.text.clone(), None),
                TypeName::Qualified { module, name } => {
                    // Slash-qualified is rejected at parse? Check for slash in module name
                    if module.text.contains('/') {
                        sink.error(
                            codes::E1010_SLASH_QUALIFIED_TYPE,
                            "slash-qualified types are not allowed; import the module and use `alias.Type`",
                            te.span,
                            None,
                        );
                        return Type::Error;
                    }
                    (name.text.clone(), Some(module.text.clone()))
                }
            };
            let args_t: Vec<Type> = args
                .iter()
                .map(|a| translate_type_expr_raw(store, sink, resolved, a, rigids, _path))
                .collect();
            match tname.as_str() {
                "Int" | "Float" | "String" | "Bool" | "Nil" | "BitArray" if args_t.is_empty() => {
                    return match tname.as_str() {
                        "Int" => Type::Int,
                        "Float" => Type::Float,
                        "String" => Type::String,
                        "Bool" => Type::Bool,
                        "Nil" => Type::Nil,
                        _ => Type::BitArray,
                    };
                }
                "List" => {
                    if args_t.len() != 1 {
                        sink.error(
                            codes::E1200_TYPE_ARITY,
                            format!("`List` takes 1 type argument, found {}", args_t.len()),
                            te.span,
                            None,
                        );
                        return Type::Error;
                    }
                    return Type::List(Box::new(args_t.into_iter().next().unwrap()));
                }
                _ => {}
            }
            // Resolve type name
            let def = if let Some(m) = &module_qual {
                // Look up module alias
                let mod_path = resolved
                    .values
                    .get(m)
                    .and_then(|v| v.from_module.strip_prefix("module:").map(|s| s.to_string()));
                // Prefer types imported under qualified access — search by from_module
                resolved
                    .types
                    .iter()
                    .find(|(n, t)| {
                        *n == &tname
                            && (mod_path
                                .as_ref()
                                .map(|p| &t.from_module == p)
                                .unwrap_or(false)
                                || t.from_module.ends_with(m.as_str()))
                    })
                    .map(|(_, t)| t.def)
                    .or_else(|| {
                        // Also allow type imported selectively with same name
                        resolved.types.get(&tname).map(|t| t.def)
                    })
            } else {
                resolved.types.get(&tname).map(|t| t.def)
            };
            let Some(def) = def else {
                // Subject without import?
                sink.error(
                    codes::E1001_UNKNOWN_TYPE,
                    format!("unknown type `{tname}`"),
                    te.span,
                    crate::resolve::did_you_mean(&tname, resolved.types.keys().map(|s| s.as_str())),
                );
                return Type::Error;
            };
            let arity = store.defs.get(&def).map(|d| d.params.len()).unwrap_or(0);
            if arity != args_t.len() {
                sink.error(
                    codes::E1200_TYPE_ARITY,
                    format!(
                        "type `{tname}` takes {arity} type argument(s), found {}",
                        args_t.len()
                    ),
                    te.span,
                    None,
                );
                return Type::Error;
            }
            // Expand alias
            if let Some(info) = store.defs.get(&def) {
                if info.kind == TypeDefKind::Alias {
                    if let Some(body) = &info.alias_body {
                        // Substitute params — simplified return body if no params
                        if info.params.is_empty() {
                            return body.clone();
                        }
                    }
                }
            }
            Type::App { def, args: args_t }
        }
        TypeKind::Fn { params, ret } => Type::Fun {
            params: params
                .iter()
                .map(|p| translate_type_expr_raw(store, sink, resolved, p, rigids, _path))
                .collect(),
            ret: Box::new(translate_type_expr_raw(
                store, sink, resolved, ret, rigids, _path,
            )),
        },
        TypeKind::Tuple(ts) => {
            if ts.len() < 2 {
                sink.error(
                    codes::E1200_TYPE_ARITY,
                    "tuple types require at least 2 elements",
                    te.span,
                    None,
                );
                return Type::Error;
            }
            Type::Tuple(
                ts.iter()
                    .map(|t| translate_type_expr_raw(store, sink, resolved, t, rigids, _path))
                    .collect(),
            )
        }
    }
}

fn translate_type_expr(
    ctx: &mut InferCtx<'_>,
    te: &TypeExpr,
    rigids: &HashMap<String, Type>,
) -> Type {
    translate_type_expr_raw(
        ctx.store,
        ctx.sink,
        ctx.resolved,
        te,
        rigids,
        &ctx.module_path,
    )
}

fn fn_deps(fns: &[&FnDef]) -> BTreeMap<String, Vec<String>> {
    let names: HashSet<_> = fns.iter().map(|f| f.name.text.clone()).collect();
    let mut deps = BTreeMap::new();
    for f in fns {
        let mut used = HashSet::new();
        walk_expr_names_in_block(&f.body, &mut used);
        let d: Vec<_> = used.into_iter().filter(|n| names.contains(n)).collect();
        deps.insert(f.name.text.clone(), d);
    }
    deps
}

fn walk_expr_names_in_block(b: &Block, out: &mut HashSet<String>) {
    for s in &b.statements {
        match s {
            Statement::Expr(e) => {
                walk_expr_names(e, out);
            }
            Statement::Let(l) => {
                walk_expr_names(&l.value, out);
            }
            Statement::Fn(f) => walk_expr_names_in_block(&f.body, out),
            Statement::Use(u) => walk_expr_names(&u.value, out),
        }
    }
}

fn walk_expr_names(e: &Expr, out: &mut HashSet<String>) {
    match &e.kind {
        ExprKind::Var(n) => {
            out.insert(n.text.clone());
        }
        ExprKind::Call { callee, args } => {
            walk_expr_names(callee, out);
            for a in args {
                if let ArgValue::Expr(e) = &a.value {
                    walk_expr_names(e, out);
                }
            }
        }
        ExprKind::Binary { left, right, .. } | ExprKind::Pipe { left, right } => {
            walk_expr_names(left, out);
            walk_expr_names(right, out);
        }
        ExprKind::Unary { expr, .. }
        | ExprKind::Paren(expr)
        | ExprKind::Echo(expr)
        | ExprKind::Field { base: expr, .. }
        | ExprKind::Assert { expr, .. } => walk_expr_names(expr, out),
        ExprKind::Tuple(xs) | ExprKind::List { items: xs, .. } => {
            for e in xs {
                walk_expr_names(e, out);
            }
        }
        ExprKind::Fn { body, .. } => walk_expr_names_in_block(body, out),
        ExprKind::Block(b) => walk_expr_names_in_block(b, out),
        ExprKind::Case { subjects, clauses } => {
            for s in subjects {
                walk_expr_names(s, out);
            }
            for c in clauses {
                walk_expr_names(&c.body, out);
            }
        }
        ExprKind::RecordUpdate { base, fields, .. } => {
            walk_expr_names(base, out);
            for (_, e) in fields {
                walk_expr_names(e, out);
            }
        }
        _ => {}
    }
}

fn strong_components(deps: &BTreeMap<String, Vec<String>>) -> Vec<Vec<String>> {
    // Tarjan simplified: Kosaraju
    let mut nodes: Vec<_> = deps.keys().cloned().collect();
    nodes.sort();
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    fn dfs1(
        n: &str,
        deps: &BTreeMap<String, Vec<String>>,
        seen: &mut HashSet<String>,
        order: &mut Vec<String>,
    ) {
        if !seen.insert(n.to_string()) {
            return;
        }
        if let Some(ds) = deps.get(n) {
            for d in ds {
                dfs1(d, deps, seen, order);
            }
        }
        order.push(n.to_string());
    }
    for n in &nodes {
        dfs1(n, deps, &mut seen, &mut order);
    }
    let mut rev: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (k, vs) in deps {
        rev.entry(k.clone()).or_default();
        for v in vs {
            rev.entry(v.clone()).or_default().push(k.clone());
        }
    }
    let mut seen = HashSet::new();
    let mut sccs = Vec::new();
    for n in order.into_iter().rev() {
        if seen.contains(&n) {
            continue;
        }
        let mut comp = Vec::new();
        fn dfs2(
            n: &str,
            rev: &BTreeMap<String, Vec<String>>,
            seen: &mut HashSet<String>,
            comp: &mut Vec<String>,
        ) {
            if !seen.insert(n.to_string()) {
                return;
            }
            comp.push(n.to_string());
            if let Some(ds) = rev.get(n) {
                for d in ds {
                    dfs2(d, rev, seen, comp);
                }
            }
        }
        dfs2(&n, &rev, &mut seen, &mut comp);
        sccs.push(comp);
    }
    // Kosaraju yields SCCs in reverse topological order for "uses" edges;
    // infer callees before callers so schemes are ready at call sites.
    sccs.reverse();
    sccs
}

fn infer_fn_scc(ctx: &mut InferCtx<'_>, fns: &[&FnDef], scc: &[String]) {
    ctx.scc = scc.iter().cloned().collect();
    ctx.level += 1;
    // Assign fresh mono types
    let mut mono: HashMap<String, Type> = HashMap::new();
    let mut rigid_maps: HashMap<String, HashMap<String, Type>> = HashMap::new();
    for name in scc {
        let f = fns.iter().find(|f| f.name.text == *name).unwrap();
        // Share annotation rigids across params + return
        let mut names = HashSet::new();
        for p in &f.params {
            if let Some(t) = &p.ty {
                collect_type_vars(t, &mut names);
            }
        }
        if let Some(t) = &f.return_type {
            collect_type_vars(t, &mut names);
        }
        let mut rigids = HashMap::new();
        for n in names {
            rigids.insert(n.clone(), ctx.store.fresh_rigid(n));
        }
        let params: Vec<Type> = f
            .params
            .iter()
            .map(|p| {
                if let Some(t) = &p.ty {
                    translate_type_expr(ctx, t, &rigids)
                } else {
                    ctx.store.fresh_var(ctx.level)
                }
            })
            .collect();
        let ret = if let Some(t) = &f.return_type {
            translate_type_expr(ctx, t, &rigids)
        } else {
            ctx.store.fresh_var(ctx.level)
        };
        rigid_maps.insert(name.clone(), rigids);
        let ty = Type::Fun {
            params: params.clone(),
            ret: Box::new(ret.clone()),
        };
        mono.insert(name.clone(), ty.clone());
        ctx.define_local(name.clone(), Scheme::mono(ty));
        // Warn unannotated pub
        if f.public {
            let missing = f.params.iter().any(|p| p.ty.is_none()) || f.return_type.is_none();
            if missing {
                ctx.sink.warning(
                    codes::W1006_UNANNOTATED_PUB,
                    format!(
                        "public function `{}` is missing a parameter or return type annotation",
                        f.name.text
                    ),
                    f.name.span,
                    Some("annotate all parameters and the return type on `pub fn`".into()),
                );
            }
        }
    }
    // Infer bodies
    for name in scc {
        let f = fns.iter().find(|f| f.name.text == *name).unwrap();
        ctx.push_scope();
        let Type::Fun { params, ret } = mono.get(name).unwrap().clone() else {
            unreachable!()
        };
        for (p, ty) in f.params.iter().zip(params.iter()) {
            ctx.define_local(p.name.text.clone(), Scheme::mono(ty.clone()));
        }
        let body_ty = infer_block(ctx, &f.body, true);
        let mut u = Unifier::new(ctx.store, ctx.sink);
        u.unify(
            &body_ty,
            &ret,
            f.body.span,
            f.return_type
                .as_ref()
                .map(|t| Origin {
                    span: t.span,
                    label: "return type annotation".into(),
                })
                .as_ref(),
        );
        ctx.pop_scope();
    }
    // Generalise — convert annotation rigids into quantified vars
    ctx.level -= 1;
    for name in scc {
        let ty = mono.get(name).unwrap().clone();
        let scheme = if let Some(rigids) = rigid_maps.get(name) {
            let pairs: Vec<_> = rigids
                .values()
                .filter_map(|t| match t {
                    Type::Rigid(id) => {
                        let n = ctx
                            .store
                            .rigids
                            .get(id)
                            .map(|r| r.name.clone())
                            .unwrap_or_default();
                        Some((*id, n))
                    }
                    _ => None,
                })
                .collect();
            if pairs.is_empty() {
                unify::generalise(ctx.store, &ty, ctx.level, false)
            } else {
                unify::generalise_rigids(ctx.store, &ty, &pairs)
            }
        } else {
            unify::generalise(ctx.store, &ty, ctx.level, false)
        };
        if let Some(v) = ctx.resolved.values.get_mut(name) {
            v.scheme = scheme.clone();
        }
        if let Some(scope) = ctx.env.last_mut() {
            scope.insert(name.clone(), scheme);
        }
    }
    ctx.scc.clear();
}

fn translate_with_annotation_rigids(ctx: &mut InferCtx<'_>, te: &TypeExpr) -> Type {
    // Build rigids for free lowercase names appearing as TypeKind::Var
    let mut names = HashSet::new();
    collect_type_vars(te, &mut names);
    let mut rigids = HashMap::new();
    for n in names {
        rigids.insert(n.clone(), ctx.store.fresh_rigid(n));
    }
    translate_type_expr(ctx, te, &rigids)
}

fn collect_type_vars(te: &TypeExpr, out: &mut HashSet<String>) {
    match &te.kind {
        TypeKind::Var(n) => {
            out.insert(n.text.clone());
        }
        TypeKind::Named { args, .. } => {
            for a in args {
                collect_type_vars(a, out);
            }
        }
        TypeKind::Fn { params, ret } => {
            for p in params {
                collect_type_vars(p, out);
            }
            collect_type_vars(ret, out);
        }
        TypeKind::Tuple(ts) => {
            for t in ts {
                collect_type_vars(t, out);
            }
        }
    }
}

fn infer_block(ctx: &mut InferCtx<'_>, block: &Block, _is_fn_body: bool) -> Type {
    ctx.push_scope();
    // Local fn groups
    let mut i = 0;
    let mut last_ty = Type::Nil;
    let stmts = &block.statements;
    while i < stmts.len() {
        if let Statement::Fn(_) = &stmts[i] {
            let start = i;
            while i < stmts.len() && matches!(&stmts[i], Statement::Fn(_)) {
                i += 1;
            }
            let group: Vec<&FnDef> = stmts[start..i]
                .iter()
                .map(|s| match s {
                    Statement::Fn(f) => f,
                    _ => unreachable!(),
                })
                .collect();
            // Infer local group like SCC
            let names: Vec<_> = group.iter().map(|f| f.name.text.clone()).collect();
            // Reuse top-level SCC infer by temporarily treating as nested
            infer_local_fn_group(ctx, &group, &names);
            last_ty = Type::Nil;
            continue;
        }
        match &stmts[i] {
            Statement::Let(l) => {
                let val_ty = infer_expr(ctx, &l.value);
                if let Some(ann) = &l.ty {
                    let ann_ty = translate_with_annotation_rigids(ctx, ann);
                    let mut u = Unifier::new(ctx.store, ctx.sink);
                    u.unify(
                        &val_ty,
                        &ann_ty,
                        l.value.span,
                        Some(&Origin {
                            span: ann.span,
                            label: "type annotation".into(),
                        }),
                    );
                }
                let mut ex = ExhaustChecker {
                    store: ctx.store,
                    sink: ctx.sink,
                    work: 0,
                };
                ex.check_irrefutable(&l.pattern, &val_ty, l.assert);
                bind_pattern(ctx, &l.pattern, &val_ty, is_expansive(&l.value));
                last_ty = Type::Nil;
            }
            Statement::Expr(e) => {
                let ty = infer_expr(ctx, e);
                let is_final = i + 1 == stmts.len();
                if !is_final {
                    let z = ctx.store.zonk(&ty);
                    if !matches!(z, Type::Nil | Type::Error) {
                        ctx.sink.warning(
                            codes::W1002_UNUSED_VALUE,
                            format!(
                                "unused value of type `{}` (assign to `_` to discard)",
                                ctx.store.display(&z)
                            ),
                            e.span,
                            Some("write `let _ = ...;` to discard deliberately".into()),
                        );
                    }
                    last_ty = Type::Nil;
                } else {
                    last_ty = ty;
                }
            }
            Statement::Use(_) => {
                // Should have been desugared
                last_ty = Type::Error;
            }
            Statement::Fn(_) => unreachable!(),
        }
        i += 1;
    }
    ctx.pop_scope();
    last_ty
}

fn infer_local_fn_group(ctx: &mut InferCtx<'_>, group: &[&FnDef], names: &[String]) {
    ctx.level += 1;
    let mut mono = HashMap::new();
    for f in group {
        let params: Vec<_> = f
            .params
            .iter()
            .map(|p| {
                p.ty.as_ref()
                    .map(|t| translate_with_annotation_rigids(ctx, t))
                    .unwrap_or_else(|| ctx.store.fresh_var(ctx.level))
            })
            .collect();
        let ret = f
            .return_type
            .as_ref()
            .map(|t| translate_with_annotation_rigids(ctx, t))
            .unwrap_or_else(|| ctx.store.fresh_var(ctx.level));
        let ty = Type::Fun {
            params,
            ret: Box::new(ret),
        };
        mono.insert(f.name.text.clone(), ty.clone());
        ctx.define_local(f.name.text.clone(), Scheme::mono(ty));
    }
    for f in group {
        ctx.push_scope();
        let Type::Fun { params, ret } = mono.get(&f.name.text).unwrap().clone() else {
            unreachable!()
        };
        for (p, ty) in f.params.iter().zip(params) {
            ctx.define_local(p.name.text.clone(), Scheme::mono(ty));
        }
        let body_ty = infer_block(ctx, &f.body, true);
        let mut u = Unifier::new(ctx.store, ctx.sink);
        u.unify(&body_ty, &ret, f.body.span, None);
        ctx.pop_scope();
    }
    ctx.level -= 1;
    for name in names {
        let ty = mono.get(name).unwrap();
        let scheme = unify::generalise(ctx.store, ty, ctx.level, false);
        ctx.define_local(name.clone(), scheme);
    }
}

fn is_expansive(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Var(_)
        | ExprKind::Fn { .. } => false,
        ExprKind::Constructor(_) => false,
        ExprKind::Tuple(xs) => xs.iter().any(is_expansive),
        ExprKind::List { items, spread } => {
            items.iter().any(is_expansive) || spread.as_ref().is_some_and(|s| is_expansive(s))
        }
        ExprKind::Call { .. }
        | ExprKind::RecordUpdate { .. }
        | ExprKind::Field { .. }
        | ExprKind::Block(_)
        | ExprKind::Case { .. }
        | ExprKind::Pipe { .. }
        | ExprKind::Todo { .. }
        | ExprKind::Panic { .. }
        | ExprKind::Assert { .. }
        | ExprKind::Echo(_)
        | ExprKind::Binary { .. }
        | ExprKind::Unary { .. } => true,
        ExprKind::Paren(inner) => is_expansive(inner),
        ExprKind::BitArray(_) => true,
    }
}

fn bind_pattern(ctx: &mut InferCtx<'_>, pat: &Pattern, ty: &Type, expansive: bool) {
    match &pat.kind {
        PatternKind::Var(n) => {
            let scheme = unify::generalise(ctx.store, ty, ctx.level, expansive);
            ctx.define_local(n.text.clone(), scheme);
            ctx.local_bindings.push((n.text.clone(), n.span, false));
        }
        PatternKind::UnderscoreName(n) => {
            let scheme = unify::generalise(ctx.store, ty, ctx.level, expansive);
            ctx.define_local(n.text.clone(), scheme);
            // `_name` suppresses unused warnings
            ctx.local_bindings.push((n.text.clone(), n.span, true));
        }
        PatternKind::Discard => {}
        PatternKind::Alias { pattern, name } => {
            let scheme = unify::generalise(ctx.store, ty, ctx.level, expansive);
            ctx.define_local(name.text.clone(), scheme);
            ctx.local_bindings
                .push((name.text.clone(), name.span, false));
            bind_pattern(ctx, pattern, ty, expansive);
        }
        PatternKind::Tuple(ps) => {
            let z = ctx.store.zonk(ty);
            if let Type::Tuple(ts) = z {
                for (p, t) in ps.iter().zip(ts.iter()) {
                    bind_pattern(ctx, p, t, expansive);
                }
            }
        }
        PatternKind::Constructor {
            constructor,
            args: Some(args),
        } => {
            let z = ctx.store.zonk(ty);
            if let Type::App { def, args: targs } = z {
                if let Some(info) = ctx.store.defs.get(&def).cloned() {
                    if let Some(v) = info
                        .variants
                        .iter()
                        .find(|v| v.name == constructor.name.text)
                    {
                        for (i, a) in args.iter().enumerate() {
                            if a.spread {
                                continue;
                            }
                            if let Some(p) = &a.pattern {
                                let ft = v
                                    .fields
                                    .get(i)
                                    .map(|f| subst_params(&f.ty, &info.params, &targs))
                                    .unwrap_or(Type::Error);
                                bind_pattern(ctx, p, &ft, expansive);
                            }
                        }
                    }
                }
            }
        }
        PatternKind::List { items, spread } => {
            let z = ctx.store.zonk(ty);
            let elem = match z {
                Type::List(t) => *t,
                _ => Type::Error,
            };
            for p in items {
                bind_pattern(ctx, p, &elem, expansive);
            }
            if let Some(s) = spread {
                bind_pattern(ctx, s, &Type::List(Box::new(elem)), expansive);
            }
        }
        _ => {}
    }
}

#[allow(clippy::only_used_in_recursion)]
fn subst_params(ty: &Type, params: &[String], args: &[Type]) -> Type {
    match ty {
        Type::Rigid(id) => {
            // Can't look up name without store — return as-is; approximate by index if Error
            let _ = id;
            Type::Error
        }
        Type::List(t) => Type::List(Box::new(subst_params(t, params, args))),
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| subst_params(t, params, args)).collect()),
        Type::Fun { params: ps, ret } => Type::Fun {
            params: ps.iter().map(|t| subst_params(t, params, args)).collect(),
            ret: Box::new(subst_params(ret, params, args)),
        },
        Type::App { def, args: as_ } => Type::App {
            def: *def,
            args: as_.iter().map(|t| subst_params(t, params, args)).collect(),
        },
        other => other.clone(),
    }
}

pub fn infer_expr(ctx: &mut InferCtx<'_>, expr: &Expr) -> Type {
    match &expr.kind {
        ExprKind::Int(lit) => {
            let base = match lit.base {
                IntBase::Decimal => 10,
                IntBase::Hex => 16,
                IntBase::Octal => 8,
                IntBase::Binary => 2,
            };
            let _ = const_eval::check_int_lit(&lit.digits, base, false, expr.span, ctx.sink);
            Type::Int
        }
        ExprKind::Float(lit) => {
            if crate::numeric::parse_float_literal(&lit.raw).is_err() {
                ctx.sink.error(
                    codes::E1401_FLOAT_RANGE,
                    "float literal outside the finite binary64 range",
                    expr.span,
                    None,
                );
            }
            Type::Float
        }
        ExprKind::String(_) => Type::String,
        ExprKind::Var(n) => {
            if let Some((scheme, _, _)) = ctx.lookup_value(&n.text) {
                // If in SCC and looking up self at different type — poly recursion
                if ctx.scc.contains(&n.text) {
                    return scheme.body;
                }
                unify::instantiate(ctx.store, &scheme, ctx.level)
            } else {
                ctx.sink.error(
                    codes::E1000_UNKNOWN_NAME,
                    format!("unknown variable `{}`", n.text),
                    n.span,
                    crate::resolve::did_you_mean(
                        &n.text,
                        ctx.resolved.values.keys().map(|s| s.as_str()),
                    ),
                );
                Type::Error
            }
        }
        ExprKind::Constructor(c) => infer_constructor(ctx, c, expr.span),
        ExprKind::Tuple(items) => {
            if items.len() < 2 {
                ctx.sink.error(
                    codes::E1200_TYPE_ARITY,
                    "tuples require at least 2 elements",
                    expr.span,
                    None,
                );
                return Type::Error;
            }
            Type::Tuple(items.iter().map(|e| infer_expr(ctx, e)).collect())
        }
        ExprKind::List { items, spread } => {
            let elem = ctx.store.fresh_var(ctx.level);
            for e in items {
                let t = infer_expr(ctx, e);
                let mut u = Unifier::new(ctx.store, ctx.sink);
                u.unify(&t, &elem, e.span, None);
            }
            if let Some(s) = spread {
                let st = infer_expr(ctx, s);
                let mut u = Unifier::new(ctx.store, ctx.sink);
                u.unify(&st, &Type::List(Box::new(elem.clone())), s.span, None);
            }
            Type::List(Box::new(elem))
        }
        ExprKind::Unary { op, expr: inner } => {
            // Direct neg of int lit
            if matches!(op, UnaryOp::Neg) {
                if let ExprKind::Int(lit) = &inner.kind {
                    let base = match lit.base {
                        IntBase::Decimal => 10,
                        IntBase::Hex => 16,
                        IntBase::Octal => 8,
                        IntBase::Binary => 2,
                    };
                    let _ = const_eval::check_int_lit(&lit.digits, base, true, expr.span, ctx.sink);
                    return Type::Int;
                }
                if let ExprKind::Paren(p) = &inner.kind {
                    if let ExprKind::Int(lit) = &p.kind {
                        // -(LIT) is NOT directly negated
                        let base = match lit.base {
                            IntBase::Decimal => 10,
                            IntBase::Hex => 16,
                            IntBase::Octal => 8,
                            IntBase::Binary => 2,
                        };
                        let _ =
                            const_eval::check_int_lit(&lit.digits, base, false, p.span, ctx.sink);
                    }
                }
            }
            let t = infer_expr(ctx, inner);
            match op {
                UnaryOp::Not => {
                    let mut u = Unifier::new(ctx.store, ctx.sink);
                    u.unify(&t, &Type::Bool, expr.span, None);
                    Type::Bool
                }
                UnaryOp::Neg => {
                    let z = ctx.store.zonk(&t);
                    match z {
                        Type::Int => Type::Int,
                        Type::Float => Type::Float,
                        Type::Var(_) | Type::Rigid(_) => {
                            unify::add_constraint(ctx.store, &t, ConstraintSet::neg());
                            t
                        }
                        Type::Error => Type::Error,
                        other => {
                            ctx.sink.error(
                                codes::E1351_NO_NEG,
                                format!("cannot negate `{}`", ctx.store.display(&other)),
                                expr.span,
                                Some("`Neg` is only satisfied by `Int` and `Float`".into()),
                            );
                            Type::Error
                        }
                    }
                }
            }
        }
        ExprKind::Binary { left, op, right } => infer_binop(ctx, left, *op, right, expr.span),
        ExprKind::Call { callee, args } => infer_call(ctx, callee, args, expr.span),
        ExprKind::Field { base, field } => infer_field(ctx, base, field, expr.span),
        ExprKind::RecordUpdate {
            constructor,
            base,
            fields,
        } => infer_record_update(ctx, constructor, base, fields, expr.span),
        ExprKind::Fn {
            params,
            return_type,
            body,
        } => {
            ctx.level += 1;
            ctx.push_scope();
            let mut pts = Vec::new();
            for p in params {
                let ty =
                    p.ty.as_ref()
                        .map(|t| translate_with_annotation_rigids(ctx, t))
                        .unwrap_or_else(|| ctx.store.fresh_var(ctx.level));
                ctx.define_local(p.name.text.clone(), Scheme::mono(ty.clone()));
                pts.push(ty);
            }
            let body_ty = infer_block(ctx, body, true);
            let ret = if let Some(t) = return_type {
                let rt = translate_with_annotation_rigids(ctx, t);
                let mut u = Unifier::new(ctx.store, ctx.sink);
                u.unify(&body_ty, &rt, body.span, None);
                rt
            } else {
                body_ty
            };
            ctx.pop_scope();
            ctx.level -= 1;
            Type::Fun {
                params: pts,
                ret: Box::new(ret),
            }
        }
        ExprKind::Block(b) => infer_block(ctx, b, false),
        ExprKind::Paren(e) => infer_expr(ctx, e),
        ExprKind::Case { subjects, clauses } => infer_case(ctx, subjects, clauses, expr.span),
        ExprKind::Todo { .. } => {
            ctx.sink
                .warning(codes::W1005_TODO, "`todo` placeholder", expr.span, None);
            ctx.store.fresh_var(ctx.level)
        }
        ExprKind::Panic { .. } => ctx.store.fresh_var(ctx.level),
        ExprKind::Assert { expr: e, .. } => {
            let t = infer_expr(ctx, e);
            let mut u = Unifier::new(ctx.store, ctx.sink);
            u.unify(&t, &Type::Bool, e.span, None);
            Type::Nil
        }
        ExprKind::Echo(e) => infer_expr(ctx, e),
        ExprKind::Pipe { .. } => {
            // Should be desugared
            Type::Error
        }
        ExprKind::BitArray(segs) => {
            check_bit_array(ctx, segs);
            Type::BitArray
        }
    }
}

fn infer_constructor(ctx: &mut InferCtx<'_>, c: &ConstructorRef, span: Span) -> Type {
    let name = &c.name.text;
    if let Some(module) = &c.module {
        let Some(path) = ctx.module_path_for_alias(&module.text) else {
            ctx.sink.error(
                codes::E1000_UNKNOWN_NAME,
                format!("unknown module alias `{}`", module.text),
                module.span,
                None,
            );
            return Type::Error;
        };
        ctx.resolved
            .imports_used
            .insert(format!("mod:{}", module.text));
        // Look up in deps through resolved — values from that module
        if let Some((_, v)) = ctx
            .resolved
            .values
            .iter()
            .find(|(n, v)| *n == name && v.from_module == path)
        {
            return unify::instantiate(ctx.store, &v.scheme, ctx.level);
        }
        // Try name directly if selectively imported
        if let Some((scheme, _, _)) = ctx.lookup_value(name) {
            return unify::instantiate(ctx.store, &scheme, ctx.level);
        }
        ctx.sink.error(
            codes::E1000_UNKNOWN_NAME,
            format!("unknown constructor `{}.{}`", module.text, name),
            span,
            None,
        );
        return Type::Error;
    }
    if let Some((scheme, _, _)) = ctx.lookup_value(name) {
        unify::instantiate(ctx.store, &scheme, ctx.level)
    } else {
        ctx.sink.error(
            codes::E1000_UNKNOWN_NAME,
            format!("unknown constructor `{name}`"),
            span,
            None,
        );
        Type::Error
    }
}

fn infer_binop(ctx: &mut InferCtx<'_>, left: &Expr, op: BinOp, right: &Expr, span: Span) -> Type {
    let lt = infer_expr(ctx, left);
    let rt = infer_expr(ctx, right);
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
            let lz = ctx.store.zonk(&lt);
            if matches!(lz, Type::Float) {
                ctx.sink.error(
                    codes::E1352_OP_TYPE,
                    format!("operator `{}` expects `Int`, found `Float`", op.as_str()),
                    span,
                    Some(format!("use `{}` for Float", float_op_hint(op))),
                );
                return Type::Error;
            }
            {
                let mut u = Unifier::new(ctx.store, ctx.sink);
                u.unify(&lt, &Type::Int, left.span, None);
                u.unify(&rt, &Type::Int, right.span, None);
            }
            Type::Int
        }
        BinOp::AddFloat | BinOp::SubFloat | BinOp::MulFloat | BinOp::DivFloat => {
            let mut u = Unifier::new(ctx.store, ctx.sink);
            u.unify(&lt, &Type::Float, left.span, None);
            u.unify(&rt, &Type::Float, right.span, None);
            Type::Float
        }
        BinOp::Concat => {
            let mut u = Unifier::new(ctx.store, ctx.sink);
            u.unify(&lt, &Type::String, left.span, None);
            u.unify(&rt, &Type::String, right.span, None);
            Type::String
        }
        BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
            let mut u = Unifier::new(ctx.store, ctx.sink);
            u.unify(&lt, &Type::Int, left.span, None);
            u.unify(&rt, &Type::Int, right.span, None);
            Type::Bool
        }
        BinOp::LtFloat | BinOp::LtEqFloat | BinOp::GtFloat | BinOp::GtEqFloat => {
            let mut u = Unifier::new(ctx.store, ctx.sink);
            u.unify(&lt, &Type::Float, left.span, None);
            u.unify(&rt, &Type::Float, right.span, None);
            Type::Bool
        }
        BinOp::Eq | BinOp::NotEq => {
            {
                let mut u = Unifier::new(ctx.store, ctx.sink);
                u.unify(&lt, &rt, span, None);
            }
            let z = ctx.store.zonk(&lt);
            match &z {
                Type::Var(_) | Type::Rigid(_) => {
                    unify::add_constraint(ctx.store, &lt, ConstraintSet::eq());
                }
                other if !eq_capability::has_eq(ctx.store, other) => {
                    ctx.sink.error(
                        codes::E1350_NO_EQ,
                        format!(
                            "type `{}` does not support equality",
                            ctx.store.display(other)
                        ),
                        span,
                        Some("functions, selectors, and tasks are not Eq".into()),
                    );
                }
                _ => {}
            }
            Type::Bool
        }
        BinOp::And | BinOp::Or => {
            let mut u = Unifier::new(ctx.store, ctx.sink);
            u.unify(&lt, &Type::Bool, left.span, None);
            u.unify(&rt, &Type::Bool, right.span, None);
            Type::Bool
        }
    }
}

fn float_op_hint(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+.",
        BinOp::Sub => "-.",
        BinOp::Mul => "*.",
        BinOp::Div => "/.",
        _ => "+.",
    }
}

fn infer_call(ctx: &mut InferCtx<'_>, callee: &Expr, args: &[Arg], span: Span) -> Type {
    // Resolve labelled calls only for named functions/ctors
    let (fty, labels) = match &callee.kind {
        ExprKind::Var(n) => {
            if let Some((scheme, labels, ctor_of)) = ctx.lookup_value(&n.text) {
                if ctor_of.is_some() {
                    let inst = unify::instantiate(ctx.store, &scheme, ctx.level);
                    return finish_call_ctor(ctx, inst, &labels, args, span);
                }
                if ctx.scc.contains(&n.text) {
                    // Monomorphic recursive reference — do not instantiate.
                    let inst = ctx.store.zonk(&scheme.body);
                    ctx.scc_rec_call = true;
                    let ret = finish_call(ctx, inst, &labels, args, span);
                    ctx.scc_rec_call = false;
                    return ret;
                }
                let inst = unify::instantiate(ctx.store, &scheme, ctx.level);
                (inst, labels)
            } else {
                (infer_expr(ctx, callee), vec![])
            }
        }
        ExprKind::Constructor(c) => {
            let t = infer_constructor(ctx, c, callee.span);
            let labels = if c.module.is_none() {
                ctx.resolved
                    .values
                    .get(&c.name.text)
                    .map(|v| v.labels.clone())
                    .unwrap_or_default()
            } else {
                vec![]
            };
            return finish_call_ctor(ctx, t, &labels, args, span);
        }
        ExprKind::Field { base, field } => {
            // module.func
            if let ExprKind::Var(m) = &base.kind {
                if let Some(mod_path) = ctx.module_path_for_alias(&m.text) {
                    ctx.resolved.imports_used.insert(format!("mod:{}", m.text));
                    let fname = match field {
                        FieldName::Name(n) => n.text.clone(),
                        FieldName::UName(n) => n.text.clone(),
                    };
                    // Prefer the dependency interface so local names cannot shadow `mod.f`.
                    if let Some(v) = ctx
                        .deps
                        .get(&mod_path)
                        .and_then(|iface| iface.values.get(&fname))
                        .cloned()
                    {
                        let inst = unify::instantiate(ctx.store, &v.scheme, ctx.level);
                        return finish_call(ctx, inst, &v.labels, args, span);
                    }
                    if let Some(v) = ctx
                        .resolved
                        .values
                        .get(&fname)
                        .filter(|v| v.from_module == mod_path)
                        .cloned()
                    {
                        let inst = unify::instantiate(ctx.store, &v.scheme, ctx.level);
                        return finish_call(ctx, inst, &v.labels, args, span);
                    }
                    ctx.sink.error(
                        codes::E1000_UNKNOWN_NAME,
                        format!("unknown name `{}.{}`", m.text, fname),
                        span,
                        None,
                    );
                    return Type::Error;
                }
            }
            (infer_expr(ctx, callee), vec![])
        }
        _ => (infer_expr(ctx, callee), vec![]),
    };
    // Check labels on value: labelled args require a statically known named fn/ctor
    // with declared labels. Locals and arbitrary callees have empty label lists.
    let has_labels = args.iter().any(|a| a.label.is_some());
    if has_labels && labels.is_empty() {
        ctx.sink.error(
            codes::E1213_LABEL_ON_VALUE,
            "labelled arguments require a statically resolved named function or constructor",
            span,
            None,
        );
    }
    finish_call(ctx, fty, &labels, args, span)
}

fn check_call_labels_only(
    ctx: &mut InferCtx<'_>,
    labels: &[Option<String>],
    args: &[Arg],
    span: Span,
) {
    let has_labels = args.iter().any(|a| a.label.is_some());
    if has_labels && labels.is_empty() {
        ctx.sink.error(
            codes::E1213_LABEL_ON_VALUE,
            "labelled arguments require a statically resolved named function or constructor",
            span,
            None,
        );
        return;
    }
    let mut seen_labels = HashSet::new();
    let mut filled = vec![false; labels.len()];
    let mut positional_idx = 0usize;
    for arg in args {
        if let Some(lab) = &arg.label {
            if !seen_labels.insert(lab.text.clone()) {
                ctx.sink.error(
                    codes::E1212_LABEL_DUP,
                    format!("duplicate argument label `{}`", lab.text),
                    lab.span,
                    None,
                );
                continue;
            }
            let Some(idx) = labels
                .iter()
                .position(|l| l.as_deref() == Some(lab.text.as_str()))
            else {
                ctx.sink.error(
                    codes::E1211_LABEL_UNKNOWN,
                    format!("unknown argument label `{}`", lab.text),
                    lab.span,
                    None,
                );
                continue;
            };
            if idx < filled.len() {
                filled[idx] = true;
            }
        } else if !labels.is_empty() {
            while positional_idx < filled.len() && filled[positional_idx] {
                positional_idx += 1;
            }
            if positional_idx >= labels.len() {
                ctx.sink.error(
                    codes::E1214_CALL_ARITY,
                    format!("function expects {} argument(s), got more", labels.len()),
                    arg.span,
                    None,
                );
            } else {
                filled[positional_idx] = true;
                positional_idx += 1;
            }
        }
    }
    if !labels.is_empty() && filled.iter().any(|f| !f) {
        ctx.sink.error(
            codes::E1214_CALL_ARITY,
            format!(
                "function expects {} argument(s), got {}",
                labels.len(),
                filled.iter().filter(|f| **f).count()
            ),
            span,
            None,
        );
    }
}

fn concrete_conflict(store: &mut TypeStore, a: &Type, b: &Type) -> bool {
    let a = store.zonk(a);
    let b = store.zonk(b);
    match (&a, &b) {
        (Type::Error, _) | (_, Type::Error) | (Type::Var(_), _) | (_, Type::Var(_)) => false,
        (Type::Rigid(_), _) | (_, Type::Rigid(_)) => false,
        (Type::Int, Type::Int)
        | (Type::Float, Type::Float)
        | (Type::String, Type::String)
        | (Type::Bool, Type::Bool)
        | (Type::Nil, Type::Nil)
        | (Type::BitArray, Type::BitArray) => false,
        (Type::List(x), Type::List(y)) => concrete_conflict(store, x, y),
        (Type::Tuple(xs), Type::Tuple(ys)) if xs.len() == ys.len() => xs
            .iter()
            .zip(ys)
            .any(|(x, y)| concrete_conflict(store, x, y)),
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
            ps.iter()
                .zip(qs)
                .any(|(x, y)| concrete_conflict(store, x, y))
                || concrete_conflict(store, r1, r2)
        }
        (Type::App { def: d1, args: a1 }, Type::App { def: d2, args: a2 })
            if d1 == d2 && a1.len() == a2.len() =>
        {
            a1.iter()
                .zip(a2)
                .any(|(x, y)| concrete_conflict(store, x, y))
        }
        _ => true,
    }
}

fn finish_call(
    ctx: &mut InferCtx<'_>,
    fty: Type,
    labels: &[Option<String>],
    args: &[Arg],
    span: Span,
) -> Type {
    finish_call_ex(ctx, fty, labels, args, span, false)
}

fn finish_call_ctor(
    ctx: &mut InferCtx<'_>,
    fty: Type,
    labels: &[Option<String>],
    args: &[Arg],
    span: Span,
) -> Type {
    finish_call_ex(ctx, fty, labels, args, span, true)
}

fn finish_call_ex(
    ctx: &mut InferCtx<'_>,
    fty: Type,
    labels: &[Option<String>],
    args: &[Arg],
    span: Span,
    is_ctor: bool,
) -> Type {
    let fty = ctx.store.zonk(&fty);
    let (params, ret) = match fty {
        Type::Fun { params, ret } => (params, *ret),
        Type::Var(_) | Type::Rigid(_) => {
            let param_tys: Vec<Type> = (0..args.len())
                .map(|_| ctx.store.fresh_var(ctx.level))
                .collect();
            let ret = ctx.store.fresh_var(ctx.level);
            let expected = Type::Fun {
                params: param_tys.clone(),
                ret: Box::new(ret.clone()),
            };
            {
                let mut u = Unifier::new(ctx.store, ctx.sink);
                u.unify(&fty, &expected, span, None);
            }
            (param_tys, ret)
        }
        Type::Error => {
            check_call_labels_only(ctx, labels, args, span);
            return Type::Error;
        }
        other => {
            ctx.sink.error(
                codes::E1214_CALL_ARITY,
                format!("expected a function, found `{}`", ctx.store.display(&other)),
                span,
                None,
            );
            return Type::Error;
        }
    };
    let arity_code = if is_ctor {
        codes::E1202_CTOR_ARITY
    } else {
        codes::E1214_CALL_ARITY
    };
    // Assign args to params
    let mut filled = vec![false; params.len()];
    let mut arg_tys = vec![None; params.len()];
    let mut positional_idx = 0usize;
    let mut seen_labels = HashSet::new();
    for arg in args {
        if let Some(lab) = &arg.label {
            if !seen_labels.insert(lab.text.clone()) {
                ctx.sink.error(
                    codes::E1212_LABEL_DUP,
                    format!("duplicate argument label `{}`", lab.text),
                    lab.span,
                    None,
                );
                continue;
            }
            let Some(idx) = labels
                .iter()
                .position(|l| l.as_deref() == Some(lab.text.as_str()))
            else {
                ctx.sink.error(
                    codes::E1211_LABEL_UNKNOWN,
                    format!("unknown argument label `{}`", lab.text),
                    lab.span,
                    None,
                );
                continue;
            };
            if filled[idx] {
                ctx.sink.error(
                    codes::E1212_LABEL_DUP,
                    format!("parameter `{}` already supplied", lab.text),
                    lab.span,
                    None,
                );
                continue;
            }
            filled[idx] = true;
            let ty = match &arg.value {
                ArgValue::Expr(e) => infer_expr(ctx, e),
                ArgValue::Hole => Type::Error,
            };
            arg_tys[idx] = Some(ty);
        } else {
            while positional_idx < filled.len() && filled[positional_idx] {
                positional_idx += 1;
            }
            if positional_idx >= params.len() {
                ctx.sink.error(
                    arity_code,
                    format!(
                        "{} expects {} argument(s), got more",
                        if is_ctor { "constructor" } else { "function" },
                        params.len()
                    ),
                    arg.span,
                    None,
                );
                continue;
            }
            filled[positional_idx] = true;
            let ty = match &arg.value {
                ArgValue::Expr(e) => infer_expr(ctx, e),
                ArgValue::Hole => Type::Error,
            };
            arg_tys[positional_idx] = Some(ty);
            positional_idx += 1;
        }
    }
    if filled.iter().any(|f| !f) {
        ctx.sink.error(
            arity_code,
            format!(
                "{} expects {} argument(s), got {}",
                if is_ctor { "constructor" } else { "function" },
                params.len(),
                filled.iter().filter(|f| **f).count()
            ),
            span,
            None,
        );
    }
    for (i, at) in arg_tys.into_iter().enumerate() {
        if let Some(at) = at {
            if ctx.scc_rec_call {
                let a = ctx.store.zonk(&at);
                let p = ctx.store.zonk(&params[i]);
                if concrete_conflict(ctx.store, &a, &p) {
                    ctx.sink.error(
                        codes::E1303_POLY_RECURSION,
                        "polymorphic recursion is not allowed: recursive calls must use a single monomorphic type",
                        span,
                        Some("annotate the function or split the recursive group".into()),
                    );
                    continue;
                }
            }
            let mut u = Unifier::new(ctx.store, ctx.sink);
            u.unify(&at, &params[i], span, None);
            // Discharge constraints on concrete instantiation
            discharge_after_unify(ctx, &params[i], span);
        }
    }
    discharge_after_unify(ctx, &ret, span);
    ret
}

fn discharge_after_unify(ctx: &mut InferCtx<'_>, ty: &Type, span: Span) {
    let z = ctx.store.zonk(ty);
    if let Type::Var(id) = &z {
        if let Some(info) = ctx.store.vars.get(id) {
            let c = info.constraints.clone();
            if let Some(link) = info.link.clone() {
                let concrete = ctx.store.zonk(&link);
                if c.eq && !eq_capability::has_eq(ctx.store, &concrete) {
                    ctx.sink.error(
                        codes::E1350_NO_EQ,
                        format!(
                            "type `{}` does not support equality (`Eq`)",
                            ctx.store.display(&concrete)
                        ),
                        span,
                        Some(
                            "constraint originated from an equality use or Dict/Set operation"
                                .into(),
                        ),
                    );
                }
                if c.neg
                    && !matches!(
                        concrete,
                        Type::Int | Type::Float | Type::Error | Type::Var(_)
                    )
                {
                    ctx.sink.error(
                        codes::E1351_NO_NEG,
                        format!(
                            "type `{}` does not support negation (`Neg`)",
                            ctx.store.display(&concrete)
                        ),
                        span,
                        None,
                    );
                }
            }
        }
    }
}

fn infer_field(ctx: &mut InferCtx<'_>, base: &Expr, field: &FieldName, span: Span) -> Type {
    // Module-qualified value: process.send as field access used as callee is handled in infer_call.
    // Here: record field access OR module.value as expression
    if let ExprKind::Var(m) = &base.kind {
        if let Some(mod_path) = ctx.module_path_for_alias(&m.text) {
            let fname = match field {
                FieldName::Name(n) => n.text.clone(),
                FieldName::UName(n) => n.text.clone(),
            };
            ctx.resolved.imports_used.insert(format!("mod:{}", m.text));
            if let Some(v) = ctx
                .deps
                .get(&mod_path)
                .and_then(|iface| iface.values.get(&fname))
            {
                return unify::instantiate(ctx.store, &v.scheme, ctx.level);
            }
            if let Some(v) = ctx
                .resolved
                .values
                .get(&fname)
                .filter(|v| v.from_module == mod_path)
            {
                return unify::instantiate(ctx.store, &v.scheme, ctx.level);
            }
            ctx.sink.error(
                codes::E1000_UNKNOWN_NAME,
                format!("unknown name `{}.{}`", m.text, fname),
                span,
                None,
            );
            return Type::Error;
        }
    }
    let bt = infer_expr(ctx, base);
    let z = ctx.store.zonk(&bt);
    let field_name = match field {
        FieldName::Name(n) => n.text.clone(),
        FieldName::UName(n) => n.text.clone(),
    };
    match z {
        Type::Var(_) | Type::Rigid(_) => {
            ctx.sink.error(
                codes::E1210_UNKNOWN_RECEIVER,
                format!(
                    "cannot access field `{field_name}` when the receiver type is not yet known"
                ),
                span,
                Some("annotate the receiver's type so field access can be checked".into()),
            );
            Type::Error
        }
        Type::App { def, args } => {
            let Some(info) = ctx.store.defs.get(&def).cloned() else {
                return Type::Error;
            };
            if info.kind == TypeDefKind::Opaque && !ctx.defining_opaques.contains(&def) {
                ctx.sink.error(
                    codes::E1207_OPAQUE_USE,
                    format!("cannot access fields of opaque type `{}`", info.name),
                    span,
                    Some(
                        "opaque constructors and fields are only available in the defining module"
                            .into(),
                    ),
                );
                return Type::Error;
            }
            // Field must exist on every variant with same type
            if info.variants.is_empty() {
                ctx.sink.error(
                    codes::E1204_UNKNOWN_FIELD,
                    format!("type `{}` has no fields", info.name),
                    span,
                    None,
                );
                return Type::Error;
            }
            let mut field_ty: Option<Type> = None;
            for v in &info.variants {
                let ft = v
                    .fields
                    .iter()
                    .find(|f| f.label.as_deref() == Some(field_name.as_str()));
                match ft {
                    None => {
                        ctx.sink.error(
                            codes::E1205_FIELD_ACCESS,
                            format!(
                                "field `{field_name}` is not present on all variants of `{}`",
                                info.name
                            ),
                            span,
                            None,
                        );
                        return Type::Error;
                    }
                    Some(f) => {
                        // subst params
                        let concrete = apply_tdef_args(ctx.store, &f.ty, &info, &args);
                        if let Some(prev) = &field_ty {
                            // Must be same — structural compare via unify
                            let mut u = Unifier::new(ctx.store, ctx.sink);
                            u.unify(prev, &concrete, span, None);
                        } else {
                            field_ty = Some(concrete);
                        }
                    }
                }
            }
            field_ty.unwrap_or(Type::Error)
        }
        Type::Error => Type::Error,
        other => {
            ctx.sink.error(
                codes::E1205_FIELD_ACCESS,
                format!(
                    "cannot access field `{field_name}` on `{}`",
                    ctx.store.display(&other)
                ),
                span,
                None,
            );
            Type::Error
        }
    }
}

fn apply_tdef_args(store: &mut TypeStore, ty: &Type, info: &TypeDefInfo, args: &[Type]) -> Type {
    // Replace rigids named as params with args
    match ty {
        Type::Rigid(id) => {
            if let Some(r) = store.rigids.get(id) {
                if let Some(idx) = info.params.iter().position(|p| p == &r.name) {
                    return args.get(idx).cloned().unwrap_or(Type::Error);
                }
            }
            Type::Rigid(*id)
        }
        Type::List(t) => Type::List(Box::new(apply_tdef_args(store, t, info, args))),
        Type::Tuple(ts) => Type::Tuple(
            ts.iter()
                .map(|t| apply_tdef_args(store, t, info, args))
                .collect(),
        ),
        Type::Fun { params, ret } => Type::Fun {
            params: params
                .iter()
                .map(|t| apply_tdef_args(store, t, info, args))
                .collect(),
            ret: Box::new(apply_tdef_args(store, ret, info, args)),
        },
        Type::App { def, args: as_ } => Type::App {
            def: *def,
            args: as_
                .iter()
                .map(|t| apply_tdef_args(store, t, info, args))
                .collect(),
        },
        other => other.clone(),
    }
}

fn infer_record_update(
    ctx: &mut InferCtx<'_>,
    constructor: &ConstructorRef,
    base: &Expr,
    fields: &[(Name, Expr)],
    span: Span,
) -> Type {
    let base_ty = infer_expr(ctx, base);
    let ctor_ty = infer_constructor(ctx, constructor, constructor.span);
    // Determine ADT def from constructor
    let ctor_name = &constructor.name.text;
    let Some(vinfo) = ctx.resolved.values.get(ctor_name).cloned() else {
        return Type::Error;
    };
    let Some(def) = vinfo.constructor_of else {
        ctx.sink.error(
            codes::E1206_RECORD_UPDATE,
            "record update requires a constructor",
            span,
            None,
        );
        return Type::Error;
    };
    let Some(info) = ctx.store.defs.get(&def).cloned() else {
        return Type::Error;
    };
    if info.kind == TypeDefKind::Opaque && !ctx.defining_opaques.contains(&def) {
        ctx.sink.error(
            codes::E1207_OPAQUE_USE,
            format!("cannot update opaque type `{}`", info.name),
            span,
            None,
        );
        return Type::Error;
    }
    if info.variants.len() != 1 {
        ctx.sink.error(
            codes::E1209_MULTI_VARIANT_UPDATE,
            format!(
                "record update is only allowed on single-variant types (`{}` has {} variants)",
                info.name,
                info.variants.len()
            ),
            span,
            Some("multi-variant updates would require a hidden runtime panic".into()),
        );
        return Type::Error;
    }
    let ret = match ctx.store.zonk(&ctor_ty) {
        Type::Fun { ret, .. } => *ret,
        other => other,
    };
    {
        let mut u = Unifier::new(ctx.store, ctx.sink);
        u.unify(&base_ty, &ret, base.span, None);
    }
    for (fname, fexpr) in fields {
        let Some(field) = info.variants[0]
            .fields
            .iter()
            .find(|f| f.label.as_deref() == Some(fname.text.as_str()))
        else {
            ctx.sink.error(
                codes::E1204_UNKNOWN_FIELD,
                format!("unknown field `{}` on `{}`", fname.text, info.name),
                fname.span,
                None,
            );
            continue;
        };
        let ft = infer_expr(ctx, fexpr);
        // Unify with field type — approximate
        let _ = field;
        let mut u = Unifier::new(ctx.store, ctx.sink);
        u.unify(&ft, &field.ty, fexpr.span, None);
    }
    ret
}

fn infer_case(ctx: &mut InferCtx<'_>, subjects: &[Expr], clauses: &[Clause], span: Span) -> Type {
    let sub_tys: Vec<Type> = subjects.iter().map(|s| infer_expr(ctx, s)).collect();
    let result = ctx.store.fresh_var(ctx.level);
    for clause in clauses {
        ctx.push_scope();
        for (row_i, row) in clause.patterns.iter().enumerate() {
            if row.patterns.len() != sub_tys.len() {
                ctx.sink.error(
                    codes::E1220_PATTERN_ARITY,
                    format!(
                        "pattern row has {} pattern(s) but case has {} subject(s)",
                        row.patterns.len(),
                        sub_tys.len()
                    ),
                    row.span,
                    None,
                );
            }
            for (p, t) in row.patterns.iter().zip(sub_tys.iter()) {
                bind_pattern(ctx, p, t, true);
                let _ = row_i;
            }
        }
        if let Some(g) = &clause.guard {
            let gt = infer_expr(ctx, g);
            let mut u = Unifier::new(ctx.store, ctx.sink);
            u.unify(&gt, &Type::Bool, g.span, None);
        }
        let bt = infer_expr(ctx, &clause.body);
        let mut u = Unifier::new(ctx.store, ctx.sink);
        u.unify(&bt, &result, clause.body.span, None);
        ctx.pop_scope();
    }
    let mut ex = ExhaustChecker {
        store: ctx.store,
        sink: ctx.sink,
        work: 0,
    };
    ex.check_case(&sub_tys, clauses, span);
    result
}

fn check_bit_array(ctx: &mut InferCtx<'_>, segs: &[BitSegment]) {
    for (i, seg) in segs.iter().enumerate() {
        let mut is_utf8 = false;
        let mut is_bytes = false;
        let mut is_bits = false;
        let mut width: Option<i64> = None;
        let mut little = false;
        for opt in &seg.options {
            match opt {
                BitOption::Named(n) => match n.as_str() {
                    "utf8" => is_utf8 = true,
                    "bytes" => is_bytes = true,
                    "bits" => is_bits = true,
                    "little" => little = true,
                    "big" | "signed" | "unsigned" => {}
                    other => {
                        ctx.sink.error(
                            codes::E1217_BIT_SPEC,
                            format!("unknown bit-array option `{other}`"),
                            seg.span,
                            None,
                        );
                    }
                },
                BitOption::Size(e) => {
                    if let ExprKind::Int(lit) = &e.kind {
                        if let Ok(v) = crate::numeric::int_literal_value(&lit.digits, 10) {
                            width = Some(v);
                        }
                    } else {
                        ctx.sink.error(
                            codes::E1217_BIT_SPEC,
                            "bit-array size must be a literal or bound variable",
                            e.span,
                            None,
                        );
                    }
                }
            }
        }
        let vt = infer_expr(ctx, &seg.value);
        if is_utf8 {
            let mut u = Unifier::new(ctx.store, ctx.sink);
            u.unify(&vt, &Type::String, seg.value.span, None);
        } else if is_bytes || is_bits {
            let mut u = Unifier::new(ctx.store, ctx.sink);
            u.unify(&vt, &Type::BitArray, seg.value.span, None);
            if width.is_none() && i + 1 != segs.len() {
                // unsized only as final — for patterns; for construction ok?
            }
        } else {
            let mut u = Unifier::new(ctx.store, ctx.sink);
            u.unify(&vt, &Type::Int, seg.value.span, None);
            if let Some(w) = width {
                if !(1..=64).contains(&w) {
                    ctx.sink.error(
                        codes::E1217_BIT_SPEC,
                        format!("bit-array segment width {w} is not in 1..=64"),
                        seg.span,
                        None,
                    );
                }
                if little && w % 8 != 0 {
                    ctx.sink.error(
                        codes::E1217_BIT_SPEC,
                        "little-endian bit-array widths must be a multiple of 8",
                        seg.span,
                        None,
                    );
                }
            }
        }
    }
}

fn warn_unused(ctx: &mut InferCtx<'_>, module: &Module) {
    for (name, span, suppressed) in ctx.local_bindings.clone() {
        if suppressed || name.starts_with('_') {
            continue;
        }
        if !ctx.used_values.contains(&name) {
            ctx.sink.warning(
                codes::W1000_UNUSED_VAR,
                format!("unused variable `{name}`"),
                span,
                Some("prefix with `_` to suppress this warning".into()),
            );
        }
    }
    for (key, span) in ctx.resolved.import_keys.clone() {
        let used = if let Some(name) = key.strip_prefix("mod:") {
            ctx.resolved.imports_used.contains(&format!("mod:{name}"))
                || ctx.used_values.iter().any(|u| u.starts_with(name))
        } else if let Some(name) = key.strip_prefix("type:") {
            ctx.used_types.contains(name) || ctx.resolved.imports_used.contains(name)
        } else {
            ctx.used_values.contains(&key) || ctx.resolved.imports_used.contains(&key)
        };
        if !used {
            ctx.sink.warning(
                codes::W1001_UNUSED_IMPORT,
                format!("unused import `{key}`"),
                span,
                None,
            );
        }
    }
    // Unused private fn/const
    for item in &module.items {
        match item {
            ModuleItem::Fn(f) if !f.public => {
                if !ctx.used_values.contains(&f.name.text) && f.name.text != "main" {
                    // Self-reference counts — used_values may include self from body
                    // Conservative: if never looked up from outside body... we mark during infer
                    // Skip if referenced in SCC deps
                    ctx.sink.warning(
                        codes::W1003_UNUSED_PRIVATE,
                        format!("unused private function `{}`", f.name.text),
                        f.name.span,
                        None,
                    );
                }
            }
            ModuleItem::Const(c) if !c.public && !ctx.used_values.contains(&c.name.text) => {
                ctx.sink.warning(
                    codes::W1003_UNUSED_PRIVATE,
                    format!("unused private constant `{}`", c.name.text),
                    c.name.span,
                    None,
                );
            }
            _ => {}
        }
    }
}

fn build_interface(path: &str, ctx: &InferCtx<'_>) -> ModuleInterface {
    let mut iface = ModuleInterface::empty(path);
    for (name, v) in &ctx.resolved.values {
        if v.from_module != path {
            continue;
        }
        if !v.public {
            continue;
        }
        // Ensure no unresolved non-generalised vars escape
        let body = v.scheme.body.clone();
        iface.values.insert(
            name.clone(),
            ExportedValue {
                name: name.clone(),
                scheme: Scheme {
                    vars: v.scheme.vars.clone(),
                    constraints: v.scheme.constraints.clone(),
                    body,
                },
                constructor_of: v.constructor_of,
                labels: v.labels.clone(),
                is_const: v.is_const,
            },
        );
    }
    for (name, t) in &ctx.resolved.types {
        if t.from_module != path || !t.public {
            continue;
        }
        let info = ctx.store.defs.get(&t.def).cloned();
        if let Some(info) = info {
            iface.types.insert(
                name.clone(),
                ExportedType {
                    name: name.clone(),
                    def: t.def,
                    kind: info.kind,
                    params: info.params.clone(),
                    eq_params: info.eq_params.clone(),
                    always_eq: info.always_eq,
                    never_eq: info.never_eq,
                    public: true,
                },
            );
            iface.type_defs.insert(t.def, info);
        }
    }
    iface.recompute_hash();
    iface
}
