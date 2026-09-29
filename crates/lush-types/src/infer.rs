//! Hindley–Milner inference with value restriction and sealed constraints.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

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
use crate::typed::{
    BindingAnnotation, BindingId, ResolvedReference, ResolvedValueKind, TypedModule,
};
use crate::unify::{self, Origin, Unifier};

pub struct InferCtx<'a> {
    pub store: &'a mut TypeStore,
    pub sink: &'a mut TypeSink,
    pub resolved: &'a mut ResolvedModule,
    pub level: u32,
    pub env: Vec<HashMap<String, Scheme>>,
    /// Shared monomorphic type nodes for DAG-shaped lets (let-doubling).
    pub shared_env: Vec<HashMap<String, Rc<Type>>>,
    pub binding_env: Vec<HashMap<String, BindingId>>,
    pub next_binding_id: u32,
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
    /// Names currently being bound by a `let` (for E1012 anon self-ref).
    pub binding_names: HashSet<String>,
    pub typed: &'a mut TypedModule,
}

impl<'a> InferCtx<'a> {
    pub fn push_scope(&mut self) {
        self.env.push(HashMap::new());
        self.shared_env.push(HashMap::new());
        self.binding_env.push(HashMap::new());
    }
    pub fn pop_scope(&mut self) {
        self.env.pop();
        self.binding_env.pop();
        if let Some(scope) = self.shared_env.pop() {
            for rc in scope.values() {
                self.store.closed.remove(&Rc::as_ptr(rc));
            }
        }
    }
    pub fn define_local(&mut self, name: String, scheme: Scheme) -> BindingId {
        self.define_local_at(name, scheme, Span::default())
    }

    pub fn define_local_at(&mut self, name: String, scheme: Scheme, span: Span) -> BindingId {
        let id = BindingId(self.next_binding_id);
        self.next_binding_id = self.next_binding_id.saturating_add(1);
        let local = self.env.len() > 1;
        let compiler_generated = self.typed.generated_names.contains(&name);
        self.typed.record_binding(BindingAnnotation {
            id,
            name: name.clone(),
            span,
            local,
            compiler_generated,
        });
        let shared = Rc::new(scheme.body.clone());
        self.store.mark_closed_rc(&shared);
        if let Some(scope) = self.env.last_mut() {
            scope.insert(name.clone(), scheme);
        }
        if let Some(scope) = self.shared_env.last_mut() {
            scope.insert(name.clone(), shared);
        }
        if let Some(scope) = self.binding_env.last_mut() {
            scope.insert(name, id);
        }
        id
    }

    pub fn lookup_binding_id(&self, name: &str) -> Option<BindingId> {
        self.binding_env
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }
    pub fn lookup_shared(&self, name: &str) -> Option<Rc<Type>> {
        for scope in self.shared_env.iter().rev() {
            if let Some(t) = scope.get(name) {
                return Some(t.clone());
            }
        }
        None
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

    fn resolved_reference_for_value(&self, name: &str, value: &ValueInfo) -> ResolvedReference {
        let target_name = self
            .resolved
            .value_origins
            .get(name)
            .map(String::as_str)
            .unwrap_or(name);
        if let Some(module) = value.from_module.strip_prefix("module:") {
            return ResolvedReference::ModuleAlias {
                module: module.into(),
            };
        }
        let builtin_tag = if value.from_module == "prelude" {
            match target_name {
                "True" => Some(0),
                "False" => Some(1),
                "Nil" => Some(0),
                _ => None,
            }
        } else {
            None
        };
        let kind = if let Some(tag) = builtin_tag {
            ResolvedValueKind::Constructor {
                type_def: None,
                variant_tag: Some(tag),
                arity: 0,
                field_labels: vec![],
            }
        } else if let Some(type_def) = value.constructor_of {
            let (variant_tag, arity) = self
                .store
                .defs
                .get(&type_def)
                .map(|definition| {
                    (
                        definition
                            .variants
                            .iter()
                            .position(|variant| variant.name == target_name)
                            .map(|index| index as u32),
                        value.labels.len(),
                    )
                })
                .unwrap_or((None, value.labels.len()));
            ResolvedValueKind::Constructor {
                type_def: Some(type_def),
                variant_tag,
                arity,
                field_labels: value.labels.clone(),
            }
        } else if value.is_const {
            ResolvedValueKind::Constant
        } else {
            ResolvedValueKind::Function
        };
        if value.from_module == self.module_path {
            ResolvedReference::TopLevel {
                module: value.from_module.clone(),
                name: target_name.into(),
                kind,
            }
        } else {
            ResolvedReference::Imported {
                module: value.from_module.clone(),
                name: target_name.into(),
                kind,
            }
        }
    }

    fn record_module_qualifier(&mut self, expr: &Expr, module: &str) {
        self.typed.record_module_qualifier(expr);
        self.typed.record_reference(
            expr,
            ResolvedReference::ModuleAlias {
                module: module.to_string(),
            },
        );
    }

    fn record_var_reference(&mut self, expr: &Expr, name: &str) {
        let local = self.lookup_binding_id(name).and_then(|id| {
            self.typed
                .bindings
                .get(&id)
                .filter(|binding| binding.local)
                .map(|_| ResolvedReference::LocalBinding(id))
        });
        let reference = local.or_else(|| {
            self.resolved
                .values
                .get(name)
                .cloned()
                .map(|value| self.resolved_reference_for_value(name, &value))
        });
        if let Some(reference) = reference {
            self.typed.record_reference(expr, reference);
        }
    }

    fn record_constructor_reference(&mut self, expr: &Expr, constructor: &ConstructorRef) {
        if let Some(reference) = self.constructor_reference(constructor) {
            self.typed.record_reference(expr, reference);
        }
    }

    fn record_constructor_pattern_reference(
        &mut self,
        pattern: &Pattern,
        constructor: &ConstructorRef,
    ) {
        if let Some(reference) = self.constructor_reference(constructor) {
            self.typed.record_pattern_reference(pattern, reference);
        }
    }

    fn constructor_reference(&self, constructor: &ConstructorRef) -> Option<ResolvedReference> {
        if let Some(module_alias) = &constructor.module {
            let module_path = self.module_path_for_alias(&module_alias.text)?;
            let export = self
                .deps
                .get(&module_path)?
                .values
                .get(&constructor.name.text)?;
            let value = ValueInfo {
                scheme: export.scheme.clone(),
                public: true,
                from_module: module_path,
                labels: export.labels.clone(),
                constructor_of: export.constructor_of,
                is_const: export.is_const,
                span: constructor.span,
            };
            return Some(self.resolved_reference_for_value(&constructor.name.text, &value));
        }
        let value = self.resolved.values.get(&constructor.name.text)?;
        Some(self.resolved_reference_for_value(&constructor.name.text, value))
    }
}

pub struct InferModuleOptions<'a> {
    pub entry: bool,
    pub typed: &'a mut TypedModule,
}

