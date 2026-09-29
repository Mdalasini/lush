//! Stack-safe [`Clone`] / [`Drop`] for deep expression trees.
//!
//! A left-nested 4096-term `1 + 1 + …` chain is ~4096 `Box<Expr>` frames. The
//! derived `Clone`/`Drop` recurse on that spine and overflow the native stack.
//! These impls walk with an explicit heap stack instead.

use std::collections::HashMap;

use crate::ast::*;

impl Clone for Expr {
    fn clone(&self) -> Self {
        clone_expr(self)
    }
}

impl Drop for Expr {
    fn drop(&mut self) {
        let mut stack: Vec<Expr> = Vec::new();
        drain_expr(self, &mut stack);
        while let Some(mut e) = stack.pop() {
            drain_expr(&mut e, &mut stack);
        }
    }
}

fn drain_expr(expr: &mut Expr, stack: &mut Vec<Expr>) {
    let kind = std::mem::replace(&mut expr.kind, ExprKind::Todo { message: None });
    match kind {
        ExprKind::Tuple(xs) => stack.extend(xs),
        ExprKind::List { items, spread } => {
            stack.extend(items);
            if let Some(s) = spread {
                stack.push(*s);
            }
        }
        ExprKind::BitArray(segs) => {
            for s in segs {
                stack.push(s.value);
                for opt in s.options {
                    if let BitOption::Size(e) = opt {
                        stack.push(e);
                    }
                }
            }
        }
        ExprKind::RecordUpdate { base, fields, .. } => {
            stack.push(*base);
            for (_, e) in fields {
                stack.push(e);
            }
        }
        ExprKind::Call { callee, args } => {
            stack.push(*callee);
            for a in args {
                if let ArgValue::Expr(e) = a.value {
                    stack.push(e);
                }
            }
        }
        ExprKind::Field { base, .. } => stack.push(*base),
        ExprKind::Binary { left, right, .. } | ExprKind::Pipe { left, right } => {
            stack.push(*left);
            stack.push(*right);
        }
        ExprKind::Unary { expr, .. }
        | ExprKind::Paren(expr)
        | ExprKind::Echo(expr)
        | ExprKind::Assert { expr, .. } => stack.push(*expr),
        ExprKind::Fn { body, .. } => drain_block(body, stack),
        ExprKind::Block(body) => drain_block(body, stack),
        ExprKind::Case { subjects, clauses } => {
            stack.extend(subjects);
            for c in clauses {
                for row in c.patterns {
                    for p in row.patterns {
                        drain_pattern_tree(p, stack);
                    }
                }
                if let Some(g) = c.guard {
                    stack.push(g);
                }
                stack.push(c.body);
            }
        }
        other => drop(other),
    }
}

fn drain_block(block: Block, stack: &mut Vec<Expr>) {
    for s in block.statements {
        match s {
            Statement::Expr(e) => stack.push(e),
            Statement::Let(l) => {
                drain_pattern_tree(l.pattern, stack);
                stack.push(l.value);
            }
            Statement::Use(u) => {
                for p in u.patterns {
                    drain_pattern_tree(p, stack);
                }
                stack.push(u.value);
            }
            Statement::Fn(f) => drain_block(f.body, stack),
        }
    }
}

fn drain_pattern_tree(pat: Pattern, stack: &mut Vec<Expr>) {
    let mut pstack = vec![pat];
    while let Some(p) = pstack.pop() {
        match p.kind {
            PatternKind::Constructor {
                args: Some(args), ..
            } => {
                for a in args {
                    if let Some(inner) = a.pattern {
                        pstack.push(inner);
                    }
                }
            }
            PatternKind::Tuple(ps) => pstack.extend(ps),
            PatternKind::List { items, spread } => {
                pstack.extend(items);
                if let Some(s) = spread {
                    pstack.push(*s);
                }
            }
            PatternKind::BitArray(segs) => {
                for s in segs {
                    pstack.push(s.pattern);
                    for opt in s.options {
                        if let BitOption::Size(e) = opt {
                            stack.push(e);
                        }
                    }
                }
            }
            PatternKind::StringPrefix { rest, .. } | PatternKind::Alias { pattern: rest, .. } => {
                pstack.push(*rest);
            }
            _ => {}
        }
    }
}

