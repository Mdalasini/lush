//! Register bytecode emission from a typed module.

use std::collections::{BTreeMap, HashMap, HashSet};

use lush_syntax::ast::*;
use lush_syntax::diagnostic::{Diagnostic, DiagnosticKind};
use lush_syntax::span::Span;
use lush_syntax::token::IntBase;
use lush_types::numeric;
use lush_types::typed::{BitSegmentKind, FieldSite, TypedModule};

use crate::bytecode::{BitSegEnc, Builtin, ConstId, Constant, FuncId, Function, Op, Program, Reg};
use crate::codes;
use crate::limits::{MAX_INSTRUCTIONS_PER_FUNCTION, MAX_REGISTERS};

struct ModuleEmitter<'a> {
    typed: &'a TypedModule,
    constants: Vec<Constant>,
    functions: Vec<Function>,
    func_ids: HashMap<(String, String), FuncId>,
    diagnostics: Vec<Diagnostic>,
    /// Module path → source text (for per-op line/col).
    sources: &'a BTreeMap<String, String>,
    /// Cached line-start byte offsets per module path.
    line_starts: HashMap<String, Vec<usize>>,
    /// Counter for synthetic closure function names (`$fnN`).
    gensym: u32,
}

struct FnEmitter<'a, 'm> {
    m: &'m mut ModuleEmitter<'a>,
    module: String,
    name: String,
    /// Next unused register index / high-water mark (`usize` so the E2001
    /// guard can fire at `MAX_REGISTERS` without wrapping a `u8`).
    regs: usize,
    /// Recycled temporary registers (dead after their last use).
    free: Vec<Reg>,
    /// Registers currently holding unnamed temporaries (safe to recycle).
    temps: HashSet<Reg>,
    /// True after the first `E2001` for this function (avoid duplicate diags).
    reg_limit_reported: bool,
    code: Vec<Op>,
    lines: Vec<(u32, u32)>,
    env: Vec<HashMap<String, Reg>>,
    arity: u8,
    /// Captures occupy `arity .. arity + n_captures` (not recycled).
    n_captures: u8,
    /// Local / mutual-group function names → FuncId (direct Call/TailCall).
    local_fns: HashMap<String, FuncId>,
}

