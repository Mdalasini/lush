//! Declaration-only interface stubs for `lush/*` (stand-ins until step 10).
//!
//! # Stub file format
//!
//! Stubs are **not** Lush source. The parser requires function bodies; stubs use
//! a bodyless declaration language loaded only under the reserved `lush/`
//! namespace.
//!
//! ```text
//! # comment
//! module lush/process
//!
//! opaque Pid
//! opaque Subject(msg)
//!
//! type Crash {
//!   Crash(message: String, trace: List(String))
//! }
//!
//! pub fn spawn(fn() -> Nil) -> Pid
//! @eq(k)
//! pub fn insert(Dict(k, v), k, v) -> Dict(k, v)
//! ```
//!
//! - `opaque Name` / `opaque Name(a, b)` — opaque type constructors.
//! - `type Name { Variant ... }` — public ADTs (constructors exported).
//! - `pub fn name(params) -> Ret` — fully annotated signature.
//! - `@eq(a)` / `@neg(a)` on the line before a `pub fn` attach sealed constraints
//!   to type variables in that scheme (internal to the stub loader).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::interface::{ExportedType, ExportedValue, ModuleInterface};
use crate::ty::{
    ConstraintSet, FieldInfo, Scheme, TvId, Type, TypeDefId, TypeDefInfo, TypeDefKind, TypeStore,
    VariantInfo,
};

const STUB_SOURCES: &[(&str, &str)] = &[
    ("lush/prelude", include_str!("../stubs/prelude.stub")),
    ("lush/process", include_str!("../stubs/process.stub")),
    ("lush/actor", include_str!("../stubs/actor.stub")),
    ("lush/supervisor", include_str!("../stubs/supervisor.stub")),
    ("lush/task", include_str!("../stubs/task.stub")),
    ("lush/list", include_str!("../stubs/list.stub")),
    ("lush/int", include_str!("../stubs/int.stub")),
    ("lush/io", include_str!("../stubs/io.stub")),
    ("lush/result", include_str!("../stubs/result.stub")),
    ("lush/dict", include_str!("../stubs/dict.stub")),
    ("lush/set", include_str!("../stubs/set.stub")),
];

/// Global name → type def, including `module.Name` qualified forms.
type TypeEnv = HashMap<String, TypeDefId>;
type StubField = (Option<String>, String);
type StubVariant = (String, Vec<StubField>);

pub fn is_stub_path(path: &str) -> bool {
    STUB_SOURCES.iter().any(|(p, _)| *p == path)
}

pub fn stub_paths() -> Vec<&'static str> {
    STUB_SOURCES.iter().map(|(p, _)| *p).collect()
}

/// Load every bundled stub. Call after [`install_prelude_builtins`].
pub fn load_all_stubs(store: &mut TypeStore) -> BTreeMap<String, ModuleInterface> {
    let mut out = BTreeMap::new();
    let mut type_env = TypeEnv::new();
    // Seed from existing defs
    for (id, def) in &store.defs {
        type_env.insert(def.name.clone(), *id);
        if !def.module.is_empty() {
            let short = def.module.rsplit('/').next().unwrap_or(&def.module);
            type_env.insert(format!("{short}.{}", def.name), *id);
            type_env.insert(format!("{}.{}", def.module, def.name), *id);
        }
    }
    for (path, src) in STUB_SOURCES {
        let iface = parse_stub(path, src, store, &mut type_env);
        // Register this module's types for later stubs
        for (name, t) in &iface.types {
            type_env.insert(name.clone(), t.def);
            let short = path.rsplit('/').next().unwrap_or(path);
            type_env.insert(format!("{short}.{name}"), t.def);
            type_env.insert(format!("{path}.{name}"), t.def);
        }
        out.insert((*path).to_string(), iface);
    }
    out
}