fn clone_expr(root: &Expr) -> Expr {
    let mut done: HashMap<*const Expr, Expr> = HashMap::new();
    let mut stack: Vec<(*const Expr, bool)> = vec![(root as *const Expr, false)];
    while let Some((ptr, visited)) = stack.pop() {
        let src = unsafe { &*ptr };
        if !visited {
            stack.push((ptr, true));
            push_expr_children(src, &mut stack);
            continue;
        }
        let kind = clone_kind(src, &mut done);
        done.insert(
            ptr,
            Expr {
                kind,
                span: src.span,
                id: src.id,
            },
        );
    }
    done.remove(&(root as *const Expr)).expect("root cloned")
}

fn push_expr_children(e: &Expr, stack: &mut Vec<(*const Expr, bool)>) {
    match &e.kind {
        ExprKind::Tuple(xs) => {
            for x in xs {
                stack.push((x as *const Expr, false));
            }
        }
        ExprKind::List { items, spread } => {
            for x in items {
                stack.push((x as *const Expr, false));
            }
            if let Some(s) = spread {
                stack.push((s.as_ref() as *const Expr, false));
            }
        }
        ExprKind::BitArray(segs) => {
            for s in segs {
                stack.push((&s.value as *const Expr, false));
                for opt in &s.options {
                    if let BitOption::Size(e) = opt {
                        stack.push((e as *const Expr, false));
                    }
                }
            }
        }
        ExprKind::RecordUpdate { base, fields, .. } => {
            stack.push((base.as_ref() as *const Expr, false));
            for (_, e) in fields {
                stack.push((e as *const Expr, false));
            }
        }
        ExprKind::Call { callee, args } => {
            stack.push((callee.as_ref() as *const Expr, false));
            for a in args {
                if let ArgValue::Expr(e) = &a.value {
                    stack.push((e as *const Expr, false));
                }
            }
        }
        ExprKind::Field { base, .. } => stack.push((base.as_ref() as *const Expr, false)),
        ExprKind::Binary { left, right, .. } | ExprKind::Pipe { left, right } => {
            stack.push((right.as_ref() as *const Expr, false));
            stack.push((left.as_ref() as *const Expr, false));
        }
        ExprKind::Unary { expr, .. }
        | ExprKind::Paren(expr)
        | ExprKind::Echo(expr)
        | ExprKind::Assert { expr, .. } => {
            stack.push((expr.as_ref() as *const Expr, false));
        }
        ExprKind::Case { subjects, clauses } => {
            for s in subjects {
                stack.push((s as *const Expr, false));
            }
            for c in clauses {
                if let Some(g) = &c.guard {
                    stack.push((g as *const Expr, false));
                }
                stack.push((&c.body as *const Expr, false));
            }
        }
        ExprKind::Fn { body, .. } | ExprKind::Block(body) => push_block_exprs(body, stack),
        _ => {}
    }
}

fn push_block_exprs(block: &Block, stack: &mut Vec<(*const Expr, bool)>) {
    for s in &block.statements {
        match s {
            Statement::Expr(e) => stack.push((e as *const Expr, false)),
            Statement::Let(l) => stack.push((&l.value as *const Expr, false)),
            Statement::Use(u) => stack.push((&u.value as *const Expr, false)),
            Statement::Fn(f) => push_block_exprs(&f.body, stack),
        }
    }
}

fn take_child(done: &mut HashMap<*const Expr, Expr>, e: &Expr) -> Expr {
    done.remove(&(e as *const Expr))
        .expect("child must be cloned before parent")
}

