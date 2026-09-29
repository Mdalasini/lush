//! Name resolution: imports, prelude, privacy, duplicates, alias cycles.

use std::collections::{BTreeMap, HashMap, HashSet};

use lush_syntax::ast::*;
use lush_syntax::span::Span;

use crate::codes;
use crate::diag::TypeSink;
use crate::interface::ModuleInterface;
use crate::limits::MAX_EDIT_DISTANCE;
use crate::ty::{Scheme, Type, TypeDefId, TypeStore};

#[derive(Clone, Debug)]
pub enum ValueBinding {
    Scheme(Scheme),
    /// Module alias → path for qualified access.
    ModuleAlias(String),
}

#[derive(Clone, Debug)]
pub struct ResolvedModule {
    pub path: String,
    pub values: HashMap<String, ValueInfo>,
    /// Imported binding name to the original exported name (for `as` aliases).
    pub value_origins: HashMap<String, String>,
    pub types: HashMap<String, TypeInfo>,
    pub imports_used: HashSet<String>,
    pub import_keys: Vec<(String, Span)>, // for unused warnings
}

#[derive(Clone, Debug)]
pub struct ValueInfo {
    pub scheme: Scheme,
    pub public: bool,
    pub from_module: String,
    pub labels: Vec<Option<String>>,
    pub constructor_of: Option<TypeDefId>,
    pub is_const: bool,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct TypeInfo {
    pub def: TypeDefId,
    pub public: bool,
    pub from_module: String,
    pub span: Span,
}

pub struct ResolveCtx<'a> {
    pub store: &'a mut TypeStore,
    pub sink: &'a mut TypeSink,
    pub deps: &'a BTreeMap<String, ModuleInterface>,
    pub prelude: &'a ModuleInterface,
    pub prefixes: &'a [String],
}

pub fn resolve_module(path: &str, module: &Module, ctx: &mut ResolveCtx<'_>) -> ResolvedModule {
    let mut resolved = ResolvedModule {
        path: path.to_string(),
        values: HashMap::new(),
        value_origins: HashMap::new(),
        types: HashMap::new(),
        imports_used: HashSet::new(),
        import_keys: Vec::new(),
    };

    // Reject lush/ shadowing for user modules
    if path.starts_with("lush/") && !crate::stubs::is_stub_path(path) {
        ctx.sink.error(
            codes::E1007_RESERVED_LUSH,
            format!("module path `{path}` shadows the reserved `lush/` namespace"),
            module.span,
            Some("choose a path outside `lush/`".into()),
        );
    }

    // Install prelude
    install_interface(&mut resolved, ctx.prelude, true, Span::default());

    // Process imports
    for item in &module.items {
        if let ModuleItem::Import(imp) = item {
            resolve_import(imp, &mut resolved, ctx);
        }
    }

    // Collect local type defs first (for mutual recursion)
    for item in &module.items {
        if let ModuleItem::Type(td) = item {
            define_local_type(path, td, &mut resolved, ctx);
        }
    }
    // Alias cycle check
    check_alias_cycles(&resolved, ctx);

    // Local values: consts, fns, constructors (from type defs)
    for item in &module.items {
        match item {
            ModuleItem::Const(c) => {
                insert_value(
                    &mut resolved,
                    ctx.sink,
                    c.name.text.clone(),
                    ValueInfo {
                        scheme: Scheme::mono(Type::Error), // filled by inference
                        public: c.public,
                        from_module: path.to_string(),
                        labels: vec![],
                        constructor_of: None,
                        is_const: true,
                        span: c.name.span,
                    },
                    c.name.span,
                );
            }
            ModuleItem::Fn(f) => {
                let labels: Vec<_> = f
                    .params
                    .iter()
                    .map(|p| {
                        p.label
                            .as_ref()
                            .map(|l| l.text.clone())
                            .or_else(|| Some(p.name.text.clone()))
                    })
                    .collect();
                insert_value(
                    &mut resolved,
                    ctx.sink,
                    f.name.text.clone(),
                    ValueInfo {
                        scheme: Scheme::mono(Type::Error),
                        public: f.public,
                        from_module: path.to_string(),
                        labels,
                        constructor_of: None,
                        is_const: false,
                        span: f.name.span,
                    },
                    f.name.span,
                );
            }
            _ => {}
        }
    }

    resolved
}

fn install_interface(
    resolved: &mut ResolvedModule,
    iface: &ModuleInterface,
    _public: bool,
    span: Span,
) {
    for (name, v) in &iface.values {
        resolved.values.entry(name.clone()).or_insert(ValueInfo {
            scheme: v.scheme.clone(),
            public: true,
            from_module: iface.path.clone(),
            labels: v.labels.clone(),
            constructor_of: v.constructor_of,
            is_const: v.is_const,
            span,
        });
    }
    for (name, t) in &iface.types {
        resolved.types.entry(name.clone()).or_insert(TypeInfo {
            def: t.def,
            public: t.public,
            from_module: iface.path.clone(),
            span,
        });
    }
}

