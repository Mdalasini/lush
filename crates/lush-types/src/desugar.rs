//! Desugaring of captures, pipes, and `use` before inference (§5.3, §5.5, §5.8).

use lush_syntax::ast::*;
use lush_syntax::span::Span;

use crate::codes;
use crate::diag::TypeSink;
use crate::limits::MAX_CHAIN;

/// Marker stored on desugared call args for the implicit `use` callback.
#[derive(Clone, Debug)]
pub struct DesugarMeta {
    /// True when this call originated from `use` and the last arg is an implicit callback.
    pub use_callback: bool,
    /// True when the call is in tail position.
    pub tail: bool,
}

/// Desugar a module in place. Returns capture gensym names used.
pub fn desugar_module(module: &mut Module, sink: &mut TypeSink) -> Vec<String> {
    let mut used_names = collect_all_names(module);
    let mut gensyms = Vec::new();
    for item in &mut module.items {
        match item {
            ModuleItem::Fn(f) => {
                desugar_block(&mut f.body, sink, &mut used_names, &mut gensyms, true)
            }
            ModuleItem::Const(c) => desugar_expr(&mut c.value, sink, &mut used_names, &mut gensyms),
            _ => {}
        }
    }
    gensyms
}

fn collect_all_names(module: &Module) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    for item in &module.items {
        match item {
            ModuleItem::Fn(f) => {
                set.insert(f.name.text.clone());
                collect_block_names(&f.body, &mut set);
            }
            ModuleItem::Const(c) => {
                set.insert(c.name.text.clone());
            }
            ModuleItem::Type(t) => {
                set.insert(t.name.text.clone());
                if let TypeDefBody::Adt(vs) = &t.body {
                    for v in vs {
                        set.insert(v.name.text.clone());
                    }
                }
            }
            ModuleItem::Import(i) => {
                if let Some(a) = &i.alias {
                    set.insert(a.text.clone());
                }
            }
        }
    }
    set
}

fn collect_block_names(b: &Block, set: &mut std::collections::HashSet<String>) {
    for s in &b.statements {
        match s {
            Statement::Fn(f) => {
                set.insert(f.name.text.clone());
                collect_block_names(&f.body, set);
            }
            Statement::Let(l) => collect_pat_names(&l.pattern, set),
            Statement::Use(u) => {
                for p in &u.patterns {
                    collect_pat_names(p, set);
                }
            }
            Statement::Expr(e) => collect_expr_names(e, set),
        }
    }
}

fn collect_pat_names(p: &Pattern, set: &mut std::collections::HashSet<String>) {
    match &p.kind {
        PatternKind::Var(n) | PatternKind::UnderscoreName(n) => {
            set.insert(n.text.clone());
        }
        PatternKind::Alias { pattern, name } => {
            set.insert(name.text.clone());
            collect_pat_names(pattern, set);
        }
        PatternKind::Tuple(ps)
        | PatternKind::List {
            items: ps,
            spread: None,
        } => {
            for p in ps {
                collect_pat_names(p, set);
            }
        }
        PatternKind::List { items, spread } => {
            for p in items {
                collect_pat_names(p, set);
            }
            if let Some(s) = spread {
                collect_pat_names(s, set);
            }
        }
        PatternKind::Constructor {
            args: Some(args), ..
        } => {
            for a in args {
                if let Some(p) = &a.pattern {
                    collect_pat_names(p, set);
                }
            }
        }
        PatternKind::StringPrefix { rest, .. } => collect_pat_names(rest, set),
        _ => {}
    }
}

fn collect_expr_names(e: &Expr, set: &mut std::collections::HashSet<String>) {
    match &e.kind {
        ExprKind::Fn { params, body, .. } => {
            for p in params {
                set.insert(p.name.text.clone());
            }
            collect_block_names(body, set);
        }
        ExprKind::Block(b) => collect_block_names(b, set),
        ExprKind::Call { callee, args } => {
            collect_expr_names(callee, set);
            for a in args {
                if let ArgValue::Expr(e) = &a.value {
                    collect_expr_names(e, set);
                }
            }
        }
        _ => {}
    }
}

