//! Register bytecode emission from a typed module.

use std::collections::HashMap;

use lush_syntax::ast::*;
use lush_syntax::diagnostic::{Diagnostic, DiagnosticKind};
use lush_syntax::span::Span;
use lush_syntax::token::IntBase;
use lush_types::numeric;
use lush_types::typed::TypedModule;

use crate::bytecode::{Builtin, ConstId, Constant, FuncId, Function, Op, Program, Reg};
use crate::codes;
use crate::limits::{MAX_INSTRUCTIONS_PER_FUNCTION, MAX_REGISTERS};

struct ModuleEmitter<'a> {
    typed: &'a TypedModule,
    constants: Vec<Constant>,
    functions: Vec<Function>,
    func_ids: HashMap<(String, String), FuncId>,
    diagnostics: Vec<Diagnostic>,
}

struct FnEmitter<'a, 'm> {
    m: &'m mut ModuleEmitter<'a>,
    module: String,
    name: String,
    regs: u8,
    code: Vec<Op>,
    lines: Vec<(u32, u32)>,
    env: Vec<HashMap<String, Reg>>,
    arity: u8,
}

pub fn emit_program(
    modules: &[(String, TypedModule)],
    entry_path: &str,
) -> Result<Program, Vec<Diagnostic>> {
    let mut em = ModuleEmitter {
        typed: &modules[0].1, // overwritten per module
        constants: Vec::new(),
        functions: Vec::new(),
        func_ids: HashMap::new(),
        diagnostics: Vec::new(),
    };

    // First pass: assign FuncIds.
    for (path, typed) in modules {
        for item in &typed.module.items {
            if let ModuleItem::Fn(f) = item {
                let id = em.functions.len() as FuncId;
                em.func_ids.insert((path.clone(), f.name.text.clone()), id);
                em.functions.push(Function {
                    module: path.clone(),
                    name: f.name.text.clone(),
                    arity: f.params.len() as u8,
                    regs: 0,
                    code: vec![],
                    lines: vec![],
                    source_path: format!("src/{path}.lush"),
                });
            }
        }
    }

    // Second pass: emit bodies.
    for (path, typed) in modules {
        em.typed = typed;
        for item in &typed.module.items {
            if let ModuleItem::Fn(f) = item {
                let fid = *em
                    .func_ids
                    .get(&(path.clone(), f.name.text.clone()))
                    .expect("func id");
                let mut fe = FnEmitter {
                    m: &mut em,
                    module: path.clone(),
                    name: f.name.text.clone(),
                    regs: f.params.len() as u8,
                    code: vec![],
                    lines: vec![],
                    env: vec![HashMap::new()],
                    arity: f.params.len() as u8,
                };
                for (i, p) in f.params.iter().enumerate() {
                    fe.define(p.name.text.clone(), i as Reg);
                }
                if let Some(ret) = fe.emit_block(&f.body, true) {
                    // Always land a Return unless the body already ends in a
                    // real terminator. A trailing defensive `Panic` from `case`
                    // is not a normal exit — successful arms jump past it to
                    // this Return.
                    if !matches!(
                        fe.code.last(),
                        Some(Op::Return { .. } | Op::TailCall { .. } | Op::TailCallClosure { .. })
                    ) {
                        fe.emit(Op::Return { src: ret }, f.span);
                    }
                } else if !matches!(
                    fe.code.last(),
                    Some(
                        Op::Return { .. }
                            | Op::TailCall { .. }
                            | Op::TailCallClosure { .. }
                            | Op::Panic { .. }
                    )
                ) {
                    if let Some(r) = fe.fresh() {
                        fe.emit(Op::LoadNil { dst: r }, f.span);
                        fe.emit(Op::Return { src: r }, f.span);
                    }
                }
                let regs = fe.regs.max(fe.arity);
                let code = std::mem::take(&mut fe.code);
                let lines = std::mem::take(&mut fe.lines);
                let func = &mut em.functions[fid as usize];
                func.regs = regs;
                func.code = code;
                func.lines = lines;
            }
        }
    }

    if !em.diagnostics.is_empty() {
        return Err(em.diagnostics);
    }

    let entry = em
        .func_ids
        .get(&(entry_path.to_string(), "main".into()))
        .copied()
        .ok_or_else(|| {
            vec![Diagnostic::error(
                codes::E2010_NO_ENTRY,
                format!("entry module `{entry_path}` has no `main`"),
                Span::default(),
                None,
                DiagnosticKind::Type,
            )]
        })?;

    let program = Program {
        functions: em.functions,
        constants: em.constants,
        entry,
        sources: std::collections::BTreeMap::new(),
    };
    if let Err(msg) = program.verify() {
        return Err(vec![Diagnostic::error(
            codes::E2008_VERIFY,
            msg,
            Span::default(),
            None,
            DiagnosticKind::Type,
        )]);
    }
    Ok(program)
}

