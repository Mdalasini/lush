//! Case exhaustiveness and redundancy (`spec.md` §5.4 / §4.3).
//!
//! Uses constructor coverage with nested field checks for algebraic types and
//! treats Int/String/Float/BitArray/string-prefix patterns as requiring a
//! wildcard fallback. Guarded arms do not contribute to exhaustiveness.

use crate::error::{TypeError, TypeWarning};
use crate::ty::Type;
use lush_syntax::ast::{Clause, Pattern, PatternField, PatternKind};
use lush_syntax::Span;
use std::collections::HashMap;

/// Check exhaustiveness and redundancy for a `case` expression.
pub(crate) fn check_case(
    subject_tys: &[Type],
    clauses: &[Clause],
    adt_variants: &HashMap<String, Vec<String>>,
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

    // Redundancy is checked per alternative; guards never establish coverage.
    let mut prior: Vec<&[Pattern]> = Vec::new();
    for clause in clauses {
        for row in &clause.patterns {
            if row.patterns.len() != subject_tys.len() {
                continue;
            }
            if !is_useful(&row.patterns, &prior, subject_tys, adt_variants) {
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

    if !is_exhaustive(subject_tys, &prior, adt_variants) {
        errors.push(TypeError::Other {
            span: case_span,
            message: format!(
                "non-exhaustive case: missing patterns for {}",
                missing_hint(subject_tys, &prior, adt_variants)
            ),
        });
    }

    (errors, warnings)
}

fn is_exhaustive(
    subject_tys: &[Type],
    matrix: &[&[Pattern]],
    adt_variants: &HashMap<String, Vec<String>>,
) -> bool {
    if matrix
        .iter()
        .any(|row| row.iter().all(is_irrefutable_cover))
    {
        return true;
    }
    if subject_tys.len() == 1 {
        return exhaustive_one(&subject_tys[0], matrix, adt_variants);
    }
    // Multi-subject: require a fully irrefutable row (conservative).
    matrix
        .iter()
        .any(|row| row.iter().all(is_irrefutable_cover))
}

fn exhaustive_one(
    ty: &Type,
    matrix: &[&[Pattern]],
    adt_variants: &HashMap<String, Vec<String>>,
) -> bool {
    if matrix
        .iter()
        .any(|row| row.first().map(is_irrefutable_cover).unwrap_or(false))
    {
        return true;
    }

    let Some(ctors_needed) = expected_constructors(ty, adt_variants) else {
        // Infinite / opaque domains (including List without a full rest cover).
        return false;
    };
    if ctors_needed.is_empty() {
        return false;
    }

    for ctor in &ctors_needed {
        if !constructor_fully_covered(ctor, ty, matrix, adt_variants) {
            return false;
        }
    }
    true
}

/// Whether every value of `ctor` is covered by the matrix (including nested fields).
fn constructor_fully_covered(
    ctor: &str,
    ty: &Type,
    matrix: &[&[Pattern]],
    adt_variants: &HashMap<String, Vec<String>>,
) -> bool {
    let mut field_rows: Vec<Vec<&Pattern>> = Vec::new();
    let mut saw_nullary = false;
    let mut max_fields = 0usize;

    for row in matrix {
        let Some(p) = row.first() else { continue };
        let p = peel_as(p);
        match &p.kind {
            PatternKind::Var(_) | PatternKind::Discard => return true,
            PatternKind::Constructor {
                name,
                fields,
                with_spread,
                ..
            } if name == ctor => {
                if fields.is_empty() && !*with_spread {
                    saw_nullary = true;
                    continue;
                }
                max_fields = max_fields.max(fields.len());
                let pats: Vec<&Pattern> = fields.iter().map(|f| &f.pattern).collect();
                let field_tys = ctor_field_types(ctor, ty, pats.len().max(1));
                // One row covers the whole constructor when every field covers its type
                // (irrefutable, nested-exhaustive, or unknown imported constructor).
                if pats.iter().enumerate().all(|(i, fp)| {
                    let fty = field_tys.get(i).unwrap_or(ty);
                    field_covers_type(fp, fty, adt_variants)
                }) {
                    return true;
                }
                field_rows.push(pats);
            }
            _ => {}
        }
    }

    if saw_nullary && field_rows.is_empty() {
        return true;
    }
    if field_rows.is_empty() {
        return false;
    }

    // Nested exhaustiveness for each field position (conservative product).
    let field_tys = ctor_field_types(ctor, ty, max_fields);
    for i in 0..max_fields {
        let owned: Vec<Vec<Pattern>> = field_rows
            .iter()
            .filter(|r| r.len() > i)
            .map(|r| vec![(*r[i]).clone()])
            .collect();
        let refs: Vec<&[Pattern]> = owned.iter().map(|v| v.as_slice()).collect();
        let fty = field_tys.get(i).cloned().unwrap_or_else(|| Type::Named {
            module: None,
            name: "_".into(),
            args: vec![],
        });
        if !exhaustive_one(&fty, &refs, adt_variants) {
            // Allow a column of unknown imported constructors to count as covered.
            let col_covers = field_rows
                .iter()
                .filter(|r| r.len() > i)
                .any(|r| field_covers_type(r[i], &fty, adt_variants));
            if !col_covers {
                return false;
            }
        }
    }
    true
}

/// Whether a single field pattern covers every value of `ty`.
fn field_covers_type(
    pat: &Pattern,
    ty: &Type,
    adt_variants: &HashMap<String, Vec<String>>,
) -> bool {
    if is_irrefutable_cover(pat) {
        return true;
    }
    let pat = peel_as(pat);
    match &pat.kind {
        PatternKind::Constructor { .. } => match expected_constructors(ty, adt_variants) {
            // Known finite ADT: a single constructor arm is not enough by itself.
            Some(_) => false,
            // Unknown / imported / variable payload: accept constructor witness.
            // Open literal domains never use constructor patterns for coverage.
            None => !is_open_domain(ty),
        },
        _ => false,
    }
}

fn is_open_domain(ty: &Type) -> bool {
    match ty {
        Type::Named { name, .. } => matches!(
            name.as_str(),
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
        ),
        Type::Var(_) | Type::Fn { .. } | Type::Tuple(_) => false,
    }
}

fn ctor_field_types(ctor: &str, ty: &Type, arity: usize) -> Vec<Type> {
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
        _ => (0..arity)
            .map(|_| Type::Named {
                module: None,
                name: "_".into(),
                args: vec![],
            })
            .collect(),
    }
}

fn expected_constructors(
    ty: &Type,
    adt_variants: &HashMap<String, Vec<String>>,
) -> Option<Vec<String>> {
    match ty {
        Type::Named { name, .. } => match name.as_str() {
            "Bool" => Some(vec!["True".into(), "False".into()]),
            "Nil" => Some(vec!["Nil".into()]),
            "Option" => Some(vec!["Some".into(), "None".into()]),
            "Result" => Some(vec!["Ok".into(), "Error".into()]),
            // Infinite literal domains, List, collections: need wildcard / rest.
            "Int" | "Float" | "String" | "BitArray" | "Pid" | "Subject" | "Monitor" | "Timer"
            | "Selector" | "List" | "Dict" | "Set" | "Vector" | "_" => None,
            other => adt_variants.get(other).cloned(),
        },
        Type::Tuple(_) | Type::Fn { .. } | Type::Var(_) => None,
    }
}

fn is_useful(
    row: &[Pattern],
    prior: &[&[Pattern]],
    subject_tys: &[Type],
    adt_variants: &HashMap<String, Vec<String>>,
) -> bool {
    if prior.is_empty() {
        return true;
    }
    // Useful unless the union of prior rows already covers every value `row` matches.
    !collective_covers(prior, row, subject_tys, adt_variants)
}

fn collective_covers(
    prior: &[&[Pattern]],
    row: &[Pattern],
    subject_tys: &[Type],
    adt_variants: &HashMap<String, Vec<String>>,
) -> bool {
    if prior
        .iter()
        .any(|p| row_covers(p, row, subject_tys, adt_variants))
    {
        return true;
    }
    // Union coverage for a single subject: if `row` is irrefutable and prior is
    // exhaustive, or prior exhausts the constructor/literal space that `row` matches.
    if subject_tys.len() == 1 && row.len() == 1 {
        let ty = &subject_tys[0];
        let pat = peel_as(&row[0]);
        if is_irrefutable_cover(pat) {
            return is_exhaustive(subject_tys, prior, adt_variants);
        }
        if let PatternKind::Constructor { name, fields, .. } = &pat.kind {
            // Covered if prior exhaustively covers this constructor's values and
            // the row's fields are nested-covered by the specialized prior matrix.
            let specialized: Vec<&[Pattern]> = prior
                .iter()
                .copied()
                .filter(|p| {
                    p.first().is_some_and(|q| {
                        let q = peel_as(q);
                        is_irrefutable_cover(q)
                            || matches!(&q.kind, PatternKind::Constructor { name: n, .. } if n == name)
                    })
                })
                .collect();
            if specialized
                .iter()
                .any(|p| p.first().is_some_and(is_irrefutable_cover))
            {
                return true;
            }
            if fields.is_empty() {
                return specialized.iter().any(|p| {
                    matches!(
                        &peel_as(p.first().unwrap()).kind,
                        PatternKind::Constructor { name: n, fields: f, with_spread: s, .. }
                            if n == name && (f.is_empty() || (*s && f.iter().all(|fp| is_irrefutable_cover(&fp.pattern))))
                    )
                });
            }
            // Build field-only matrix from prior constructors of the same name.
            let mut field_matrix: Vec<Vec<Pattern>> = Vec::new();
            for p in &specialized {
                let q = peel_as(p.first().unwrap());
                if let PatternKind::Constructor {
                    name: n,
                    fields: pf,
                    with_spread,
                    ..
                } = &q.kind
                {
                    if n != name {
                        continue;
                    }
                    if *with_spread && pf.iter().all(|fp| is_irrefutable_cover(&fp.pattern)) {
                        return true;
                    }
                    if pf.iter().all(|fp| is_irrefutable_cover(&fp.pattern)) && !*with_spread {
                        return true;
                    }
                    // Align by label when present.
                    let aligned = align_fields_for_cover(pf, fields);
                    field_matrix.push(aligned);
                }
            }
            if field_matrix.is_empty() {
                return false;
            }
            let field_tys = ctor_field_types(name, ty, fields.len());
            for (i, fp) in fields.iter().enumerate() {
                let owned: Vec<Vec<Pattern>> = field_matrix
                    .iter()
                    .filter(|r| r.len() > i)
                    .map(|r| vec![r[i].clone()])
                    .collect();
                let refs: Vec<&[Pattern]> = owned.iter().map(|v| v.as_slice()).collect();
                let row_field = [fp.pattern.clone()];
                let fty = field_tys.get(i).cloned().unwrap_or_else(|| Type::Named {
                    module: None,
                    name: "_".into(),
                    args: vec![],
                });
                if is_useful(&row_field, &refs, &[fty], adt_variants) {
                    return false;
                }
            }
            return true;
        }
    }
    false
}

fn align_fields_for_cover(prior: &[PatternField], row: &[PatternField]) -> Vec<Pattern> {
    if row.iter().any(|f| f.label.is_some()) || prior.iter().any(|f| f.label.is_some()) {
        row.iter()
            .map(|rf| {
                if let Some(label) = &rf.label {
                    prior
                        .iter()
                        .find(|pf| pf.label.as_ref() == Some(label))
                        .map(|pf| pf.pattern.clone())
                        .unwrap_or_else(|| Pattern {
                            kind: PatternKind::Discard,
                            span: rf.span,
                        })
                } else {
                    Pattern {
                        kind: PatternKind::Discard,
                        span: rf.span,
                    }
                }
            })
            .collect()
    } else {
        prior.iter().map(|f| f.pattern.clone()).collect()
    }
}

/// Whether `prior` covers all values that `row` can match (same arity).
fn row_covers(
    prior: &[Pattern],
    row: &[Pattern],
    subject_tys: &[Type],
    adt_variants: &HashMap<String, Vec<String>>,
) -> bool {
    if prior.len() != row.len() {
        return false;
    }
    prior
        .iter()
        .zip(row.iter())
        .zip(subject_tys.iter())
        .all(|((p, q), ty)| pattern_covers(p, q, ty, adt_variants))
}

fn pattern_covers(
    prior: &Pattern,
    row: &Pattern,
    ty: &Type,
    adt_variants: &HashMap<String, Vec<String>>,
) -> bool {
    if is_irrefutable_cover(prior) {
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
        ) if n1 == n2 => constructor_fields_cover(f1, *s1, f2, *s2, ty, adt_variants),
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
        ) if a == b => pattern_covers(ra, rb, ty, adt_variants),
        (PatternKind::Tuple(a), PatternKind::Tuple(b)) if a.len() == b.len() => a
            .iter()
            .zip(b.iter())
            .all(|(x, y)| pattern_covers(x, y, ty, adt_variants)),
        (PatternKind::List { items: a, rest: ra }, PatternKind::List { items: b, rest: rb }) => {
            // Prior cannot have a longer fixed prefix than the candidate.
            // Equal lengths are required when the prior has no rest.
            if a.len() > b.len() || (ra.is_none() && a.len() != b.len()) {
                return false;
            }
            let prefix_ok = a
                .iter()
                .zip(b.iter())
                .all(|(x, y)| pattern_covers(x, y, ty, adt_variants));
            if !prefix_ok {
                return false;
            }
            if a.len() == b.len() {
                return match (ra, rb) {
                    (None, None) => true,
                    (Some(r), Some(s)) => pattern_covers(r, s, ty, adt_variants),
                    (Some(r), None) => is_irrefutable_cover(r),
                    (None, Some(_)) => false,
                };
            }
            // Candidate has a longer fixed prefix; prior rest must cover the suffix.
            match ra {
                Some(r) if is_irrefutable_cover(r) => true,
                Some(r) => {
                    // Conservative: only when rest is itself a list pattern cover.
                    let suffix = Pattern {
                        kind: PatternKind::List {
                            items: b[a.len()..].to_vec(),
                            rest: rb.clone(),
                        },
                        span: row.span,
                    };
                    pattern_covers(r, &suffix, ty, adt_variants)
                }
                None => false,
            }
        }
        (_, PatternKind::Var(_)) | (_, PatternKind::Discard) => false,
        _ => false,
    }
}