fn fresh_capture_name(
    used: &mut std::collections::HashSet<String>,
    out: &mut Vec<String>,
) -> String {
    // Start at `out.len()` so the n-th gensym is O(1), not O(n) probes from 0
    // (which made long pipe chains O(n²) in desugar alone).
    let mut i = out.len() as u32;
    loop {
        let name = format!("__lush_cap_{i}");
        if used.insert(name.clone()) {
            out.push(name.clone());
            return name;
        }
        i = i.saturating_add(1);
    }
}

fn desugar_block(
    block: &mut Block,
    sink: &mut TypeSink,
    used: &mut std::collections::HashSet<String>,
    gensyms: &mut Vec<String>,
    _tail_block: bool,
) {
    // Process `use` from the end: each use consumes the remainder of the block.
    let mut i = 0;
    while i < block.statements.len() {
        if matches!(&block.statements[i], Statement::Use(_)) {
            let rest: Vec<Statement> = block.statements.drain(i + 1..).collect();
            let Statement::Use(use_stmt) = block.statements.remove(i) else {
                unreachable!()
            };
            let callback_body = Block {
                statements: rest,
                span: use_stmt.span,
            };
            let desugared = desugar_use(use_stmt, callback_body, sink, used, gensyms);
            block.statements.insert(i, Statement::Expr(desugared));
            // Continue desugaring inside (including nested uses in the callback — already in rest)
            if let Statement::Expr(e) = &mut block.statements[i] {
                desugar_expr(e, sink, used, gensyms);
            }
            break;
        }
        match &mut block.statements[i] {
            Statement::Fn(f) => desugar_block(&mut f.body, sink, used, gensyms, true),
            Statement::Let(l) => desugar_expr(&mut l.value, sink, used, gensyms),
            Statement::Expr(e) => desugar_expr(e, sink, used, gensyms),
            Statement::Use(_) => unreachable!(),
        }
        i += 1;
    }
}

fn desugar_use(
    use_stmt: UseStmt,
    callback_body: Block,
    sink: &mut TypeSink,
    used: &mut std::collections::HashSet<String>,
    gensyms: &mut Vec<String>,
) -> Expr {
    let span = use_stmt.span;
    // Check patterns are irrefutable — deferred to exhaust/irrefutability; soft check here
    for p in &use_stmt.patterns {
        if !is_irrefutable_syntax(p) {
            sink.error(
                codes::E1103_USE_REFUTABLE,
                "use patterns must be irrefutable",
                p.span,
                Some("use a variable, `_`, tuple of irrefutable patterns, or a single-variant constructor".into()),
            );
        }
    }
    let params: Vec<Param> = use_stmt
        .patterns
        .iter()
        .enumerate()
        .map(|(idx, p)| {
            let name = match &p.kind {
                PatternKind::Var(n) => n.clone(),
                PatternKind::Discard => Name {
                    text: fresh_capture_name(used, gensyms),
                    span: p.span,
                },
                PatternKind::UnderscoreName(n) => n.clone(),
                _ => Name {
                    text: format!("__lush_use_{idx}"),
                    span: p.span,
                },
            };
            used.insert(name.text.clone());
            Param {
                label: None,
                name,
                ty: None,
                span: p.span,
            }
        })
        .collect();
    let mut body = callback_body;
    if body.statements.is_empty() {
        body.statements.push(Statement::Expr(Expr {
            kind: ExprKind::Constructor(ConstructorRef {
                module: None,
                name: UName {
                    text: "Nil".into(),
                    span,
                },
                span,
            }),
            span,
        }));
    }
    desugar_block(&mut body, sink, used, gensyms, true);
    let callback = Expr {
        kind: ExprKind::Fn {
            params,
            return_type: None,
            body,
        },
        span,
    };
    // RHS must be call or function reference
    let mut rhs = use_stmt.value;
    desugar_expr(&mut rhs, sink, used, gensyms);
    match &mut rhs.kind {
        ExprKind::Call { args, .. } => {
            // If the final explicit argument already looks like a callback, the
            // implicit `use` parameter is already supplied.
            if let Some(last) = args.last() {
                if let ArgValue::Expr(e) = &last.value {
                    if matches!(e.kind, ExprKind::Fn { .. }) {
                        sink.error(
                            codes::E1104_USE_PARAM_SUPPLIED,
                            "the final parameter of this `use` call is already supplied",
                            last.span,
                            Some("remove the explicit callback or drop the `use`".into()),
                        );
                    }
                }
            }
            args.push(Arg {
                label: None,
                value: ArgValue::Expr(callback),
                span,
            });
            rhs
        }
        ExprKind::Var(_) | ExprKind::Field { .. } | ExprKind::Constructor(_) => {
            // Treat as call with no explicit args
            Expr {
                kind: ExprKind::Call {
                    callee: Box::new(rhs),
                    args: vec![Arg {
                        label: None,
                        value: ArgValue::Expr(callback),
                        span,
                    }],
                },
                span,
            }
        }
        _ => {
            sink.error(
                codes::E1102_USE_RHS,
                "`use` right-hand side must be a call or function reference",
                rhs.span,
                Some("write `use x <- f(args);` or `use x <- f;`".into()),
            );
            Expr {
                kind: ExprKind::Call {
                    callee: Box::new(rhs),
                    args: vec![Arg {
                        label: None,
                        value: ArgValue::Expr(callback),
                        span,
                    }],
                },
                span,
            }
        }
    }
}