pub fn infer_module(
    path: &str,
    module: &Module,
    resolved: &mut ResolvedModule,
    store: &mut TypeStore,
    sink: &mut TypeSink,
    deps: &BTreeMap<String, ModuleInterface>,
    options: InferModuleOptions<'_>,
) -> ModuleInterface {
    let InferModuleOptions { entry, typed } = options;
    // Elaborate type definitions
    elaborate_types(path, module, resolved, store, sink, deps);

    let mut ctx = InferCtx {
        store,
        sink,
        resolved,
        level: 0,
        env: vec![HashMap::new()],
        shared_env: vec![HashMap::new()],
        binding_env: vec![HashMap::new()],
        next_binding_id: 0,
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
        binding_names: HashSet::new(),
        typed,
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
    let mut const_type_cache = HashMap::new();
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
            // Empty lists are polymorphic: introduce the element variable above the
            // ambient level so generalisation can quantify it (`[]` is non-expansive).
            let ty = match &val {
                ConstValue::List(items) if items.is_empty() => {
                    Type::list(ctx.store.fresh_var(ctx.level + 1))
                }
                _ => (*const_value_type_shared(&val, &mut const_type_cache)).clone(),
            };
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
            let scheme = unify::generalise(ctx.store, &ty, ctx.level, false);
            if let Some(v) = ctx.resolved.values.get_mut(&c.name.text) {
                v.scheme = scheme.clone();
            }
            ctx.define_local_at(c.name.text.clone(), scheme, c.name.span);
            ctx.const_env.types.insert(c.name.text.clone(), ty);
        }
        ctx.const_env.visiting.pop();
    }

    // Constant values were evaluated above; infer their source expressions now
    // so every expression in the typed handoff also receives a type annotation.
    for c in &consts {
        if ctx.const_env.values.contains_key(&c.name.text) {
            let _ = infer_expr(&mut ctx, &c.value);
        }
    }

    // Const-to-const references count as uses for unused-private warnings.
    for c in &consts {
        mark_const_refs_used(&mut ctx, &c.value);
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
    // Build once: rebuilding per SCC is O(n²) for n independent functions.
    let by_name: HashMap<&str, &FnDef> = fns.iter().map(|f| (f.name.text.as_str(), *f)).collect();
    for scc in sccs {
        infer_fn_scc(&mut ctx, &by_name, &scc);
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

    // Finalize the handoff only after all unifications have completed.
    ctx.typed.zonk_types(ctx.store);
    ctx.typed.constants = ctx
        .const_env
        .values
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();

    // E1305: a type walk hit MAX_DEPTH (e.g. Mairson composition).
    if ctx.store.too_deep {
        ctx.sink.error(
            codes::E1305_TOO_DEEP,
            "type checking exceeded maximum nesting depth",
            ctx.store.too_deep_span,
            Some("simplify nested types or expressions".into()),
        );
    }

    // E1306: inference work budget (distinct from exhaustiveness E1451).
    if ctx.store.work > crate::limits::MAX_EXHAUST_WORK.saturating_mul(4) {
        ctx.sink.error(
            codes::E1306_TOO_COMPLEX,
            "type checking exceeded the work budget for this module",
            Span::default(),
            Some("simplify large definitions or split the module".into()),
        );
    }

    build_interface(path, &mut ctx)
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

fn const_value_type_shared(value: &ConstValue, cache: &mut HashMap<usize, Rc<Type>>) -> Rc<Type> {
    match value {
        ConstValue::Int(_) => Rc::new(Type::Int),
        ConstValue::Float(_) => Rc::new(Type::Float),
        ConstValue::String(_) => Rc::new(Type::String),
        ConstValue::Bool(_) => Rc::new(Type::Bool),
        ConstValue::Nil => Rc::new(Type::Nil),
        ConstValue::Tuple(items) => {
            let key = Rc::as_ptr(items) as usize;
            if let Some(ty) = cache.get(&key) {
                return ty.clone();
            }
            let fields = items
                .iter()
                .map(|item| const_value_type_shared(item, cache))
                .collect();
            let ty = Rc::new(Type::Tuple(fields));
            cache.insert(key, ty.clone());
            ty
        }
        ConstValue::List(items) => {
            let key = Rc::as_ptr(items) as usize;
            if let Some(ty) = cache.get(&key) {
                return ty.clone();
            }
            let elem = items
                .first()
                .map(|item| const_value_type_shared(item, cache))
                .unwrap_or_else(|| Rc::new(Type::Error));
            let ty = Rc::new(Type::List(elem));
            cache.insert(key, ty.clone());
            ty
        }
        ConstValue::Adt { .. } => Rc::new(Type::Error), // nominal filled elsewhere
    }
}

fn elaborate_types(
    path: &str,
    module: &Module,
    resolved: &mut ResolvedModule,
    store: &mut TypeStore,
    sink: &mut TypeSink,
    deps: &BTreeMap<String, ModuleInterface>,
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
                let body =
                    translate_type_expr_raw(store, sink, resolved, te, &tvar_rigids, path, deps);
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
                                deps,
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
                    let ret = Type::app(ti.def, app_args);
                    let param_tys: Vec<Type> = fields.iter().map(|f| f.ty.clone()).collect();
                    let body_ty = if param_tys.is_empty() {
                        ret
                    } else {
                        Type::fun(param_tys.clone(), ret)
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
    deps: &BTreeMap<String, ModuleInterface>,
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
                .map(|a| translate_type_expr_raw(store, sink, resolved, a, rigids, _path, deps))
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
                    return Type::list(args_t.into_iter().next().unwrap());
                }
                _ => {}
            }
            // Resolve type name
            let def = if let Some(m) = &module_qual {
                // Look up module alias → dependency interface type.
                let mod_path = resolved
                    .values
                    .get(m)
                    .and_then(|v| v.from_module.strip_prefix("module:").map(|s| s.to_string()));
                if let Some(path) = &mod_path {
                    if let Some(t) = deps.get(path).and_then(|iface| iface.types.get(&tname)) {
                        Some(t.def)
                    } else {
                        resolved
                            .types
                            .iter()
                            .find(|(n, t)| *n == &tname && &t.from_module == path)
                            .map(|(_, t)| t.def)
                            .or_else(|| resolved.types.get(&tname).map(|t| t.def))
                    }
                } else {
                    // Unknown alias — still try selective import of the bare name.
                    resolved.types.get(&tname).map(|t| t.def)
                }
            } else {
                resolved.types.get(&tname).map(|t| t.def)
            };
            let Some(def) = def else {
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
            // Expand alias (including parameterised aliases)
            if let Some(info) = store.defs.get(&def) {
                if info.kind == TypeDefKind::Alias {
                    if let Some(body) = &info.alias_body {
                        if info.params.is_empty() {
                            return body.clone();
                        }
                        // Substitute type arguments into the alias body.
                        let params = info.params.clone();
                        let body = body.clone();
                        return subst_params_store(store, &body, &params, &args_t);
                    }
                }
            }
            Type::app(def, args_t)
        }
        TypeKind::Fn { params, ret } => Type::fun(
            params
                .iter()
                .map(|p| translate_type_expr_raw(store, sink, resolved, p, rigids, _path, deps))
                .collect(),
            translate_type_expr_raw(store, sink, resolved, ret, rigids, _path, deps),
        ),
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
            Type::tuple(
                ts.iter()
                    .map(|t| translate_type_expr_raw(store, sink, resolved, t, rigids, _path, deps))
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
    mark_type_names_used(ctx, te);
    translate_type_expr_raw(
        ctx.store,
        ctx.sink,
        ctx.resolved,
        te,
        rigids,
        &ctx.module_path,
        ctx.deps,
    )
}

fn mark_type_names_used(ctx: &mut InferCtx<'_>, te: &TypeExpr) {
    match &te.kind {
        TypeKind::Named { name, args } => {
            match name {
                TypeName::Unqualified(u) => {
                    ctx.used_types.insert(u.text.clone());
                    if let Some(t) = ctx.resolved.types.get(&u.text) {
                        // Mark the module import as used when the type comes from elsewhere.
                        if t.from_module != ctx.module_path && !t.from_module.is_empty() {
                            let seg = t.from_module.rsplit('/').next().unwrap_or(&t.from_module);
                            ctx.resolved.imports_used.insert(format!("mod:{seg}"));
                            ctx.resolved.imports_used.insert(t.from_module.clone());
                        }
                    }
                }
                TypeName::Qualified { module, name } => {
                    ctx.used_types.insert(name.text.clone());
                    ctx.resolved
                        .imports_used
                        .insert(format!("mod:{}", module.text));
                }
            }
            for a in args {
                mark_type_names_used(ctx, a);
            }
        }
        TypeKind::Var(_) => {}
        TypeKind::Fn { params, ret } => {
            for p in params {
                mark_type_names_used(ctx, p);
            }
            mark_type_names_used(ctx, ret);
        }
        TypeKind::Tuple(ts) => {
            for t in ts {
                mark_type_names_used(ctx, t);
            }
        }
    }
}

fn fn_deps(fns: &[&FnDef]) -> BTreeMap<String, Vec<String>> {
    let names: HashSet<_> = fns.iter().map(|f| f.name.text.as_str()).collect();
    let mut deps = BTreeMap::new();
    for f in fns {
        let mut used = HashSet::new();
        let mut locals = HashSet::new();
        // Parameters shadow top-level names.
        for p in &f.params {
            locals.insert(p.name.text.as_str());
        }
        walk_expr_names_scoped(&f.body, &mut used, &mut locals);
        let d: Vec<_> = used
            .into_iter()
            .filter(|n| names.contains(n.as_str()))
            .collect();
        deps.insert(f.name.text.clone(), d);
    }
    deps
}

fn walk_expr_names_scoped<'a>(
    b: &'a Block,
    out: &mut HashSet<String>,
    locals: &mut HashSet<&'a str>,
) {
    for s in &b.statements {
        match s {
            Statement::Expr(e) => walk_expr_names_scoped_expr(e, out, locals),
            Statement::Let(l) => {
                walk_expr_names_scoped_expr(&l.value, out, locals);
                bind_pat_names(&l.pattern, locals);
            }
            Statement::Fn(f) => {
                let mut nested = locals.clone();
                for p in &f.params {
                    nested.insert(p.name.text.as_str());
                }
                nested.insert(f.name.text.as_str());
                walk_expr_names_scoped(&f.body, out, &mut nested);
            }
            Statement::Use(u) => walk_expr_names_scoped_expr(&u.value, out, locals),
        }
    }
}

fn bind_pat_names<'a>(p: &'a Pattern, locals: &mut HashSet<&'a str>) {
    match &p.kind {
        PatternKind::Var(n) | PatternKind::UnderscoreName(n) => {
            locals.insert(n.text.as_str());
        }
        PatternKind::Alias { pattern, name } => {
            locals.insert(name.text.as_str());
            bind_pat_names(pattern, locals);
        }
        PatternKind::Tuple(ps) => {
            for p in ps {
                bind_pat_names(p, locals);
            }
        }
        PatternKind::List { items, spread } => {
            for p in items {
                bind_pat_names(p, locals);
            }
            if let Some(s) = spread {
                bind_pat_names(s, locals);
            }
        }
        PatternKind::Constructor {
            args: Some(args), ..
        } => {
            for a in args {
                if let Some(p) = &a.pattern {
                    bind_pat_names(p, locals);
                }
            }
        }
        PatternKind::StringPrefix { rest, .. } => bind_pat_names(rest, locals),
        PatternKind::BitArray(segs) => {
            for s in segs {
                bind_pat_names(&s.pattern, locals);
            }
        }
        _ => {}
    }
}