fn resolve_import(imp: &Import, resolved: &mut ResolvedModule, ctx: &mut ResolveCtx<'_>) {
    let path = resolve_import_path(&imp.path, ctx);
    let Some(path) = path else {
        ctx.sink.error(
            codes::E1002_UNKNOWN_MODULE,
            format!("unknown module `{}`", imp.path.segments.join("/")),
            imp.path.span,
            did_you_mean_module(&imp.path.segments.join("/"), ctx.deps),
        );
        return;
    };
    let Some(iface) = ctx.deps.get(&path) else {
        ctx.sink.error(
            codes::E1002_UNKNOWN_MODULE,
            format!("unknown module `{path}`"),
            imp.path.span,
            None,
        );
        return;
    };

    if let Some(items) = &imp.items {
        // Selective import
        for item in items {
            let name = name_or_uname_text(&item.name);
            let bind_as = item
                .alias
                .as_ref()
                .map(name_or_uname_text)
                .unwrap_or_else(|| name.clone());
            if item.is_type {
                match iface.types.get(&name) {
                    Some(t) if t.public => {
                        resolved.types.insert(
                            bind_as.clone(),
                            TypeInfo {
                                def: t.def,
                                public: true,
                                from_module: path.clone(),
                                span: item.span,
                            },
                        );
                        resolved
                            .import_keys
                            .push((format!("type:{bind_as}"), item.span));
                    }
                    Some(_) => {
                        ctx.sink.error(
                            codes::E1003_PRIVATE,
                            format!("type `{name}` is not `pub` in `{path}`"),
                            item.span,
                            Some("export it with `pub` or import from the defining module".into()),
                        );
                    }
                    None => {
                        ctx.sink.error(
                            codes::E1011_IMPORT_ITEM,
                            format!("module `{path}` has no type `{name}`"),
                            item.span,
                            did_you_mean(&name, iface.types.keys().map(|s| s.as_str())),
                        );
                    }
                }
            } else {
                match iface.values.get(&name) {
                    Some(v) => {
                        resolved.value_origins.insert(bind_as.clone(), name.clone());
                        resolved.values.insert(
                            bind_as.clone(),
                            ValueInfo {
                                scheme: v.scheme.clone(),
                                public: true,
                                from_module: path.clone(),
                                labels: v.labels.clone(),
                                constructor_of: v.constructor_of,
                                is_const: v.is_const,
                                span: item.span,
                            },
                        );
                        resolved.import_keys.push((bind_as, item.span));
                    }
                    None => {
                        ctx.sink.error(
                            codes::E1011_IMPORT_ITEM,
                            format!("module `{path}` has no value `{name}`"),
                            item.span,
                            did_you_mean(&name, iface.values.keys().map(|s| s.as_str())),
                        );
                    }
                }
            }
        }
    }

    // Qualified / alias binding
    let alias_name = imp
        .alias
        .as_ref()
        .map(|a| a.text.clone())
        .unwrap_or_else(|| {
            imp.path
                .segments
                .last()
                .cloned()
                .unwrap_or_else(|| path.clone())
        });
    // Always allow qualified access when not selective-only... Spec: same module
    // imported both qualified and selectively is accepted.
    // If selective-only without `as`, still bind the last segment for `process.send`.
    if imp.items.is_none() || imp.alias.is_some() || true {
        // Store module alias as a special value marker via empty scheme + from_module
        resolved.values.insert(
            alias_name.clone(),
            ValueInfo {
                scheme: Scheme::mono(Type::Error),
                public: true,
                from_module: format!("module:{path}"),
                labels: vec![],
                constructor_of: None,
                is_const: false,
                span: imp.span,
            },
        );
        resolved
            .import_keys
            .push((format!("mod:{alias_name}"), imp.span));
    }
}

fn resolve_import_path(path: &ImportPath, ctx: &ResolveCtx<'_>) -> Option<String> {
    let joined = path.segments.join("/");
    if ctx.deps.contains_key(&joined) {
        return Some(joined);
    }
    // Longest matching declared prefix for domain-prefixed deps
    let mut best: Option<String> = None;
    for prefix in ctx.prefixes {
        if (joined == *prefix || joined.starts_with(&format!("{prefix}/")))
            && best.as_ref().is_none_or(|b| prefix.len() > b.len())
        {
            best = Some(prefix.clone());
        }
    }
    if let Some(p) = best {
        // Map to the full path if present
        if ctx.deps.contains_key(&joined) {
            return Some(joined);
        }
        // Prefix alone
        if ctx.deps.contains_key(&p) {
            return Some(joined);
        }
    }
    // Try lush/ stubs
    if joined.starts_with("lush/") && ctx.deps.contains_key(&joined) {
        return Some(joined);
    }
    None
}