fn parse_stub(
    path: &str,
    src: &str,
    store: &mut TypeStore,
    type_env: &mut TypeEnv,
) -> ModuleInterface {
    let mut iface = ModuleInterface::empty(path);
    let mut pending: Vec<(String, ConstraintSet)> = Vec::new();
    let lines: Vec<&str> = src.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        let line = strip_line_comment(raw).trim();
        i += 1;
        if line.is_empty() || line.starts_with("module ") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("@eq(") {
            pending.push((
                rest.trim_end_matches(')').trim().to_string(),
                ConstraintSet::eq(),
            ));
            continue;
        }
        if let Some(rest) = line.strip_prefix("@neg(") {
            pending.push((
                rest.trim_end_matches(')').trim().to_string(),
                ConstraintSet::neg(),
            ));
            continue;
        }
        if let Some(rest) = line.strip_prefix("opaque ") {
            let (name, params) = parse_name_params(rest);
            let id = TypeDefId(crate::ty::fresh_id());
            let (always_eq, never_eq) = match name.as_str() {
                "Pid" | "Monitor" | "Timer" | "Subject" => (true, false),
                "Selector" | "Task" => (false, true),
                _ => (false, false),
            };
            let info = TypeDefInfo {
                id,
                module: path.to_string(),
                name: name.clone(),
                params: params.clone(),
                kind: TypeDefKind::Opaque,
                variants: vec![],
                alias_body: None,
                public: true,
                eq_params: BTreeSet::new(),
                always_eq,
                never_eq,
            };
            store.defs.insert(id, info.clone());
            type_env.insert(name.clone(), id);
            iface.types.insert(
                name.clone(),
                ExportedType {
                    name: name.clone(),
                    def: id,
                    kind: TypeDefKind::Opaque,
                    params,
                    eq_params: BTreeSet::new(),
                    always_eq,
                    never_eq,
                    public: true,
                },
            );
            iface.type_defs.insert(id, info);
            continue;
        }
        if line.starts_with("type ") {
            let mut block = line.to_string();
            if !line.contains('}') {
                while i < lines.len() {
                    block.push('\n');
                    block.push_str(lines[i]);
                    let done = lines[i].contains('}');
                    i += 1;
                    if done {
                        break;
                    }
                }
            }
            if let Some((name, params, variants)) = parse_adt_block(&block) {
                register_adt(path, store, &mut iface, type_env, name, params, variants);
            }
            continue;
        }
        if let Some(rest) = line
            .strip_prefix("pub fn ")
            .or_else(|| line.strip_prefix("fn "))
        {
            if let Some((name, params_str, ret_str)) = parse_fn_sig(rest) {
                let mut tvar_map: HashMap<String, TvId> = HashMap::new();
                let param_parts = split_top_level(params_str, ',');
                let mut param_tys = Vec::new();
                let mut labels = Vec::new();
                for p in param_parts {
                    let p = p.trim();
                    if p.is_empty() {
                        continue;
                    }
                    let (lab, ty_s) = split_label(p);
                    labels.push(lab);
                    param_tys.push(parse_type(ty_s, store, type_env, &mut tvar_map));
                }
                let ret_ty = parse_type(ret_str.trim(), store, type_env, &mut tvar_map);
                let mut constraints = HashMap::new();
                for (vname, c) in pending.drain(..) {
                    if let Some(id) = tvar_map.get(&vname) {
                        constraints
                            .entry(*id)
                            .or_insert_with(ConstraintSet::empty)
                            .merge(&c);
                    }
                }
                let mut vars: Vec<_> = tvar_map.values().copied().collect();
                vars.sort();
                iface.values.insert(
                    name.to_string(),
                    ExportedValue {
                        name: name.to_string(),
                        scheme: Scheme {
                            vars,
                            constraints,
                            body: Type::Fun {
                                params: param_tys,
                                ret: Box::new(ret_ty),
                            },
                        },
                        constructor_of: None,
                        labels,
                        is_const: false,
                    },
                );
            }
        }
    }
    iface.recompute_hash();
    iface
}

