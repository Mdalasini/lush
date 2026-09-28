//! Case exhaustiveness and redundancy (`spec.md` §5.4 / §4.3).
//!
//! Maranget-style constructor specialization for single- and multi-subject
//! matches, with nested field checks for ADTs. Guarded arms do not contribute
//! to exhaustiveness.

use crate::error::{TypeError, TypeWarning};
use crate::ty::Type;
use lush_syntax::ast::{Clause, Pattern, PatternField, PatternKind};
use lush_syntax::Span;
use std::collections::HashMap;

fn is_literal_open_name(name: &str) -> bool {
    matches!(
        name,
        "Int"
            | "Float"
            | "String"
            | "BitArray"
            | "List"
            | "Dict"
            | "Set"
            | "Vector"
            | "Pid"
            | "Subject"
            | "Monitor"
            | "Timer"
            | "Selector"
            | "Task"
    )
}

/// ADT / prelude constructor metadata used by exhaustiveness.
#[derive(Clone, Default)]
pub(crate) struct ExhaustEnv {
    /// Type name -> variant constructor names.
    pub adt_variants: HashMap<String, Vec<String>>,
    /// Constructor name -> field types (may reference ADT type parameters).
    pub ctor_fields: HashMap<String, Vec<Type>>,
    /// Constructor name -> declaration-order field labels.
    pub ctor_labels: HashMap<String, Vec<Option<String>>>,
    /// Type name -> ADT type-parameter variable ids (declaration order).
    pub adt_param_vars: HashMap<String, Vec<u32>>,
}

/// Check exhaustiveness and redundancy for a `case` expression.
pub(crate) fn check_case(
    subject_tys: &[Type],
    clauses: &[Clause],
    env: &ExhaustEnv,
    case_span: Span,
) -> (Vec<TypeError>, Vec<TypeWarning>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    if subject_tys.is_empty() || clauses.is_empty() {
        if clauses.is_empty() {
            errors.push(TypeError::Other {
                span: case_span,
                message: "non-exhaustive case: no clauses".into(),
            });
        }
        return (errors, warnings);
    }

    let mut prior: Vec<&[Pattern]> = Vec::new();
    for clause in clauses {
        for row in &clause.patterns {
            if row.patterns.len() != subject_tys.len() {
                continue;
            }
            if !is_useful(&row.patterns, &prior, subject_tys, env) {
                warnings.push(TypeWarning::RedundantPattern {
                    span: row.span,
                    message: "unreachable pattern".into(),
                });
            }
        }
        if clause.guard.is_none() {
            for row in &clause.patterns {
                if row.patterns.len() == subject_tys.len() {
                    prior.push(&row.patterns);
                }
            }
        }
    }

    if !is_exhaustive(subject_tys, &prior, env) {
        errors.push(TypeError::Other {
            span: case_span,
            message: format!(
                "non-exhaustive case: missing patterns for {}",
                missing_hint(subject_tys, &prior, env)
            ),
        });
    }

    (errors, warnings)
}

fn is_exhaustive(tys: &[Type], matrix: &[&[Pattern]], env: &ExhaustEnv) -> bool {
    if tys.is_empty() {
        return !matrix.is_empty();
    }
    if matrix.is_empty() {
        return false;
    }
    if matrix.iter().any(|row| row_irrefutable(row, tys)) {
        return true;
    }

    let first = &tys[0];
    if let Type::Tuple(elems) = first {
        return exhaustive_tuple_head(elems, &tys[1..], matrix, env);
    }
    if is_list_ty(first) {
        return exhaustive_list_head(first, &tys[1..], matrix, env);
    }

    match expected_constructors(first, env) {
        Some(ctors) => {
            for ctor in &ctors {
                let (rest_tys, specialized) = specialize(ctor, first, &tys[1..], matrix, env);
                let refs: Vec<&[Pattern]> = specialized.iter().map(|v| v.as_slice()).collect();
                if !is_exhaustive(&rest_tys, &refs, env) {
                    return false;
                }
            }
            true
        }
        None => {
            // Open domains: literals need an irrefutable cover. Type variables and
            // unknown imported nominals may be witnessed by a constructor pattern.
            let allow_ctor_witness = match first {
                Type::Var(_) => true,
                Type::Named { name, module, .. } => {
                    module.is_some()
                        || (!is_literal_open_name(name)
                            && !env.adt_variants.contains_key(name)
                            && name != "_")
                }
                _ => false,
            };
            let mut rest_rows: Vec<Vec<Pattern>> = Vec::new();
            for row in matrix {
                let Some(p) = row.first() else { continue };
                let covers = is_irrefutable_cover(p, Some(first))
                    || (allow_ctor_witness
                        && matches!(peel_as(p).kind, PatternKind::Constructor { .. }));
                if covers {
                    rest_rows.push(row[1..].to_vec());
                }
            }
            let refs: Vec<&[Pattern]> = rest_rows.iter().map(|v| v.as_slice()).collect();
            is_exhaustive(&tys[1..], &refs, env)
        }
    }
}

