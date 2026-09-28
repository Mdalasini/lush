//! Exhaustiveness and redundancy checking (§5.4).

use lush_syntax::ast::*;
use lush_syntax::span::Span;

use crate::codes;
use crate::diag::TypeSink;
use crate::limits::MAX_EXHAUST_WORK;
use crate::ty::{Type, TypeDefId, TypeStore};

pub struct ExhaustChecker<'a> {
    pub store: &'a mut TypeStore,
    pub sink: &'a mut TypeSink,
    pub work: u64,
}

impl<'a> ExhaustChecker<'a> {
    pub fn check_case(&mut self, subjects: &[Type], clauses: &[Clause], span: Span) {
        if subjects.is_empty() {
            return;
        }
        // Budget: each arm × constructor branching
        let estimate = (clauses.len() as u64)
            .saturating_mul(subjects.len() as u64)
            .saturating_mul(8);
        self.work = self.work.saturating_add(estimate);
        if self.work > MAX_EXHAUST_WORK || subjects.len() >= 20 {
            self.sink.error(
                codes::E1451_MATCH_COMPLEX,
                "pattern match is too complex to check for exhaustiveness",
                span,
                Some("simplify the match or split it into smaller cases".into()),
            );
            return;
        }

        let mut covered_wildcard = false;
        let mut saw_unguarded_catch_all = false;
        for (i, clause) in clauses.iter().enumerate() {
            let has_guard = clause.guard.is_some();
            if let Some(g) = &clause.guard {
                check_guard(g, self.sink);
            }
            // Or-pattern binding agreement
            if clause.patterns.len() > 1 {
                check_or_bindings(&clause.patterns, self.sink);
            }
            let is_catch_all = clause.patterns.iter().all(|row| {
                row.patterns.len() == subjects.len() && row.patterns.iter().all(is_wildcard_pat)
            });
            if is_catch_all && !has_guard {
                if saw_unguarded_catch_all {
                    self.sink.warning(
                        codes::W1004_REDUNDANT_PATTERN,
                        "redundant pattern: unreachable arm",
                        clause.span,
                        None,
                    );
                }
                saw_unguarded_catch_all = true;
                covered_wildcard = true;
            } else if saw_unguarded_catch_all {
                self.sink.warning(
                    codes::W1004_REDUNDANT_PATTERN,
                    "redundant pattern: unreachable arm after catch-all",
                    clause.span,
                    None,
                );
            } else if has_guard && i + 1 < clauses.len() {
                // guarded arms never establish exhaustiveness / redundancy for later
            }
            let _ = covered_wildcard;
        }

        // Simple exhaustiveness for Bool, Nil, Option, Result, ADTs with nullary + catch-all
        if !saw_unguarded_catch_all {
            if let Some(missing) = missing_counterexample(self.store, subjects, clauses) {
                self.sink.error(
                    codes::E1450_NON_EXHAUSTIVE,
                    format!("non-exhaustive patterns: `{missing}` not covered"),
                    span,
                    Some("add a case for the missing pattern or a catch-all `_`".into()),
                );
            }
        }
    }

    pub fn check_irrefutable(&mut self, pat: &Pattern, ty: &Type, allow_refutable: bool) {
        if allow_refutable {
            return;
        }
        if !pattern_irrefutable(self.store, pat, ty) {
            self.sink.error(
                codes::E1216_REFUTABLE_LET,
                "refutable pattern in `let` binding (use `let assert`)",
                pat.span,
                Some("`let` patterns must match all values of the type".into()),
            );
        }
    }
}

fn is_wildcard_pat(p: &Pattern) -> bool {
    matches!(
        p.kind,
        PatternKind::Discard | PatternKind::Var(_) | PatternKind::UnderscoreName(_)
    )
}

fn check_guard(g: &Expr, sink: &mut TypeSink) {
    if !guard_ok(g) {
        sink.error(
            codes::E1218_GUARD_EXPR,
            "invalid expression in pattern guard",
            g.span,
            Some(
                "guards allow literals, nullary constructors, bound variables, field access, \
                 parentheses, prefix `-`/`!`, and §5.7 operators except `|>`; \
                 calls, blocks, `case`, `fn`, and `todo`/`panic`/`assert`/`echo` are rejected"
                    .into(),
            ),
        );
    }
}

fn guard_ok(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Var(_)
        | ExprKind::Constructor(ConstructorRef { .. }) => {
            // nullary ctor only — with args would be Call
            true
        }
        ExprKind::Paren(inner) => guard_ok(inner),
        ExprKind::Field { base, .. } => guard_ok(base),
        ExprKind::Unary {
            op: UnaryOp::Neg | UnaryOp::Not,
            expr,
        } => guard_ok(expr),
        ExprKind::Binary { left, op, right } => {
            // all §5.7 binops except pipe (pipe is ExprKind::Pipe)
            let _ = op;
            guard_ok(left) && guard_ok(right)
        }
        ExprKind::Pipe { .. }
        | ExprKind::Call { .. }
        | ExprKind::Block(_)
        | ExprKind::Case { .. }
        | ExprKind::Fn { .. }
        | ExprKind::Todo { .. }
        | ExprKind::Panic { .. }
        | ExprKind::Assert { .. }
        | ExprKind::Echo(_) => false,
        _ => false,
    }
}