fn register_adt(
    path: &str,
    store: &mut TypeStore,
    iface: &mut ModuleInterface,
    type_env: &mut TypeEnv,
    name: String,
    params: Vec<String>,
    variants: Vec<StubVariant>,
) {
    if iface.types.contains_key(&name) {
        return;
    }
    let id = TypeDefId(crate::ty::fresh_id());
    // Pre-register so recursive fields resolve
    type_env.insert(name.clone(), id);

    let mut tvar_rigids: HashMap<String, Type> = HashMap::new();
    for p in &params {
        tvar_rigids.insert(p.clone(), store.fresh_rigid(p.clone()));
    }
    let mut variant_infos = Vec::new();
    for (vname, fields) in &variants {
        let mut field_infos = Vec::new();
        for (label, ty_str) in fields {
            let mut dummy = HashMap::new();
            let ty = parse_type_rigids(ty_str, store, type_env, &tvar_rigids, &mut dummy);
            field_infos.push(FieldInfo {
                label: label.clone(),
                ty,
            });
        }
        variant_infos.push(VariantInfo {
            name: vname.clone(),
            fields: field_infos.clone(),
            public: true,
        });
        let param_tys: Vec<Type> = field_infos.iter().map(|f| f.ty.clone()).collect();
        let app_args: Vec<Type> = params
            .iter()
            .filter_map(|p| tvar_rigids.get(p).cloned())
            .collect();
        let ret = Type::App {
            def: id,
            args: app_args,
        };
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
                Type::Rigid(rid) => {
                    let n = store
                        .rigids
                        .get(rid)
                        .map(|r| r.name.clone())
                        .unwrap_or_default();
                    Some((*rid, n))
                }
                _ => None,
            })
            .collect();
        let scheme = crate::unify::generalise_rigids(store, &body_ty, &pairs);
        let labels: Vec<Option<String>> = field_infos.iter().map(|f| f.label.clone()).collect();
        iface.values.insert(
            vname.clone(),
            ExportedValue {
                name: vname.clone(),
                scheme,
                constructor_of: Some(id),
                labels,
                is_const: param_tys.is_empty(),
            },
        );
    }
    let info = TypeDefInfo {
        id,
        module: path.to_string(),
        name: name.clone(),
        params: params.clone(),
        kind: TypeDefKind::Adt,
        variants: variant_infos,
        alias_body: None,
        public: true,
        eq_params: BTreeSet::new(),
        always_eq: false,
        never_eq: false,
    };
    store.defs.insert(id, info);
    let (eq_params, always_eq, never_eq) = crate::eq_capability::compute_eq_params(store, id);
    if let Some(d) = store.defs.get_mut(&id) {
        d.eq_params = eq_params.clone();
        d.always_eq = always_eq;
        d.never_eq = never_eq;
    }
    iface.types.insert(
        name.clone(),
        ExportedType {
            name,
            def: id,
            kind: TypeDefKind::Adt,
            params,
            eq_params,
            always_eq,
            never_eq,
            public: true,
        },
    );
    iface
        .type_defs
        .insert(id, store.defs.get(&id).unwrap().clone());
}

fn strip_line_comment(line: &str) -> &str {
    let t = line.trim_start();
    if t.starts_with('#') || t.starts_with("//") {
        return "";
    }
    line
}

fn parse_name_params(s: &str) -> (String, Vec<String>) {
    let s = s.trim();
    if let Some((name, rest)) = s.split_once('(') {
        let params = rest
            .trim_end_matches(')')
            .split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        (name.trim().to_string(), params)
    } else {
        (s.to_string(), vec![])
    }
}

fn parse_fn_sig(s: &str) -> Option<(&str, &str, &str)> {
    let (name, rest) = s.split_once('(')?;
    let (params, rest) = split_matching_paren(rest)?;
    let ret = rest.trim().strip_prefix("->")?.trim();
    Some((name.trim(), params, ret))
}