fn exhaustive_tuple_head(
    elems: &[Type],
    rest_tys: &[Type],
    matrix: &[&[Pattern]],
    env: &ExhaustEnv,
) -> bool {
    let mut expanded: Vec<Vec<Pattern>> = Vec::new();
    for row in matrix {
        let Some(p) = row.first() else { continue };
        let p = peel_as(p);
        match &p.kind {
            PatternKind::Var(_) | PatternKind::Discard => {
                let mut wilds: Vec<Pattern> = elems
                    .iter()
                    .map(|_| Pattern {
                        kind: PatternKind::Discard,
                        span: p.span,
                    })
                    .collect();
                wilds.extend(row[1..].iter().cloned());
                expanded.push(wilds);
            }
            PatternKind::Tuple(ps) if ps.len() == elems.len() => {
                let mut r = ps.clone();
                r.extend(row[1..].iter().cloned());
                expanded.push(r);
            }
            _ => {}
        }
    }
    let mut all_tys: Vec<Type> = elems.to_vec();
    all_tys.extend_from_slice(rest_tys);
    let refs: Vec<&[Pattern]> = expanded.iter().map(|v| v.as_slice()).collect();
    is_exhaustive(&all_tys, &refs, env)
}

fn is_list_ty(ty: &Type) -> bool {
    matches!(ty, Type::Named { name, .. } if name == "List")
}

fn discard_pat(span: Span) -> Pattern {
    Pattern {
        kind: PatternKind::Discard,
        span,
    }
}

/// List Nil/Cons partition: `[]` covers empty; `[h, ..t]` / `[..rest]` cover cons.
fn exhaustive_list_head(
    list_ty: &Type,
    rest_tys: &[Type],
    matrix: &[&[Pattern]],
    env: &ExhaustEnv,
) -> bool {
    let elem_ty = match list_ty {
        Type::Named { args, .. } if !args.is_empty() => args[0].clone(),
        _ => Type::Named {
            module: None,
            name: "_".into(),
            args: vec![],
        },
    };

    let mut nil_rows: Vec<Vec<Pattern>> = Vec::new();
    let mut cons_rows: Vec<Vec<Pattern>> = Vec::new();
    for row in matrix {
        let Some(p) = row.first() else { continue };
        let p = peel_as(p);
        match &p.kind {
            PatternKind::Var(_) | PatternKind::Discard => {
                nil_rows.push(row[1..].to_vec());
                let mut cons = vec![discard_pat(p.span), discard_pat(p.span)];
                cons.extend(row[1..].iter().cloned());
                cons_rows.push(cons);
            }
            PatternKind::List { items, rest } if items.is_empty() => match rest {
                None => nil_rows.push(row[1..].to_vec()),
                Some(r) if is_irrefutable_cover(r, Some(list_ty)) => {
                    nil_rows.push(row[1..].to_vec());
                    let mut cons = vec![discard_pat(p.span), discard_pat(p.span)];
                    cons.extend(row[1..].iter().cloned());
                    cons_rows.push(cons);
                }
                Some(_) => {}
            },
            PatternKind::List { items, rest } => {
                let head = items[0].clone();
                let tail = Pattern {
                    kind: PatternKind::List {
                        items: items[1..].to_vec(),
                        rest: rest.clone(),
                    },
                    span: p.span,
                };
                let mut cons = vec![head, tail];
                cons.extend(row[1..].iter().cloned());
                cons_rows.push(cons);
            }
            _ => {}
        }
    }

    let nil_refs: Vec<&[Pattern]> = nil_rows.iter().map(|v| v.as_slice()).collect();
    let mut cons_tys = vec![elem_ty, list_ty.clone()];
    cons_tys.extend_from_slice(rest_tys);
    let cons_refs: Vec<&[Pattern]> = cons_rows.iter().map(|v| v.as_slice()).collect();
    is_exhaustive(rest_tys, &nil_refs, env) && is_exhaustive(&cons_tys, &cons_refs, env)
}