impl<'a, 'm> FnEmitter<'a, 'm> {
    fn error(&mut self, code: &str, msg: impl Into<String>, span: Span) {
        self.m.diagnostics.push(Diagnostic::error(
            code,
            msg.into(),
            span,
            None,
            DiagnosticKind::Type,
        ));
    }

    fn fresh(&mut self) -> Option<Reg> {
        if self.regs as usize >= MAX_REGISTERS {
            self.error(
                codes::E2001_TOO_MANY_REGISTERS,
                format!("function `{}` exceeds {MAX_REGISTERS} registers", self.name),
                Span::default(),
            );
            return None;
        }
        let r = self.regs;
        self.regs += 1;
        Some(r)
    }

    fn emit(&mut self, op: Op, span: Span) {
        if self.code.len() >= MAX_INSTRUCTIONS_PER_FUNCTION {
            self.error(
                codes::E2005_TOO_MANY_INSTRUCTIONS,
                format!("function `{}` exceeds instruction limit", self.name),
                span,
            );
            return;
        }
        self.code.push(op);
        self.lines.push((1, 1));
        let _ = span;
    }

    fn define(&mut self, name: String, r: Reg) {
        if let Some(scope) = self.env.last_mut() {
            scope.insert(name, r);
        }
    }

    fn lookup(&self, name: &str) -> Option<Reg> {
        for scope in self.env.iter().rev() {
            if let Some(r) = scope.get(name) {
                return Some(*r);
            }
        }
        None
    }

    fn push_scope(&mut self) {
        self.env.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.env.pop();
    }

    fn intern_string(&mut self, s: String) -> ConstId {
        if let Some(i) = self
            .m
            .constants
            .iter()
            .position(|c| matches!(c, Constant::String(t) if *t == s))
        {
            return i as ConstId;
        }
        let id = self.m.constants.len() as ConstId;
        self.m.constants.push(Constant::String(s));
        id
    }

    fn emit_block(&mut self, block: &Block, tail: bool) -> Option<Reg> {
        self.push_scope();
        let n = block.statements.len();
        let mut last = None;
        for (i, stmt) in block.statements.iter().enumerate() {
            let is_last = i + 1 == n;
            match stmt {
                Statement::Expr(e) => {
                    last = self.emit_expr(e, tail && is_last);
                }
                Statement::Let(l) => {
                    let v = self.emit_expr(&l.value, false)?;
                    self.bind_pattern(&l.pattern, v)?;
                    if is_last {
                        let r = self.fresh()?;
                        self.emit(Op::LoadNil { dst: r }, l.span);
                        last = Some(r);
                    }
                }
                Statement::Fn(_) => {
                    self.error(
                        codes::E2011_LOWER,
                        "local functions are not yet lowered in this checkpoint",
                        Span::default(),
                    );
                    return None;
                }
                Statement::Use(_) => {
                    self.error(
                        codes::E2011_LOWER,
                        "`use` should have been desugared before lowering",
                        Span::default(),
                    );
                    return None;
                }
            }
        }
        self.pop_scope();
        if last.is_none() {
            let r = self.fresh()?;
            self.emit(Op::LoadNil { dst: r }, block.span);
            Some(r)
        } else {
            last
        }
    }

    fn bind_pattern(&mut self, pat: &Pattern, src: Reg) -> Option<()> {
        match &pat.kind {
            PatternKind::Var(n) | PatternKind::UnderscoreName(n) => {
                let r = self.fresh()?;
                self.emit(Op::Move { dst: r, src }, pat.span);
                self.define(n.text.clone(), r);
                Some(())
            }
            PatternKind::Discard => Some(()),
            PatternKind::Tuple(ps) => {
                for (i, p) in ps.iter().enumerate() {
                    let r = self.fresh()?;
                    self.emit(
                        Op::GetField {
                            dst: r,
                            base: src,
                            index: i as u16,
                        },
                        p.span,
                    );
                    self.bind_pattern(p, r)?;
                }
                Some(())
            }
            _ => {
                self.error(
                    codes::E2011_LOWER,
                    "unsupported let pattern in this checkpoint",
                    pat.span,
                );
                None
            }
        }
    }

