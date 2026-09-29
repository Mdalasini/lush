//! Post-desugar `NodeId` assignment for the typed-AST handoff.
//!
//! Walks the desugared module in source preorder and writes a dense id into every
//! [`Expr`] and [`Pattern`]. Ids start at 0; [`NodeId::NONE`] (`u32::MAX`) is never
//! assigned, so a valid id never collides with the sentinel.

use lush_syntax::ast::*;

/// Assign unique dense [`NodeId`]s to every expression and pattern in `module`.
/// Returns `(expr_count, pattern_count)`.
pub fn assign_node_ids(module: &mut Module) -> (u32, u32) {
    let mut next_expr = 0u32;
    let mut next_pat = 0u32;
    for item in &mut module.items {
        match item {
            ModuleItem::Fn(f) => assign_fn_def(f, &mut next_expr, &mut next_pat),
            ModuleItem::Const(c) => {
                assign_expr(&mut c.value, &mut next_expr, &mut next_pat);
            }
            ModuleItem::Type(_) | ModuleItem::Import(_) => {}
        }
    }
    (next_expr, next_pat)
}

fn assign_fn_def(f: &mut FnDef, next_expr: &mut u32, next_pat: &mut u32) {
    assign_block(&mut f.body, next_expr, next_pat);
}

fn assign_block(block: &mut Block, next_expr: &mut u32, next_pat: &mut u32) {
    for s in &mut block.statements {
        assign_statement(s, next_expr, next_pat);
    }
}

fn assign_statement(stmt: &mut Statement, next_expr: &mut u32, next_pat: &mut u32) {
    match stmt {
        Statement::Expr(e) => assign_expr(e, next_expr, next_pat),
        Statement::Let(l) => {
            assign_pattern(&mut l.pattern, next_expr, next_pat);
            assign_expr(&mut l.value, next_expr, next_pat);
        }
        Statement::Use(u) => {
            for p in &mut u.patterns {
                assign_pattern(p, next_expr, next_pat);
            }
            assign_expr(&mut u.value, next_expr, next_pat);
        }
        Statement::Fn(f) => assign_fn_def(f, next_expr, next_pat),
    }
}

fn assign_expr(expr: &mut Expr, next_expr: &mut u32, next_pat: &mut u32) {
    debug_assert_eq!(
        expr.id,
        NodeId::NONE,
        "expression already numbered: {:?}",
        expr.id
    );
    expr.id = NodeId(*next_expr);
    *next_expr = next_expr.saturating_add(1);
    match &mut expr.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Var(_)
        | ExprKind::Constructor(_)
        | ExprKind::Todo { .. }
        | ExprKind::Panic { .. } => {}
        ExprKind::Paren(e) | ExprKind::Echo(e) | ExprKind::Unary { expr: e, .. } => {
            assign_expr(e, next_expr, next_pat);
        }
        ExprKind::Binary { left, right, .. } | ExprKind::Pipe { left, right } => {
            assign_expr(left, next_expr, next_pat);
            assign_expr(right, next_expr, next_pat);
        }
        ExprKind::Call { callee, args } => {
            assign_expr(callee, next_expr, next_pat);
            for a in args {
                if let ArgValue::Expr(e) = &mut a.value {
                    assign_expr(e, next_expr, next_pat);
                }
            }
        }
        ExprKind::Field { base, .. } => assign_expr(base, next_expr, next_pat),
        ExprKind::Tuple(xs) => {
            for x in xs {
                assign_expr(x, next_expr, next_pat);
            }
        }
        ExprKind::List { items, spread } => {
            for x in items {
                assign_expr(x, next_expr, next_pat);
            }
            if let Some(s) = spread {
                assign_expr(s, next_expr, next_pat);
            }
        }
        ExprKind::Block(b) => assign_block(b, next_expr, next_pat),
        ExprKind::Case { subjects, clauses } => {
            for s in subjects {
                assign_expr(s, next_expr, next_pat);
            }
            for cl in clauses {
                for row in &mut cl.patterns {
                    for p in &mut row.patterns {
                        assign_pattern(p, next_expr, next_pat);
                    }
                }
                if let Some(g) = &mut cl.guard {
                    assign_expr(g, next_expr, next_pat);
                }
                assign_expr(&mut cl.body, next_expr, next_pat);
            }
        }
        ExprKind::Assert { expr: e, .. } => assign_expr(e, next_expr, next_pat),
        ExprKind::Fn { body, .. } => assign_block(body, next_expr, next_pat),
        ExprKind::RecordUpdate { base, fields, .. } => {
            assign_expr(base, next_expr, next_pat);
            for (_, v) in fields {
                assign_expr(v, next_expr, next_pat);
            }
        }
        ExprKind::BitArray(segs) => {
            for s in segs {
                assign_expr(&mut s.value, next_expr, next_pat);
                for opt in &mut s.options {
                    if let BitOption::Size(e) = opt {
                        assign_expr(e, next_expr, next_pat);
                    }
                }
            }
        }
    }
}

fn assign_pattern(pat: &mut Pattern, next_expr: &mut u32, next_pat: &mut u32) {
    debug_assert_eq!(
        pat.id,
        NodeId::NONE,
        "pattern already numbered: {:?}",
        pat.id
    );
    pat.id = NodeId(*next_pat);
    *next_pat = next_pat.saturating_add(1);
    match &mut pat.kind {
        PatternKind::Int(_)
        | PatternKind::Float(_)
        | PatternKind::String(_)
        | PatternKind::Var(_)
        | PatternKind::Discard
        | PatternKind::UnderscoreName(_) => {}
        PatternKind::Constructor { args, .. } => {
            if let Some(pargs) = args {
                for a in pargs {
                    if let Some(p) = &mut a.pattern {
                        assign_pattern(p, next_expr, next_pat);
                    }
                }
            }
        }
        PatternKind::Tuple(ps) => {
            for p in ps {
                assign_pattern(p, next_expr, next_pat);
            }
        }
        PatternKind::List { items, spread } => {
            for p in items {
                assign_pattern(p, next_expr, next_pat);
            }
            if let Some(s) = spread {
                assign_pattern(s, next_expr, next_pat);
            }
        }
        PatternKind::BitArray(segs) => {
            for s in segs {
                assign_pattern(&mut s.pattern, next_expr, next_pat);
                for opt in &mut s.options {
                    if let BitOption::Size(e) = opt {
                        assign_expr(e, next_expr, next_pat);
                    }
                }
            }
        }
        PatternKind::StringPrefix { rest, .. } => assign_pattern(rest, next_expr, next_pat),
        PatternKind::Alias { pattern, .. } => assign_pattern(pattern, next_expr, next_pat),
    }
}