/// Specialize the matrix on constructor `ctor` of `head_ty`.
fn specialize(
    ctor: &str,
    head_ty: &Type,
    rest_tys: &[Type],
    matrix: &[&[Pattern]],
    env: &ExhaustEnv,
) -> (Vec<Type>, Vec<Vec<Pattern>>) {
    let field_tys = ctor_field_types(ctor, head_ty, env);
    let mut out_tys = field_tys.clone();
    out_tys.extend_from_slice(rest_tys);

    let mut rows: Vec<Vec<Pattern>> = Vec::new();
    for row in matrix {
        let Some(p) = row.first() else { continue };
        let p = peel_as(p);
        match &p.kind {
            PatternKind::Var(_) | PatternKind::Discard => {
                let mut r: Vec<Pattern> = field_tys
                    .iter()
                    .map(|_| Pattern {
                        kind: PatternKind::Discard,
                        span: p.span,
                    })
                    .collect();
                r.extend(row[1..].iter().cloned());
                rows.push(r);
            }
            PatternKind::Constructor {
                name,
                fields,
                with_spread,
                ..
            } if name == ctor => {
                let labels = env.ctor_labels.get(ctor).map(|v| v.as_slice());
                let aligned =
                    align_ctor_fields(fields, *with_spread, field_tys.len(), labels, p.span);
                let mut r = aligned;
                r.extend(row[1..].iter().cloned());
                rows.push(r);
            }
            _ => {}
        }
    }
    (out_tys, rows)
}

fn align_ctor_fields(
    fields: &[PatternField],
    with_spread: bool,
    arity: usize,
    decl_labels: Option<&[Option<String>]>,
    span: Span,
) -> Vec<Pattern> {
    if arity == 0 {
        return Vec::new();
    }
    let discard = || Pattern {
        kind: PatternKind::Discard,
        span,
    };
    let labelled = fields.iter().any(|f| f.label.is_some());
    if labelled {
        if let Some(labels) = decl_labels {
            let mut out = vec![discard(); arity.max(labels.len())];
            for f in fields {
                if let Some(label) = &f.label {
                    if let Some(idx) = labels.iter().position(|l| l.as_ref() == Some(label)) {
                        out[idx] = f.pattern.clone();
                    }
                }
            }
            out.truncate(arity);
            if with_spread {
                for slot in out.iter_mut() {
                    if matches!(slot.kind, PatternKind::Discard) {
                        // already discard
                    }
                }
            }
            return out;
        }
        // Fallback: source order when declaration labels are unknown.
        let mut out: Vec<Pattern> = fields.iter().map(|f| f.pattern.clone()).collect();
        while out.len() < arity {
            out.push(discard());
        }
        out.truncate(arity);
        out
    } else {
        let mut out: Vec<Pattern> = fields.iter().map(|f| f.pattern.clone()).collect();
        if with_spread {
            while out.len() < arity {
                out.push(discard());
            }
        }
        out.truncate(arity);
        out
    }
}

fn expected_constructors(ty: &Type, env: &ExhaustEnv) -> Option<Vec<String>> {
    match ty {
        Type::Named { name, .. } => match name.as_str() {
            "Bool" => Some(vec!["True".into(), "False".into()]),
            "Nil" => Some(vec!["Nil".into()]),
            "Option" => Some(vec!["Some".into(), "None".into()]),
            "Result" => Some(vec!["Ok".into(), "Error".into()]),
            "Int" | "Float" | "String" | "BitArray" | "Pid" | "Subject" | "Monitor" | "Timer"
            | "Selector" | "List" | "Dict" | "Set" | "Vector" | "_" | "Task" => None,
            other => env.adt_variants.get(other).cloned(),
        },
        Type::Tuple(_) | Type::Fn { .. } | Type::Var(_) => None,
    }
}

