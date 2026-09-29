//! Post-desugar `NodeId` assignment for the typed-AST handoff.
//!
//! Walks the desugared module in source preorder and writes a dense id into every
//! [`Expr`] and [`Pattern`]. Ids start at 0; [`NodeId::NONE`] (`u32::MAX`) is never
//! assigned, so a valid id never collides with the sentinel.
//!
//! The walk is iterative (explicit stack) so a left-nested 4096-term `+` chain
//! does not overflow the native stack.

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
    // Preorder: number a node, then push children so the left spine is processed
    // next (stack is LIFO — push right then left).
    let mut stack: Vec<*mut Expr> = vec![expr as *mut Expr];
    while let Some(ptr) = stack.pop() {
        // SAFETY: every pointer was taken from a unique `&mut Expr` in this
        // module tree; we never alias two live mutable refs.
        let expr = unsafe { &mut *ptr };
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
                stack.push(e.as_mut() as *mut Expr);
            }
            ExprKind::Binary { left, right, .. } | ExprKind::Pipe { left, right } => {
                stack.push(right.as_mut() as *mut Expr);
                stack.push(left.as_mut() as *mut Expr);
            }
            ExprKind::Call { callee, args } => {
                for a in args.iter_mut().rev() {
                    if let ArgValue::Expr(e) = &mut a.value {
                        stack.push(e as *mut Expr);
                    }
                }
                stack.push(callee.as_mut() as *mut Expr);
            }
            ExprKind::Field { base, .. } => stack.push(base.as_mut() as *mut Expr),
            ExprKind::Tuple(xs) => {
                for x in xs.iter_mut().rev() {
                    stack.push(x as *mut Expr);
                }
            }
            ExprKind::List { items, spread } => {
                if let Some(s) = spread {
                    stack.push(s.as_mut() as *mut Expr);
                }
                for x in items.iter_mut().rev() {
                    stack.push(x as *mut Expr);
                }
            }
            ExprKind::Block(b) => assign_block(b, next_expr, next_pat),
            ExprKind::Case { subjects, clauses } => {
                // Patterns / guards / bodies interleave with subjects in source
                // order: subjects first, then each clause's patterns, guard, body.
                for cl in clauses.iter_mut().rev() {
                    stack.push(&mut cl.body as *mut Expr);
                    if let Some(g) = &mut cl.guard {
                        stack.push(g as *mut Expr);
                    }
                    for row in cl.patterns.iter_mut().rev() {
                        for p in row.patterns.iter_mut().rev() {
                            assign_pattern(p, next_expr, next_pat);
                        }
                    }
                }
                for s in subjects.iter_mut().rev() {
                    stack.push(s as *mut Expr);
                }
            }
            ExprKind::Assert { expr: e, .. } => stack.push(e.as_mut() as *mut Expr),
            ExprKind::Fn { body, .. } => assign_block(body, next_expr, next_pat),
            ExprKind::RecordUpdate { base, fields, .. } => {
                for (_, v) in fields.iter_mut().rev() {
                    stack.push(v as *mut Expr);
                }
                stack.push(base.as_mut() as *mut Expr);
            }
            ExprKind::BitArray(segs) => {
                for s in segs.iter_mut().rev() {
                    for opt in s.options.iter_mut().rev() {
                        if let BitOption::Size(e) = opt {
                            stack.push(e as *mut Expr);
                        }
                    }
                    stack.push(&mut s.value as *mut Expr);
                }
            }
        }
    }
}

fn assign_pattern(pat: &mut Pattern, next_expr: &mut u32, next_pat: &mut u32) {
    let mut stack: Vec<*mut Pattern> = vec![pat as *mut Pattern];
    while let Some(ptr) = stack.pop() {
        let pat = unsafe { &mut *ptr };
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
            | PatternKind::NegatedInt(_)
            | PatternKind::Float(_)
            | PatternKind::String(_)
            | PatternKind::Var(_)
            | PatternKind::Discard
            | PatternKind::UnderscoreName(_) => {}
            PatternKind::Constructor { args, .. } => {
                if let Some(pargs) = args {
                    for a in pargs.iter_mut().rev() {
                        if let Some(p) = &mut a.pattern {
                            stack.push(p as *mut Pattern);
                        }
                    }
                }
            }
            PatternKind::Tuple(ps) => {
                for p in ps.iter_mut().rev() {
                    stack.push(p as *mut Pattern);
                }
            }
            PatternKind::List { items, spread } => {
                if let Some(s) = spread {
                    stack.push(s.as_mut() as *mut Pattern);
                }
                for p in items.iter_mut().rev() {
                    stack.push(p as *mut Pattern);
                }
            }
            PatternKind::BitArray(segs) => {
                for s in segs.iter_mut().rev() {
                    for opt in s.options.iter_mut().rev() {
                        if let BitOption::Size(e) = opt {
                            assign_expr(e, next_expr, next_pat);
                        }
                    }
                    stack.push(&mut s.pattern as *mut Pattern);
                }
            }
            PatternKind::StringPrefix { rest, .. } => {
                stack.push(rest.as_mut() as *mut Pattern);
            }
            PatternKind::Alias { pattern, .. } => {
                stack.push(pattern.as_mut() as *mut Pattern);
            }
        }
    }
}