    fn emit_expr(&mut self, expr: &Expr, tail: bool) -> Option<Reg> {
        match &expr.kind {
            ExprKind::Int(lit) => {
                let base = match lit.base {
                    IntBase::Decimal => 10,
                    IntBase::Hex => 16,
                    IntBase::Octal => 8,
                    IntBase::Binary => 2,
                };
                let v = match numeric::int_literal_value(&lit.digits, base) {
                    Ok(v) => v,
                    Err(_) => return None,
                };
                let dst = self.fresh()?;
                self.emit(Op::LoadInt { dst, value: v }, expr.span);
                Some(dst)
            }
            ExprKind::Float(lit) => {
                let v = numeric::parse_float_literal(&lit.raw).ok()?;
                let idx = self.m.constants.len() as ConstId;
                self.m.constants.push(Constant::Float(v));
                let dst = self.fresh()?;
                self.emit(Op::LoadConst { dst, idx }, expr.span);
                Some(dst)
            }
            ExprKind::String(s) => {
                let idx = self.intern_string(s.value.clone());
                let dst = self.fresh()?;
                self.emit(Op::LoadConst { dst, idx }, expr.span);
                Some(dst)
            }
            ExprKind::Var(n) => {
                if let Some(r) = self.lookup(&n.text) {
                    return Some(r);
                }
                // True/False/Nil as constructors may appear as Var after resolve? Usually Constructor.
                match n.text.as_str() {
                    "True" => {
                        let dst = self.fresh()?;
                        self.emit(Op::LoadBool { dst, value: true }, expr.span);
                        Some(dst)
                    }
                    "False" => {
                        let dst = self.fresh()?;
                        self.emit(Op::LoadBool { dst, value: false }, expr.span);
                        Some(dst)
                    }
                    "Nil" => {
                        let dst = self.fresh()?;
                        self.emit(Op::LoadNil { dst }, expr.span);
                        Some(dst)
                    }
                    _ => {
                        self.error(
                            codes::E2011_LOWER,
                            format!("unresolved name `{}`", n.text),
                            n.span,
                        );
                        None
                    }
                }
            }
            ExprKind::Constructor(c) => match c.name.text.as_str() {
                "True" => {
                    let dst = self.fresh()?;
                    self.emit(Op::LoadBool { dst, value: true }, expr.span);
                    Some(dst)
                }
                "False" => {
                    let dst = self.fresh()?;
                    self.emit(Op::LoadBool { dst, value: false }, expr.span);
                    Some(dst)
                }
                "Nil" => {
                    let dst = self.fresh()?;
                    self.emit(Op::LoadNil { dst }, expr.span);
                    Some(dst)
                }
                name => {
                    let variant = match name {
                        "Ok" | "Some" => 0u16,
                        "Error" | "None" => 1u16,
                        _ => 0u16,
                    };
                    let dst = self.fresh()?;
                    self.emit(
                        Op::MakeAdt {
                            dst,
                            type_tag: 0,
                            variant,
                            fields: vec![],
                        },
                        expr.span,
                    );
                    Some(dst)
                }
            },
            ExprKind::Paren(e) => self.emit_expr(e, tail),
            ExprKind::Block(b) => self.emit_block(b, tail),
            ExprKind::Unary { op, expr: inner } => {
                let src = self.emit_expr(inner, false)?;
                let dst = self.fresh()?;
                match op {
                    UnaryOp::Neg => self.emit(Op::Neg { dst, src }, expr.span),
                    UnaryOp::Not => {
                        // !x => x == False
                        let f = self.fresh()?;
                        self.emit(
                            Op::LoadBool {
                                dst: f,
                                value: false,
                            },
                            expr.span,
                        );
                        self.emit(Op::Eq { dst, a: src, b: f }, expr.span);
                    }
                }
                Some(dst)
            }
            ExprKind::Binary { op, left, right } => {
                if matches!(op, BinOp::And | BinOp::Or) {
                    return self.emit_and_or(*op, left, right, expr.span, tail);
                }
                let a = self.emit_expr(left, false)?;
                let b = self.emit_expr(right, false)?;
                let dst = self.fresh()?;
                let opcode = match op {
                    BinOp::Add | BinOp::AddFloat => Op::Add { dst, a, b },
                    BinOp::Sub | BinOp::SubFloat => Op::Sub { dst, a, b },
                    BinOp::Mul | BinOp::MulFloat => Op::Mul { dst, a, b },
                    BinOp::Div | BinOp::DivFloat => Op::Div { dst, a, b },
                    BinOp::Rem => Op::Rem { dst, a, b },
                    BinOp::Concat => Op::Concat { dst, a, b },
                    BinOp::Eq => Op::Eq { dst, a, b },
                    BinOp::NotEq => Op::Ne { dst, a, b },
                    BinOp::Lt | BinOp::LtFloat => Op::Lt { dst, a, b },
                    BinOp::LtEq | BinOp::LtEqFloat => Op::Le { dst, a, b },
                    BinOp::Gt | BinOp::GtFloat => Op::Gt { dst, a, b },
                    BinOp::GtEq | BinOp::GtEqFloat => Op::Ge { dst, a, b },
                    BinOp::And | BinOp::Or => unreachable!(),
                };
                self.emit(opcode, expr.span);
                Some(dst)
            }
            ExprKind::Call { callee, args } => self.emit_call(callee, args, expr.span, tail),
            ExprKind::Case { subjects, clauses } => {
                self.emit_case(subjects, clauses, expr.span, tail)
            }
            ExprKind::Todo { message } | ExprKind::Panic { message } => {
                let msg = message
                    .as_ref()
                    .map(|s| s.value.clone())
                    .unwrap_or_else(|| {
                        if matches!(expr.kind, ExprKind::Todo { .. }) {
                            "todo".into()
                        } else {
                            "panic".into()
                        }
                    });
                let idx = self.intern_string(msg);
                self.emit(Op::Panic { msg: idx }, expr.span);
                self.fresh() // unreachable reg for type continuity
            }
            ExprKind::Assert { expr: e, message } => {
                let c = self.emit_expr(e, false)?;
                let after = self.code.len() as u32 + 3;
                self.emit(Op::JumpIfFalse { cond: c, target: 0 }, expr.span);
                let jmp_idx = self.code.len() - 1;
                let ok = self.fresh()?;
                self.emit(Op::LoadNil { dst: ok }, expr.span);
                self.emit(Op::Jump { target: after }, expr.span);
                let panic_pc = self.code.len() as u32;
                if let Op::JumpIfFalse { target, .. } = &mut self.code[jmp_idx] {
                    *target = panic_pc;
                }
                let msg = message
                    .as_ref()
                    .map(|s| s.value.clone())
                    .unwrap_or_else(|| "assert failed".into());
                let idx = self.intern_string(msg);
                self.emit(Op::Panic { msg: idx }, expr.span);
                // patch after
                let end = self.code.len() as u32;
                if let Op::Jump { target } = &mut self.code[jmp_idx + 2] {
                    *target = end;
                }
                Some(ok)
            }
            ExprKind::Echo(e) => {
                // echo: evaluate, builtin-less debug via eprintln of inspect — simplify: just yield value
                self.emit_expr(e, false)
            }
            ExprKind::Tuple(xs) => {
                let mut fields = Vec::new();
                for x in xs {
                    fields.push(self.emit_expr(x, false)?);
                }
                let dst = self.fresh()?;
                self.emit(Op::MakeTuple { dst, fields }, expr.span);
                Some(dst)
            }
            ExprKind::List { items, spread } => {
                let mut acc = if let Some(s) = spread {
                    self.emit_expr(s, false)?
                } else {
                    let r = self.fresh()?;
                    self.emit(Op::MakeEmptyList { dst: r }, expr.span);
                    r
                };
                for item in items.iter().rev() {
                    let h = self.emit_expr(item, false)?;
                    let dst = self.fresh()?;
                    self.emit(
                        Op::MakeCons {
                            dst,
                            head: h,
                            tail: acc,
                        },
                        expr.span,
                    );
                    acc = dst;
                }
                Some(acc)
            }
            other => {
                self.error(
                    codes::E2011_LOWER,
                    format!("expression form `{other:?}` not lowered in this checkpoint"),
                    expr.span,
                );
                None
            }
        }
    }