fn is_irrefutable_syntax(p: &Pattern) -> bool {
    match &p.kind {
        PatternKind::Var(_) | PatternKind::Discard | PatternKind::UnderscoreName(_) => true,
        PatternKind::Alias { pattern, .. } => is_irrefutable_syntax(pattern),
        PatternKind::Tuple(ps) => ps.iter().all(is_irrefutable_syntax),
        // Constructor patterns are treated as potentially refutable at desugar time;
        // multi-variant ADTs are rejected here conservatively for `use`.
        PatternKind::Constructor { args: None, .. } => true, // nullary may still be refutable
        PatternKind::Constructor { .. } => false,
        _ => false,
    }
}

fn desugar_expr(
    expr: &mut Expr,
    sink: &mut TypeSink,
    used: &mut std::collections::HashSet<String>,
    gensyms: &mut Vec<String>,
) {
    match &mut expr.kind {
        ExprKind::Pipe { .. } => {
            // Flatten a left-associative pipe chain into one block of lets so
            // inference sees a single scope (nested pipe blocks were O(n²) in
            // environment depth). `((a |> f) |> g) |> h` →
            // `{ let c0 = a; let c1 = f(c0); let c2 = g(c1); h(c2); }`.
            let mut stages: Vec<Expr> = Vec::new();
            let mut cur = std::mem::replace(
                expr,
                Expr {
                    kind: ExprKind::Var(Name {
                        text: "_".into(),
                        span: Span::default(),
                    }),
                    span: Span::default(),
                },
            );
            while let ExprKind::Pipe { left, right } = cur.kind {
                stages.push(*right);
                cur = *left;
            }
            stages.reverse();
            desugar_expr(&mut cur, sink, used, gensyms);
            for stage in &mut stages {
                desugar_expr(stage, sink, used, gensyms);
            }
            if !chain_length_ok(stages.len(), sink, cur.span) {
                *expr = cur;
                return;
            }
            *expr = desugar_pipe_chain(cur, stages, sink, used, gensyms);
        }
        ExprKind::Call { callee, args } => {
            desugar_expr(callee, sink, used, gensyms);
            let hole_count = args
                .iter()
                .filter(|a| matches!(a.value, ArgValue::Hole))
                .count();
            // A `_` belongs to the innermost enclosing *call*. Nested captures such
            // as `f(g(_))` / `add(5, _)` are valid; a hole together with a nested
            // hole (`f(_, g(_))`) or a hole under a non-call (`f(1 + _)`) is not.
            let nested = args.iter().any(|a| {
                if let ArgValue::Expr(e) = &a.value {
                    contains_hole(e)
                } else {
                    false
                }
            });
            if hole_count >= 1 && nested {
                sink.error(
                    codes::E1101_NESTED_HOLE,
                    "nested placeholders are not allowed when the outer call also has `_`",
                    expr.span,
                    Some(
                        "each `_` belongs to the innermost call; write `f(g(_))` or `f(_)`, not `f(_, g(_))`"
                            .into(),
                    ),
                );
            } else if nested && hole_count == 0 {
                // Innermost-first: desugar argument expressions so nested captures
                // form before this call is considered.
                for a in args.iter_mut() {
                    if let ArgValue::Expr(e) = &mut a.value {
                        desugar_expr(e, sink, used, gensyms);
                    }
                }
            }
            if hole_count > 1 {
                sink.error(
                    codes::E1100_MULTI_HOLE,
                    "exactly one `_` placeholder is allowed in a capture",
                    expr.span,
                    Some("use a single `_` argument".into()),
                );
            }
            if hole_count == 1 && !nested {
                // Capture: add(5, _) => fn(x) { add(5, x); }
                let name = fresh_capture_name(used, gensyms);
                let mut new_args = Vec::new();
                for a in args.drain(..) {
                    match a.value {
                        ArgValue::Hole => {
                            new_args.push(Arg {
                                label: a.label,
                                value: ArgValue::Expr(Expr {
                                    kind: ExprKind::Var(Name {
                                        text: name.clone(),
                                        span: a.span,
                                    }),
                                    span: a.span,
                                }),
                                span: a.span,
                            });
                        }
                        ArgValue::Expr(mut e) => {
                            desugar_expr(&mut e, sink, used, gensyms);
                            new_args.push(Arg {
                                label: a.label,
                                value: ArgValue::Expr(e),
                                span: a.span,
                            });
                        }
                    }
                }
                let callee = std::mem::replace(
                    callee.as_mut(),
                    Expr {
                        kind: ExprKind::Var(Name {
                            text: "_".into(),
                            span: Span::default(),
                        }),
                        span: Span::default(),
                    },
                );
                let call = Expr {
                    kind: ExprKind::Call {
                        callee: Box::new(callee),
                        args: new_args,
                    },
                    span: expr.span,
                };
                *expr = Expr {
                    kind: ExprKind::Fn {
                        params: vec![Param {
                            label: None,
                            name: Name {
                                text: name,
                                span: expr.span,
                            },
                            ty: None,
                            span: expr.span,
                        }],
                        return_type: None,
                        body: Block {
                            statements: vec![Statement::Expr(call)],
                            span: expr.span,
                        },
                    },
                    span: expr.span,
                };
            } else if hole_count == 0 && !nested {
                for a in args.iter_mut() {
                    if let ArgValue::Expr(e) = &mut a.value {
                        desugar_expr(e, sink, used, gensyms);
                    }
                }
            } else if hole_count >= 1 {
                // Error path: still desugar remaining exprs for further diagnostics
                for a in args.iter_mut() {
                    if let ArgValue::Expr(e) = &mut a.value {
                        desugar_expr(e, sink, used, gensyms);
                    }
                }
            }
        }
        ExprKind::Binary { left, right, .. } => {
            desugar_expr(left, sink, used, gensyms);
            desugar_expr(right, sink, used, gensyms);
        }
        ExprKind::Field { base: left, .. } => {
            desugar_expr(left, sink, used, gensyms);
        }
        ExprKind::Unary { expr: inner, .. }
        | ExprKind::Paren(inner)
        | ExprKind::Echo(inner)
        | ExprKind::Assert { expr: inner, .. } => {
            desugar_expr(inner, sink, used, gensyms);
        }
        ExprKind::Tuple(items) | ExprKind::List { items, .. } => {
            for e in items {
                desugar_expr(e, sink, used, gensyms);
            }
            if let ExprKind::List {
                spread: Some(s), ..
            } = &mut expr.kind
            {
                desugar_expr(s, sink, used, gensyms);
            }
        }
        ExprKind::RecordUpdate { base, fields, .. } => {
            desugar_expr(base, sink, used, gensyms);
            for (_, e) in fields {
                desugar_expr(e, sink, used, gensyms);
            }
        }
        ExprKind::Fn { body, .. } => desugar_block(body, sink, used, gensyms, true),
        ExprKind::Block(b) => desugar_block(b, sink, used, gensyms, true),
        ExprKind::Case { subjects, clauses } => {
            for s in subjects {
                desugar_expr(s, sink, used, gensyms);
            }
            for c in clauses {
                if let Some(g) = &mut c.guard {
                    desugar_expr(g, sink, used, gensyms);
                }
                desugar_expr(&mut c.body, sink, used, gensyms);
            }
        }
        ExprKind::BitArray(segs) => {
            for s in segs {
                desugar_expr(&mut s.value, sink, used, gensyms);
            }
        }
        _ => {}
    }
}