fn clone_kind(src: &Expr, done: &mut HashMap<*const Expr, Expr>) -> ExprKind {
    match &src.kind {
        ExprKind::Int(v) => ExprKind::Int(v.clone()),
        ExprKind::Float(v) => ExprKind::Float(v.clone()),
        ExprKind::String(v) => ExprKind::String(v.clone()),
        ExprKind::Var(v) => ExprKind::Var(v.clone()),
        ExprKind::Constructor(v) => ExprKind::Constructor(v.clone()),
        ExprKind::Todo { message } => ExprKind::Todo {
            message: message.clone(),
        },
        ExprKind::Panic { message } => ExprKind::Panic {
            message: message.clone(),
        },
        ExprKind::Tuple(xs) => ExprKind::Tuple(xs.iter().map(|x| take_child(done, x)).collect()),
        ExprKind::List { items, spread } => ExprKind::List {
            items: items.iter().map(|x| take_child(done, x)).collect(),
            spread: spread.as_ref().map(|s| Box::new(take_child(done, s))),
        },
        ExprKind::BitArray(segs) => ExprKind::BitArray(
            segs.iter()
                .map(|s| BitSegment {
                    value: take_child(done, &s.value),
                    options: s
                        .options
                        .iter()
                        .map(|o| match o {
                            BitOption::Size(e) => BitOption::Size(take_child(done, e)),
                            BitOption::Named(n) => BitOption::Named(n.clone()),
                        })
                        .collect(),
                    span: s.span,
                })
                .collect(),
        ),
        ExprKind::RecordUpdate {
            constructor,
            base,
            fields,
        } => ExprKind::RecordUpdate {
            constructor: constructor.clone(),
            base: Box::new(take_child(done, base)),
            fields: fields
                .iter()
                .map(|(n, e)| (n.clone(), take_child(done, e)))
                .collect(),
        },
        ExprKind::Call { callee, args } => ExprKind::Call {
            callee: Box::new(take_child(done, callee)),
            args: args
                .iter()
                .map(|a| Arg {
                    label: a.label.clone(),
                    value: match &a.value {
                        ArgValue::Expr(e) => ArgValue::Expr(take_child(done, e)),
                        ArgValue::Hole => ArgValue::Hole,
                    },
                    span: a.span,
                })
                .collect(),
        },
        ExprKind::Field { base, field } => ExprKind::Field {
            base: Box::new(take_child(done, base)),
            field: field.clone(),
        },
        ExprKind::Binary { left, op, right } => ExprKind::Binary {
            left: Box::new(take_child(done, left)),
            op: *op,
            right: Box::new(take_child(done, right)),
        },
        ExprKind::Unary { op, expr } => ExprKind::Unary {
            op: *op,
            expr: Box::new(take_child(done, expr)),
        },
        ExprKind::Pipe { left, right } => ExprKind::Pipe {
            left: Box::new(take_child(done, left)),
            right: Box::new(take_child(done, right)),
        },
        ExprKind::Fn {
            params,
            return_type,
            body,
        } => ExprKind::Fn {
            params: params.clone(),
            return_type: return_type.clone(),
            body: clone_block(body, done),
        },
        ExprKind::Block(body) => ExprKind::Block(clone_block(body, done)),
        ExprKind::Case { subjects, clauses } => ExprKind::Case {
            subjects: subjects.iter().map(|s| take_child(done, s)).collect(),
            clauses: clauses
                .iter()
                .map(|c| Clause {
                    patterns: c.patterns.clone(),
                    guard: c.guard.as_ref().map(|g| take_child(done, g)),
                    body: take_child(done, &c.body),
                    span: c.span,
                })
                .collect(),
        },
        ExprKind::Assert { expr, message } => ExprKind::Assert {
            expr: Box::new(take_child(done, expr)),
            message: message.clone(),
        },
        ExprKind::Echo(e) => ExprKind::Echo(Box::new(take_child(done, e))),
        ExprKind::Paren(e) => ExprKind::Paren(Box::new(take_child(done, e))),
    }
}

fn clone_block(body: &Block, done: &mut HashMap<*const Expr, Expr>) -> Block {
    Block {
        statements: body
            .statements
            .iter()
            .map(|s| match s {
                Statement::Expr(e) => Statement::Expr(take_child(done, e)),
                Statement::Let(l) => Statement::Let(Box::new(LetStmt {
                    assert: l.assert,
                    pattern: l.pattern.clone(),
                    ty: l.ty.clone(),
                    value: take_child(done, &l.value),
                    message: l.message.clone(),
                    span: l.span,
                })),
                Statement::Use(u) => Statement::Use(UseStmt {
                    patterns: u.patterns.clone(),
                    value: take_child(done, &u.value),
                    span: u.span,
                }),
                Statement::Fn(f) => Statement::Fn(FnDef {
                    public: f.public,
                    name: f.name.clone(),
                    params: f.params.clone(),
                    return_type: f.return_type.clone(),
                    body: clone_block(&f.body, done),
                    span: f.span,
                }),
            })
            .collect(),
        span: body.span,
    }
}