    fn emit_and_or(
        &mut self,
        op: BinOp,
        left: &Expr,
        right: &Expr,
        span: Span,
        tail: bool,
    ) -> Option<Reg> {
        let a = self.emit_expr(left, false)?;
        let dst = self.fresh()?;
        self.emit(Op::Move { dst, src: a }, span);
        // if And: if !a jump end; else evaluate right into dst
        // if Or: if a jump end; else evaluate right into dst
        let skip_jmp = self.code.len();
        if matches!(op, BinOp::And) {
            self.emit(Op::JumpIfFalse { cond: a, target: 0 }, span);
        } else {
            // jump if true: JumpIfFalse on !a — synthesize by jumping when a is true via inverted logic
            // Use: if a == False then evaluate right; else keep a
            let f = self.fresh()?;
            self.emit(
                Op::LoadBool {
                    dst: f,
                    value: false,
                },
                span,
            );
            let is_false = self.fresh()?;
            self.emit(
                Op::Eq {
                    dst: is_false,
                    a,
                    b: f,
                },
                span,
            );
            self.emit(
                Op::JumpIfFalse {
                    cond: is_false,
                    target: 0,
                },
                span,
            ); // if a is True, is_false=False, jump end
        }
        let b = self.emit_expr(right, tail)?;
        self.emit(Op::Move { dst, src: b }, span);
        let end = self.code.len() as u32;
        if let Op::JumpIfFalse { target, .. } = &mut self.code[skip_jmp] {
            *target = end;
        }
        // For Or the jump is the second JumpIfFalse
        if matches!(op, BinOp::Or) {
            if let Op::JumpIfFalse { target, .. } = &mut self.code[skip_jmp + 3] {
                *target = end;
            }
        }
        Some(dst)
    }