fn subst_vars(ty: &Type, map: &HashMap<u32, Type>) -> Type {
    match ty {
        Type::Var(v) => map.get(v).cloned().unwrap_or_else(|| ty.clone()),
        Type::Named { module, name, args } => Type::Named {
            module: module.clone(),
            name: name.clone(),
            args: args.iter().map(|a| subst_vars(a, map)).collect(),
        },
        Type::Fn { params, ret } => Type::Fn {
            params: params.iter().map(|p| subst_vars(p, map)).collect(),
            ret: Box::new(subst_vars(ret, map)),
        },
        Type::Tuple(elems) => Type::Tuple(elems.iter().map(|e| subst_vars(e, map)).collect()),
    }
}

fn ctor_field_types(ctor: &str, ty: &Type, env: &ExhaustEnv) -> Vec<Type> {
    if let Some(fields) = env.ctor_fields.get(ctor) {
        if let Type::Named { name, args, .. } = ty {
            if let Some(pvars) = env.adt_param_vars.get(name) {
                let mut map = HashMap::new();
                for (pv, arg) in pvars.iter().zip(args.iter()) {
                    map.insert(*pv, arg.clone());
                }
                if !map.is_empty() {
                    return fields.iter().map(|f| subst_vars(f, &map)).collect();
                }
            }
        }
        return fields.clone();
    }
    match (ctor, ty) {
        ("Some", Type::Named { name, args, .. }) if name == "Option" && args.len() == 1 => {
            vec![args[0].clone()]
        }
        ("Ok", Type::Named { name, args, .. }) if name == "Result" && args.len() == 2 => {
            vec![args[0].clone()]
        }
        ("Error", Type::Named { name, args, .. }) if name == "Result" && args.len() == 2 => {
            vec![args[1].clone()]
        }
        ("True" | "False" | "None" | "Nil", _) => vec![],
        _ => Vec::new(),
    }
}

fn is_useful(row: &[Pattern], prior: &[&[Pattern]], tys: &[Type], env: &ExhaustEnv) -> bool {
    if prior.is_empty() {
        return true;
    }
    if prior.iter().any(|p| row_covers(p, row, tys, env)) {
        return false;
    }
    // Union coverage: row is useless when prior already exhausts its matched space.
    // Approximate by checking whether adding `row` does not extend exhaustiveness
    // of the type relative to prior alone when row is a full cover candidate, or
    // when prior is already exhaustive for `tys`.
    if is_exhaustive(tys, prior, env) {
        return false;
    }
    // Constructor-specialized union: if row is a constructor and prior covers that ctor.
    if tys.len() == 1 && row.len() == 1 {
        let ty = &tys[0];
        let pat = peel_as(&row[0]);
        if is_irrefutable_cover(pat, Some(ty)) {
            return !is_exhaustive(tys, prior, env);
        }
        if let PatternKind::Constructor { name, .. } = &pat.kind {
            let (rest_tys, specialized) = specialize(name, ty, &[], prior, env);
            let refs: Vec<&[Pattern]> = specialized.iter().map(|v| v.as_slice()).collect();
            let row_spec = {
                let (_, mine) = specialize(name, ty, &[], &[row], env);
                mine
            };
            if row_spec.is_empty() {
                return true;
            }
            let row_refs: Vec<&[Pattern]> = row_spec.iter().map(|v| v.as_slice()).collect();
            // Useful if the row's specialization is not covered by prior's specialization.
            return is_useful(row_refs[0], &refs, &rest_tys, env);
        }
    }
    true
}

fn row_covers(prior: &[Pattern], row: &[Pattern], tys: &[Type], env: &ExhaustEnv) -> bool {
    if prior.len() != row.len() || prior.len() != tys.len() {
        return false;
    }
    prior
        .iter()
        .zip(row.iter())
        .zip(tys.iter())
        .all(|((p, q), ty)| pattern_covers(p, q, ty, env))
}