pub fn emit_program(
    modules: &[(String, TypedModule)],
    entry_path: &str,
    sources: &BTreeMap<String, String>,
) -> Result<Program, Vec<Diagnostic>> {
    let mut em = ModuleEmitter {
        typed: &modules[0].1, // overwritten per module
        constants: Vec::new(),
        functions: Vec::new(),
        func_ids: HashMap::new(),
        diagnostics: Vec::new(),
        sources,
        line_starts: HashMap::new(),
        gensym: 0,
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
                    n_captures: 0,
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
                    regs: f.params.len(),
                    free: Vec::new(),
                    temps: HashSet::new(),
                    reg_limit_reported: false,
                    code: vec![],
                    lines: vec![],
                    env: vec![HashMap::new()],
                    arity: f.params.len() as u8,
                    n_captures: 0,
                    local_fns: HashMap::new(),
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
                let regs = fe.regs.max(fe.arity as usize) as u16;
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
        if let Some(r) = self.free.pop() {
            self.temps.insert(r);
            return Some(r);
        }
        if self.regs >= MAX_REGISTERS {
            if !self.reg_limit_reported {
                self.error(
                    codes::E2001_TOO_MANY_REGISTERS,
                    format!("function `{}` exceeds {MAX_REGISTERS} registers", self.name),
                    Span::default(),
                );
                self.reg_limit_reported = true;
            }
            return None;
        }
        let r = self.regs as Reg;
        self.regs += 1;
        self.temps.insert(r);
        Some(r)
    }

    /// Recycle a temporary register after its last use.
    fn recycle(&mut self, r: Reg) {
        if self.temps.remove(&r) {
            self.free_reg(r);
        }
    }

    /// Recycle temporary argument registers after a call/builtin consumes them.
    /// Locals are left alone (`recycle` is a no-op for non-temps).
    fn recycle_regs(&mut self, regs: &[Reg]) {
        for &r in regs {
            self.recycle(r);
        }
    }

    /// Return a register to the free list (parameters and captures are never freed).
    fn free_reg(&mut self, r: Reg) {
        if (r as usize) < self.arity as usize + self.n_captures as usize {
            return;
        }
        self.temps.remove(&r);
        if !self.free.contains(&r) {
            self.free.push(r);
        }
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
        let (line, col) = self.span_line_col(span);
        self.code.push(op);
        self.lines.push((line, col));
    }

    fn span_line_col(&mut self, span: Span) -> (u32, u32) {
        if !self.m.line_starts.contains_key(&self.module) {
            let src = self
                .m
                .sources
                .get(&self.module)
                .map(String::as_str)
                .unwrap_or("");
            let starts = line_start_offsets(src);
            self.m.line_starts.insert(self.module.clone(), starts);
        }
        let starts = &self.m.line_starts[&self.module];
        let src = self
            .m
            .sources
            .get(&self.module)
            .map(String::as_str)
            .unwrap_or("");
        byte_to_line_col(src, starts, span.start.0 as usize)
    }

    fn define(&mut self, name: String, r: Reg) {
        // Bindings stay live until last use / scope exit — not temps.
        self.temps.remove(&r);
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
        if let Some(scope) = self.env.pop() {
            for (_, r) in scope {
                self.free_reg(r);
            }
        }
    }

    /// Free locals in the current scope whose last use was statement `stmt_i`.
    fn free_dead_locals(&mut self, last_use: &HashMap<String, usize>, stmt_i: usize) {
        let dead: Vec<(String, Reg)> = {
            let Some(scope) = self.env.last() else {
                return;
            };
            scope
                .iter()
                .filter_map(|(name, r)| {
                    if last_use.get(name).copied() == Some(stmt_i) {
                        Some((name.clone(), *r))
                    } else {
                        None
                    }
                })
                .collect()
        };
        for (name, r) in dead {
            if let Some(scope) = self.env.last_mut() {
                scope.remove(&name);
            }
            self.free_reg(r);
        }
    }

    /// Free locals bound in this scope that are never read (`last_use` has no entry).
    fn free_unread_locals(&mut self, last_use: &HashMap<String, usize>) {
        let dead: Vec<(String, Reg)> = {
            let Some(scope) = self.env.last() else {
                return;
            };
            scope
                .iter()
                .filter_map(|(name, r)| {
                    if !last_use.contains_key(name) {
                        Some((name.clone(), *r))
                    } else {
                        None
                    }
                })
                .collect()
        };
        for (name, r) in dead {
            if let Some(scope) = self.env.last_mut() {
                scope.remove(&name);
            }
            self.free_reg(r);
        }
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
        let last_use = last_uses_in_block(block);
        let n = block.statements.len();
        let mut last = None;
        let mut i = 0;
        while i < n {
            let is_last = i + 1 == n;
            match &block.statements[i] {
                Statement::Expr(e) => {
                    let r = self.emit_expr(e, tail && is_last);
                    if is_last {
                        last = r;
                    } else if let Some(r) = r {
                        // Non-final expression statements are dead after the `;`.
                        self.free_reg(r);
                    }
                    self.free_dead_locals(&last_use, i);
                    i += 1;
                }
                Statement::Let(l) => {
                    let v = self.emit_expr(&l.value, false)?;
                    self.bind_pattern(&l.pattern, v)?;
                    // Locals with no later use are dead on arrival.
                    self.free_unread_locals(&last_use);
                    if is_last {
                        let r = self.fresh()?;
                        self.emit(Op::LoadNil { dst: r }, l.span);
                        last = Some(r);
                    }
                    self.free_dead_locals(&last_use, i);
                    i += 1;
                }
                Statement::Fn(_) => {
                    // Maximal consecutive local-fn group (mutually recursive).
                    let start = i;
                    while i < n && matches!(&block.statements[i], Statement::Fn(_)) {
                        i += 1;
                    }
                    let group_end = i;
                    let group_last = group_end == n;
                    self.emit_local_fn_group(&block.statements[start..group_end])?;
                    if group_last {
                        let r = self.fresh()?;
                        let span = match &block.statements[start] {
                            Statement::Fn(f) => f.span,
                            _ => block.span,
                        };
                        self.emit(Op::LoadNil { dst: r }, span);
                        last = Some(r);
                    }
                    // Free locals whose last use was any statement in the group.
                    for stmt_i in start..group_end {
                        self.free_dead_locals(&last_use, stmt_i);
                    }
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
                // Promote a temporary into the binding instead of copying.
                if self.temps.remove(&src) {
                    self.define(n.text.clone(), src);
                } else {
                    let r = self.fresh()?;
                    self.emit(Op::Move { dst: r, src }, pat.span);
                    self.define(n.text.clone(), r);
                }
                Some(())
            }
            PatternKind::Discard => {
                self.recycle(src);
                Some(())
            }
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
                self.recycle(src);
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
                // Top-level function used as a value → closure with zero captures.
                if let Some((module, name)) = self.resolve_fn(&n.text) {
                    let fid = *self
                        .m
                        .func_ids
                        .get(&(module, name))
                        .expect("func id for value");
                    let dst = self.fresh()?;
                    self.emit(
                        Op::MakeClosure {
                            dst,
                            func: fid,
                            captures: vec![],
                        },
                        expr.span,
                    );
                    return Some(dst);
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
                    let variant = self.constructor_tag(name).unwrap_or(0);
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
                // Direct `-9223372036854775808` is `MIN_INT` (spec §5.7); it must
                // load as an immediate, not go through `Op::Neg` (which panics).
                if matches!(op, UnaryOp::Neg) {
                    if let ExprKind::Int(lit) = &inner.kind {
                        let base = match lit.base {
                            IntBase::Decimal => 10,
                            IntBase::Hex => 16,
                            IntBase::Octal => 8,
                            IntBase::Binary => 2,
                        };
                        let v = numeric::negated_int_literal_value(&lit.digits, base).ok()?;
                        let dst = self.fresh()?;
                        self.emit(Op::LoadInt { dst, value: v }, expr.span);
                        return Some(dst);
                    }
                }
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
                        self.recycle(f);
                    }
                }
                self.recycle(src);
                Some(dst)
            }
            ExprKind::Binary { op, .. } => {
                if matches!(op, BinOp::And | BinOp::Or) {
                    let ExprKind::Binary { left, right, .. } = &expr.kind else {
                        unreachable!()
                    };
                    return self.emit_and_or(*op, left, right, expr.span, tail);
                }
                if is_left_assoc_chainable(*op) {
                    return self.emit_left_assoc_chain(*op, expr);
                }
                let ExprKind::Binary { left, right, .. } = &expr.kind else {
                    unreachable!()
                };
                self.emit_binary_op(*op, left, right, expr.span)
            }
            ExprKind::Call { callee, args } => {
                self.emit_call(expr.id, callee, args, expr.span, tail)
            }
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
                let src = self.emit_expr(e, false)?;
                let dst = self.fresh()?;
                self.emit(Op::Echo { dst, src }, expr.span);
                Some(dst)
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
                    self.recycle(h);
                    self.recycle(acc);
                    acc = dst;
                }
                Some(acc)
            }
            ExprKind::Field { base, field } => {
                // Module.alias for values shouldn't appear; treat as field access.
                let base_r = self.emit_expr(base, false)?;
                let name = field_name(field);
                let site = self.m.typed.expr(expr.id).and_then(|i| i.field.clone());
                let dst = self.emit_field_access(base_r, site.as_ref(), &name, expr.span)?;
                self.recycle(base_r);
                Some(dst)
            }
            ExprKind::Fn { params, body, .. } => self.emit_anon_fn(params, body, expr.span),
            ExprKind::BitArray(segs) => self.emit_bit_array_expr(expr, segs),
            ExprKind::RecordUpdate {
                constructor,
                base,
                fields,
            } => self.emit_record_update(constructor, base, fields, expr.span),
            other => {
                let msg = match other {
                    ExprKind::Pipe { .. } => "pipe expressions should have been desugared",
                    _ => "this expression form is not yet lowered",
                };
                self.error(codes::E2011_LOWER, msg, expr.span);
                None
            }
        }
    }

    fn emit_record_update(
        &mut self,
        constructor: &ConstructorRef,
        base: &Expr,
        fields: &[(Name, Expr)],
        span: Span,
    ) -> Option<Reg> {
        let base_r = self.emit_expr(base, false)?;
        let (variant, labels) =
            self.constructor_layout(&constructor.name.text, constructor.span)?;
        let mut replacements: HashMap<String, Reg> = HashMap::new();
        for (name, e) in fields {
            let r = self.emit_expr(e, false)?;
            replacements.insert(name.text.clone(), r);
        }
        let mut field_regs = Vec::with_capacity(labels.len());
        let mut copied = Vec::new();
        for (i, label) in labels.iter().enumerate() {
            if let Some(lab) = label {
                if let Some(&r) = replacements.get(lab) {
                    field_regs.push(r);
                    continue;
                }
            }
            let r = self.fresh()?;
            self.emit(
                Op::GetField {
                    dst: r,
                    base: base_r,
                    index: i as u16,
                },
                span,
            );
            field_regs.push(r);
            copied.push(r);
        }
        let dst = self.fresh()?;
        self.emit(
            Op::MakeAdt {
                dst,
                type_tag: 0,
                variant,
                fields: field_regs.clone(),
            },
            span,
        );
        self.recycle(base_r);
        self.recycle_regs(&copied);
        for r in replacements.into_values() {
            self.recycle(r);
        }
        Some(dst)
    }

    /// Variant tag and ordered field labels for a constructor name.
    fn constructor_layout(&mut self, name: &str, span: Span) -> Option<(u16, Vec<Option<String>>)> {
        for def in self.m.typed.store.defs.values() {
            if let Some((i, v)) = def
                .variants
                .iter()
                .enumerate()
                .find(|(_, v)| v.name == name)
            {
                let labs: Vec<_> = v.fields.iter().map(|f| f.label.clone()).collect();
                return Some((i as u16, labs));
            }
        }
        self.error(
            codes::E2011_LOWER,
            format!("unknown constructor `{name}` in record update"),
            span,
        );
        None
    }

    fn emit_bit_array_expr(&mut self, expr: &Expr, segs: &[BitSegment]) -> Option<Reg> {
        let kinds = self
            .m
            .typed
            .expr(expr.id)
            .and_then(|i| i.bit_segments.clone())
            .unwrap_or_else(|| segs.iter().map(|s| fallback_bit_kind(&s.options)).collect());
        if kinds.len() != segs.len() {
            self.error(
                codes::E2011_LOWER,
                "bit-array segment kind count mismatch",
                expr.span,
            );
            return None;
        }
        let mut values = Vec::new();
        let mut specs = Vec::new();
        for (seg, kind) in segs.iter().zip(kinds.iter()) {
            // Reject dynamic sizes in construction (typed already requires literals).
            for opt in &seg.options {
                if let BitOption::Size(e) = opt {
                    if !matches!(e.kind, ExprKind::Int(_)) {
                        self.error(
                            codes::E2011_LOWER,
                            "bit-array construction size must be a literal",
                            e.span,
                        );
                        return None;
                    }
                }
            }
            let v = self.emit_expr(&seg.value, false)?;
            values.push(v);
            specs.push(bit_kind_to_enc(kind));
        }
        let dst = self.fresh()?;
        self.emit(
            Op::MakeBitArray {
                dst,
                values: values.clone(),
                specs,
            },
            expr.span,
        );
        self.recycle_regs(&values);
        Some(dst)
    }

    fn emit_binary_op(&mut self, op: BinOp, left: &Expr, right: &Expr, span: Span) -> Option<Reg> {
        let a = self.emit_expr(left, false)?;
        let b = self.emit_expr(right, false)?;
        let dst = self.fresh()?;
        self.emit(binop_op(op, dst, a, b), span);
        self.recycle(a);
        self.recycle(b);
        Some(dst)
    }

    /// Flatten a left-associative operator spine iteratively so a 4096-term
    /// chain does not recurse on the native stack, and recycle operand temps
    /// so the chain fits in a handful of registers.
    fn emit_left_assoc_chain(&mut self, op: BinOp, expr: &Expr) -> Option<Reg> {
        let mut rights: Vec<(&Expr, Span)> = Vec::new();
        let mut cur = expr;
        while let ExprKind::Binary {
            op: o, left, right, ..
        } = &cur.kind
        {
            if *o != op {
                break;
            }
            rights.push((right.as_ref(), cur.span));
            cur = left.as_ref();
        }
        let mut acc = self.emit_expr(cur, false)?;
        for (right, span) in rights.into_iter().rev() {
            let b = self.emit_expr(right, false)?;
            let dst = self.fresh()?;
            self.emit(binop_op(op, dst, acc, b), span);
            self.recycle(acc);
            self.recycle(b);
            acc = dst;
        }
        Some(acc)
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

    fn emit_call(
        &mut self,
        call_id: NodeId,
        callee: &Expr,
        args: &[Arg],
        span: Span,
        tail: bool,
    ) -> Option<Reg> {
        // Evaluate arguments in source order, then reorder into parameter order
        // using CallInfo.arg_to_param from lush-types (§5.2 / #24 §2).
        let mut src_regs = Vec::new();
        for a in args {
            match &a.value {
                ArgValue::Expr(e) => src_regs.push(self.emit_expr(e, false)?),
                ArgValue::Hole => {
                    self.error(codes::E2011_LOWER, "capture hole survived desugaring", span);
                    return None;
                }
            }
        }
        let arg_regs = self.reorder_args(call_id, src_regs, args.len());

        match &callee.kind {
            ExprKind::Var(n) => {
                // Same-group local fn → direct Call (captures copied by the VM).
                if let Some(&fid) = self.local_fns.get(&n.text) {
                    return self.emit_func_call(fid, arg_regs, span, tail);
                }
                if let Some((module, name)) = self.resolve_fn(&n.text) {
                    return self.emit_direct_call(&module, &name, arg_regs, span, tail);
                }
                // Local binding holding a closure.
                if let Some(clo) = self.lookup(&n.text) {
                    return self.emit_closure_call(clo, arg_regs, span, tail);
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
                let variant = self.constructor_tag(&c.name.text).unwrap_or(0);
                let dst = self.fresh()?;
                self.emit(
                    Op::MakeAdt {
                        dst,
                        type_tag: 0,
                        variant,
                        fields: arg_regs.clone(),
                    },
                    span,
                );
                self.recycle_regs(&arg_regs);
                Some(dst)
            }
            _ => {
                let clo = self.emit_expr(callee, false)?;
                self.emit_closure_call(clo, arg_regs, span, tail)
            }
        }
    }

    /// Reorder source-order argument registers into parameter order using
    /// `CallInfo.arg_to_param` when present; otherwise leave them as-is.
    fn reorder_args(&self, call_id: NodeId, src_regs: Vec<Reg>, n_args: usize) -> Vec<Reg> {
        let Some(info) = self.m.typed.expr(call_id) else {
            return src_regs;
        };
        let Some(call) = &info.call else {
            return src_regs;
        };
        if call.arg_to_param.len() != n_args || call.arg_to_param.len() != src_regs.len() {
            return src_regs;
        }
        let mut arity = 0usize;
        for &p in &call.arg_to_param {
            arity = arity.max(p + 1);
        }
        let mut ordered = vec![None; arity];
        for (src_i, &param_i) in call.arg_to_param.iter().enumerate() {
            if param_i < ordered.len() {
                ordered[param_i] = Some(src_regs[src_i]);
            }
        }
        if ordered.iter().any(|r| r.is_none()) {
            return src_regs;
        }
        ordered.into_iter().map(|r| r.unwrap()).collect()
    }

    fn emit_closure_call(
        &mut self,
        clo: Reg,
        args: Vec<Reg>,
        span: Span,
        tail: bool,
    ) -> Option<Reg> {
        if tail {
            self.emit(Op::TailCallClosure { clo, args }, span);
            self.fresh()
        } else {
            let dst = self.fresh()?;
            self.emit(
                Op::CallClosure {
                    dst,
                    clo,
                    args: args.clone(),
                },
                span,
            );
            self.recycle(clo);
            self.recycle_regs(&args);
            Some(dst)
        }
    }

    fn emit_func_call(
        &mut self,
        fid: FuncId,
        args: Vec<Reg>,
        span: Span,
        tail: bool,
    ) -> Option<Reg> {
        if tail {
            self.emit(Op::TailCall { func: fid, args }, span);
            self.fresh()
        } else {
            let dst = self.fresh()?;
            self.emit(
                Op::Call {
                    dst,
                    func: fid,
                    args: args.clone(),
                },
                span,
            );
            self.recycle_regs(&args);
            Some(dst)
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
        self.emit_func_call(fid, args, span, tail)
    }

    /// Lower an anonymous `fn` / capture desugar to a synthetic function + `MakeClosure`.
    fn emit_anon_fn(&mut self, params: &[Param], body: &Block, span: Span) -> Option<Reg> {
        let capture_names = self.free_vars_of_fn(params, body, &HashSet::new());
        let syn = self.fresh_synth_name();
        let fid =
            self.emit_closure_function(&syn, params, body, &capture_names, &HashMap::new(), span)?;
        let mut caps = Vec::with_capacity(capture_names.len());
        for name in &capture_names {
            let Some(reg) = self.lookup(name) else {
                self.error(
                    codes::E2011_LOWER,
                    format!("capture `{name}` has no register"),
                    span,
                );
                return None;
            };
            caps.push(reg);
        }
        let dst = self.fresh()?;
        self.emit(
            Op::MakeClosure {
                dst,
                func: fid,
                captures: caps,
            },
            span,
        );
        Some(dst)
    }

    /// Lower a maximal consecutive group of local `fn` statements.
    fn emit_local_fn_group(&mut self, stmts: &[Statement]) -> Option<()> {
        let defs: Vec<&FnDef> = stmts
            .iter()
            .map(|s| match s {
                Statement::Fn(f) => f,
                _ => unreachable!(),
            })
            .collect();
        if defs.is_empty() {
            return Some(());
        }

        let peer_names: HashSet<String> = defs.iter().map(|f| f.name.text.clone()).collect();

        // Shared capture environment = union of free vars of every peer body,
        // excluding peer names themselves (resolved via direct Call).
        let mut capture_names: Vec<String> = Vec::new();
        let mut seen_caps: HashSet<String> = HashSet::new();
        for f in &defs {
            for name in self.free_vars_of_fn(&f.params, &f.body, &peer_names) {
                if seen_caps.insert(name.clone()) {
                    capture_names.push(name);
                }
            }
        }

        // Pass 1: stubs + name→FuncId so mutual recursion can Call by FuncId.
        let mut local_fns: HashMap<String, FuncId> = HashMap::new();
        let mut fids: Vec<FuncId> = Vec::with_capacity(defs.len());
        for f in &defs {
            let n = self.m.gensym;
            self.m.gensym += 1;
            let syn = format!("${}_{}", f.name.text, n);
            let fid = self.m.functions.len() as FuncId;
            if fid as usize >= crate::limits::MAX_FUNCTIONS_PER_MODULE {
                self.error(
                    codes::E2003_TOO_MANY_FUNCTIONS,
                    "too many functions in module",
                    f.span,
                );
                return None;
            }
            self.m.functions.push(Function {
                module: self.module.clone(),
                name: syn,
                arity: f.params.len() as u8,
                n_captures: capture_names.len() as u8,
                regs: 0,
                code: vec![],
                lines: vec![],
                source_path: format!("src/{}.lush", self.module),
            });
            local_fns.insert(f.name.text.clone(), fid);
            fids.push(fid);
        }

        // Pass 2: bodies with the full peer map.
        for (f, &fid) in defs.iter().zip(fids.iter()) {
            self.fill_closure_function(
                fid,
                &f.params,
                &f.body,
                &capture_names,
                &local_fns,
                f.span,
            )?;
        }

        // Pass 3: MakeClosure at the group site and bind each name.
        let mut cap_regs = Vec::with_capacity(capture_names.len());
        for name in &capture_names {
            match self.lookup(name) {
                Some(r) => cap_regs.push(r),
                None => {
                    self.error(
                        codes::E2011_LOWER,
                        format!("capture `{name}` has no register"),
                        defs[0].span,
                    );
                    return None;
                }
            }
        }
        for (f, &fid) in defs.iter().zip(fids.iter()) {
            let dst = self.fresh()?;
            self.emit(
                Op::MakeClosure {
                    dst,
                    func: fid,
                    captures: cap_regs.clone(),
                },
                f.span,
            );
            self.define(f.name.text.clone(), dst);
        }
        Some(())
    }

    fn fresh_synth_name(&mut self) -> String {
        let n = self.m.gensym;
        self.m.gensym += 1;
        // #24 §1: compiler-generated names must not look like user identifiers.
        format!("<fn{n}>")
    }

    fn emit_closure_function(
        &mut self,
        name: &str,
        params: &[Param],
        body: &Block,
        capture_names: &[String],
        local_fns: &HashMap<String, FuncId>,
        span: Span,
    ) -> Option<FuncId> {
        let fid = self.m.functions.len() as FuncId;
        if fid as usize >= crate::limits::MAX_FUNCTIONS_PER_MODULE {
            self.error(
                codes::E2003_TOO_MANY_FUNCTIONS,
                "too many functions in module",
                span,
            );
            return None;
        }
        self.m.functions.push(Function {
            module: self.module.clone(),
            name: name.to_string(),
            arity: params.len() as u8,
            n_captures: capture_names.len() as u8,
            regs: 0,
            code: vec![],
            lines: vec![],
            source_path: format!("src/{}.lush", self.module),
        });
        self.fill_closure_function(fid, params, body, capture_names, local_fns, span)?;
        Some(fid)
    }

    fn fill_closure_function(
        &mut self,
        fid: FuncId,
        params: &[Param],
        body: &Block,
        capture_names: &[String],
        local_fns: &HashMap<String, FuncId>,
        span: Span,
    ) -> Option<()> {
        let module = self.module.clone();
        let name = self.m.functions[fid as usize].name.clone();
        let arity = params.len() as u8;
        let n_captures = capture_names.len() as u8;
        let mut nested = FnEmitter {
            m: self.m,
            module,
            name,
            regs: params.len() + capture_names.len(),
            free: Vec::new(),
            temps: HashSet::new(),
            reg_limit_reported: false,
            code: vec![],
            lines: vec![],
            env: vec![HashMap::new()],
            arity,
            n_captures,
            local_fns: local_fns.clone(),
        };
        for (i, p) in params.iter().enumerate() {
            nested.define(p.name.text.clone(), i as Reg);
        }
        for (i, c) in capture_names.iter().enumerate() {
            nested.define(c.clone(), (params.len() + i) as Reg);
        }
        if let Some(ret) = nested.emit_block(body, true) {
            if !matches!(
                nested.code.last(),
                Some(Op::Return { .. } | Op::TailCall { .. } | Op::TailCallClosure { .. })
            ) {
                nested.emit(Op::Return { src: ret }, span);
            }
        } else if !matches!(
            nested.code.last(),
            Some(
                Op::Return { .. }
                    | Op::TailCall { .. }
                    | Op::TailCallClosure { .. }
                    | Op::Panic { .. }
            )
        ) {
            if let Some(r) = nested.fresh() {
                nested.emit(Op::LoadNil { dst: r }, span);
                nested.emit(Op::Return { src: r }, span);
            }
        }
        let regs = nested
            .regs
            .max(nested.arity as usize + nested.n_captures as usize) as u16;
        let code = std::mem::take(&mut nested.code);
        let lines = std::mem::take(&mut nested.lines);
        let func = &mut self.m.functions[fid as usize];
        func.regs = regs;
        func.code = code;
        func.lines = lines;
        Some(())
    }

    /// Names read in `body` that resolve to the *enclosing* emitter's env and
    /// are not parameters, not in `extra_bound` (peer local-fn names), and not
    /// top-level functions.
    fn free_vars_of_fn(
        &self,
        params: &[Param],
        body: &Block,
        extra_bound: &HashSet<String>,
    ) -> Vec<String> {
        let mut bound: HashSet<String> = params.iter().map(|p| p.name.text.clone()).collect();
        bound.extend(extra_bound.iter().cloned());
        let mut found: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        collect_free_vars_block(body, &mut bound, &mut |name| {
            if self.lookup(name).is_some()
                && !self.local_fns.contains_key(name)
                && self.resolve_fn(name).is_none()
                && !extra_bound.contains(name)
                && seen.insert(name.to_string())
            {
                found.push(name.to_string());
            }
        });
        found
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
        self.emit(
            Op::Builtin {
                dst,
                builtin,
                args: args.clone(),
            },
            span,
        );
        self.recycle_regs(&args);
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
                        self.recycle(ok);
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
                        self.recycle(b);
                        self.recycle(ok);
                    }
                    PatternKind::Constructor {
                        constructor: c,
                        args,
                    } if !matches!(c.name.text.as_str(), "True" | "False") => {
                        let tag: u16 = self.constructor_tag(&c.name.text).unwrap_or(0);
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
                                    PatternKind::Discard | PatternKind::UnderscoreName(_) => {
                                        self.recycle(field);
                                    }
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
                        let mut cur_is_temp = false;
                        for item in items {
                            let is_empty = self.fresh()?;
                            self.emit(
                                Op::IsEmptyList {
                                    dst: is_empty,
                                    src: cur,
                                },
                                pat.span,
                            );
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
                            self.recycle(is_empty);
                            self.recycle(f);
                            self.recycle(ok);
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
                                PatternKind::Discard | PatternKind::UnderscoreName(_) => {
                                    self.recycle(head);
                                }
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
                                    self.recycle(ok);
                                    self.recycle(head);
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
                            if cur_is_temp {
                                self.recycle(cur);
                            }
                            cur = tail;
                            cur_is_temp = true;
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
                                self.recycle(is_empty);
                                if cur_is_temp {
                                    self.recycle(cur);
                                }
                            }
                            Some(sp) => match &sp.kind {
                                PatternKind::Var(n) => self.define(n.text.clone(), cur),
                                PatternKind::Discard | PatternKind::UnderscoreName(_) => {
                                    if cur_is_temp {
                                        self.recycle(cur);
                                    }
                                }
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
                                PatternKind::Discard | PatternKind::UnderscoreName(_) => {
                                    self.recycle(field);
                                }
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
                    PatternKind::BitArray(segs) => {
                        self.emit_bit_array_pattern(pat, segs, *scrut, &mut fail_jumps)?;
                    }
                    PatternKind::StringPrefix { prefix, rest } => {
                        let expected = self.fresh()?;
                        let idx = self.intern_string(prefix.value.clone());
                        self.emit(Op::LoadConst { dst: expected, idx }, pat.span);
                        let ok = self.fresh()?;
                        let rest_r = self.fresh()?;
                        self.emit(
                            Op::StringTakePrefix {
                                ok,
                                rest: rest_r,
                                src: *scrut,
                                expected,
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
                        self.recycle(expected);
                        self.recycle(ok);
                        match &rest.kind {
                            PatternKind::Var(n) => self.define(n.text.clone(), rest_r),
                            PatternKind::Discard | PatternKind::UnderscoreName(_) => {
                                self.recycle(rest_r);
                            }
                            _ => {
                                self.error(
                                    codes::E2011_LOWER,
                                    "nested string-prefix rest pattern not supported yet",
                                    rest.span,
                                );
                                return None;
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
                self.recycle(gv);
            }
            // Track whether *this* arm's body emitted a terminating op. Looking at
            // `code.last()` alone is wrong when the body is a bare local (no new
            // ops): we'd see the previous arm's TailCall and skip Move/Jump, so
            // failure jumps land on the defensive "no arm matched" panic.
            let body_start = self.code.len();
            let body = self.emit_expr(&clause.body, tail)?;
            let body_terminated = self.code.len() > body_start
                && matches!(
                    self.code.last(),
                    Some(Op::TailCall { .. } | Op::TailCallClosure { .. } | Op::Panic { .. })
                );
            if !body_terminated {
                self.emit(Op::Move { dst, src: body }, clause.body.span);
                self.recycle(body);
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
        // Scrutinees are dead once every arm has been lowered.
        self.recycle_regs(&scruts);
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

    /// Lower a bit-array pattern: walk segments with Take* ops, then require empty
    /// unless the final segment was an unsized `:bytes` / `:bits` remainder.
    fn emit_bit_array_pattern(
        &mut self,
        pat: &Pattern,
        segs: &[BitSegmentPat],
        scrut: Reg,
        fail_jumps: &mut Vec<usize>,
    ) -> Option<()> {
        let kinds = self
            .m
            .typed
            .pattern(pat.id)
            .and_then(|i| i.bit_segments.clone())
            .unwrap_or_else(|| segs.iter().map(|s| fallback_bit_kind(&s.options)).collect());
        if kinds.len() != segs.len() {
            self.error(
                codes::E2011_LOWER,
                "bit-array pattern kind count mismatch",
                pat.span,
            );
            return None;
        }
        // Reject dynamic sizes for this slice.
        for seg in segs {
            for opt in &seg.options {
                if let BitOption::Size(e) = opt {
                    if !matches!(e.kind, ExprKind::Int(_)) {
                        self.error(
                            codes::E2011_LOWER,
                            "dynamic bit-array pattern sizes are not yet lowered",
                            e.span,
                        );
                        return None;
                    }
                }
            }
        }

        let mut cur = scrut;
        let mut consumed_all = false;
        for (i, (seg, kind)) in segs.iter().zip(kinds.iter()).enumerate() {
            let is_last = i + 1 == segs.len();
            match kind {
                BitSegmentKind::Int {
                    size,
                    signed,
                    little,
                } => {
                    let ok = self.fresh()?;
                    let value = self.fresh()?;
                    let rest = self.fresh()?;
                    self.emit(
                        Op::BitArrayTakeInt {
                            ok,
                            value,
                            rest,
                            src: cur,
                            size: *size,
                            signed: *signed,
                            little: *little,
                        },
                        seg.span,
                    );
                    let jmp = self.code.len();
                    self.emit(
                        Op::JumpIfFalse {
                            cond: ok,
                            target: 0,
                        },
                        seg.span,
                    );
                    fail_jumps.push(jmp);
                    self.recycle(ok);
                    self.bind_bit_int_pattern(&seg.pattern, value, fail_jumps)?;
                    if cur != scrut {
                        self.recycle(cur);
                    }
                    cur = rest;
                }
                BitSegmentKind::Utf8 => {
                    let PatternKind::String(lit) = &seg.pattern.kind else {
                        self.error(
                            codes::E2011_LOWER,
                            "utf8 bit-array pattern requires a string literal",
                            seg.span,
                        );
                        return None;
                    };
                    let idx = self.intern_string(lit.value.clone());
                    let expected = self.fresh()?;
                    self.emit(Op::LoadConst { dst: expected, idx }, seg.span);
                    let ok = self.fresh()?;
                    let rest = self.fresh()?;
                    self.emit(
                        Op::BitArrayTakeUtf8 {
                            ok,
                            rest,
                            src: cur,
                            expected,
                        },
                        seg.span,
                    );
                    let jmp = self.code.len();
                    self.emit(
                        Op::JumpIfFalse {
                            cond: ok,
                            target: 0,
                        },
                        seg.span,
                    );
                    fail_jumps.push(jmp);
                    self.recycle(ok);
                    self.recycle(expected);
                    if cur != scrut {
                        self.recycle(cur);
                    }
                    cur = rest;
                }
                BitSegmentKind::Bytes { size } | BitSegmentKind::Bits { size } => {
                    let require_aligned = matches!(kind, BitSegmentKind::Bytes { .. });
                    if size.is_some() {
                        self.error(
                            codes::E2011_LOWER,
                            "sized bytes/bits bit-array patterns are not yet lowered",
                            seg.span,
                        );
                        return None;
                    }
                    if !is_last {
                        self.error(
                            codes::E2011_LOWER,
                            "unsized bytes/bits must be the final bit-array pattern segment",
                            seg.span,
                        );
                        return None;
                    }
                    let ok = self.fresh()?;
                    let value = self.fresh()?;
                    self.emit(
                        Op::BitArrayTakeRest {
                            ok,
                            value,
                            src: cur,
                            require_byte_aligned: require_aligned,
                        },
                        seg.span,
                    );
                    let jmp = self.code.len();
                    self.emit(
                        Op::JumpIfFalse {
                            cond: ok,
                            target: 0,
                        },
                        seg.span,
                    );
                    fail_jumps.push(jmp);
                    self.recycle(ok);
                    match &seg.pattern.kind {
                        PatternKind::Var(n) => self.define(n.text.clone(), value),
                        PatternKind::Discard | PatternKind::UnderscoreName(_) => {
                            self.recycle(value);
                        }
                        _ => {
                            self.error(
                                codes::E2011_LOWER,
                                "bytes/bits remainder must bind a variable or `_`",
                                seg.pattern.span,
                            );
                            return None;
                        }
                    }
                    if cur != scrut {
                        self.recycle(cur);
                    }
                    consumed_all = true;
                }
            }
        }
        if !consumed_all {
            let empty = self.fresh()?;
            self.emit(
                Op::BitArrayIsEmpty {
                    dst: empty,
                    src: cur,
                },
                pat.span,
            );
            let jmp = self.code.len();
            self.emit(
                Op::JumpIfFalse {
                    cond: empty,
                    target: 0,
                },
                pat.span,
            );
            fail_jumps.push(jmp);
            self.recycle(empty);
            if cur != scrut {
                self.recycle(cur);
            }
        }
        Some(())
    }

    fn bind_bit_int_pattern(
        &mut self,
        pattern: &Pattern,
        value: Reg,
        fail_jumps: &mut Vec<usize>,
    ) -> Option<()> {
        match &pattern.kind {
            PatternKind::Var(n) => {
                self.define(n.text.clone(), value);
            }
            PatternKind::Discard | PatternKind::UnderscoreName(_) => {
                self.recycle(value);
            }
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
                        src: value,
                        value: v,
                    },
                    pattern.span,
                );
                let jmp = self.code.len();
                self.emit(
                    Op::JumpIfFalse {
                        cond: ok,
                        target: 0,
                    },
                    pattern.span,
                );
                fail_jumps.push(jmp);
                self.recycle(value);
            }
            _ => {
                self.error(
                    codes::E2011_LOWER,
                    "unsupported bit-array integer segment pattern",
                    pattern.span,
                );
                return None;
            }
        }
        Some(())
    }

    /// Declaration-order variant tag for a constructor name from the typed store.
    fn constructor_tag(&self, name: &str) -> Option<u16> {
        // Deterministic scan: HashMap iteration order is not stable across runs.
        let mut defs: Vec<_> = self.m.typed.store.defs.values().collect();
        defs.sort_by(|a, b| (&a.module, &a.name).cmp(&(&b.module, &b.name)));
        for def in defs {
            if let Some((i, _)) = def
                .variants
                .iter()
                .enumerate()
                .find(|(_, v)| v.name == name)
            {
                return Some(i as u16);
            }
        }
        match name {
            "Ok" | "Some" => Some(0),
            "Error" | "None" => Some(1),
            _ => None,
        }
    }

    /// Lower `base.field` using the call site's [`FieldSite`] when present.
    /// Multi-variant fields with differing indices become `SwitchTag` + `GetField`.
    fn emit_field_access(
        &mut self,
        base_r: Reg,
        site: Option<&FieldSite>,
        name: &str,
        span: Span,
    ) -> Option<Reg> {
        let dst = self.fresh()?;
        let arms: Vec<(u16, u16)> = site
            .map(|s| {
                s.indices_by_variant
                    .iter()
                    .enumerate()
                    .filter_map(|(vi, oi)| oi.map(|idx| (vi as u16, idx)))
                    .collect()
            })
            .unwrap_or_default();
        if arms.is_empty() {
            let index = match name {
                "x" | "first" => 0,
                "y" | "second" => 1,
                _ => 0,
            };
            self.emit(
                Op::GetField {
                    dst,
                    base: base_r,
                    index,
                },
                span,
            );
            return Some(dst);
        }
        let all_same = arms.windows(2).all(|w| w[0].1 == w[1].1);
        if arms.len() == 1 || all_same {
            self.emit(
                Op::GetField {
                    dst,
                    base: base_r,
                    index: arms[0].1,
                },
                span,
            );
            return Some(dst);
        }
        let switch_idx = self.code.len();
        self.emit(
            Op::SwitchTag {
                scrutinee: base_r,
                arms: vec![],
                default: 0,
            },
            span,
        );
        let mut end_jumps = Vec::new();
        let mut switch_arms = Vec::with_capacity(arms.len());
        for (tag, index) in &arms {
            let arm_pc = self.code.len() as u32;
            switch_arms.push((*tag, arm_pc));
            self.emit(
                Op::GetField {
                    dst,
                    base: base_r,
                    index: *index,
                },
                span,
            );
            let jmp = self.code.len();
            self.emit(Op::Jump { target: 0 }, span);
            end_jumps.push(jmp);
        }
        let default_pc = self.code.len() as u32;
        if let Op::SwitchTag {
            arms: a, default, ..
        } = &mut self.code[switch_idx]
        {
            *a = switch_arms;
            *default = default_pc;
        }
        let msg = self.intern_string("internal error: field access unmatched tag".into());
        self.emit(Op::Panic { msg }, span);
        let end = self.code.len() as u32;
        for j in end_jumps {
            if let Op::Jump { target } = &mut self.code[j] {
                *target = end;
            }
        }
        // Real instruction so end jumps land past the panic.
        self.emit(Op::Move { dst, src: dst }, span);
        Some(dst)
    }
}

fn field_name(f: &FieldName) -> String {
    match f {
        FieldName::Name(n) => n.text.clone(),
        FieldName::UName(n) => n.text.clone(),
    }
}

fn bit_kind_to_enc(kind: &BitSegmentKind) -> BitSegEnc {
    match kind {
        BitSegmentKind::Int {
            size,
            signed,
            little,
        } => BitSegEnc::Int {
            size: *size,
            signed: *signed,
            little: *little,
        },
        BitSegmentKind::Utf8 => BitSegEnc::Utf8,
        BitSegmentKind::Bytes { size } => BitSegEnc::Bytes { size_bytes: *size },
        BitSegmentKind::Bits { size } => BitSegEnc::Bits { size_bits: *size },
    }
}

/// Best-effort kind recovery when typed tables lack `bit_segments`.
fn fallback_bit_kind(options: &[BitOption]) -> BitSegmentKind {
    let mut is_utf8 = false;
    let mut is_bytes = false;
    let mut is_bits = false;
    let mut signed = false;
    let mut little = false;
    let mut width: Option<u32> = None;
    for opt in options {
        match opt {
            BitOption::Named(n) => match n.as_str() {
                "utf8" => is_utf8 = true,
                "bytes" => is_bytes = true,
                "bits" => is_bits = true,
                "signed" => signed = true,
                "unsigned" => signed = false,
                "little" => little = true,
                "big" => little = false,
                _ => {}
            },
            BitOption::Size(e) => {
                if let ExprKind::Int(lit) = &e.kind {
                    if let Ok(v) = numeric::int_literal_value(&lit.digits, 10) {
                        width = Some(v as u32);
                    }
                }
            }
        }
    }
    if is_utf8 {
        BitSegmentKind::Utf8
    } else if is_bytes {
        BitSegmentKind::Bytes { size: width }
    } else if is_bits {
        BitSegmentKind::Bits { size: width }
    } else {
        BitSegmentKind::Int {
            size: width.unwrap_or(8).clamp(1, 64) as u8,
            signed,
            little,
        }
    }
}

fn is_left_assoc_chainable(op: BinOp) -> bool {
    matches!(
        op,
        BinOp::Add
            | BinOp::AddFloat
            | BinOp::Sub
            | BinOp::SubFloat
            | BinOp::Mul
            | BinOp::MulFloat
            | BinOp::Div
            | BinOp::DivFloat
            | BinOp::Rem
            | BinOp::Concat
    )
}

fn binop_op(op: BinOp, dst: Reg, a: Reg, b: Reg) -> Op {
    match op {
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
        BinOp::And | BinOp::Or => unreachable!("short-circuit ops use emit_and_or"),
    }
}

/// Walk `body`, tracking locally bound names, and invoke `on_free` for each
/// variable use that is not bound in this function.
fn collect_free_vars_block(
    block: &Block,
    bound: &mut HashSet<String>,
    on_free: &mut dyn FnMut(&str),
) {
    let mut i = 0;
    while i < block.statements.len() {
        match &block.statements[i] {
            Statement::Expr(e) => collect_free_vars_expr(e, bound, on_free),
            Statement::Let(l) => {
                collect_free_vars_expr(&l.value, bound, on_free);
                note_pat_binds(&l.pattern, bound);
            }
            Statement::Use(u) => {
                collect_free_vars_expr(&u.value, bound, on_free);
                for p in &u.patterns {
                    note_pat_binds(p, bound);
                }
            }
            Statement::Fn(_) => {
                let start = i;
                while i < block.statements.len() && matches!(&block.statements[i], Statement::Fn(_))
                {
                    i += 1;
                }
                // Peer names are bound for the whole group.
                for s in &block.statements[start..i] {
                    if let Statement::Fn(f) = s {
                        bound.insert(f.name.text.clone());
                    }
                }
                for s in &block.statements[start..i] {
                    if let Statement::Fn(f) = s {
                        let mut nested_bound = bound.clone();
                        for p in &f.params {
                            nested_bound.insert(p.name.text.clone());
                        }
                        collect_free_vars_block(&f.body, &mut nested_bound, on_free);
                    }
                }
                continue;
            }
        }
        i += 1;
    }
}

fn collect_free_vars_expr(expr: &Expr, bound: &mut HashSet<String>, on_free: &mut dyn FnMut(&str)) {
    let mut stack: Vec<&Expr> = vec![expr];
    while let Some(e) = stack.pop() {
        match &e.kind {
            ExprKind::Var(n) => {
                if !bound.contains(&n.text) {
                    on_free(&n.text);
                }
            }
            ExprKind::Binary { left, right, .. } | ExprKind::Pipe { left, right } => {
                stack.push(right);
                stack.push(left);
            }
            ExprKind::Call { callee, args } => {
                stack.push(callee);
                for a in args {
                    if let ArgValue::Expr(inner) = &a.value {
                        stack.push(inner);
                    }
                }
            }
            ExprKind::Tuple(xs) => stack.extend(xs.iter()),
            ExprKind::List { items, spread } => {
                stack.extend(items.iter());
                if let Some(s) = spread {
                    stack.push(s);
                }
            }
            ExprKind::Paren(inner)
            | ExprKind::Unary { expr: inner, .. }
            | ExprKind::Echo(inner)
            | ExprKind::Field { base: inner, .. }
            | ExprKind::Assert { expr: inner, .. } => stack.push(inner),
            ExprKind::Block(b) => {
                let mut nested = bound.clone();
                collect_free_vars_block(b, &mut nested, on_free);
            }
            ExprKind::Fn { params, body, .. } => {
                let mut nested = bound.clone();
                for p in params {
                    nested.insert(p.name.text.clone());
                }
                collect_free_vars_block(body, &mut nested, on_free);
            }
            ExprKind::Case { subjects, clauses } => {
                for s in subjects {
                    stack.push(s);
                }
                for c in clauses {
                    let mut arm_bound = bound.clone();
                    for row in &c.patterns {
                        for p in &row.patterns {
                            note_pat_binds(p, &mut arm_bound);
                        }
                    }
                    if let Some(g) = &c.guard {
                        collect_free_vars_expr(g, &mut arm_bound, on_free);
                    }
                    collect_free_vars_expr(&c.body, &mut arm_bound, on_free);
                }
            }
            ExprKind::RecordUpdate { base, fields, .. } => {
                stack.push(base);
                for (_, v) in fields {
                    stack.push(v);
                }
            }
            ExprKind::BitArray(segs) => {
                for s in segs {
                    stack.push(&s.value);
                }
            }
            _ => {}
        }
    }
}

/// Map each name bound in this block to the index of the last statement that
/// uses it (so registers can be freed after that statement).
fn last_uses_in_block(block: &Block) -> HashMap<String, usize> {
    let mut last = HashMap::new();
    let mut bound: HashSet<String> = HashSet::new();
    let mut i = 0;
    while i < block.statements.len() {
        match &block.statements[i] {
            Statement::Expr(e) => {
                note_var_uses(e, i, &bound, &mut last);
                i += 1;
            }
            Statement::Let(l) => {
                note_var_uses(&l.value, i, &bound, &mut last);
                note_pat_binds(&l.pattern, &mut bound);
                i += 1;
            }
            Statement::Use(u) => {
                note_var_uses(&u.value, i, &bound, &mut last);
                for p in &u.patterns {
                    note_pat_binds(p, &mut bound);
                }
                i += 1;
            }
            Statement::Fn(_) => {
                let start = i;
                while i < block.statements.len() && matches!(&block.statements[i], Statement::Fn(_))
                {
                    i += 1;
                }
                // Bodies may read outer locals; attribute those uses to the
                // last statement of the group (captures stay live through it).
                let use_i = i - 1;
                for s in &block.statements[start..i] {
                    if let Statement::Fn(f) = s {
                        note_block_uses(&f.body, use_i, &bound, &mut last);
                    }
                }
                // Local fn names are bound for the rest of the block.
                for s in &block.statements[start..i] {
                    if let Statement::Fn(f) = s {
                        bound.insert(f.name.text.clone());
                    }
                }
            }
        }
    }
    last
}

fn note_pat_binds(pat: &Pattern, bound: &mut HashSet<String>) {
    match &pat.kind {
        PatternKind::Var(n) | PatternKind::UnderscoreName(n) => {
            bound.insert(n.text.clone());
        }
        PatternKind::Alias { pattern, name } => {
            bound.insert(name.text.clone());
            note_pat_binds(pattern, bound);
        }
        PatternKind::Tuple(ps) => {
            for p in ps {
                note_pat_binds(p, bound);
            }
        }
        PatternKind::List { items, spread } => {
            for p in items {
                note_pat_binds(p, bound);
            }
            if let Some(s) = spread {
                note_pat_binds(s, bound);
            }
        }
        PatternKind::Constructor {
            args: Some(args), ..
        } => {
            for a in args {
                if let Some(p) = &a.pattern {
                    note_pat_binds(p, bound);
                }
            }
        }
        PatternKind::StringPrefix { rest, .. } => note_pat_binds(rest, bound),
        PatternKind::BitArray(segs) => {
            for s in segs {
                note_pat_binds(&s.pattern, bound);
            }
        }
        _ => {}
    }
}

fn note_block_uses(
    block: &Block,
    stmt_i: usize,
    bound: &HashSet<String>,
    last: &mut HashMap<String, usize>,
) {
    for s in &block.statements {
        match s {
            Statement::Expr(e) => note_var_uses(e, stmt_i, bound, last),
            Statement::Let(l) => note_var_uses(&l.value, stmt_i, bound, last),
            Statement::Use(u) => note_var_uses(&u.value, stmt_i, bound, last),
            Statement::Fn(f) => note_block_uses(&f.body, stmt_i, bound, last),
        }
    }
}

fn note_var_uses(
    expr: &Expr,
    stmt_i: usize,
    bound: &HashSet<String>,
    last: &mut HashMap<String, usize>,
) {
    let mut stack: Vec<&Expr> = vec![expr];
    while let Some(e) = stack.pop() {
        match &e.kind {
            ExprKind::Var(n) => {
                if bound.contains(&n.text) {
                    last.insert(n.text.clone(), stmt_i);
                }
            }
            ExprKind::Binary { left, right, .. } | ExprKind::Pipe { left, right } => {
                stack.push(right);
                stack.push(left);
            }
            ExprKind::Call { callee, args } => {
                stack.push(callee);
                for a in args {
                    if let ArgValue::Expr(inner) = &a.value {
                        stack.push(inner);
                    }
                }
            }
            ExprKind::Tuple(xs) => stack.extend(xs.iter()),
            ExprKind::List { items, spread } => {
                stack.extend(items.iter());
                if let Some(s) = spread {
                    stack.push(s);
                }
            }
            ExprKind::Paren(inner)
            | ExprKind::Unary { expr: inner, .. }
            | ExprKind::Echo(inner)
            | ExprKind::Field { base: inner, .. }
            | ExprKind::Assert { expr: inner, .. } => stack.push(inner),
            ExprKind::Block(b) | ExprKind::Fn { body: b, .. } => {
                note_block_uses(b, stmt_i, bound, last);
            }
            ExprKind::Case { subjects, clauses } => {
                for s in subjects {
                    stack.push(s);
                }
                for c in clauses {
                    if let Some(g) = &c.guard {
                        stack.push(g);
                    }
                    stack.push(&c.body);
                }
            }
            ExprKind::RecordUpdate { base, fields, .. } => {
                stack.push(base);
                for (_, v) in fields {
                    stack.push(v);
                }
            }
            ExprKind::BitArray(segs) => {
                for s in segs {
                    stack.push(&s.value);
                }
            }
            _ => {}
        }
    }
}

fn line_start_offsets(src: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// Map a byte offset to 1-based `(line, column)` counted in Unicode scalar values.
fn byte_to_line_col(src: &str, line_starts: &[usize], byte: usize) -> (u32, u32) {
    let byte = byte.min(src.len());
    let line_idx = match line_starts.binary_search(&byte) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    };
    let line_start = line_starts.get(line_idx).copied().unwrap_or(0);
    let col = src
        .get(line_start..byte)
        .map(|s| s.chars().count())
        .unwrap_or(0);
    ((line_idx as u32) + 1, (col as u32) + 1)
}