    fn emit_call(&mut self, callee: &Expr, args: &[Arg], span: Span, tail: bool) -> Option<Reg> {
        // Resolve callee to a function or builtin.
        let mut arg_regs = Vec::new();
        for a in args {
            match &a.value {
                ArgValue::Expr(e) => arg_regs.push(self.emit_expr(e, false)?),
                ArgValue::Hole => {
                    self.error(codes::E2011_LOWER, "capture hole survived desugaring", span);
                    return None;
                }
            }
        }

        match &callee.kind {
            ExprKind::Var(n) => {
                // Local? shouldn't be a call target usually.
                if let Some((module, name)) = self.resolve_fn(&n.text) {
                    return self.emit_direct_call(&module, &name, arg_regs, span, tail);
                }
                self.error(
                    codes::E2011_LOWER,
                    format!("cannot call unresolved `{}`", n.text),
                    span,
                );
                None
            }
            ExprKind::Field { base, field } => {
                // module.name or value.field — for builtins: io.println
                if let ExprKind::Var(m) = &base.kind {
                    let module = match m.text.as_str() {
                        "io" => "lush/io".to_string(),
                        "int" => "lush/int".to_string(),
                        other => {
                            // import alias — look up in typed resolutions later; try lush/ prefix
                            if let Some(path) = self.alias_path(other) {
                                path
                            } else {
                                other.to_string()
                            }
                        }
                    };
                    let name = field_name(field);
                    if module.starts_with("lush/") {
                        return self.emit_builtin(&module, &name, arg_regs, span);
                    }
                    return self.emit_direct_call(&module, &name, arg_regs, span, tail);
                }
                self.error(codes::E2011_LOWER, "unsupported callee form", span);
                None
            }
            ExprKind::Constructor(c) => {
                let variant = match c.name.text.as_str() {
                    "Ok" | "Some" => 0u16,
                    "Error" | "None" => 1u16,
                    _ => 0u16,
                };
                let dst = self.fresh()?;
                self.emit(
                    Op::MakeAdt {
                        dst,
                        type_tag: 0,
                        variant,
                        fields: arg_regs,
                    },
                    span,
                );
                Some(dst)
            }
            _ => {
                // Closure call
                let clo = self.emit_expr(callee, false)?;
                if tail {
                    self.emit(
                        Op::TailCallClosure {
                            clo,
                            args: arg_regs,
                        },
                        span,
                    );
                    self.fresh()
                } else {
                    let dst = self.fresh()?;
                    self.emit(
                        Op::CallClosure {
                            dst,
                            clo,
                            args: arg_regs,
                        },
                        span,
                    );
                    Some(dst)
                }
            }
        }
    }

    fn alias_path(&self, alias: &str) -> Option<String> {
        // From typed module imports.
        for item in &self.m.typed.module.items {
            if let ModuleItem::Import(imp) = item {
                if let Some(a) = &imp.alias {
                    if a.text == alias {
                        return Some(imp.path.segments.join("/"));
                    }
                }
                // unqualified last segment match
                if imp.alias.is_none() {
                    if let Some(last) = imp.path.segments.last() {
                        if last == alias {
                            return Some(imp.path.segments.join("/"));
                        }
                    }
                }
            }
        }
        None
    }

