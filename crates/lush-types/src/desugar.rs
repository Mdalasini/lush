//! `use` desugaring before type checking (`spec.md` §5.5).

use lush_syntax::ast::*;

/// Desugar `use` statements inside a module (recursive on blocks).
pub fn desugar_module(module: &mut Module) {
    for def in &mut module.definitions {
        if let Definition::Fn(f) = def {
            desugar_block(&mut f.body);
        }
    }
}

fn desugar_block(block: &mut Block) {
    // First, rewrite leading `use` chains into callback calls.
    if let Some(idx) = block
        .statements
        .iter()
        .position(|s| matches!(s, Statement::Use(_)))
    {
        let Statement::Use(use_stmt) = block.statements.remove(idx) else {
            unreachable!()
        };
        let rest: Vec<Statement> = block.statements.drain(idx..).collect();
        let mut callback_body = Block {
            statements: rest,
            span: use_stmt.span,
        };
        desugar_block(&mut callback_body);
        let params = use_stmt
            .patterns
            .iter()
            .map(pattern_to_param)
            .collect::<Vec<_>>();
        let callback = Expr {
            span: use_stmt.span,
            kind: ExprKind::Fn {
                params,
                return_type: None,
                body: callback_body,
            },
        };
        let call = append_callback_arg(use_stmt.value, callback);
        block.statements.push(Statement::Expr(call));
    }

    for stmt in &mut block.statements {
        match stmt {
            Statement::Fn(f) => desugar_block(&mut f.body),
            Statement::Let(l) => desugar_expr(&mut l.value),
            Statement::Expr(e) => desugar_expr(e),
            Statement::Use(_) => {}
        }
    }
}

fn desugar_expr(expr: &mut Expr) {
    match &mut expr.kind {
        ExprKind::Block(b) => desugar_block(b),
        ExprKind::Fn { body, .. } => desugar_block(body),
        ExprKind::Call { callee, args } => {
            desugar_expr(callee);
            for a in args {
                if let ArgValue::Expr(e) = &mut a.value {
                    desugar_expr(e);
                }
            }
        }
        ExprKind::Binary { left, right, .. } | ExprKind::Pipe { left, right } => {
            desugar_expr(left);
            desugar_expr(right);
        }
        ExprKind::Unary { expr, .. }
        | ExprKind::Echo { value: expr }
        | ExprKind::Group(expr)
        | ExprKind::Field { base: expr, .. } => desugar_expr(expr),
        ExprKind::Assert { condition, message } => {
            desugar_expr(condition);
            if let Some(m) = message {
                desugar_expr(m);
            }
        }
        ExprKind::Todo { message } | ExprKind::Panic { message } => {
            if let Some(m) = message {
                desugar_expr(m);
            }
        }
        ExprKind::Case { subjects, clauses } => {
            for s in subjects {
                desugar_expr(s);
            }
            for c in clauses {
                if let Some(g) = &mut c.guard {
                    desugar_expr(g);
                }
                desugar_expr(&mut c.body);
            }
        }
        ExprKind::List { items, spread } => {
            for i in items {
                desugar_expr(i);
            }
            if let Some(s) = spread {
                desugar_expr(s);
            }
        }
        ExprKind::Tuple(elems) => {
            for e in elems {
                desugar_expr(e);
            }
        }
        ExprKind::RecordUpdate { base, fields, .. } => {
            desugar_expr(base);
            for (_, v) in fields {
                desugar_expr(v);
            }
        }
        ExprKind::BitArray(segs) => {
            for seg in segs {
                if let BitSegmentValue::Expr(e) = &mut seg.value {
                    desugar_expr(e);
                }
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Ident(_)
        | ExprKind::Constructor(_) => {}
    }
}

fn pattern_to_param(pattern: &Pattern) -> Param {
    let name = match &pattern.kind {
        PatternKind::Var(n) => n.clone(),
        PatternKind::Discard => "_".into(),
        _ => "x".into(),
    };
    Param {
        label: None,
        name,
        ty: None,
        span: pattern.span,
    }
}

fn append_callback_arg(callee_expr: Expr, callback: Expr) -> Expr {
    match callee_expr.kind {
        ExprKind::Call { callee, mut args } => {
            args.push(Arg {
                label: None,
                value: ArgValue::Expr(callback.clone()),
                span: callback.span,
            });
            Expr {
                span: callee.span.merge(callback.span),
                kind: ExprKind::Call { callee, args },
            }
        }
        _ => Expr {
            span: callee_expr.span.merge(callback.span),
            kind: ExprKind::Call {
                callee: Box::new(callee_expr),
                args: vec![Arg {
                    label: None,
                    value: ArgValue::Expr(callback.clone()),
                    span: callback.span,
                }],
            },
        },
    }
}