fn walk_expr_names_scoped_expr<'a>(
    e: &'a Expr,
    out: &mut HashSet<String>,
    locals: &HashSet<&'a str>,
) {
    match &e.kind {
        ExprKind::Var(n) => {
            if !locals.contains(n.text.as_str()) {
                out.insert(n.text.clone());
            }
        }
        ExprKind::Call { callee, args } => {
            walk_expr_names_scoped_expr(callee, out, locals);
            for a in args {
                if let ArgValue::Expr(e) = &a.value {
                    walk_expr_names_scoped_expr(e, out, locals);
                }
            }
        }
        ExprKind::Pipe { left, right } | ExprKind::Binary { left, right, .. } => {
            walk_expr_names_scoped_expr(left, out, locals);
            walk_expr_names_scoped_expr(right, out, locals);
        }
        ExprKind::Unary { expr, .. }
        | ExprKind::Paren(expr)
        | ExprKind::Echo(expr)
        | ExprKind::Field { base: expr, .. }
        | ExprKind::Assert { expr, .. } => walk_expr_names_scoped_expr(expr, out, locals),
        ExprKind::Tuple(xs) | ExprKind::List { items: xs, .. } => {
            for x in xs {
                walk_expr_names_scoped_expr(x, out, locals);
            }
            if let ExprKind::List {
                spread: Some(s), ..
            } = &e.kind
            {
                walk_expr_names_scoped_expr(s, out, locals);
            }
        }
        ExprKind::Fn { params, body, .. } => {
            let mut nested = locals.clone();
            for p in params {
                nested.insert(p.name.text.as_str());
            }
            walk_expr_names_scoped(body, out, &mut nested);
        }
        ExprKind::Block(b) => {
            let mut nested = locals.clone();
            walk_expr_names_scoped(b, out, &mut nested);
        }
        ExprKind::Case { subjects, clauses } => {
            for s in subjects {
                walk_expr_names_scoped_expr(s, out, locals);
            }
            for c in clauses {
                let mut nested = locals.clone();
                for row in &c.patterns {
                    for p in &row.patterns {
                        bind_pat_names(p, &mut nested);
                    }
                }
                if let Some(g) = &c.guard {
                    walk_expr_names_scoped_expr(g, out, &nested);
                }
                walk_expr_names_scoped_expr(&c.body, out, &nested);
            }
        }
        ExprKind::RecordUpdate { base, fields, .. } => {
            walk_expr_names_scoped_expr(base, out, locals);
            for (_, e) in fields {
                walk_expr_names_scoped_expr(e, out, locals);
            }
        }
        ExprKind::BitArray(segs) => {
            for s in segs {
                walk_expr_names_scoped_expr(&s.value, out, locals);
            }
        }
        ExprKind::Constructor(c) => {
            if let Some(m) = &c.module {
                if !locals.contains(m.text.as_str()) {
                    out.insert(m.text.clone());
                }
            }
        }
        _ => {}
    }
}

fn strong_components(deps: &BTreeMap<String, Vec<String>>) -> Vec<Vec<String>> {
    // Iterative Tarjan over integer node ids (no recursive DFS).
    let mut nodes: Vec<String> = deps.keys().cloned().collect();
    nodes.sort();
    let index_of: HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let n = nodes.len();
    let mut adj: Vec<Vec<usize>> = vec![vec![]; n];
    for (name, ds) in deps {
        let Some(&u) = index_of.get(name.as_str()) else {
            continue;
        };
        for d in ds {
            if let Some(&v) = index_of.get(d.as_str()) {
                adj[u].push(v);
            }
        }
    }

    let mut indices = vec![None; n];
    let mut lowlink = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut sccs: Vec<Vec<String>> = Vec::new();
    let mut next_index = 0usize;

    // Explicit DFS stack: (node, next_edge_idx, entering)
    for start in 0..n {
        if indices[start].is_some() {
            continue;
        }
        let mut work: Vec<(usize, usize)> = vec![(start, 0)];
        indices[start] = Some(next_index);
        lowlink[start] = next_index;
        next_index += 1;
        stack.push(start);
        on_stack[start] = true;

        while let Some((v, edge_i)) = work.pop() {
            if edge_i < adj[v].len() {
                work.push((v, edge_i + 1));
                let w = adj[v][edge_i];
                if indices[w].is_none() {
                    indices[w] = Some(next_index);
                    lowlink[w] = next_index;
                    next_index += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    work.push((w, 0));
                } else if on_stack[w] {
                    lowlink[v] = lowlink[v].min(indices[w].unwrap());
                }
            } else {
                // Post-order: update parent lowlink and maybe pop SCC
                if let Some(&(parent, _)) = work.last() {
                    lowlink[parent] = lowlink[parent].min(lowlink[v]);
                }
                if lowlink[v] == indices[v].unwrap() {
                    let mut comp = Vec::new();
                    loop {
                        let w = stack.pop().unwrap();
                        on_stack[w] = false;
                        comp.push(nodes[w].clone());
                        if w == v {
                            break;
                        }
                    }
                    sccs.push(comp);
                }
            }
        }
    }

    // Tarjan emits each component after its successors are explored. With edges
    // meaning "depends on" / "calls", that is already callees-before-callers
    // (topological order of the condensation). Do not reverse.
    sccs
}