fn define_local_type(
    path: &str,
    td: &TypeDef,
    resolved: &mut ResolvedModule,
    ctx: &mut ResolveCtx<'_>,
) {
    if td.opaque && matches!(td.body, TypeDefBody::Alias(_)) {
        ctx.sink.error(
            codes::E1208_OPAQUE_ALIAS,
            "opaque type aliases are not allowed; opacity applies only to ADT forms",
            td.span,
            Some("use `opaque type Name { ... }` or a transparent `type Name = ...`".into()),
        );
        return;
    }
    let id = TypeDefId(crate::ty::fresh_id());
    let kind = if td.opaque {
        crate::ty::TypeDefKind::Opaque
    } else if matches!(td.body, TypeDefBody::Alias(_)) {
        crate::ty::TypeDefKind::Alias
    } else {
        crate::ty::TypeDefKind::Adt
    };
    let params: Vec<String> = td.tvars.iter().map(|t| t.text.clone()).collect();
    // Placeholder def; infer fills fields
    let info = crate::ty::TypeDefInfo {
        id,
        module: path.to_string(),
        name: td.name.text.clone(),
        params: params.clone(),
        kind,
        variants: vec![],
        alias_body: None,
        public: td.public,
        eq_params: Default::default(),
        always_eq: false,
        never_eq: false,
    };
    ctx.store.defs.insert(id, info);
    if let Some(prev) = resolved.types.insert(
        td.name.text.clone(),
        TypeInfo {
            def: id,
            public: td.public,
            from_module: path.to_string(),
            span: td.name.span,
        },
    ) {
        ctx.sink.error(
            codes::E1004_DUPLICATE_DEF,
            format!("duplicate type definition `{}`", td.name.text),
            td.name.span,
            Some(format!("previous definition at {}", prev.span)),
        );
    }
}

fn check_alias_cycles(resolved: &ResolvedModule, ctx: &mut ResolveCtx<'_>) {
    // Simplified: walk alias bodies once filled — full check happens after type elaboration
    let _ = (resolved, ctx);
}

fn insert_value(
    resolved: &mut ResolvedModule,
    sink: &mut TypeSink,
    name: String,
    info: ValueInfo,
    span: Span,
) {
    // Allow prelude shadowing by local defs
    if let Some(prev) = resolved.values.get(&name) {
        if prev.from_module == info.from_module && !prev.from_module.starts_with("module:") {
            // Duplicate in same module — constructors vs fns
            if prev.constructor_of.is_some() || info.constructor_of.is_some() {
                sink.error(
                    codes::E1005_DUPLICATE_CTOR,
                    format!("duplicate constructor or value `{name}`"),
                    span,
                    None,
                );
                return;
            }
            sink.error(
                codes::E1004_DUPLICATE_DEF,
                format!("duplicate definition `{name}`"),
                span,
                None,
            );
            return;
        }
    }
    resolved.values.insert(name, info);
}

fn name_or_uname_text(n: &NameOrUName) -> String {
    match n {
        NameOrUName::Name(n) => n.text.clone(),
        NameOrUName::UName(n) => n.text.clone(),
    }
}

pub fn did_you_mean<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut best: Option<(usize, &str)> = None;
    for c in candidates {
        let d = edit_distance(name, c);
        if d > 0 && d <= MAX_EDIT_DISTANCE && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, c));
        }
    }
    best.map(|(_, c)| format!("did you mean `{c}`?"))
}

fn did_you_mean_module(name: &str, deps: &BTreeMap<String, ModuleInterface>) -> Option<String> {
    did_you_mean(name, deps.keys().map(|s| s.as_str()))
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Detect import cycles among module paths. Returns cycle path if found.
pub fn find_import_cycle(graph: &BTreeMap<String, Vec<String>>) -> Option<Vec<String>> {
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    let mut stack = Vec::new();
    for start in graph.keys() {
        if let Some(c) = dfs(start, graph, &mut visiting, &mut visited, &mut stack) {
            return Some(c);
        }
    }
    None
}

fn dfs(
    node: &str,
    graph: &BTreeMap<String, Vec<String>>,
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
    stack: &mut Vec<String>,
) -> Option<Vec<String>> {
    if visited.contains(node) {
        return None;
    }
    if !visiting.insert(node.to_string()) {
        let idx = stack.iter().position(|s| s == node).unwrap_or(0);
        let mut cycle = stack[idx..].to_vec();
        cycle.push(node.to_string());
        return Some(cycle);
    }
    stack.push(node.to_string());
    if let Some(deps) = graph.get(node) {
        for d in deps {
            if let Some(c) = dfs(d, graph, visiting, visited, stack) {
                return Some(c);
            }
        }
    }
    stack.pop();
    visiting.remove(node);
    visited.insert(node.to_string());
    None
}