fn check_or_bindings(rows: &[PatternRow], sink: &mut TypeSink) {
    let mut first_vars: Option<Vec<String>> = None;
    for row in rows {
        let mut vars = Vec::new();
        for p in &row.patterns {
            collect_vars(p, &mut vars);
        }
        vars.sort();
        if let Some(f) = &first_vars {
            if *f != vars {
                sink.error(
                    codes::E1219_OR_PATTERN_BINDINGS,
                    "alternatives in an or-pattern must bind the same variables",
                    row.span,
                    None,
                );
            }
        } else {
            first_vars = Some(vars);
        }
    }
}

fn collect_vars(p: &Pattern, out: &mut Vec<String>) {
    match &p.kind {
        PatternKind::Var(n) => out.push(n.text.clone()),
        PatternKind::Alias { pattern, name } => {
            out.push(name.text.clone());
            collect_vars(pattern, out);
        }
        PatternKind::Tuple(ps) => {
            for p in ps {
                collect_vars(p, out);
            }
        }
        PatternKind::List { items, spread } => {
            for p in items {
                collect_vars(p, out);
            }
            if let Some(s) = spread {
                collect_vars(s, out);
            }
        }
        PatternKind::Constructor {
            args: Some(args), ..
        } => {
            for a in args {
                if let Some(p) = &a.pattern {
                    collect_vars(p, out);
                }
            }
        }
        PatternKind::StringPrefix { rest, .. } => collect_vars(rest, out),
        _ => {}
    }
}

fn pattern_irrefutable(store: &TypeStore, pat: &Pattern, ty: &Type) -> bool {
    match &pat.kind {
        PatternKind::Var(_) | PatternKind::Discard | PatternKind::UnderscoreName(_) => true,
        PatternKind::Alias { pattern, .. } => pattern_irrefutable(store, pattern, ty),
        PatternKind::Tuple(ps) => {
            if let Type::Tuple(ts) = ty {
                ps.len() == ts.len()
                    && ps
                        .iter()
                        .zip(ts)
                        .all(|(p, t)| pattern_irrefutable(store, p, t))
            } else {
                false
            }
        }
        PatternKind::Constructor { constructor, args } => {
            // Irrefutable only for single-variant types
            let Type::App { def, .. } = ty else {
                return false;
            };
            let Some(info) = store.defs.get(def) else {
                return true;
            };
            if info.variants.len() != 1 {
                return false;
            }
            if info.variants[0].name != constructor.name.text {
                return false;
            }
            if let Some(args) = args {
                args.iter().all(|a| {
                    a.spread
                        || a.pattern
                            .as_ref()
                            .map(|p| pattern_irrefutable(store, p, &Type::Error))
                            .unwrap_or(true)
                })
            } else {
                true
            }
        }
        _ => false,
    }
}

fn missing_counterexample(
    store: &TypeStore,
    subjects: &[Type],
    clauses: &[Clause],
) -> Option<String> {
    if subjects.len() != 1 {
        // Multi-subject: require catch-all for now unless all bool
        return Some("_".into());
    }
    let ty = &subjects[0];
    match ty {
        Type::Bool => {
            let has_true = covers_ctor(clauses, "True");
            let has_false = covers_ctor(clauses, "False");
            if !has_true {
                return Some("True".into());
            }
            if !has_false {
                return Some("False".into());
            }
            None
        }
        Type::Nil => {
            if covers_ctor(clauses, "Nil") || has_wildcard(clauses) {
                None
            } else {
                Some("Nil".into())
            }
        }
        Type::App { def, .. } => missing_adt(store, *def, clauses),
        Type::Int | Type::Float | Type::String | Type::BitArray => {
            if has_wildcard(clauses) {
                None
            } else {
                Some("_".into())
            }
        }
        _ => {
            if has_wildcard(clauses) {
                None
            } else {
                Some("_".into())
            }
        }
    }
}

fn missing_adt(store: &TypeStore, def: TypeDefId, clauses: &[Clause]) -> Option<String> {
    let info = store.defs.get(&def)?;
    if has_wildcard(clauses) {
        return None;
    }
    for v in &info.variants {
        if !covers_ctor(clauses, &v.name) {
            return Some(v.name.clone());
        }
    }
    None
}

fn covers_ctor(clauses: &[Clause], name: &str) -> bool {
    clauses.iter().any(|c| {
        c.guard.is_none()
            && c.patterns.iter().any(|row| {
                row.patterns.iter().any(|p| match &p.kind {
                    PatternKind::Constructor { constructor, .. } => constructor.name.text == name,
                    PatternKind::Var(n) if n.text == name => true,
                    _ => false,
                })
            })
    })
}

fn has_wildcard(clauses: &[Clause]) -> bool {
    clauses.iter().any(|c| {
        c.guard.is_none()
            && c.patterns
                .iter()
                .any(|row| row.patterns.iter().all(is_wildcard_pat))
    })
}

/// Soundness helper: when checker says exhaustive for small finite types, verify.
pub fn brute_force_confirm_bool(clauses: &[Clause]) -> bool {
    let values = ["True", "False"];
    values.iter().all(|v| {
        clauses.iter().any(|c| {
            c.guard.is_none()
                && c.patterns.iter().any(|row| {
                    row.patterns.iter().any(|p| match &p.kind {
                        PatternKind::Constructor { constructor, .. } => constructor.name.text == *v,
                        PatternKind::Discard
                        | PatternKind::Var(_)
                        | PatternKind::UnderscoreName(_) => true,
                        _ => false,
                    })
                })
        })
    })
}