fn contains_hole(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Call { args, callee } => {
            args.iter().any(|a| matches!(a.value, ArgValue::Hole))
                || contains_hole(callee)
                || args.iter().any(|a| {
                    if let ArgValue::Expr(e) = &a.value {
                        contains_hole(e)
                    } else {
                        false
                    }
                })
        }
        ExprKind::Pipe { left, right } => contains_hole(left) || contains_hole(right),
        ExprKind::Binary { left, right, .. } => contains_hole(left) || contains_hole(right),
        ExprKind::Unary { expr, .. }
        | ExprKind::Paren(expr)
        | ExprKind::Echo(expr)
        | ExprKind::Field { base: expr, .. } => contains_hole(expr),
        ExprKind::Tuple(xs) | ExprKind::List { items: xs, .. } => xs.iter().any(contains_hole),
        ExprKind::Fn { .. } => false,
        _ => false,
    }
}

fn pipe_apply(value: Expr, right: Expr, sink: &mut TypeSink) -> Expr {
    let value_span = value.span;
    let span = Span::new(value_span.start.as_usize(), right.span.end.as_usize());
    match right.kind {
        ExprKind::Call { callee, mut args } => {
            let hole_idx: Vec<usize> = args
                .iter()
                .enumerate()
                .filter(|(_, a)| matches!(a.value, ArgValue::Hole))
                .map(|(i, _)| i)
                .collect();
            if hole_idx.len() > 1 {
                sink.error(
                    codes::E1100_MULTI_HOLE,
                    "exactly one `_` placeholder is allowed in a piped call",
                    span,
                    None,
                );
            }
            if hole_idx.len() == 1 {
                let i = hole_idx[0];
                args[i].value = ArgValue::Expr(value);
            } else {
                args.insert(
                    0,
                    Arg {
                        label: None,
                        value: ArgValue::Expr(value),
                        span: value_span,
                    },
                );
            }
            Expr {
                kind: ExprKind::Call { callee, args },
                span,
            }
        }
        ExprKind::Var(_)
        | ExprKind::Field { .. }
        | ExprKind::Constructor(_)
        | ExprKind::Fn { .. } => Expr {
            kind: ExprKind::Call {
                callee: Box::new(right),
                args: vec![Arg {
                    label: None,
                    value: ArgValue::Expr(value),
                    span: value_span,
                }],
            },
            span,
        },
        _ => {
            sink.error(
                codes::E1105_BAD_PIPE_RHS,
                "pipe right-hand side must be a function, constructor, or call",
                right.span,
                None,
            );
            Expr {
                kind: ExprKind::Call {
                    callee: Box::new(right),
                    args: vec![Arg {
                        label: None,
                        value: ArgValue::Expr(value),
                        span: value_span,
                    }],
                },
                span,
            }
        }
    }
}