fn constructor_fields_cover(
    prior_fields: &[PatternField],
    prior_spread: bool,
    row_fields: &[PatternField],
    row_spread: bool,
    ty: &Type,
    adt_variants: &HashMap<String, Vec<String>>,
) -> bool {
    let labelled = prior_fields.iter().any(|f| f.label.is_some())
        || row_fields.iter().any(|f| f.label.is_some());

    if labelled {
        for rf in row_fields {
            let Some(label) = &rf.label else {
                return false;
            };
            if let Some(pf) = prior_fields
                .iter()
                .find(|p| p.label.as_ref() == Some(label))
            {
                if !pattern_covers(&pf.pattern, &rf.pattern, ty, adt_variants) {
                    return false;
                }
            } else if !prior_spread {
                return false;
            }
            // else: covered by prior spread
        }
        if row_spread {
            for pf in prior_fields {
                let Some(label) = &pf.label else {
                    continue;
                };
                let mentioned = row_fields.iter().any(|r| r.label.as_ref() == Some(label));
                if !mentioned && !is_irrefutable_cover(&pf.pattern) {
                    return false;
                }
            }
        }
        return true;
    }

    // Positional fields.
    for (i, rf) in row_fields.iter().enumerate() {
        if let Some(pf) = prior_fields.get(i) {
            if !pattern_covers(&pf.pattern, &rf.pattern, ty, adt_variants) {
                return false;
            }
        } else if !prior_spread {
            return false;
        }
    }
    if row_spread {
        for pf in prior_fields.iter().skip(row_fields.len()) {
            if !is_irrefutable_cover(&pf.pattern) {
                return false;
            }
        }
    } else if !prior_spread && prior_fields.len() != row_fields.len() {
        // Exact constructors must agree on arity when neither side spreads.
        return prior_fields.len() >= row_fields.len()
            && prior_fields[row_fields.len()..]
                .iter()
                .all(|pf| is_irrefutable_cover(&pf.pattern));
    }
    true
}