    fn resolve_fn(&self, name: &str) -> Option<(String, String)> {
        let key = (self.module.clone(), name.to_string());
        if self.m.func_ids.contains_key(&key) {
            return Some(key);
        }
        None
    }

    fn emit_direct_call(
        &mut self,
        module: &str,
        name: &str,
        args: Vec<Reg>,
        span: Span,
        tail: bool,
    ) -> Option<Reg> {
        let fid = match self.m.func_ids.get(&(module.to_string(), name.to_string())) {
            Some(id) => *id,
            None => {
                self.error(
                    codes::E2011_LOWER,
                    format!("unknown function `{module}.{name}`"),
                    span,
                );
                return None;
            }
        };
        if tail {
            self.emit(Op::TailCall { func: fid, args }, span);
            self.fresh()
        } else {
            let dst = self.fresh()?;
            self.emit(
                Op::Call {
                    dst,
                    func: fid,
                    args,
                },
                span,
            );
            Some(dst)
        }
    }

    fn emit_builtin(
        &mut self,
        module: &str,
        name: &str,
        args: Vec<Reg>,
        span: Span,
    ) -> Option<Reg> {
        let builtin = match (module, name) {
            ("lush/io", "print") => Builtin::Print,
            ("lush/io", "println") => Builtin::Println,
            ("lush/io", "eprintln") => Builtin::Eprintln,
            ("lush/int", "to_string") => Builtin::IntToString,
            _ => {
                self.error(
                    codes::E2000_UNAVAILABLE_BUILTIN,
                    format!("`{module}.{name}` is not available in build step 3 (see later steps)"),
                    span,
                );
                return None;
            }
        };
        let dst = self.fresh()?;
        self.emit(Op::Builtin { dst, builtin, args }, span);
        Some(dst)
    }