fn infer_fn_scc(ctx: &mut InferCtx<'_>, by_name: &HashMap<&str, &FnDef>, scc: &[String]) {
    ctx.scc = scc.iter().cloned().collect();
    ctx.level += 1;
    // Assign fresh mono types
    let mut mono: HashMap<String, Type> = HashMap::new();
    let mut rigid_maps: HashMap<String, HashMap<String, Type>> = HashMap::new();
    for name in scc {
        let f = by_name.get(name.as_str()).copied().unwrap();
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
        let ty = Type::fun(params.clone(), ret.clone());
        mono.insert(name.clone(), ty.clone());
        ctx.define_local_at(name.clone(), Scheme::mono(ty), f.name.span);
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
        let f = by_name.get(name.as_str()).copied().unwrap();
        ctx.push_scope();
        let Type::Fun { params, ret } = mono.get(name).unwrap().clone() else {
            unreachable!()
        };
        for (p, ty) in f.params.iter().zip(params.iter()) {
            ctx.define_local_at(
                p.name.text.clone(),
                Scheme::mono((**ty).clone()),
                p.name.span,
            );
        }
        let body_ty = infer_block(ctx, &f.body, true);
        let mut u = Unifier::new(ctx.store, ctx.sink);
        u.unify(
            &body_ty,
            ret.as_ref(),
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
                // Collect pattern-bound names so anonymous RHS self-refs get E1012.
                let mut names = HashSet::new();
                collect_pat_bind_names(&l.pattern, &mut names);
                for n in &names {
                    ctx.binding_names.insert(n.clone());
                }
                // Infer the RHS one level up so non-expansive bindings can be
                // generalised. Expansive RHSs then have their free variables
                // lowered back to the enclosing level (value restriction).
                let expansive = is_expansive(&l.value);
                ctx.level += 1;
                let val_ty = infer_expr(ctx, &l.value);
                ctx.level -= 1;
                if expansive {
                    unify::lower_levels(ctx.store, &val_ty, ctx.level);
                }
                for n in &names {
                    ctx.binding_names.remove(n);
                }
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
                infer_pattern(ctx, &l.pattern, &val_ty, expansive);
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
        let ty = Type::fun(params, ret);
        mono.insert(f.name.text.clone(), ty.clone());
        ctx.define_local_at(f.name.text.clone(), Scheme::mono(ty), f.name.span);
    }
    for f in group {
        ctx.push_scope();
        let Type::Fun { params, ret } = mono.get(&f.name.text).unwrap().clone() else {
            unreachable!()
        };
        for (p, ty) in f.params.iter().zip(params.iter()) {
            ctx.define_local_at(
                p.name.text.clone(),
                Scheme::mono((**ty).clone()),
                p.name.span,
            );
        }
        let body_ty = infer_block(ctx, &f.body, true);
        let mut u = Unifier::new(ctx.store, ctx.sink);
        u.unify(&body_ty, ret.as_ref(), f.body.span, None);
        ctx.pop_scope();
    }
    ctx.level -= 1;
    for name in names {
        let ty = mono.get(name).unwrap();
        let scheme = unify::generalise(ctx.store, ty, ctx.level, false);
        let span = group
            .iter()
            .find(|function| function.name.text == *name)
            .map(|function| function.name.span)
            .unwrap_or_default();
        ctx.define_local_at(name.clone(), scheme, span);
    }
}

fn collect_pat_bind_names(p: &Pattern, out: &mut HashSet<String>) {
    match &p.kind {
        PatternKind::Var(n) | PatternKind::UnderscoreName(n) => {
            out.insert(n.text.clone());
        }
        PatternKind::Alias { pattern, name } => {
            out.insert(name.text.clone());
            collect_pat_bind_names(pattern, out);
        }
        PatternKind::Tuple(ps) => {
            for p in ps {
                collect_pat_bind_names(p, out);
            }
        }
        PatternKind::List { items, spread } => {
            for p in items {
                collect_pat_bind_names(p, out);
            }
            if let Some(s) = spread {
                collect_pat_bind_names(s, out);
            }
        }
        PatternKind::Constructor {
            args: Some(args), ..
        } => {
            for a in args {
                if let Some(p) = &a.pattern {
                    collect_pat_bind_names(p, out);
                }
            }
        }
        PatternKind::StringPrefix { rest, .. } => collect_pat_bind_names(rest, out),
        PatternKind::BitArray(segs) => {
            for s in segs {
                collect_pat_bind_names(&s.pattern, out);
            }
        }
        _ => {}
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

fn infer_pattern(ctx: &mut InferCtx<'_>, pat: &Pattern, expected: &Type, expansive: bool) {
    ctx.typed.record_pattern(pat, expected.clone());
    match &pat.kind {
        PatternKind::Var(n) => {
            let scheme = unify::generalise(ctx.store, expected, ctx.level, expansive);
            ctx.define_local_at(n.text.clone(), scheme, n.span);
            ctx.local_bindings.push((n.text.clone(), n.span, false));
        }
        PatternKind::UnderscoreName(n) => {
            let scheme = unify::generalise(ctx.store, expected, ctx.level, expansive);
            ctx.define_local_at(n.text.clone(), scheme, n.span);
            ctx.local_bindings.push((n.text.clone(), n.span, true));
        }
        PatternKind::Discard => {}
        PatternKind::Alias { pattern, name } => {
            let scheme = unify::generalise(ctx.store, expected, ctx.level, expansive);
            ctx.define_local_at(name.text.clone(), scheme, name.span);
            ctx.local_bindings
                .push((name.text.clone(), name.span, false));
            infer_pattern(ctx, pattern, expected, expansive);
        }
        PatternKind::Int(lit) => {
            let base = match lit.base {
                IntBase::Decimal => 10,
                IntBase::Hex => 16,
                IntBase::Octal => 8,
                IntBase::Binary => 2,
            };
            let _ = const_eval::check_int_lit(&lit.digits, base, false, pat.span, ctx.sink);
            let mut u = Unifier::new(ctx.store, ctx.sink);
            u.unify(expected, &Type::Int, pat.span, None);
        }
        PatternKind::Float(lit) => {
            if crate::numeric::parse_float_literal(&lit.raw).is_err() {
                ctx.sink.error(
                    codes::E1401_FLOAT_RANGE,
                    "float literal outside the finite binary64 range",
                    pat.span,
                    None,
                );
            }
            let mut u = Unifier::new(ctx.store, ctx.sink);
            u.unify(expected, &Type::Float, pat.span, None);
        }
        PatternKind::String(_) => {
            let mut u = Unifier::new(ctx.store, ctx.sink);
            u.unify(expected, &Type::String, pat.span, None);
        }
        PatternKind::Tuple(ps) => {
            if ps.len() < 2 {
                ctx.sink.error(
                    codes::E1200_TYPE_ARITY,
                    "tuples require at least 2 elements",
                    pat.span,
                    None,
                );
                return;
            }
            let fresh: Vec<Type> = (0..ps.len())
                .map(|_| ctx.store.fresh_var(ctx.level))
                .collect();
            let tup = Type::tuple(fresh.clone());
            {
                let mut u = Unifier::new(ctx.store, ctx.sink);
                u.unify(expected, &tup, pat.span, None);
            }
            for (p, t) in ps.iter().zip(fresh.iter()) {
                infer_pattern(ctx, p, t, expansive);
            }
        }
        PatternKind::List { items, spread } => {
            let elem = ctx.store.fresh_var(ctx.level);
            let list_ty = Type::list(elem.clone());
            {
                let mut u = Unifier::new(ctx.store, ctx.sink);
                u.unify(expected, &list_ty, pat.span, None);
            }
            for p in items {
                infer_pattern(ctx, p, &elem, expansive);
            }
            if let Some(s) = spread {
                infer_pattern(ctx, s, &Type::list(elem), expansive);
            }
        }
        PatternKind::Constructor { constructor, args } => {
            ctx.record_constructor_pattern_reference(pat, constructor);
            infer_ctor_pattern(
                ctx,
                constructor,
                args.as_deref(),
                expected,
                expansive,
                pat.span,
            );
        }
        PatternKind::StringPrefix { prefix: _, rest } => {
            let mut u = Unifier::new(ctx.store, ctx.sink);
            u.unify(expected, &Type::String, pat.span, None);
            // Rest binds a String.
            infer_pattern(ctx, rest, &Type::String, expansive);
        }
        PatternKind::BitArray(segs) => {
            let mut u = Unifier::new(ctx.store, ctx.sink);
            u.unify(expected, &Type::BitArray, pat.span, None);
            check_bit_array_pattern(ctx, pat, segs, expansive);
        }
    }
}

fn infer_ctor_pattern(
    ctx: &mut InferCtx<'_>,
    constructor: &ConstructorRef,
    args: Option<&[PatternArg]>,
    expected: &Type,
    expansive: bool,
    span: Span,
) {
    // Resolve constructor scheme (same path as expressions).
    let ctor_name = &constructor.name.text;
    let (scheme, labels, ctor_of) = if let Some(mod_name) = &constructor.module {
        // Qualified: module.Ctor
        let Some(path) = ctx.module_path_for_alias(&mod_name.text) else {
            ctx.sink.error(
                codes::E1002_UNKNOWN_MODULE,
                format!("unknown module `{}`", mod_name.text),
                mod_name.span,
                None,
            );
            return;
        };
        let Some(iface) = ctx.deps.get(&path) else {
            ctx.sink.error(
                codes::E1002_UNKNOWN_MODULE,
                format!("unknown module `{path}`"),
                mod_name.span,
                None,
            );
            return;
        };
        let Some(v) = iface.values.get(ctor_name).cloned() else {
            // Opaque constructors are omitted from the interface value table —
            // still diagnose E1207 when the name is a known opaque variant.
            for t in iface.types.values() {
                if let Some(info) = iface
                    .type_defs
                    .get(&t.def)
                    .or_else(|| ctx.store.defs.get(&t.def))
                {
                    if info.kind == TypeDefKind::Opaque
                        && info.variants.iter().any(|v| v.name == *ctor_name)
                    {
                        ctx.sink.error(
                            codes::E1207_OPAQUE_USE,
                            format!(
                                "cannot pattern-match on opaque type `{}` outside its defining module",
                                info.name
                            ),
                            span,
                            Some(
                                "opaque constructors are only available in the defining module"
                                    .into(),
                            ),
                        );
                        return;
                    }
                }
            }
            ctx.sink.error(
                codes::E1000_UNKNOWN_NAME,
                format!("unknown constructor `{ctor_name}`"),
                constructor.name.span,
                None,
            );
            return;
        };
        if let Some(def) = v.constructor_of {
            if let Some(info) = ctx.store.defs.get(&def) {
                if info.kind == TypeDefKind::Opaque && info.module != ctx.module_path {
                    ctx.sink.error(
                        codes::E1207_OPAQUE_USE,
                        format!(
                            "cannot pattern-match on opaque type `{}` outside its defining module",
                            info.name
                        ),
                        span,
                        Some(
                            "opaque constructors are only available in the defining module".into(),
                        ),
                    );
                    return;
                }
            }
        }
        (v.scheme.clone(), v.labels.clone(), v.constructor_of)
    } else if let Some((s, labs, c)) = ctx.lookup_value(ctor_name) {
        (s, labs, c)
    } else {
        // Nullary Bool/Nil sugar: `True` / `False` / `Nil` as patterns
        match ctor_name.as_str() {
            "True" | "False" => {
                let mut u = Unifier::new(ctx.store, ctx.sink);
                u.unify(expected, &Type::Bool, span, None);
                if args.is_some_and(|a| !a.is_empty()) {
                    ctx.sink.error(
                        codes::E1220_PATTERN_ARITY,
                        format!("constructor `{ctor_name}` takes 0 argument(s)"),
                        span,
                        None,
                    );
                }
                return;
            }
            "Nil" => {
                let mut u = Unifier::new(ctx.store, ctx.sink);
                u.unify(expected, &Type::Nil, span, None);
                if args.is_some_and(|a| !a.is_empty()) {
                    ctx.sink.error(
                        codes::E1220_PATTERN_ARITY,
                        "constructor `Nil` takes 0 argument(s)",
                        span,
                        None,
                    );
                }
                return;
            }
            _ => {
                ctx.sink.error(
                    codes::E1000_UNKNOWN_NAME,
                    format!("unknown constructor `{ctor_name}`"),
                    constructor.name.span,
                    crate::resolve::did_you_mean(
                        ctor_name,
                        ctx.resolved.values.keys().map(|s| s.as_str()),
                    ),
                );
                return;
            }
        }
    };

    // Opaque check for unqualified constructors from another module.
    if let Some(def) = ctor_of {
        if let Some(info) = ctx.store.defs.get(&def) {
            if info.kind == TypeDefKind::Opaque
                && info.module != ctx.module_path
                && !ctx.defining_opaques.contains(&def)
            {
                ctx.sink.error(
                    codes::E1207_OPAQUE_USE,
                    format!(
                        "cannot pattern-match on opaque type `{}` outside its defining module",
                        info.name
                    ),
                    span,
                    Some("opaque constructors are only available in the defining module".into()),
                );
                return;
            }
        }
    }

    let inst = unify::instantiate(ctx.store, &scheme, ctx.level);
    let (field_tys, ret_ty) = match ctx.store.zonk(&inst) {
        Type::Fun { params, ret } => (
            params.iter().map(|p| (**p).clone()).collect::<Vec<_>>(),
            (*ret).clone(),
        ),
        other => (vec![], other),
    };
    {
        let mut u = Unifier::new(ctx.store, ctx.sink);
        u.unify(expected, &ret_ty, span, None);
    }

    let arg_list = args.unwrap_or(&[]);
    let n_payload = arg_list.iter().filter(|a| !a.spread).count();
    if n_payload != field_tys.len() && !arg_list.iter().any(|a| a.spread) {
        ctx.sink.error(
            codes::E1220_PATTERN_ARITY,
            format!(
                "constructor `{ctor_name}` takes {} argument(s), found {n_payload}",
                field_tys.len()
            ),
            span,
            None,
        );
    }
    let _ = labels;
    let mut fi = 0usize;
    for a in arg_list {
        if a.spread {
            continue;
        }
        if let Some(p) = &a.pattern {
            let ft = field_tys.get(fi).cloned().unwrap_or(Type::Error);
            infer_pattern(ctx, p, &ft, expansive);
        }
        fi += 1;
    }
}

fn resolved_bit_segment_annotation(
    options: &[BitOption],
    typed: &TypedModule,
) -> crate::typed::BitSegmentAnnotation {
    use crate::typed::{BitSegmentAnnotation, BitSegmentKind};

    let mut kind = None;
    let mut size = None;
    let mut size_expr = None;
    let mut signed = None;
    let mut little_endian = false;
    for option in options {
        match option {
            BitOption::Size(expr) => {
                if let ExprKind::Int(literal) = &expr.kind {
                    let radix = match literal.base {
                        IntBase::Decimal => 10,
                        IntBase::Hex => 16,
                        IntBase::Octal => 8,
                        IntBase::Binary => 2,
                    };
                    size = crate::numeric::int_literal_value(&literal.digits, radix).ok();
                } else {
                    size_expr = typed.expr_id(expr);
                }
            }
            BitOption::Named(name) => {
                let next_kind = match name.as_str() {
                    "utf8" => Some(BitSegmentKind::Utf8),
                    "bytes" => Some(BitSegmentKind::Bytes),
                    "bits" => Some(BitSegmentKind::Bits),
                    _ => None,
                };
                if let Some(next_kind) = next_kind {
                    if kind.is_some_and(|current| current != next_kind) {
                        kind = Some(BitSegmentKind::Invalid);
                    } else if kind != Some(BitSegmentKind::Invalid) {
                        kind = Some(next_kind);
                    }
                }
                match name.as_str() {
                    "signed" => signed = Some(true),
                    "unsigned" => signed = Some(false),
                    "little" => little_endian = true,
                    "big" => little_endian = false,
                    _ => {}
                }
            }
        }
    }
    let kind = kind.unwrap_or(BitSegmentKind::Integer);
    if kind == BitSegmentKind::Integer {
        size.get_or_insert(8);
        signed.get_or_insert(false);
    }
    BitSegmentAnnotation {
        kind,
        size,
        size_expr,
        signed,
        little_endian,
    }
}

fn check_bit_array_pattern(
    ctx: &mut InferCtx<'_>,
    pattern: &Pattern,
    segs: &[BitSegmentPat],
    expansive: bool,
) {
    let segments = segs
        .iter()
        .map(|segment| resolved_bit_segment_annotation(&segment.options, ctx.typed))
        .collect();
    ctx.typed.record_pattern_bit_segments(pattern, segments);
    for (i, seg) in segs.iter().enumerate() {
        let mut is_utf8 = false;
        let mut is_bytes = false;
        let mut is_bits = false;
        let mut sized = false;
        for opt in &seg.options {
            match opt {
                BitOption::Named(n) => match n.as_str() {
                    "utf8" => is_utf8 = true,
                    "bytes" => is_bytes = true,
                    "bits" => is_bits = true,
                    "little" | "big" | "signed" | "unsigned" => {}
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
                    sized = true;
                    match &e.kind {
                        ExprKind::Int(_) => ctx.typed.record_expr(e, Type::Int),
                        ExprKind::Var(n) => {
                            // Must be previously bound.
                            ctx.record_var_reference(e, &n.text);
                            if let Some((scheme, _, _)) = ctx.lookup_value(&n.text) {
                                let size_ty = unify::instantiate(ctx.store, &scheme, ctx.level);
                                let mut u = Unifier::new(ctx.store, ctx.sink);
                                u.unify(&size_ty, &Type::Int, e.span, None);
                                ctx.typed.record_expr(e, size_ty);
                            } else {
                                ctx.sink.error(
                                    codes::E1217_BIT_SPEC,
                                    format!(
                                        "bit-array size `{0}` must be a literal or previously bound variable",
                                        n.text
                                    ),
                                    e.span,
                                    None,
                                );
                                ctx.typed.record_expr(e, Type::Error);
                            }
                        }
                        _ => {
                            ctx.sink.error(
                                codes::E1217_BIT_SPEC,
                                "bit-array size must be a literal or previously bound variable",
                                e.span,
                                None,
                            );
                            ctx.typed.record_expr(e, Type::Error);
                        }
                    }
                }
            }
        }
        if (is_bytes || is_bits) && !sized && i + 1 != segs.len() {
            ctx.sink.error(
                codes::E1217_BIT_SPEC,
                "unsized `bytes`/`bits` segments must be the final segment in a pattern",
                seg.span,
                None,
            );
        }
        if is_utf8 {
            // utf8 only with a literal string pattern
            if !matches!(seg.pattern.kind, PatternKind::String(_)) {
                ctx.sink.error(
                    codes::E1217_BIT_SPEC,
                    "`utf8` in a bit-array pattern requires a literal string",
                    seg.span,
                    None,
                );
            }
            infer_pattern(ctx, &seg.pattern, &Type::String, expansive);
        } else if is_bytes || is_bits {
            infer_pattern(ctx, &seg.pattern, &Type::BitArray, expansive);
        } else {
            // Default: Int segment (or bind variable as Int)
            infer_pattern(ctx, &seg.pattern, &Type::Int, expansive);
        }
    }
}

fn subst_params_store(store: &TypeStore, ty: &Type, params: &[String], args: &[Type]) -> Type {
    match ty {
        Type::Rigid(id) => {
            if let Some(info) = store.rigids.get(id) {
                if let Some(idx) = params.iter().position(|p| *p == info.name) {
                    return args.get(idx).cloned().unwrap_or(Type::Error);
                }
            }
            Type::Rigid(*id)
        }
        Type::List(t) => Type::list(subst_params_store(store, t, params, args)),
        Type::Tuple(ts) => Type::tuple(
            ts.iter()
                .map(|t| subst_params_store(store, t, params, args))
                .collect(),
        ),
        Type::Fun { params: ps, ret } => Type::fun(
            ps.iter()
                .map(|t| subst_params_store(store, t, params, args))
                .collect(),
            subst_params_store(store, ret, params, args),
        ),
        Type::App { def, args: as_ } => Type::app(
            *def,
            as_.iter()
                .map(|t| subst_params_store(store, t, params, args))
                .collect(),
        ),
        other => other.clone(),
    }
}

pub fn infer_expr(ctx: &mut InferCtx<'_>, expr: &Expr) -> Type {
    ctx.store.hint_span = expr.span;
    (*infer_expr_shared(ctx, expr)).clone()
}

/// Infer an expression, returning a shared type node when the expression is a
/// monomorphic local variable (enables DAG sharing for `#(x, x)`).
fn infer_expr_shared(ctx: &mut InferCtx<'_>, expr: &Expr) -> Rc<Type> {
    let inferred = match &expr.kind {
        ExprKind::Var(n) => {
            // Mark used / emit errors via the normal path, but reuse the shared node
            // for monomorphic locals so `#(x, x)` is a true DAG.
            if let Some(shared) = ctx.lookup_shared(&n.text) {
                if let Some((scheme, _, _)) = ctx.lookup_value(&n.text) {
                    if scheme.vars.is_empty() && !ctx.scc.contains(&n.text) {
                        shared
                    } else {
                        Rc::new(unify::instantiate(ctx.store, &scheme, ctx.level))
                    }
                } else {
                    Rc::new(infer_expr_inner(ctx, expr))
                }
            } else {
                Rc::new(infer_expr_inner(ctx, expr))
            }
        }
        ExprKind::Paren(inner) => infer_expr_shared(ctx, inner),
        _ => Rc::new(infer_expr_inner(ctx, expr)),
    };
    ctx.typed.record_expr(expr, (*inferred).clone());
    match &expr.kind {
        ExprKind::Var(name) => ctx.record_var_reference(expr, &name.text),
        ExprKind::Constructor(constructor) => ctx.record_constructor_reference(expr, constructor),
        _ => {}
    }
    inferred
}

fn infer_expr_inner(ctx: &mut InferCtx<'_>, expr: &Expr) -> Type {
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
            } else if ctx.binding_names.contains(&n.text) {
                ctx.sink.error(
                    codes::E1012_SELF_REF_ANON,
                    format!(
                        "anonymous function cannot refer to `{}` being defined",
                        n.text
                    ),
                    n.span,
                    Some("use a named `fn name(...) {{ ... }}` for recursion".into()),
                );
                Type::Error
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
            // Share Rc nodes for repeated variable references (let-doubling DAG).
            let parts: Vec<Rc<Type>> = items.iter().map(|e| infer_expr_shared(ctx, e)).collect();
            Type::tuple_shared(parts)
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
                u.unify(&st, &Type::list(elem.clone()), s.span, None);
            }
            Type::list(elem)
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
                    ctx.typed.record_expr(inner, Type::Int);
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
        ExprKind::Call { callee, args } => infer_call(ctx, expr, callee, args, expr.span),
        ExprKind::Field { base, field } => infer_field(ctx, expr, base, field, expr.span),
        ExprKind::RecordUpdate {
            constructor,
            base,
            fields,
        } => infer_record_update(ctx, expr, constructor, base, fields, expr.span),
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
                ctx.define_local_at(p.name.text.clone(), Scheme::mono(ty.clone()), p.name.span);
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
            Type::fun(pts, ret)
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
            check_bit_array(ctx, expr, segs);
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
        // Prefer the dependency interface so qualified ctors resolve without selective import.
        if let Some(v) = ctx.deps.get(&path).and_then(|iface| iface.values.get(name)) {
            return unify::instantiate(ctx.store, &v.scheme, ctx.level);
        }
        if let Some((_, v)) = ctx
            .resolved
            .values
            .iter()
            .find(|(n, v)| *n == name && v.from_module == path)
        {
            return unify::instantiate(ctx.store, &v.scheme, ctx.level);
        }
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

fn infer_call(
    ctx: &mut InferCtx<'_>,
    call: &Expr,
    callee: &Expr,
    args: &[Arg],
    span: Span,
) -> Type {
    if let ExprKind::Var(name) = &callee.kind {
        ctx.record_var_reference(callee, &name.text);
    } else if let ExprKind::Constructor(constructor) = &callee.kind {
        ctx.record_constructor_reference(callee, constructor);
    }
    // Resolve labelled calls only for named functions/ctors
    let (fty, labels) = match &callee.kind {
        ExprKind::Var(n) => {
            if let Some((scheme, labels, ctor_of)) = ctx.lookup_value(&n.text) {
                if ctor_of.is_some() {
                    let inst = unify::instantiate(ctx.store, &scheme, ctx.level);
                    ctx.typed.record_expr(callee, inst.clone());
                    return finish_call_ctor(ctx, call, inst, &labels, args, span);
                }
                if ctx.scc.contains(&n.text) {
                    // Monomorphic recursive reference — do not instantiate.
                    let inst = ctx.store.zonk(&scheme.body);
                    ctx.typed.record_expr(callee, inst.clone());
                    ctx.scc_rec_call = true;
                    let ret = finish_call(ctx, call, inst, &labels, args, span);
                    ctx.scc_rec_call = false;
                    return ret;
                }
                let inst = unify::instantiate(ctx.store, &scheme, ctx.level);
                ctx.typed.record_expr(callee, inst.clone());
                (inst, labels)
            } else {
                (infer_expr(ctx, callee), vec![])
            }
        }
        ExprKind::Constructor(c) => {
            let t = infer_constructor(ctx, c, callee.span);
            ctx.typed.record_expr(callee, t.clone());
            let labels = if c.module.is_none() {
                ctx.resolved
                    .values
                    .get(&c.name.text)
                    .map(|v| v.labels.clone())
                    .unwrap_or_default()
            } else {
                vec![]
            };
            return finish_call_ctor(ctx, call, t, &labels, args, span);
        }
        ExprKind::Field { base, field } => {
            // module.func
            if let ExprKind::Var(m) = &base.kind {
                if let Some(mod_path) = ctx.module_path_for_alias(&m.text) {
                    ctx.record_module_qualifier(base, &mod_path);
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
                        let value = ValueInfo {
                            scheme: v.scheme,
                            public: true,
                            from_module: mod_path.clone(),
                            labels: v.labels,
                            constructor_of: v.constructor_of,
                            is_const: v.is_const,
                            span: callee.span,
                        };
                        let reference = ctx.resolved_reference_for_value(&fname, &value);
                        ctx.typed.record_reference(callee, reference);
                        let inst = unify::instantiate(ctx.store, &value.scheme, ctx.level);
                        ctx.typed.record_expr(callee, inst.clone());
                        return finish_call(ctx, call, inst, &value.labels, args, span);
                    }
                    if let Some(v) = ctx
                        .resolved
                        .values
                        .get(&fname)
                        .filter(|v| v.from_module == mod_path)
                        .cloned()
                    {
                        let reference = ctx.resolved_reference_for_value(&fname, &v);
                        ctx.typed.record_reference(callee, reference);
                        let inst = unify::instantiate(ctx.store, &v.scheme, ctx.level);
                        ctx.typed.record_expr(callee, inst.clone());
                        return finish_call(ctx, call, inst, &v.labels, args, span);
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
    finish_call(ctx, call, fty, &labels, args, span)
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
    let mut seen_labelled = false;
    let mut label_order_error = false;
    for arg in args {
        if let Some(lab) = &arg.label {
            seen_labelled = true;
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
            if seen_labelled {
                ctx.sink.error(
                    codes::E1213_LABEL_ON_VALUE,
                    "positional argument after a labelled argument",
                    arg.span,
                    Some("put positional arguments before labelled ones".into()),
                );
                label_order_error = true;
                continue;
            }
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
    if label_order_error {
        return;
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
    call: &Expr,
    fty: Type,
    labels: &[Option<String>],
    args: &[Arg],
    span: Span,
) -> Type {
    finish_call_ex(ctx, call, fty, labels, args, span, false)
}

fn finish_call_ctor(
    ctx: &mut InferCtx<'_>,
    call: &Expr,
    fty: Type,
    labels: &[Option<String>],
    args: &[Arg],
    span: Span,
) -> Type {
    finish_call_ex(ctx, call, fty, labels, args, span, true)
}

fn finish_call_ex(
    ctx: &mut InferCtx<'_>,
    call: &Expr,
    fty: Type,
    labels: &[Option<String>],
    args: &[Arg],
    span: Span,
    is_ctor: bool,
) -> Type {
    let fty = ctx.store.zonk(&fty);
    let (params, ret) = match fty {
        Type::Fun { params, ret } => (
            params.iter().map(|p| (**p).clone()).collect(),
            (*ret).clone(),
        ),
        Type::Var(_) | Type::Rigid(_) => {
            let param_tys: Vec<Type> = (0..args.len())
                .map(|_| ctx.store.fresh_var(ctx.level))
                .collect();
            let ret = ctx.store.fresh_var(ctx.level);
            let expected = Type::fun(param_tys.clone(), ret.clone());
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
    // Assign args to params while preserving their original source order.
    let mut filled = vec![false; params.len()];
    let mut arg_tys = vec![None; params.len()];
    let mut argument_to_parameter = vec![None; args.len()];
    let mut positional_idx = 0usize;
    let mut seen_labels = HashSet::new();
    let mut seen_labelled = false;
    let mut label_order_error = false;
    for (argument_position, arg) in args.iter().enumerate() {
        if let Some(lab) = &arg.label {
            seen_labelled = true;
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
            argument_to_parameter[argument_position] = Some(idx);
            let ty = match &arg.value {
                ArgValue::Expr(e) => infer_expr(ctx, e),
                ArgValue::Hole => Type::Error,
            };
            arg_tys[idx] = Some(ty);
        } else {
            if arg.implicit_use_callback {
                if let Some(idx) = params.len().checked_sub(1).filter(|idx| !filled[*idx]) {
                    filled[idx] = true;
                    argument_to_parameter[argument_position] = Some(idx);
                    let ty = match &arg.value {
                        ArgValue::Expr(e) => infer_expr(ctx, e),
                        ArgValue::Hole => Type::Error,
                    };
                    arg_tys[idx] = Some(ty);
                    continue;
                }
            }
            if seen_labelled {
                ctx.sink.error(
                    codes::E1213_LABEL_ON_VALUE,
                    "positional argument after a labelled argument",
                    arg.span,
                    Some("put positional arguments before labelled ones".into()),
                );
                label_order_error = true;
                continue;
            }
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
            argument_to_parameter[argument_position] = Some(positional_idx);
            let ty = match &arg.value {
                ArgValue::Expr(e) => infer_expr(ctx, e),
                ArgValue::Hole => Type::Error,
            };
            arg_tys[positional_idx] = Some(ty);
            positional_idx += 1;
        }
    }
    ctx.typed.record_call_mapping(call, argument_to_parameter);
    if label_order_error {
        return Type::Error;
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
                let concrete = ctx.store.zonk(link.as_ref());
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

fn infer_field(
    ctx: &mut InferCtx<'_>,
    expr: &Expr,
    base: &Expr,
    field: &FieldName,
    span: Span,
) -> Type {
    // Module-qualified value: process.send as field access used as callee is handled in infer_call.
    // Here: record field access OR module.value as expression
    if let ExprKind::Var(m) = &base.kind {
        if let Some(mod_path) = ctx.module_path_for_alias(&m.text) {
            ctx.record_module_qualifier(base, &mod_path);
            let fname = match field {
                FieldName::Name(n) => n.text.clone(),
                FieldName::UName(n) => n.text.clone(),
            };
            ctx.resolved.imports_used.insert(format!("mod:{}", m.text));
            if let Some(export) = ctx
                .deps
                .get(&mod_path)
                .and_then(|iface| iface.values.get(&fname))
                .cloned()
            {
                let value = ValueInfo {
                    scheme: export.scheme,
                    public: true,
                    from_module: mod_path.clone(),
                    labels: export.labels,
                    constructor_of: export.constructor_of,
                    is_const: export.is_const,
                    span,
                };
                let reference = ctx.resolved_reference_for_value(&fname, &value);
                ctx.typed.record_reference(expr, reference);
                return unify::instantiate(ctx.store, &value.scheme, ctx.level);
            }
            if let Some(value) = ctx
                .resolved
                .values
                .get(&fname)
                .filter(|value| value.from_module == mod_path)
                .cloned()
            {
                let reference = ctx.resolved_reference_for_value(&fname, &value);
                ctx.typed.record_reference(expr, reference);
                return unify::instantiate(ctx.store, &value.scheme, ctx.level);
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
    ctx.typed.record_receiver_type(expr, bt.clone());
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
            let mut field_positions = Vec::with_capacity(info.variants.len());
            for (variant_tag, variant) in info.variants.iter().enumerate() {
                let ft = variant
                    .fields
                    .iter()
                    .enumerate()
                    .find(|(_, field)| field.label.as_deref() == Some(field_name.as_str()));
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
                    Some((field_index, f)) => {
                        field_positions.push(crate::typed::VariantFieldPosition {
                            variant_tag: variant_tag as u32,
                            field_index,
                        });
                        // subst params
                        let owned: Vec<Type> = args.iter().map(|t| (**t).clone()).collect();
                        let concrete = apply_tdef_args(ctx.store, &f.ty, &info, &owned);
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
            ctx.typed.record_field_positions(expr, field_positions);
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
        Type::List(t) => Type::list(apply_tdef_args(store, t, info, args)),
        Type::Tuple(ts) => Type::tuple(
            ts.iter()
                .map(|t| apply_tdef_args(store, t, info, args))
                .collect(),
        ),
        Type::Fun { params, ret } => Type::fun(
            params
                .iter()
                .map(|t| apply_tdef_args(store, t, info, args))
                .collect(),
            apply_tdef_args(store, ret, info, args),
        ),
        Type::App { def, args: as_ } => Type::app(
            *def,
            as_.iter()
                .map(|t| apply_tdef_args(store, t, info, args))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn infer_record_update(
    ctx: &mut InferCtx<'_>,
    expr: &Expr,
    constructor: &ConstructorRef,
    base: &Expr,
    fields: &[(Name, Expr)],
    span: Span,
) -> Type {
    let base_ty = infer_expr(ctx, base);
    ctx.typed.record_receiver_type(expr, base_ty.clone());
    ctx.record_constructor_reference(expr, constructor);
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
        Type::Fun { ret, .. } => (*ret).clone(),
        other => other,
    };
    {
        let mut u = Unifier::new(ctx.store, ctx.sink);
        u.unify(&base_ty, &ret, base.span, None);
    }
    let mut field_positions = Vec::with_capacity(fields.len());
    for (fname, fexpr) in fields {
        let Some((field_index, field)) = info.variants[0]
            .fields
            .iter()
            .enumerate()
            .find(|(_, f)| f.label.as_deref() == Some(fname.text.as_str()))
        else {
            ctx.sink.error(
                codes::E1204_UNKNOWN_FIELD,
                format!("unknown field `{}` on `{}`", fname.text, info.name),
                fname.span,
                None,
            );
            continue;
        };
        field_positions.push(crate::typed::VariantFieldPosition {
            variant_tag: 0,
            field_index,
        });
        let ft = infer_expr(ctx, fexpr);
        let mut u = Unifier::new(ctx.store, ctx.sink);
        u.unify(&ft, &field.ty, fexpr.span, None);
    }
    ctx.typed.record_field_positions(expr, field_positions);
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
                infer_pattern(ctx, p, t, true);
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
    ex.check_case(&sub_tys, clauses, span, &ctx.resolved.value_origins);
    result
}

fn check_bit_array(ctx: &mut InferCtx<'_>, expr: &Expr, segs: &[BitSegment]) {
    let segments = segs
        .iter()
        .map(|segment| resolved_bit_segment_annotation(&segment.options, ctx.typed))
        .collect();
    ctx.typed.record_expr_bit_segments(expr, segments);
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
                        ctx.typed.record_expr(e, Type::Int);
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
                        ctx.typed.record_expr(e, Type::Error);
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

fn mark_const_refs_used(ctx: &mut InferCtx<'_>, e: &Expr) {
    match &e.kind {
        ExprKind::Var(n) => {
            if ctx.const_env.values.contains_key(&n.text)
                || ctx.const_env.types.contains_key(&n.text)
            {
                ctx.used_values.insert(n.text.clone());
            }
        }
        ExprKind::Tuple(xs) | ExprKind::List { items: xs, .. } => {
            for x in xs {
                mark_const_refs_used(ctx, x);
            }
            if let ExprKind::List {
                spread: Some(s), ..
            } = &e.kind
            {
                mark_const_refs_used(ctx, s);
            }
        }
        ExprKind::Binary { left, right, .. } | ExprKind::Pipe { left, right } => {
            mark_const_refs_used(ctx, left);
            mark_const_refs_used(ctx, right);
        }
        ExprKind::Unary { expr, .. }
        | ExprKind::Paren(expr)
        | ExprKind::Echo(expr)
        | ExprKind::Field { base: expr, .. } => mark_const_refs_used(ctx, expr),
        ExprKind::Call { callee, args } => {
            mark_const_refs_used(ctx, callee);
            for a in args {
                if let ArgValue::Expr(e) = &a.value {
                    mark_const_refs_used(ctx, e);
                }
            }
        }
        ExprKind::BitArray(segs) => {
            for s in segs {
                mark_const_refs_used(ctx, &s.value);
            }
        }
        _ => {}
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
                || ctx.used_types.iter().any(|t| {
                    ctx.resolved
                        .types
                        .get(t)
                        .is_some_and(|ti| ti.from_module.ends_with(name) || ti.from_module == *name)
                })
                || ctx.used_values.iter().any(|u| u.starts_with(name))
        } else if let Some(name) = key.strip_prefix("type:") {
            ctx.used_types.contains(name) || ctx.resolved.imports_used.contains(name)
        } else {
            ctx.used_values.contains(&key)
                || ctx.used_types.contains(&key)
                || ctx.resolved.imports_used.contains(&key)
        };
        if !used {
            let display = key
                .strip_prefix("mod:")
                .or_else(|| key.strip_prefix("type:"))
                .unwrap_or(key.as_str());
            ctx.sink.warning(
                codes::W1001_UNUSED_IMPORT,
                format!("unused import `{display}`"),
                span,
                None,
            );
        }
    }
    // Unused private fn/const — const-to-const refs count via used_values
    for item in &module.items {
        match item {
            ModuleItem::Fn(f) if !f.public => {
                if !ctx.used_values.contains(&f.name.text) && f.name.text != "main" {
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

fn build_interface(path: &str, ctx: &mut InferCtx<'_>) -> ModuleInterface {
    let mut iface = ModuleInterface::empty(path);
    for (name, v) in &ctx.resolved.values {
        if v.from_module != path {
            continue;
        }
        if !v.public {
            continue;
        }
        // Ensure no unresolved non-generalised vars escape
        let body = ctx.store.zonk(&v.scheme.body);
        if unify::scheme_has_escaping_vars(ctx.store, &v.scheme) {
            ctx.sink.error(
                codes::E1304_ESCAPE,
                format!("ungeneralised type variable escapes in public value `{name}`"),
                v.span,
                Some("add a type annotation or avoid expansive bindings in the export".into()),
            );
        }
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
