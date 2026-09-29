//! Sealed `Eq` / `Neg` capability checking (§4.3).

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::ty::{Type, TypeDefId, TypeDefKind, TypeStore};

/// Whether `ty` supports equality after zonking.
pub fn has_eq(store: &mut TypeStore, ty: &Type) -> bool {
    has_eq_inner(store, ty, &mut HashSet::new(), &mut HashMap::new())
}

fn has_eq_inner(
    store: &mut TypeStore,
    ty: &Type,
    visiting: &mut HashSet<TypeDefId>,
    cache: &mut HashMap<String, bool>,
) -> bool {
    store.work = store.work.saturating_add(1);
    let z = store.zonk(ty);
    let key = format!("{z:?}");
    if let Some(v) = cache.get(&key) {
        return *v;
    }
    let result = match z {
        Type::Error
        | Type::Int
        | Type::Float
        | Type::String
        | Type::Bool
        | Type::Nil
        | Type::BitArray => true,
        Type::Var(_) | Type::Rigid(_) => true, // polymorphic; constraint carried
        Type::Fun { .. } => false,
        Type::List(t) => has_eq_inner(store, &t, visiting, cache),
        Type::Tuple(ts) => ts.iter().all(|t| has_eq_inner(store, t, visiting, cache)),
        Type::App { def, args } => {
            let Some(info) = store.defs.get(&def).cloned() else {
                return false;
            };
            // Built-in opaque runtime types
            match info.name.as_str() {
                "Pid" | "Monitor" | "Timer" => return true,
                "Subject" => return true, // no requirement on payload
                "Selector" | "Task" => return false,
                "Dict" => {
                    // Eq if key and value are Eq
                    return args.iter().all(|a| has_eq_inner(store, a, visiting, cache));
                }
                "Set" | "Vector" => {
                    return args.iter().all(|a| has_eq_inner(store, a, visiting, cache));
                }
                "Result" | "Option" => {
                    return args.iter().all(|a| has_eq_inner(store, a, visiting, cache));
                }
                _ => {}
            }
            if info.never_eq {
                return false;
            }
            if info.always_eq {
                return true;
            }
            if visiting.contains(&def) {
                // Assume true while computing least fixpoint; params checked below.
                return info.eq_params.iter().all(|&i| {
                    args.get(i)
                        .map(|a| has_eq_inner(store, a, visiting, cache))
                        .unwrap_or(true)
                });
            }
            visiting.insert(def);
            let ok = if info.kind == TypeDefKind::Alias {
                if let Some(body) = &info.alias_body {
                    // Substitute params — simplified: check body with args as env not needed for capability of concrete apps
                    has_eq_inner(store, body, visiting, cache)
                } else {
                    true
                }
            } else {
                // ADT / opaque: use computed eq_params
                info.eq_params.iter().all(|&i| {
                    args.get(i)
                        .map(|a| has_eq_inner(store, a, visiting, cache))
                        .unwrap_or(true)
                }) && !info.never_eq
            };
            visiting.remove(&def);
            ok
        }
    };
    cache.insert(key, result);
    result
}

pub fn has_neg(store: &mut TypeStore, ty: &Type) -> bool {
    matches!(store.zonk(ty), Type::Int | Type::Float | Type::Error)
}

/// Compute least-fixpoint Eq requirements for an ADT's type parameters.
pub fn compute_eq_params(store: &mut TypeStore, def: TypeDefId) -> (BTreeSet<usize>, bool, bool) {
    let Some(info) = store.defs.get(&def).cloned() else {
        return (BTreeSet::new(), false, true);
    };
    if info.kind == TypeDefKind::Alias {
        return (BTreeSet::new(), false, false);
    }
    // Map param name -> index
    let param_index: HashMap<String, usize> = info
        .params
        .iter()
        .enumerate()
        .map(|(i, n)| (n.clone(), i))
        .collect();

    // Collect which params appear in field positions that need Eq.
    // Least fixpoint: start assuming all params are Eq-capable; discard those
    // that appear under a non-Eq context (functions).
    let mut never_eq = false;
    let mut required = BTreeSet::new();
    let mut always = info.params.is_empty();

    for variant in &info.variants {
        for field in &variant.fields {
            match analyze_field_eq(&field.ty, &param_index, store, def) {
                FieldEq::Always => {}
                FieldEq::NeedsParam(i) => {
                    required.insert(i);
                    always = false;
                }
                FieldEq::NeedsParams(set) => {
                    required.extend(set);
                    always = false;
                }
                FieldEq::Never => {
                    never_eq = true;
                    always = false;
                }
            }
        }
    }
    // Phantom params (never appear) → type is Eq for any of them; always_eq
    // only if no field requires anything and no Never.
    if !never_eq && required.is_empty() && !info.variants.is_empty() {
        // All fields are always-Eq (or empty variants like `Box`)
        let used = params_used_in_variants(&info, &param_index);
        if used.is_empty() {
            always = true;
        }
    }
    if info.variants.is_empty() && info.kind != TypeDefKind::Alias {
        always = true;
    }
    (required, always && !never_eq, never_eq)
}