fn is_irrefutable_cover(pattern: &Pattern) -> bool {
    match &pattern.kind {
        PatternKind::Var(_) | PatternKind::Discard => true,
        PatternKind::As { pattern, .. } => is_irrefutable_cover(pattern),
        PatternKind::Tuple(elems) => elems.iter().all(is_irrefutable_cover),
        // `[..rest]` matches every list when `rest` is irrefutable.
        PatternKind::List { items, rest } if items.is_empty() => {
            rest.as_ref().is_some_and(|r| is_irrefutable_cover(r))
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

fn missing_hint(
    subject_tys: &[Type],
    matrix: &[&[Pattern]],
    adt_variants: &HashMap<String, Vec<String>>,
) -> String {
    if subject_tys.len() == 1 {
        if let Some(needed) = expected_constructors(&subject_tys[0], adt_variants) {
            let missing: Vec<&str> = needed
                .iter()
                .filter(|c| !constructor_fully_covered(c, &subject_tys[0], matrix, adt_variants))
                .map(|s| s.as_str())
                .collect();
            if !missing.is_empty() {
                return missing.join(", ");
            }
            return "nested constructor fields".into();
        }
        return match &subject_tys[0] {
            Type::Named { name, .. } => format!("wildcard for `{name}`"),
            _ => "a wildcard pattern".into(),
        };
    }
    let _ = matrix;
    "a covering pattern row".into()
}