fn split_matching_paren(s: &str) -> Option<(&str, &str)> {
    let mut depth = 1usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&s[..i], &s[i + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

fn split_label(p: &str) -> (Option<String>, &str) {
    if let Some((lab, ty)) = p.split_once(':') {
        let lab = lab.trim();
        if lab
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
            && !lab.contains('(')
            && lab != "fn"
        {
            return (Some(lab.to_string()), ty.trim());
        }
    }
    (None, p)
}

fn parse_type(
    s: &str,
    store: &mut TypeStore,
    type_env: &TypeEnv,
    tvars: &mut HashMap<String, TvId>,
) -> Type {
    parse_type_rigids(s, store, type_env, &HashMap::new(), tvars)
}

fn parse_type_rigids(
    s: &str,
    store: &mut TypeStore,
    type_env: &TypeEnv,
    rigids: &HashMap<String, Type>,
    tvars: &mut HashMap<String, TvId>,
) -> Type {
    let s = s.trim();
    if s.is_empty() {
        return Type::Error;
    }
    if let Some(rest) = s.strip_prefix("fn(") {
        let Some((params, rest)) = split_matching_paren(rest) else {
            return Type::Error;
        };
        let ret = rest
            .trim()
            .strip_prefix("->")
            .map(|r| r.trim())
            .unwrap_or("Nil");
        let ps = split_top_level(params, ',')
            .into_iter()
            .filter(|p| !p.trim().is_empty())
            .map(|p| {
                let (_, ty) = split_label(p.trim());
                parse_type_rigids(ty, store, type_env, rigids, tvars)
            })
            .collect();
        return Type::Fun {
            params: ps,
            ret: Box::new(parse_type_rigids(ret, store, type_env, rigids, tvars)),
        };
    }
    if let Some(inner) = s.strip_prefix("#(").and_then(|r| r.strip_suffix(')')) {
        let ts = split_top_level(inner, ',')
            .into_iter()
            .filter(|p| !p.trim().is_empty())
            .map(|p| parse_type_rigids(p, store, type_env, rigids, tvars))
            .collect();
        return Type::Tuple(ts);
    }
    match s {
        "Int" => return Type::Int,
        "Float" => return Type::Float,
        "String" => return Type::String,
        "Bool" => return Type::Bool,
        "Nil" => return Type::Nil,
        "BitArray" => return Type::BitArray,
        _ => {}
    }
    if let Some((name, rest)) = s.split_once('(') {
        let name = name.trim();
        let inner = rest.strip_suffix(')').unwrap_or(rest);
        let args: Vec<Type> = split_top_level(inner, ',')
            .into_iter()
            .filter(|p| !p.trim().is_empty())
            .map(|p| parse_type_rigids(p, store, type_env, rigids, tvars))
            .collect();
        if name == "List" {
            return Type::List(Box::new(args.into_iter().next().unwrap_or(Type::Error)));
        }
        if let Some(id) = resolve_type_name(name, type_env) {
            return Type::App { def: id, args };
        }
        return Type::Error;
    }
    if let Some(t) = rigids.get(s) {
        return t.clone();
    }
    if s.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
        let id = *tvars
            .entry(s.to_string())
            .or_insert_with(|| match store.fresh_var(0) {
                Type::Var(id) => id,
                _ => TvId(0),
            });
        return Type::Var(id);
    }
    if let Some(id) = resolve_type_name(s, type_env) {
        return Type::App {
            def: id,
            args: vec![],
        };
    }
    Type::Error
}

fn resolve_type_name(name: &str, type_env: &TypeEnv) -> Option<TypeDefId> {
    type_env.get(name).copied().or_else(|| {
        name.rsplit_once('.')
            .and_then(|(_, last)| type_env.get(last).copied())
    })
}

fn parse_adt_block(block: &str) -> Option<(String, Vec<String>, Vec<StubVariant>)> {
    let block = block.trim();
    let rest = block.strip_prefix("type ")?;
    let (head, body) = rest.split_once('{')?;
    let body = body.trim().trim_end_matches('}').trim();
    let (name, params) = parse_name_params(head.trim());
    let mut variants = Vec::new();
    for part in split_variants(body) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((vname, fields_str)) = part.split_once('(') {
            let fields_inner = fields_str.trim().trim_end_matches(')');
            let fields = split_top_level(fields_inner, ',')
                .into_iter()
                .filter(|f| !f.trim().is_empty())
                .map(|f| {
                    let f = f.trim();
                    if let Some((lab, ty)) = f.split_once(':') {
                        (Some(lab.trim().to_string()), ty.trim().to_string())
                    } else {
                        (None, f.to_string())
                    }
                })
                .collect();
            variants.push((vname.trim().to_string(), fields));
        } else {
            variants.push((part.to_string(), vec![]));
        }
    }
    Some((name, params, variants))
}

