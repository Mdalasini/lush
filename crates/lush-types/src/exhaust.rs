//! Exhaustiveness and redundancy checking via Maranget usefulness (§5.4).

use lush_syntax::ast::*;
use lush_syntax::span::Span;

use crate::codes;
use crate::diag::TypeSink;
use crate::limits::MAX_EXHAUST_WORK;
use crate::ty::{Type, TypeDefKind, TypeStore};

#[derive(Clone, Debug)]
enum Head {
    Wildcard,
    Ctor { name: String, fields: Vec<Pat> },
    Tuple(Vec<Pat>),
    ListNil,
    ListCons { heads: Vec<Pat>, has_spread: bool },
    Lit(LitKind),
    Infinite,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum LitKind {
    Int(String),
    Float(String),
    String(String),
}

#[derive(Clone, Debug)]
struct Pat {
    head: Head,
}

type Row = Vec<Pat>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Budget {
    Exhausted,
}

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
        for clause in clauses {
            if let Some(g) = &clause.guard {
                check_guard(g, self.sink);
            }
            if clause.patterns.len() > 1 {
                check_or_bindings(&clause.patterns, self.sink);
            }
        }

        // Fast path: single subject over an infinite literal domain (Int/Float/
        // String/BitArray). Hash literals so n arms are O(n), not O(n²).
        if subjects.len() == 1
            && is_infinite_lit_domain(self.store, &subjects[0])
            && self.check_lit_column(clauses, span)
        {
            return;
        }

        let mut matrix: Vec<Row> = Vec::new();
        for clause in clauses {
            let rows = expand_clause(clause, subjects.len());
            if clause.guard.is_some() {
                continue; // guarded arms don't establish coverage
            }
            // Check each alternative against the matrix *including* earlier
            // alternatives of the same clause, so `1 | 1` and `True | True`
            // report redundant alternatives.
            for row in rows {
                match self.useful(&matrix, &row, subjects) {
                    Ok(false) => {
                        self.sink.warning(
                            codes::W1004_REDUNDANT_PATTERN,
                            "redundant pattern: unreachable arm",
                            clause.span,
                            None,
                        );
                    }
                    Ok(true) => {
                        matrix.push(row);
                    }
                    Err(Budget::Exhausted) => {
                        self.emit_budget(span);
                        return;
                    }
                }
            }
        }