    fn emit_case(
        &mut self,
        subjects: &[Expr],
        clauses: &[Clause],
        span: Span,
        tail: bool,
    ) -> Option<Reg> {
        // Evaluate subjects once.
        let mut scruts = Vec::new();
        for s in subjects {
            scruts.push(self.emit_expr(s, false)?);
        }
        let dst = self.fresh()?;
        let mut end_jumps = Vec::new();

        for clause in clauses {
            self.push_scope();
            // Alternatives (`p | q`): try each PatternRow until one matches.
            // Checkpoint: support a single row (no or-patterns) covering all subjects.
            let row = match clause.patterns.first() {
                Some(r) => r,
                None => {
                    self.error(codes::E2011_LOWER, "empty clause patterns", span);
                    return None;
                }
            };
            if row.patterns.len() != scruts.len() {
                self.error(codes::E2011_LOWER, "clause/subject arity mismatch", span);
                return None;
            }
            let mut fail_jumps = Vec::new();
            for (pat, scrut) in row.patterns.iter().zip(scruts.iter()) {
                match &pat.kind {
                    PatternKind::Int(lit) => {
                        let base = match lit.base {
                            IntBase::Decimal => 10,
                            IntBase::Hex => 16,
                            IntBase::Octal => 8,
                            IntBase::Binary => 2,
                        };
                        let v = numeric::int_literal_value(&lit.digits, base).ok()?;
                        let ok = self.fresh()?;
                        self.emit(
                            Op::IsInt {
                                dst: ok,
                                src: *scrut,
                                value: v,
                            },
                            pat.span,
                        );
                        let jmp = self.code.len();
                        self.emit(
                            Op::JumpIfFalse {
                                cond: ok,
                                target: 0,
                            },
                            pat.span,
                        );
                        fail_jumps.push(jmp);
                    }
                    PatternKind::Var(n) => {
                        let r = self.fresh()?;
                        self.emit(
                            Op::Move {
                                dst: r,
                                src: *scrut,
                            },
                            pat.span,
                        );
                        self.define(n.text.clone(), r);
                    }
                    PatternKind::Discard | PatternKind::UnderscoreName(_) => {}
                    PatternKind::Constructor { constructor: c, .. }
                        if c.name.text == "True" || c.name.text == "False" =>
                    {
                        let want = c.name.text == "True";
                        let b = self.fresh()?;
                        self.emit(
                            Op::LoadBool {
                                dst: b,
                                value: want,
                            },
                            pat.span,
                        );
                        let ok = self.fresh()?;
                        self.emit(
                            Op::Eq {
                                dst: ok,
                                a: *scrut,
                                b,
                            },
                            pat.span,
                        );
                        let jmp = self.code.len();
                        self.emit(
                            Op::JumpIfFalse {
                                cond: ok,
                                target: 0,
                            },
                            pat.span,
                        );
                        fail_jumps.push(jmp);
                    }
                    PatternKind::Constructor {
                        constructor: c,
                        args,
                    } if !matches!(c.name.text.as_str(), "True" | "False") => {
                        let tag: u16 = match c.name.text.as_str() {
                            "Ok" | "Some" => 0,
                            "Error" | "None" => 1,
                            _ => {
                                // User ADTs: declaration order unknown here — use 0.
                                0
                            }
                        };
                        let switch_idx = self.code.len();
                        self.emit(
                            Op::SwitchTag {
                                scrutinee: *scrut,
                                arms: vec![(tag, 0)],
                                default: 0,
                            },
                            pat.span,
                        );
                        let match_pc = self.code.len() as u32;
                        if let Op::SwitchTag { arms, .. } = &mut self.code[switch_idx] {
                            arms[0].1 = match_pc;
                        }
                        // Record switch default for fail patching (reuse fail_jumps via sentinel JumpIfFalse).
                        // Emit a nop JumpIfFalse on True so we can reuse the fail_jumps patcher for the
                        // SwitchTag default by also storing switch_idx separately.
                        // Patch default when fail_pc known: push switch_idx into a side list.
                        // Simpler: after binding fields, we rely on fail_jumps patching below — add
                        // SwitchTag default patch alongside fail_jumps by pushing switch_idx + FLAG.
                        // Use: fail_jumps.push(switch_idx | 1<<31) convention — too hacky.
                        // Instead patch default now to a placeholder and collect switch_idx in fail_jumps
                        // as a JumpIfFalse we invent:
                        fail_jumps.push(switch_idx);
                        if let Some(pargs) = args {
                            let mut field_i = 0u16;
                            for pa in pargs {
                                if pa.spread {
                                    continue;
                                }
                                let Some(inner) = &pa.pattern else {
                                    continue;
                                };
                                let field = self.fresh()?;
                                self.emit(
                                    Op::GetField {
                                        dst: field,
                                        base: *scrut,
                                        index: field_i,
                                    },
                                    inner.span,
                                );
                                field_i += 1;
                                match &inner.kind {
                                    PatternKind::Var(n) => self.define(n.text.clone(), field),
                                    PatternKind::Discard | PatternKind::UnderscoreName(_) => {}
                                    _ => {
                                        self.error(
                                            codes::E2011_LOWER,
                                            "nested constructor field pattern not supported yet",
                                            inner.span,
                                        );
                                        return None;
                                    }
                                }
                            }
                        }
                    }
                    PatternKind::List { items, spread } => {
                        // Nested list patterns beyond one cons cell are lowered
                        // by walking items then the optional spread tail.
                        let mut cur = *scrut;
                        for item in items {
                            let is_empty = self.fresh()?;
                            self.emit(
                                Op::IsEmptyList {
                                    dst: is_empty,
                                    src: cur,
                                },
                                pat.span,
                            );
                            // fail if empty when expecting a cons
                            // Fail when empty while expecting a cons cell.
                            let f = self.fresh()?;
                            self.emit(
                                Op::LoadBool {
                                    dst: f,
                                    value: false,
                                },
                                pat.span,
                            );
                            let ok = self.fresh()?;
                            self.emit(
                                Op::Eq {
                                    dst: ok,
                                    a: is_empty,
                                    b: f,
                                },
                                pat.span,
                            );
                            let jmp = self.code.len();
                            self.emit(
                                Op::JumpIfFalse {
                                    cond: ok,
                                    target: 0,
                                },
                                pat.span,
                            );
                            fail_jumps.push(jmp);
                            let head = self.fresh()?;
                            self.emit(
                                Op::GetField {
                                    dst: head,
                                    base: cur,
                                    index: 0,
                                },
                                pat.span,
                            );
                            let tail = self.fresh()?;
                            self.emit(
                                Op::GetField {
                                    dst: tail,
                                    base: cur,
                                    index: 1,
                                },
                                pat.span,
                            );
                            // Bind item pattern against head (var / discard only for now).
                            match &item.kind {
                                PatternKind::Var(n) => {
                                    self.define(n.text.clone(), head);
                                }
                                PatternKind::Discard | PatternKind::UnderscoreName(_) => {}
                                PatternKind::Int(lit) => {
                                    let base = match lit.base {
                                        IntBase::Decimal => 10,
                                        IntBase::Hex => 16,
                                        IntBase::Octal => 8,
                                        IntBase::Binary => 2,
                                    };
                                    let v = numeric::int_literal_value(&lit.digits, base).ok()?;
                                    let ok = self.fresh()?;
                                    self.emit(
                                        Op::IsInt {
                                            dst: ok,
                                            src: head,
                                            value: v,
                                        },
                                        item.span,
                                    );
                                    let jmp = self.code.len();
                                    self.emit(
                                        Op::JumpIfFalse {
                                            cond: ok,
                                            target: 0,
                                        },
                                        item.span,
                                    );
                                    fail_jumps.push(jmp);
                                }
                                _ => {
                                    self.error(
                                        codes::E2011_LOWER,
                                        "nested list item pattern not supported yet",
                                        item.span,
                                    );
                                    return None;
                                }
                            }
                            cur = tail;
                        }
                        match spread {
                            None => {
                                let is_empty = self.fresh()?;
                                self.emit(
                                    Op::IsEmptyList {
                                        dst: is_empty,
                                        src: cur,
                                    },
                                    pat.span,
                                );
                                let jmp = self.code.len();
                                self.emit(
                                    Op::JumpIfFalse {
                                        cond: is_empty,
                                        target: 0,
                                    },
                                    pat.span,
                                );
                                fail_jumps.push(jmp);
                            }
                            Some(sp) => match &sp.kind {
                                PatternKind::Var(n) => self.define(n.text.clone(), cur),
                                PatternKind::Discard | PatternKind::UnderscoreName(_) => {}
                                _ => {
                                    self.error(
                                        codes::E2011_LOWER,
                                        "complex list spread pattern not supported yet",
                                        sp.span,
                                    );
                                    return None;
                                }
                            },
                        }
                    }
                    PatternKind::Tuple(elems) => {
                        for (i, ep) in elems.iter().enumerate() {
                            let field = self.fresh()?;
                            self.emit(
                                Op::GetField {
                                    dst: field,
                                    base: *scrut,
                                    index: i as u16,
                                },
                                ep.span,
                            );
                            match &ep.kind {
                                PatternKind::Var(n) => self.define(n.text.clone(), field),
                                PatternKind::Discard | PatternKind::UnderscoreName(_) => {}
                                _ => {
                                    self.error(
                                        codes::E2011_LOWER,
                                        "nested tuple pattern not supported yet",
                                        ep.span,
                                    );
                                    return None;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            if let Some(g) = &clause.guard {
                let gv = self.emit_expr(g, false)?;
                let jmp = self.code.len();
                self.emit(
                    Op::JumpIfFalse {
                        cond: gv,
                        target: 0,
                    },
                    g.span,
                );
                fail_jumps.push(jmp);
            }
            let body = self.emit_expr(&clause.body, tail)?;
            if !matches!(
                self.code.last(),
                Some(Op::TailCall { .. } | Op::TailCallClosure { .. } | Op::Panic { .. })
            ) {
                self.emit(Op::Move { dst, src: body }, clause.body.span);
                let end_jmp = self.code.len();
                self.emit(Op::Jump { target: 0 }, span);
                end_jumps.push(end_jmp);
            }
            let fail_pc = self.code.len() as u32;
            for j in fail_jumps {
                match &mut self.code[j] {
                    Op::JumpIfFalse { target, .. } => *target = fail_pc,
                    Op::SwitchTag { default, .. } => *default = fail_pc,
                    _ => {}
                }
            }
            self.pop_scope();
        }
        // No arm matched — defensive panic
        let idx = self.intern_string("internal error: no arm matched".into());
        self.emit(Op::Panic { msg: idx }, span);
        // Successful (non-tail) arms jump here, past the panic. Emit a nop so the
        // jump target is a real instruction; the caller then appends `Return`.
        if !end_jumps.is_empty() {
            let end = self.code.len() as u32;
            for j in end_jumps {
                if let Op::Jump { target } = &mut self.code[j] {
                    *target = end;
                }
            }
            self.emit(Op::Move { dst, src: dst }, span);
        }
        Some(dst)
    }
}

fn field_name(f: &FieldName) -> String {
    match f {
        FieldName::Name(n) => n.text.clone(),
        FieldName::UName(n) => n.text.clone(),
    }
}