fn split_variants(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for token in body.split_whitespace() {
        let starts_new = depth == 0
            && !cur.is_empty()
            && token.chars().next().is_some_and(|c| c.is_ascii_uppercase());
        if starts_new {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(token);
        for c in token.chars() {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Install prelude built-in type defs and constructors.
pub fn install_prelude_builtins(store: &mut TypeStore) -> ModuleInterface {
    let mut iface = ModuleInterface::empty("prelude");
    let result_id = TypeDefId(crate::ty::fresh_id());
    let a = store.fresh_rigid("a");
    let e = store.fresh_rigid("e");
    store.defs.insert(
        result_id,
        TypeDefInfo {
            id: result_id,
            module: "prelude".into(),
            name: "Result".into(),
            params: vec!["a".into(), "e".into()],
            kind: TypeDefKind::Builtin,
            variants: vec![
                VariantInfo {
                    name: "Ok".into(),
                    fields: vec![FieldInfo {
                        label: None,
                        ty: a.clone(),
                    }],
                    public: true,
                },
                VariantInfo {
                    name: "Error".into(),
                    fields: vec![FieldInfo {
                        label: None,
                        ty: e.clone(),
                    }],
                    public: true,
                },
            ],
            alias_body: None,
            public: true,
            eq_params: BTreeSet::from([0, 1]),
            always_eq: false,
            never_eq: false,
        },
    );
    iface.types.insert(
        "Result".into(),
        ExportedType {
            name: "Result".into(),
            def: result_id,
            kind: TypeDefKind::Builtin,
            params: vec!["a".into(), "e".into()],
            eq_params: BTreeSet::from([0, 1]),
            always_eq: false,
            never_eq: false,
            public: true,
        },
    );
    iface
        .type_defs
        .insert(result_id, store.defs.get(&result_id).unwrap().clone());

    let opt_id = TypeDefId(crate::ty::fresh_id());
    let a2 = store.fresh_rigid("a");
    store.defs.insert(
        opt_id,
        TypeDefInfo {
            id: opt_id,
            module: "prelude".into(),
            name: "Option".into(),
            params: vec!["a".into()],
            kind: TypeDefKind::Builtin,
            variants: vec![
                VariantInfo {
                    name: "Some".into(),
                    fields: vec![FieldInfo {
                        label: None,
                        ty: a2.clone(),
                    }],
                    public: true,
                },
                VariantInfo {
                    name: "None".into(),
                    fields: vec![],
                    public: true,
                },
            ],
            alias_body: None,
            public: true,
            eq_params: BTreeSet::from([0]),
            always_eq: false,
            never_eq: false,
        },
    );
    iface.types.insert(
        "Option".into(),
        ExportedType {
            name: "Option".into(),
            def: opt_id,
            kind: TypeDefKind::Builtin,
            params: vec!["a".into()],
            eq_params: BTreeSet::from([0]),
            always_eq: false,
            never_eq: false,
            public: true,
        },
    );
    iface
        .type_defs
        .insert(opt_id, store.defs.get(&opt_id).unwrap().clone());

    // Also register Dict/Set/Vector as opaque builtins (overwritten by stubs if loaded)
    for (name, params, always, never) in [
        ("Dict", vec!["k".into(), "v".into()], false, false),
        ("Set", vec!["a".into()], false, false),
        ("Vector", vec!["a".into()], false, false),
    ] {
        let id = TypeDefId(crate::ty::fresh_id());
        store.defs.insert(
            id,
            TypeDefInfo {
                id,
                module: "prelude".into(),
                name: name.into(),
                params: params.clone(),
                kind: TypeDefKind::Opaque,
                variants: vec![],
                alias_body: None,
                public: true,
                eq_params: if name == "Dict" {
                    BTreeSet::from([0, 1])
                } else {
                    BTreeSet::from([0])
                },
                always_eq: always,
                never_eq: never,
            },
        );
    }

    add_ctor(
        &mut iface,
        store,
        "Ok",
        result_id,
        true,
        vec![None],
        |store| {
            let a = fresh_tv(store);
            let e = fresh_tv(store);
            Scheme {
                vars: vec![a, e],
                constraints: HashMap::new(),
                body: Type::Fun {
                    params: vec![Type::Var(a)],
                    ret: Box::new(Type::App {
                        def: result_id,
                        args: vec![Type::Var(a), Type::Var(e)],
                    }),
                },
            }
        },
    );
    add_ctor(
        &mut iface,
        store,
        "Error",
        result_id,
        true,
        vec![None],
        |store| {
            let a = fresh_tv(store);
            let e = fresh_tv(store);
            Scheme {
                vars: vec![a, e],
                constraints: HashMap::new(),
                body: Type::Fun {
                    params: vec![Type::Var(e)],
                    ret: Box::new(Type::App {
                        def: result_id,
                        args: vec![Type::Var(a), Type::Var(e)],
                    }),
                },
            }
        },
    );
    add_ctor(
        &mut iface,
        store,
        "Some",
        opt_id,
        true,
        vec![None],
        |store| {
            let a = fresh_tv(store);
            Scheme {
                vars: vec![a],
                constraints: HashMap::new(),
                body: Type::Fun {
                    params: vec![Type::Var(a)],
                    ret: Box::new(Type::App {
                        def: opt_id,
                        args: vec![Type::Var(a)],
                    }),
                },
            }
        },
    );
    add_ctor(&mut iface, store, "None", opt_id, false, vec![], |store| {
        let a = fresh_tv(store);
        Scheme {
            vars: vec![a],
            constraints: HashMap::new(),
            body: Type::App {
                def: opt_id,
                args: vec![Type::Var(a)],
            },
        }
    });
    for (n, ty) in [
        ("True", Type::Bool),
        ("False", Type::Bool),
        ("Nil", Type::Nil),
    ] {
        iface.values.insert(
            n.into(),
            ExportedValue {
                name: n.into(),
                scheme: Scheme::mono(ty),
                constructor_of: None,
                labels: vec![],
                is_const: true,
            },
        );
    }
    iface.recompute_hash();
    iface
}

fn fresh_tv(store: &mut TypeStore) -> TvId {
    match store.fresh_var(0) {
        Type::Var(id) => id,
        _ => TvId(0),
    }
}

fn add_ctor(
    iface: &mut ModuleInterface,
    store: &mut TypeStore,
    name: &str,
    def: TypeDefId,
    _has_arg: bool,
    labels: Vec<Option<String>>,
    mk: impl FnOnce(&mut TypeStore) -> Scheme,
) {
    iface.values.insert(
        name.into(),
        ExportedValue {
            name: name.into(),
            scheme: mk(store),
            constructor_of: Some(def),
            labels,
            is_const: false,
        },
    );
}