fn pattern_covers(prior: &Pattern, row: &Pattern, ty: &Type, env: &ExhaustEnv) -> bool {
    if is_irrefutable_cover(prior, Some(ty)) {
        return true;
    }
    let prior = peel_as(prior);
    let row = peel_as(row);
    match (&prior.kind, &row.kind) {
        (
            PatternKind::Constructor {
                name: n1,
                fields: f1,
                with_spread: s1,
                ..
            },
            PatternKind::Constructor {
                name: n2,
                fields: f2,
                with_spread: s2,
                ..
            },
        ) if n1 == n2 => constructor_fields_cover(n1, f1, *s1, f2, *s2, ty, env),
        (PatternKind::Int(a), PatternKind::Int(b)) => a == b,
        (PatternKind::Float(a), PatternKind::Float(b)) => a == b,
        (PatternKind::String(a), PatternKind::String(b)) => a == b,
        (
            PatternKind::StringPrefix {
                literal: a,
                rest: ra,
            },
            PatternKind::StringPrefix {
                literal: b,
                rest: rb,
            },
        ) if a == b => pattern_covers(ra, rb, ty, env),
        (PatternKind::Tuple(a), PatternKind::Tuple(b)) => match ty {
            Type::Tuple(elem_tys) if a.len() == b.len() && a.len() == elem_tys.len() => a
                .iter()
                .zip(b.iter())
                .zip(elem_tys.iter())
                .all(|((x, y), t)| pattern_covers(x, y, t, env)),
            _ => {
                a.len() == b.len()
                    && a.iter()
                        .zip(b.iter())
                        .all(|(x, y)| pattern_covers(x, y, ty, env))
            }
        },
        (PatternKind::List { items: a, rest: ra }, PatternKind::List { items: b, rest: rb }) => {
            if a.len() > b.len() || (ra.is_none() && a.len() != b.len()) {
                return false;
            }
            let elem_ty = match ty {
                Type::Named { name, args, .. } if name == "List" && !args.is_empty() => &args[0],
                _ => ty,
            };
            let prefix_ok = a
                .iter()
                .zip(b.iter())
                .all(|(x, y)| pattern_covers(x, y, elem_ty, env));
            if !prefix_ok {
                return false;
            }
            if a.len() == b.len() {
                return match (ra, rb) {
                    (None, None) => true,
                    (Some(r), Some(s)) => pattern_covers(r, s, ty, env),
                    (Some(r), None) => is_irrefutable_cover(r, Some(ty)),
                    (None, Some(_)) => false,
                };
            }
            match ra {
                Some(r) if is_irrefutable_cover(r, Some(ty)) => true,
                Some(r) => {
                    let suffix = Pattern {
                        kind: PatternKind::List {
                            items: b[a.len()..].to_vec(),
                            rest: rb.clone(),
                        },
                        span: row.span,
                    };
                    pattern_covers(r, &suffix, ty, env)
                }
                None => false,
            }
        }
        (_, PatternKind::Var(_)) | (_, PatternKind::Discard) => false,
        _ => false,
    }
}