        let wild: Row = (0..subjects.len())
            .map(|_| Pat {
                head: Head::Wildcard,
            })
            .collect();
        match self.useful(&matrix, &wild, subjects) {
            Ok(true) => {
                let witness =
                    missing_witness(self.store, &matrix, subjects).unwrap_or_else(|| "_".into());
                self.sink.error(
                    codes::E1450_NON_EXHAUSTIVE,
                    format!("non-exhaustive patterns: `{witness}` not covered"),
                    span,
                    Some("add a case for the missing pattern or a catch-all `_`".into()),
                );
            }
            Ok(false) => {}
            Err(Budget::Exhausted) => self.emit_budget(span),
        }
    }

    /// Returns true when the fast path fully handled the match.
    fn check_lit_column(&mut self, clauses: &[Clause], span: Span) -> bool {
        use std::collections::HashSet;
        let mut seen: HashSet<LitKind> = HashSet::new();
        let mut has_wild = false;
        for clause in clauses {
            if clause.guard.is_some() {
                continue;
            }
            let rows = expand_clause(clause, 1);
            for row in rows {
                self.tick().ok();
                match &row[0].head {
                    Head::Lit(lit) => {
                        if !seen.insert(lit.clone()) {
                            self.sink.warning(
                                codes::W1004_REDUNDANT_PATTERN,
                                "redundant pattern: unreachable arm",
                                clause.span,
                                None,
                            );
                        }
                    }
                    Head::Wildcard => {
                        if has_wild {
                            self.sink.warning(
                                codes::W1004_REDUNDANT_PATTERN,
                                "redundant pattern: unreachable arm",
                                clause.span,
                                None,
                            );
                        }
                        has_wild = true;
                    }
                    // Mixed constructors / nested patterns — fall back.
                    _ => return false,
                }
            }
        }
        if !has_wild {
            self.sink.error(
                codes::E1450_NON_EXHAUSTIVE,
                "non-exhaustive patterns: `_` not covered",
                span,
                Some("add a case for the missing pattern or a catch-all `_`".into()),
            );
        }
        true
    }

    fn emit_budget(&mut self, span: Span) {
        self.sink.error(
            codes::E1451_MATCH_COMPLEX,
            "pattern match is too complex to check for exhaustiveness",
            span,
            Some("simplify the match or split it into smaller cases".into()),
        );
    }

    fn tick(&mut self) -> Result<(), Budget> {
        self.work = self.work.saturating_add(1);
        self.store.work = self.store.work.saturating_add(1);
        if self.work > MAX_EXHAUST_WORK {
            Err(Budget::Exhausted)
        } else {
            Ok(())
        }
    }

    fn useful(&mut self, matrix: &[Row], row: &Row, tys: &[Type]) -> Result<bool, Budget> {
        self.tick()?;
        // Charge matrix traversal: specialisation scans every row.
        self.work = self.work.saturating_add(matrix.len() as u64);
        self.store.work = self.store.work.saturating_add(matrix.len() as u64);
        if self.work > MAX_EXHAUST_WORK {
            return Err(Budget::Exhausted);
        }
        // Empty matrix covers nothing — any row (incl. further specialisations) is useful.
        // Without this short-circuit, wildcard-on-List keeps cons-specialising forever.
        if matrix.is_empty() {
            return Ok(true);
        }
        if row.is_empty() {
            return Ok(false);
        }
        let pat = &row[0];
        let ty = follow(self.store, &tys[0]);
        match &pat.head {
            Head::Ctor { name, fields } => {
                let ftys = ctor_fields(self.store, &ty, name, fields.len());
                let m = spec_ctor(matrix, name, fields.len());
                let mut v = fields.clone();
                v.extend_from_slice(&row[1..]);
                let mut ntys = ftys;
                ntys.extend_from_slice(&tys[1..]);
                self.useful(&m, &v, &ntys)
            }
            Head::Tuple(elems) => {
                let ftys: Vec<Type> = match &ty {
                    Type::Tuple(ts) => ts.iter().map(|t| (**t).clone()).collect(),
                    _ => elems.iter().map(|_| Type::Error).collect(),
                };
                let m = spec_tuple(matrix, elems.len());
                let mut v = elems.clone();
                v.extend_from_slice(&row[1..]);
                let mut ntys = ftys;
                ntys.extend_from_slice(&tys[1..]);
                self.useful(&m, &v, &ntys)
            }
            Head::ListNil => {
                let m = spec_list_nil(matrix);
                self.useful(&m, &row[1..].to_vec(), &tys[1..])
            }
            Head::ListCons { heads, has_spread } => {
                let elem = match &ty {
                    Type::List(t) => (**t).clone(),
                    _ => Type::Error,
                };
                let m = spec_list_cons(matrix, heads.len(), *has_spread);
                let mut v = heads.clone();
                v.push(Pat {
                    head: if *has_spread {
                        Head::Wildcard
                    } else {
                        Head::ListNil
                    },
                });
                v.extend_from_slice(&row[1..]);
                let mut ntys: Vec<Type> = heads.iter().map(|_| elem.clone()).collect();
                ntys.push(Type::list(elem));
                ntys.extend_from_slice(&tys[1..]);
                self.useful(&m, &v, &ntys)
            }
            Head::Lit(lit) => {
                let m = spec_lit(matrix, lit);
                self.useful(&m, &row[1..].to_vec(), &tys[1..])
            }
            Head::Infinite => {
                let m = default_matrix(matrix);
                self.useful(&m, &row[1..].to_vec(), &tys[1..])
            }
            Head::Wildcard => {
                // Only when *every* matrix row is a wildcard in this column can we
                // skip constructor specialisation. A single `_` among constructors
                // (e.g. `_, False` next to `True, True`) does *not* cover the other
                // constructors — short-circuiting there made multi-subject matches
                // look non-exhaustive. When the whole column is wildcards, though,
                // specialising a recursive `List` would re-introduce the same
                // column forever, so fall through to the default matrix.
                let all_wild = matrix
                    .iter()
                    .all(|r| r.is_empty() || matches!(r[0].head, Head::Wildcard));
                if all_wild {
                    let m = default_matrix(matrix);
                    return self.useful(&m, &row[1..].to_vec(), &tys[1..]);
                }
                if let Some(ctors) = complete_sig(self.store, &ty) {
                    for (name, arity) in &ctors {
                        let m = spec_ctor(matrix, name, *arity);
                        let mut v: Vec<Pat> = (0..*arity)
                            .map(|_| Pat {
                                head: Head::Wildcard,
                            })
                            .collect();
                        v.extend_from_slice(&row[1..]);
                        let mut ntys = ctor_fields(self.store, &ty, name, *arity);
                        ntys.extend_from_slice(&tys[1..]);
                        if self.useful(&m, &v, &ntys)? {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                } else {
                    match &ty {
                        Type::Tuple(ts) => {
                            let n = ts.len();
                            let m = spec_tuple(matrix, n);
                            let mut v: Vec<Pat> = (0..n)
                                .map(|_| Pat {
                                    head: Head::Wildcard,
                                })
                                .collect();
                            v.extend_from_slice(&row[1..]);
                            let mut ntys: Vec<Type> = ts.iter().map(|t| (**t).clone()).collect();
                            ntys.extend_from_slice(&tys[1..]);
                            self.useful(&m, &v, &ntys)
                        }
                        Type::List(elem) => {
                            let m_nil = spec_list_nil(matrix);
                            if self.useful(&m_nil, &row[1..].to_vec(), &tys[1..])? {
                                return Ok(true);
                            }
                            let m_cons = spec_list_cons(matrix, 1, true);
                            let mut v = vec![
                                Pat {
                                    head: Head::Wildcard,
                                },
                                Pat {
                                    head: Head::Wildcard,
                                },
                            ];
                            v.extend_from_slice(&row[1..]);
                            let mut ntys = vec![(**elem).clone(), Type::list((**elem).clone())];
                            ntys.extend_from_slice(&tys[1..]);
                            self.useful(&m_cons, &v, &ntys)
                        }
                        _ => {
                            // Infinite domain: needs an explicit wildcard in the matrix.
                            let m = default_matrix(matrix);
                            self.useful(&m, &row[1..].to_vec(), &tys[1..])
                        }
                    }
                }
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

fn is_infinite_lit_domain(store: &TypeStore, ty: &Type) -> bool {
    matches!(
        follow(store, ty),
        Type::Int | Type::Float | Type::String | Type::BitArray
    )
}

fn follow(store: &TypeStore, ty: &Type) -> Type {
    match ty {
        Type::Var(id) => {
            if let Some(info) = store.vars.get(id) {
                if let Some(link) = &info.link {
                    return follow(store, link);
                }
            }
            ty.clone()
        }
        Type::App { def, args } => {
            if let Some(info) = store.defs.get(def) {
                if info.kind == TypeDefKind::Alias {
                    if let Some(body) = &info.alias_body {
                        if info.params.is_empty() {
                            return follow(store, body);
                        }
                        // Keep App for parameterised aliases; pattern matching uses variants
                        // of the underlying type only after expansion in infer.
                        let _ = args;
                    }
                }
            }
            ty.clone()
        }
        _ => ty.clone(),
    }
}

fn complete_sig(store: &TypeStore, ty: &Type) -> Option<Vec<(String, usize)>> {
    match ty {
        Type::Bool => Some(vec![("True".into(), 0), ("False".into(), 0)]),
        Type::Nil => Some(vec![("Nil".into(), 0)]),
        Type::App { def, .. } => {
            let info = store.defs.get(def)?;
            if info.variants.is_empty() {
                return None;
            }
            Some(
                info.variants
                    .iter()
                    .map(|v| (v.name.clone(), v.fields.len()))
                    .collect(),
            )
        }
        _ => None,
    }
}

fn ctor_fields(store: &TypeStore, ty: &Type, name: &str, arity: usize) -> Vec<Type> {
    if let Type::App { def, args } = ty {
        if let Some(info) = store.defs.get(def) {
            if let Some(v) = info.variants.iter().find(|v| v.name == name) {
                // RigidIds in imported TypeDefInfo may not exist in this store's
                // `rigids` map (they were allocated in the stub/prelude store).
                // Fall back to the order of first appearance of distinct Rigids
                // across this def's variants, which matches `params` order.
                let mut rigid_order: Vec<crate::ty::RigidId> = Vec::new();
                for vv in &info.variants {
                    for f in &vv.fields {
                        if let Type::Rigid(rid) = &f.ty {
                            if !rigid_order.contains(rid) {
                                rigid_order.push(*rid);
                            }
                        }
                    }
                }
                return v
                    .fields
                    .iter()
                    .map(|f| match &f.ty {
                        Type::Rigid(id) => {
                            if let Some(r) = store.rigids.get(id) {
                                if let Some(idx) = info.params.iter().position(|p| *p == r.name) {
                                    return args
                                        .get(idx)
                                        .map(|t| (**t).clone())
                                        .unwrap_or(Type::Error);
                                }
                            }
                            if let Some(idx) = rigid_order.iter().position(|r| r == id) {
                                return args.get(idx).map(|t| (**t).clone()).unwrap_or(Type::Error);
                            }
                            if info.params.len() == 1 {
                                return args.first().map(|t| (**t).clone()).unwrap_or(Type::Error);
                            }
                            f.ty.clone()
                        }
                        other => other.clone(),
                    })
                    .collect();
            }
        }
    }
    (0..arity).map(|_| Type::Error).collect()
}

fn expand_clause(clause: &Clause, n: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    for prow in &clause.patterns {
        let mut nodes: Vec<Pat> = prow.patterns.iter().map(ast_pat).collect();
        nodes.resize(
            n,
            Pat {
                head: Head::Wildcard,
            },
        );
        nodes.truncate(n);
        rows.push(nodes);
    }
    rows
}

fn ast_pat(p: &Pattern) -> Pat {
    match &p.kind {
        PatternKind::Discard | PatternKind::Var(_) | PatternKind::UnderscoreName(_) => Pat {
            head: Head::Wildcard,
        },
        PatternKind::Alias { pattern, .. } => ast_pat(pattern),
        PatternKind::Int(lit) => Pat {
            head: Head::Lit(LitKind::Int(lit.digits.clone())),
        },
        PatternKind::Float(lit) => Pat {
            head: Head::Lit(LitKind::Float(lit.raw.clone())),
        },
        PatternKind::String(lit) => Pat {
            head: Head::Lit(LitKind::String(lit.value.clone())),
        },
        PatternKind::Tuple(ps) => Pat {
            head: Head::Tuple(ps.iter().map(ast_pat).collect()),
        },
        PatternKind::List { items, spread } => {
            if items.is_empty() && spread.is_none() {
                Pat {
                    head: Head::ListNil,
                }
            } else {
                Pat {
                    head: Head::ListCons {
                        heads: items.iter().map(ast_pat).collect(),
                        has_spread: spread.is_some(),
                    },
                }
            }
        }
        PatternKind::Constructor { constructor, args } => {
            let fields = match args {
                Some(args) => args
                    .iter()
                    .filter(|a| !a.spread)
                    .map(|a| {
                        a.pattern.as_ref().map(ast_pat).unwrap_or(Pat {
                            head: Head::Wildcard,
                        })
                    })
                    .collect(),
                None => vec![],
            };
            Pat {
                head: Head::Ctor {
                    name: constructor.name.text.clone(),
                    fields,
                },
            }
        }
        PatternKind::StringPrefix { .. } | PatternKind::BitArray(_) => Pat {
            head: Head::Infinite,
        },
    }
}

fn spec_ctor(matrix: &[Row], name: &str, arity: usize) -> Vec<Row> {
    let mut out = Vec::new();
    for row in matrix {
        if row.is_empty() {
            continue;
        }
        match &row[0].head {
            Head::Ctor {
                name: n, fields, ..
            } if n == name => {
                let mut r = fields.clone();
                r.resize(
                    arity,
                    Pat {
                        head: Head::Wildcard,
                    },
                );
                r.truncate(arity);
                r.extend_from_slice(&row[1..]);
                out.push(r);
            }
            Head::Wildcard => {
                let mut r: Vec<Pat> = (0..arity)
                    .map(|_| Pat {
                        head: Head::Wildcard,
                    })
                    .collect();
                r.extend_from_slice(&row[1..]);
                out.push(r);
            }
            _ => {}
        }
    }
    out
}

fn spec_tuple(matrix: &[Row], arity: usize) -> Vec<Row> {
    let mut out = Vec::new();
    for row in matrix {
        if row.is_empty() {
            continue;
        }
        match &row[0].head {
            Head::Tuple(elems) => {
                let mut r = elems.clone();
                r.resize(
                    arity,
                    Pat {
                        head: Head::Wildcard,
                    },
                );
                r.truncate(arity);
                r.extend_from_slice(&row[1..]);
                out.push(r);
            }
            Head::Wildcard => {
                let mut r: Vec<Pat> = (0..arity)
                    .map(|_| Pat {
                        head: Head::Wildcard,
                    })
                    .collect();
                r.extend_from_slice(&row[1..]);
                out.push(r);
            }
            _ => {}
        }
    }
    out
}

fn spec_list_nil(matrix: &[Row]) -> Vec<Row> {
    let mut out = Vec::new();
    for row in matrix {
        if row.is_empty() {
            continue;
        }
        match &row[0].head {
            Head::ListNil | Head::Wildcard => out.push(row[1..].to_vec()),
            Head::ListCons { heads, has_spread } if heads.is_empty() && *has_spread => {
                out.push(row[1..].to_vec());
            }
            _ => {}
        }
    }
    out
}

fn spec_list_cons(matrix: &[Row], n_heads: usize, _has_spread: bool) -> Vec<Row> {
    let mut out = Vec::new();
    for row in matrix {
        if row.is_empty() {
            continue;
        }
        match &row[0].head {
            Head::ListCons { heads, has_spread } => {
                let mut r = heads.clone();
                while r.len() < n_heads {
                    r.push(Pat {
                        head: Head::Wildcard,
                    });
                }
                let rest = r.split_off(n_heads.min(r.len()));
                r.truncate(n_heads);
                if rest.is_empty() {
                    r.push(Pat {
                        head: if *has_spread {
                            Head::Wildcard
                        } else {
                            Head::ListNil
                        },
                    });
                } else {
                    r.push(Pat {
                        head: Head::ListCons {
                            heads: rest,
                            has_spread: *has_spread,
                        },
                    });
                }
                r.extend_from_slice(&row[1..]);
                out.push(r);
            }
            Head::Wildcard => {
                let mut r: Vec<Pat> = (0..n_heads)
                    .map(|_| Pat {
                        head: Head::Wildcard,
                    })
                    .collect();
                r.push(Pat {
                    head: Head::Wildcard,
                });
                r.extend_from_slice(&row[1..]);
                out.push(r);
            }
            _ => {}
        }
    }
    out
}

fn spec_lit(matrix: &[Row], lit: &LitKind) -> Vec<Row> {
    let mut out = Vec::new();
    for row in matrix {
        if row.is_empty() {
            continue;
        }
        match &row[0].head {
            Head::Lit(l) if l == lit => out.push(row[1..].to_vec()),
            Head::Wildcard => out.push(row[1..].to_vec()),
            _ => {}
        }
    }
    out
}

fn default_matrix(matrix: &[Row]) -> Vec<Row> {
    matrix
        .iter()
        .filter_map(|row| {
            if row.is_empty() {
                return None;
            }
            match &row[0].head {
                Head::Wildcard => Some(row[1..].to_vec()),
                _ => None,
            }
        })
        .collect()
}

fn missing_witness(store: &TypeStore, matrix: &[Row], tys: &[Type]) -> Option<String> {
    if tys.is_empty() {
        return Some("_".into());
    }
    let ty = follow(store, &tys[0]);
    if let Some(ctors) = complete_sig(store, &ty) {
        for (name, arity) in ctors {
            let m = spec_ctor(matrix, &name, arity);
            // If specialized matrix doesn't cover wildcards for remaining...
            // Simplified witness: constructor name if no row mentions it at top level.
            let covered = matrix.iter().any(|r| {
                !r.is_empty() && matches!(&r[0].head, Head::Ctor { name: n, .. } if *n == name)
                    || matches!(&r[0].head, Head::Wildcard)
            });
            let _ = m;
            if !covered {
                return Some(if arity == 0 {
                    name
                } else {
                    format!("{name}(..)")
                });
            }
            // Sub-pattern gap
            if arity > 0 {
                let only_wild = matrix.iter().all(|r| {
                    r.is_empty()
                        || matches!(&r[0].head, Head::Wildcard)
                        || matches!(&r[0].head, Head::Ctor { name: n, fields } if *n == name && fields.iter().all(|f| matches!(f.head, Head::Wildcard)))
                });
                if !only_wild {
                    // Check if some ctor specialization leaves a gap — use name(..)
                    let m = spec_ctor(matrix, &name, arity);
                    let wild_rest: Row = (0..arity + tys.len() - 1)
                        .map(|_| Pat {
                            head: Head::Wildcard,
                        })
                        .collect();
                    let mut ntys = ctor_fields(store, &ty, &name, arity);
                    ntys.extend_from_slice(&tys[1..]);
                    // Can't call useful without &mut self — approximate:
                    if m.is_empty() {
                        return Some(format!("{name}(..)"));
                    }
                    let _ = wild_rest;
                }
            }
        }
        // Nested gap common case: Some(True) missing Some(False)
        for (name, arity) in complete_sig(store, &ty).unwrap_or_default() {
            if arity == 0 {
                continue;
            }
            let m = spec_ctor(matrix, &name, arity);
            let ftys = ctor_fields(store, &ty, &name, arity);
            if let Some(w) = missing_witness(store, &m, &{
                let mut t = ftys.clone();
                t.extend_from_slice(&tys[1..]);
                t
            }) {
                if w != "_" {
                    return Some(format!("{name}({w})"));
                }
                // Check if first field type has a complete sig not covered
                if let Some(inner) = ftys.first() {
                    if let Some(sig) = complete_sig(store, inner) {
                        for (iname, _) in sig {
                            let covered = m.iter().any(|r| {
                                !r.is_empty()
                                    && (matches!(&r[0].head, Head::Ctor { name: n, .. } if *n == iname)
                                        || matches!(&r[0].head, Head::Wildcard))
                            });
                            if !covered {
                                return Some(format!("{name}({iname})"));
                            }
                        }
                    }
                }
            }
        }
        return Some("_".into());
    }
    match &ty {
        Type::List(_) => {
            let has_nil = matrix.iter().any(|r| {
                !r.is_empty()
                    && (matches!(r[0].head, Head::ListNil | Head::Wildcard)
                        || matches!(&r[0].head, Head::ListCons { heads, has_spread } if heads.is_empty() && *has_spread))
            });
            let has_cons = matrix.iter().any(|r| {
                !r.is_empty() && (matches!(r[0].head, Head::ListCons { .. } | Head::Wildcard))
            });
            if !has_nil {
                return Some("[]".into());
            }
            if !has_cons {
                return Some("[_, ..]".into());
            }
            Some("_".into())
        }
        Type::Tuple(_) => Some("#(..)".into()),
        _ => Some("_".into()),
    }
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
        | ExprKind::Constructor(_) => true,
        ExprKind::Paren(inner) => guard_ok(inner),
        ExprKind::Field { base, .. } => guard_ok(base),
        ExprKind::Unary {
            op: UnaryOp::Neg | UnaryOp::Not,
            expr,
        } => guard_ok(expr),
        ExprKind::Binary { left, right, .. } => guard_ok(left) && guard_ok(right),
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
        PatternKind::BitArray(segs) => {
            for s in segs {
                collect_vars(&s.pattern, out);
            }
        }
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

/// Soundness helper kept for tests.
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

#[cfg(test)]
mod budget_tests {
    use super::*;
    use crate::diag::TypeSink;
    use crate::ty::TypeStore;

    #[test]
    fn usefulness_respects_budget() {
        let mut store = TypeStore::new();
        let mut sink = TypeSink::new(0);
        let mut ex = ExhaustChecker {
            store: &mut store,
            sink: &mut sink,
            work: MAX_EXHAUST_WORK, // already at the limit
        };
        let matrix: Vec<Row> = vec![];
        let row = vec![Pat {
            head: Head::Wildcard,
        }];
        let tys = vec![Type::Bool];
        assert!(matches!(
            ex.useful(&matrix, &row, &tys),
            Err(Budget::Exhausted)
        ));
    }
}