fn desugar_pipe_chain(
    init: Expr,
    stages: Vec<Expr>,
    sink: &mut TypeSink,
    used: &mut std::collections::HashSet<String>,
    gensyms: &mut Vec<String>,
) -> Expr {
    let span = Span::new(
        init.span.start.as_usize(),
        stages
            .last()
            .map(|s| s.span.end.as_usize())
            .unwrap_or(init.span.end.as_usize()),
    );
    let mut statements = Vec::with_capacity(stages.len() + 1);
    let mut current = init;
    for stage in stages {
        let bind_name = fresh_capture_name(used, gensyms);
        let current_span = current.span;
        let value_var = Expr {
            kind: ExprKind::Var(Name {
                text: bind_name.clone(),
                span: current_span,
            }),
            span: current_span,
        };
        statements.push(Statement::Let(Box::new(LetStmt {
            assert: false,
            pattern: Pattern {
                kind: PatternKind::Var(Name {
                    text: bind_name,
                    span: current_span,
                }),
                span: current_span,
            },
            ty: None,
            value: current,
            message: None,
            span: current_span,
        })));
        current = pipe_apply(value_var, stage, sink);
    }
    statements.push(Statement::Expr(current));
    Expr {
        kind: ExprKind::Block(Block { statements, span }),
        span,
    }
}

/// Count pipe/binary chain length for limit tests.
pub fn chain_length_ok(n: usize, sink: &mut TypeSink, span: Span) -> bool {
    if n > MAX_CHAIN {
        sink.error(
            codes::E1305_TOO_DEEP,
            format!("expression chain exceeds maximum length ({MAX_CHAIN})"),
            span,
            None,
        );
        false
    } else {
        true
    }
}
