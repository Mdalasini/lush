//! Case exhaustiveness and redundancy (`spec.md` §5.4 / §4.3).
//!
//! Uses a constructor-coverage check for algebraic types and treats
//! Int/String/Float/BitArray/string-prefix patterns as requiring a wildcard
//! fallback. Guarded arms do not contribute to exhaustiveness.

use crate::error::{TypeError, TypeWarning};
use crate::ty::Type;
use lush_syntax::ast::{Clause, Pattern, PatternKind};
use lush_syntax::Span;
use std::collections::{HashMap, HashSet};

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

    // Redundancy: a clause is redundant when every alternative is covered by
    // prior *unguarded* clauses (guards never establish coverage).
    let mut prior: Vec<&[Pattern]> = Vec::new();
    for clause in clauses {
        let useful = clause.patterns.iter().any(|row| {
            if row.patterns.len() != subject_tys.len() {
                return true;
            }
            is_useful(&row.patterns, &prior, subject_tys, adt_variants)
        });
        if !useful {
            warnings.push(TypeWarning::RedundantPattern {
                span: clause.span,
                message: "unreachable pattern".into(),
            });
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
    // Wildcard row covers everything.
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
    let ctors_needed = match expected_constructors(ty, adt_variants) {
        Some(c) => c,
        None => {
            // Infinite / opaque domains need a variable/wildcard.
            return matrix
                .iter()
                .any(|row| row.first().map(is_irrefutable_cover).unwrap_or(false));
        }
    };
    if ctors_needed.is_empty() {
        return matrix
            .iter()
            .any(|row| row.first().map(is_irrefutable_cover).unwrap_or(false));
    }
    let mut covered: HashSet<String> = HashSet::new();
    for row in matrix {
        let Some(p) = row.first() else { continue };
        match &p.kind {
            PatternKind::Var(_) | PatternKind::Discard => return true,
            PatternKind::As { pattern, .. } => {
                if is_irrefutable_cover(pattern) {
                    return true;
                }
                if let Some(name) = constructor_name(pattern) {
                    covered.insert(name);
                }
            }
            PatternKind::Constructor { name, .. } => {
                covered.insert(name.clone());
            }
            _ => {}
        }
    }
    ctors_needed.iter().all(|c| covered.contains(c))
}

fn expected_constructors(
    ty: &Type,
    adt_variants: &HashMap<String, Vec<String>>,
) -> Option<Vec<String>> {
    match ty {
        Type::Named { name, args: _, .. } => match name.as_str() {
            "Bool" => Some(vec!["True".into(), "False".into()]),
            "Nil" => Some(vec!["Nil".into()]),
            "Option" => Some(vec!["Some".into(), "None".into()]),
            "Result" => Some(vec!["Ok".into(), "Error".into()]),
            // Infinite literal domains and BitArray: no finite constructor set.
            "Int" | "Float" | "String" | "BitArray" | "Pid" | "Subject" | "Monitor" | "Timer"
            | "Selector" | "List" | "Dict" | "Set" | "Vector" => None,
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
    // Useful if some value matched by `row` escapes all prior rows.
    !prior
        .iter()
        .any(|p| row_covers(p, row, subject_tys, adt_variants))
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
    match (&prior.kind, &row.kind) {
        (PatternKind::As { pattern, .. }, _) => pattern_covers(pattern, row, ty, adt_variants),
        (_, PatternKind::As { pattern, .. }) => pattern_covers(prior, pattern, ty, adt_variants),
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
        ) if n1 == n2 => {
            if *s1 {
                return true;
            }
            if *s2 && f1.len() > f2.len() {
                return false;
            }
            // Cover when each prior field covers the corresponding row field.
            // Positional comparison; labelled reordering is handled upstream.
            let n = f1.len().min(f2.len());
            (0..n).all(|i| pattern_covers(&f1[i].pattern, &f2[i].pattern, ty, adt_variants))
                && f1.len() >= f2.len()
        }
        (PatternKind::Constructor { name: n1, .. }, PatternKind::Constructor { name: n2, .. }) => {
            n1 == n2
        }
        (PatternKind::Int(a), PatternKind::Int(b)) => a == b,
        (PatternKind::Float(a), PatternKind::Float(b)) => a == b,
        (PatternKind::String(a), PatternKind::String(b)) => a == b,
        (PatternKind::Tuple(a), PatternKind::Tuple(b)) if a.len() == b.len() => a
            .iter()
            .zip(b.iter())
            .all(|(x, y)| pattern_covers(x, y, ty, adt_variants)),
        (PatternKind::List { items: a, rest: ra }, PatternKind::List { items: b, rest: rb }) => {
            if a.len() != b.len() {
                return false;
            }
            let items_ok = a
                .iter()
                .zip(b.iter())
                .all(|(x, y)| pattern_covers(x, y, ty, adt_variants));
            let rest_ok = match (ra, rb) {
                (None, None) => true,
                (Some(r), Some(s)) => pattern_covers(r, s, ty, adt_variants),
                (Some(r), None) => is_irrefutable_cover(r),
                (None, Some(_)) => false,
            };
            items_ok && rest_ok
        }
        // Prior constructor cannot cover a variable in `row` unless it is the
        // only constructor — handled by usefulness via exhaustiveness context.
        (_, PatternKind::Var(_)) | (_, PatternKind::Discard) => false,
        _ => false,
    }
}

fn is_irrefutable_cover(pattern: &Pattern) -> bool {
    match &pattern.kind {
        PatternKind::Var(_) | PatternKind::Discard => true,
        PatternKind::As { pattern, .. } => is_irrefutable_cover(pattern),
        PatternKind::Tuple(elems) => elems.iter().all(is_irrefutable_cover),
        _ => false,
    }
}

fn constructor_name(pattern: &Pattern) -> Option<String> {
    match &pattern.kind {
        PatternKind::Constructor { name, .. } => Some(name.clone()),
        PatternKind::As { pattern, .. } => constructor_name(pattern),
        _ => None,
    }
}

fn missing_hint(
    subject_tys: &[Type],
    matrix: &[&[Pattern]],
    adt_variants: &HashMap<String, Vec<String>>,
) -> String {
    if subject_tys.len() == 1 {
        if let Some(needed) = expected_constructors(&subject_tys[0], adt_variants) {
            let mut covered = HashSet::new();
            for row in matrix {
                if let Some(p) = row.first() {
                    if let Some(n) = constructor_name(p) {
                        covered.insert(n);
                    }
                    if is_irrefutable_cover(p) {
                        return "…".into();
                    }
                }
            }
            let missing: Vec<&String> = needed.iter().filter(|c| !covered.contains(*c)).collect();
            if !missing.is_empty() {
                return missing
                    .into_iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
            }
        }
        return match &subject_tys[0] {
            Type::Named { name, .. } => format!("wildcard for `{name}`"),
            _ => "a wildcard pattern".into(),
        };
    }
    "a covering pattern row".into()
}