enum FieldEq {
    Always,
    NeedsParam(usize),
    NeedsParams(BTreeSet<usize>),
    Never,
}

fn analyze_field_eq(
    ty: &Type,
    params: &HashMap<String, usize>,
    store: &TypeStore,
    self_def: TypeDefId,
) -> FieldEq {
    match ty {
        Type::Fun { .. } => FieldEq::Never,
        Type::Int
        | Type::Float
        | Type::String
        | Type::Bool
        | Type::Nil
        | Type::BitArray
        | Type::Error => FieldEq::Always,
        Type::Rigid(id) => {
            if let Some(info) = store.rigids.get(id) {
                if let Some(&i) = params.get(&info.name) {
                    return FieldEq::NeedsParam(i);
                }
            }
            FieldEq::Always
        }
        Type::Var(_) => FieldEq::Always,
        Type::List(t) => analyze_field_eq(t, params, store, self_def),
        Type::Tuple(ts) => {
            let mut set = BTreeSet::new();
            for t in ts {
                match analyze_field_eq(t, params, store, self_def) {
                    FieldEq::Always => {}
                    FieldEq::NeedsParam(i) => {
                        set.insert(i);
                    }
                    FieldEq::NeedsParams(s) => set.extend(s),
                    FieldEq::Never => return FieldEq::Never,
                }
            }
            if set.is_empty() {
                FieldEq::Always
            } else {
                FieldEq::NeedsParams(set)
            }
        }
        Type::App { def, args } => {
            if let Some(info) = store.defs.get(def) {
                match info.name.as_str() {
                    "Selector" | "Task" => return FieldEq::Never,
                    "Subject" | "Pid" | "Monitor" | "Timer" => return FieldEq::Always,
                    _ => {}
                }
                if info.never_eq {
                    return FieldEq::Never;
                }
                if *def == self_def {
                    // Recursive: requirements are the params used in args at eq_params positions
                    // Conservatively: union of analyzing args
                }
            }
            let mut set = BTreeSet::new();
            for a in args {
                match analyze_field_eq(a, params, store, self_def) {
                    FieldEq::Always => {}
                    FieldEq::NeedsParam(i) => {
                        set.insert(i);
                    }
                    FieldEq::NeedsParams(s) => set.extend(s),
                    FieldEq::Never => return FieldEq::Never,
                }
            }
            if set.is_empty() {
                FieldEq::Always
            } else {
                FieldEq::NeedsParams(set)
            }
        }
    }
}

fn params_used_in_variants(
    info: &crate::ty::TypeDefInfo,
    params: &HashMap<String, usize>,
) -> BTreeSet<usize> {
    let mut used = BTreeSet::new();
    for v in &info.variants {
        for f in &v.fields {
            collect_params(&f.ty, params, &mut used);
        }
    }
    used
}

fn collect_params(ty: &Type, params: &HashMap<String, usize>, out: &mut BTreeSet<usize>) {
    match ty {
        Type::Rigid(id) => {
            // name lookup happens via store; here we only see Rigid without store.
            let _ = (id, params, out);
        }
        Type::List(t) => collect_params(t, params, out),
        Type::Tuple(ts) => {
            for t in ts {
                collect_params(t, params, out);
            }
        }
        Type::Fun { params: ps, ret } => {
            for p in ps {
                collect_params(p, params, out);
            }
            collect_params(ret, params, out);
        }
        Type::App { args, .. } => {
            for a in args {
                collect_params(a, params, out);
            }
        }
        _ => {}
    }
}