fn constructor_fields_cover(
    ctor: &str,
    prior_fields: &[PatternField],
    prior_spread: bool,
    row_fields: &[PatternField],
    row_spread: bool,
    ty: &Type,
    env: &ExhaustEnv,
) -> bool {
    let labelled = prior_fields.iter().any(|f| f.label.is_some())
        || row_fields.iter().any(|f| f.label.is_some());
    let field_tys = {
        let resolved = ctor_field_types(ctor, ty, env);
        if resolved.is_empty() {
            vec![ty.clone(); prior_fields.len().max(row_fields.len()).max(1)]
        } else {
            resolved
        }
    };
    let decl_labels = env.ctor_labels.get(ctor).map(|v| v.as_slice());

    if labelled {
        let label_index = |label: &str| -> Option<usize> {
            decl_labels.and_then(|labs| {
                labs.iter()
                    .position(|l| l.as_ref().map(|s| s.as_str()) == Some(label))
            })
        };
        for rf in row_fields {
            let Some(label) = &rf.label else {
                return false;
            };
            if let Some(pf) = prior_fields
                .iter()
                .find(|p| p.label.as_ref() == Some(label))
            {
                let fty = label_index(label)
                    .and_then(|i| field_tys.get(i))
                    .unwrap_or(ty);
                if !pattern_covers(&pf.pattern, &rf.pattern, fty, env) {
                    return false;
                }
            } else if !prior_spread {
                return false;
            }
        }
        if row_spread {
            for pf in prior_fields {
                let Some(label) = &pf.label else {
                    continue;
                };
                let mentioned = row_fields.iter().any(|r| r.label.as_ref() == Some(label));
                let fty = label_index(label)
                    .and_then(|i| field_tys.get(i))
                    .or_else(|| field_tys.first());
                if !mentioned && !is_irrefutable_cover(&pf.pattern, fty) {
                    return false;
                }
            }
        }
        return true;
    }

    for (i, rf) in row_fields.iter().enumerate() {
        if let Some(pf) = prior_fields.get(i) {
            let fty = field_tys.get(i).unwrap_or(ty);
            if !pattern_covers(&pf.pattern, &rf.pattern, fty, env) {
                return false;
            }
        } else if !prior_spread {
            return false;
        }
    }
    if row_spread {
        for (i, pf) in prior_fields.iter().enumerate().skip(row_fields.len()) {
            let fty = field_tys.get(i);
            if !is_irrefutable_cover(&pf.pattern, fty) {
                return false;
            }
        }
    } else if !prior_spread && prior_fields.len() != row_fields.len() {
        return prior_fields.len() >= row_fields.len()
            && prior_fields[row_fields.len()..]
                .iter()
                .enumerate()
                .all(|(j, pf)| {
                    let fty = field_tys.get(row_fields.len() + j);
                    is_irrefutable_cover(&pf.pattern, fty)
                });
    }
    true
}

fn row_irrefutable(row: &[Pattern], tys: &[Type]) -> bool {
    row.len() == tys.len()
        && row
            .iter()
            .zip(tys.iter())
            .all(|(p, t)| is_irrefutable_cover(p, Some(t)))
}

/// Typed irrefutable cover. Tuple arity must match when `ty` is a tuple.
fn is_irrefutable_cover(pattern: &Pattern, ty: Option<&Type>) -> bool {
    match &pattern.kind {
        PatternKind::Var(_) | PatternKind::Discard => true,
        PatternKind::As { pattern, .. } => is_irrefutable_cover(pattern, ty),
        PatternKind::Tuple(elems) => match ty {
            Some(Type::Tuple(elem_tys)) => {
                elems.len() == elem_tys.len()
                    && elems
                        .iter()
                        .zip(elem_tys.iter())
                        .all(|(p, t)| is_irrefutable_cover(p, Some(t)))
            }
            // Without a tuple type, do not treat arbitrary-arity tuples as covers.
            Some(_) => false,
            None => elems.iter().all(|p| is_irrefutable_cover(p, None)),
        },
        PatternKind::List { items, rest } if items.is_empty() => {
            rest.as_ref().is_some_and(|r| is_irrefutable_cover(r, ty))
        }
        _ => false,
    }
}

fn peel_as(pattern: &Pattern) -> &Pattern {
    match &pattern.kind {
        PatternKind::As { pattern, .. } => peel_as(pattern),
        _ => pattern,
    }
}

fn missing_hint(tys: &[Type], matrix: &[&[Pattern]], env: &ExhaustEnv) -> String {
    if tys.len() == 1 {
        if is_list_ty(&tys[0]) {
            return "[] or [_, ..] list patterns".into();
        }
        if let Some(needed) = expected_constructors(&tys[0], env) {
            let missing: Vec<&str> = needed
                .iter()
                .filter(|c| {
                    let (rest, spec) = specialize(c, &tys[0], &[], matrix, env);
                    let refs: Vec<&[Pattern]> = spec.iter().map(|v| v.as_slice()).collect();
                    !is_exhaustive(&rest, &refs, env)
                })
                .map(|s| s.as_str())
                .collect();
            if !missing.is_empty() {
                return missing.join(", ");
            }
            return "nested constructor fields".into();
        }
        return match &tys[0] {
            Type::Named { name, .. } => format!("wildcard for `{name}`"),
            Type::Tuple(_) => "tuple patterns covering all arities/elements".into(),
            _ => "a wildcard pattern".into(),
        };
    }
    "a covering multi-subject pattern matrix".into()
}
