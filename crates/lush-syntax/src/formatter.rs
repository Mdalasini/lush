//! Opinionated formatter (§11.5): 2-space indent, 80-column soft limit, no options.
//! Emits required semicolons and preserves comments from the original token stream.
//!
//! Comparisons and equality are parenthesised only when nested under another
//! comparison or equality (not under `&&` / `||` / arithmetic).

use std::collections::HashSet;

use crate::ast::*;
use crate::span::Span;
use crate::token::{SpannedToken, TokenKind, TriviaKind};

const INDENT: &str = "  ";
const SOFT_LIMIT: usize = 80;

pub fn format_module(module: &Module, tokens: &[SpannedToken], source: &str) -> String {
    let mut f = Formatter::new(source, tokens);
    f.fmt_module(module);
    f.finish()
}

struct Formatter<'a> {
    source: &'a str,
    tokens: &'a [SpannedToken],
    /// Index of the next token whose leading trivia has not been walked yet.
    trivia_idx: usize,
    /// Trivia spans already emitted as trailing or before-`}` comments.
    consumed: HashSet<(usize, usize)>,
    out: String,
    indent: usize,
    at_line_start: bool,
    col: usize,
}

impl<'a> Formatter<'a> {
    fn new(source: &'a str, tokens: &'a [SpannedToken]) -> Self {
        Self {
            source,
            tokens,
            trivia_idx: 0,
            consumed: HashSet::new(),
            out: String::new(),
            indent: 0,
            at_line_start: true,
            col: 0,
        }
    }

    fn finish(mut self) -> String {
        while self.trivia_idx < self.tokens.len() {
            let tok = &self.tokens[self.trivia_idx];
            self.emit_token_comments(tok);
            self.trivia_idx += 1;
        }
        while self.out.ends_with("\n\n") {
            self.out.pop();
        }
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        self.out
    }

    fn trivia_key(span: Span) -> (usize, usize) {
        (span.start.as_usize(), span.end.as_usize())
    }

    fn emit_trivia_before(&mut self, span: Span) {
        while self.trivia_idx < self.tokens.len() {
            let tok = &self.tokens[self.trivia_idx];
            if tok.span.start.as_usize() > span.start.as_usize() {
                break;
            }
            self.emit_token_comments(tok);
            self.trivia_idx += 1;
            if tok.span.start.as_usize() == span.start.as_usize() {
                break;
            }
        }
    }

    fn emit_token_comments(&mut self, tok: &SpannedToken) {
        let mut pending_blank = false;
        for tr in &tok.leading {
            let key = Self::trivia_key(tr.span);
            if self.consumed.contains(&key) {
                continue;
            }
            let text = &self.source[tr.span.range()];
            match tr.kind {
                TriviaKind::Whitespace => {
                    let newlines = text.chars().filter(|c| *c == '\n').count();
                    if newlines >= 2 {
                        pending_blank = true;
                    }
                }
                TriviaKind::LineComment | TriviaKind::DocComment | TriviaKind::ModuleDocComment => {
                    if !self.at_line_start
                        || (pending_blank && !self.out.is_empty() && !self.out.ends_with("\n\n"))
                    {
                        self.newline();
                    }
                    pending_blank = false;
                    self.push(text);
                    self.newline();
                    self.consumed.insert(key);
                }
            }
        }
    }

    /// After `;`, emit same-line trailing comments from the following token.
    fn emit_trailing_comments(&mut self) {
        let mut idx = self.trivia_idx;
        while idx < self.tokens.len() {
            if matches!(self.tokens[idx].kind, TokenKind::Semicolon) {
                if idx + 1 < self.tokens.len() {
                    self.emit_trailing_from_token(idx + 1);
                }
                return;
            }
            idx += 1;
        }
    }

    fn emit_trailing_from_token(&mut self, tok_idx: usize) {
        let leading: Vec<_> = self.tokens[tok_idx]
            .leading
            .iter()
            .map(|tr| (tr.kind, tr.span))
            .collect();
        for (kind, span) in leading {
            let key = Self::trivia_key(span);
            if self.consumed.contains(&key) {
                continue;
            }
            let text = &self.source[span.range()];
            match kind {
                TriviaKind::Whitespace => {
                    if text.contains('\n') {
                        return;
                    }
                }
                TriviaKind::LineComment | TriviaKind::DocComment | TriviaKind::ModuleDocComment => {
                    if !self.at_line_start && !self.out.ends_with(' ') {
                        self.out.push(' ');
                        self.col += 1;
                    }
                    self.out.push_str(text);
                    self.col += text.chars().count();
                    self.at_line_start = false;
                    self.consumed.insert(key);
                }
            }
        }
    }

    /// Before `}`, emit remaining leading comments on that brace at the current indent.
    fn emit_leading_comments_for_rbrace(&mut self) {
        let mut idx = self.trivia_idx;
        while idx < self.tokens.len() {
            if matches!(self.tokens[idx].kind, TokenKind::RBrace) {
                let leading: Vec<_> = self.tokens[idx]
                    .leading
                    .iter()
                    .map(|tr| (tr.kind, tr.span))
                    .collect();
                for (kind, span) in leading {
                    let key = Self::trivia_key(span);
                    if self.consumed.contains(&key) {
                        continue;
                    }
                    match kind {
                        TriviaKind::Whitespace => {}
                        TriviaKind::LineComment
                        | TriviaKind::DocComment
                        | TriviaKind::ModuleDocComment => {
                            if !self.at_line_start {
                                self.newline();
                            }
                            let text = &self.source[span.range()];
                            self.push(text);
                            self.newline();
                            self.consumed.insert(key);
                        }
                    }
                }
                self.trivia_idx = idx + 1;
                return;
            }
            idx += 1;
        }
    }

    fn newline(&mut self) {
        self.out.push('\n');
        self.at_line_start = true;
        self.col = 0;
    }

    fn write_indent(&mut self) {
        if self.at_line_start {
            for _ in 0..self.indent {
                self.out.push_str(INDENT);
                self.col += INDENT.len();
            }
            self.at_line_start = false;
        }
    }

    fn push(&mut self, s: &str) {
        self.write_indent();
        self.out.push_str(s);
        self.col += s.chars().count();
    }

    fn space(&mut self) {
        if !self.at_line_start && !self.out.ends_with(' ') && !self.out.ends_with('\n') {
            self.out.push(' ');
            self.col += 1;
        }
    }

    fn fmt_module(&mut self, module: &Module) {
        for (i, item) in module.items.iter().enumerate() {
            if i > 0 {
                let prev_import = matches!(module.items[i - 1], ModuleItem::Import(_));
                let cur_import = matches!(item, ModuleItem::Import(_));
                if !self.at_line_start {
                    self.newline();
                }
                if !(prev_import && cur_import) {
                    self.newline();
                }
            }
            self.fmt_item(item);
            if !self.at_line_start {
                self.newline();
            }
        }
        while self.out.ends_with("\n\n") {
            self.out.pop();
        }
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    fn fmt_item(&mut self, item: &ModuleItem) {
        match item {
            ModuleItem::Import(i) => self.fmt_import(i),
            ModuleItem::Const(c) => self.fmt_const(c),
            ModuleItem::Fn(f) => self.fmt_fn(f),
            ModuleItem::Type(t) => self.fmt_type_def(t),
        }
    }

    fn fmt_import(&mut self, import: &Import) {
        self.emit_trivia_before(import.span);
        self.push("import ");
        self.push(&import.path.segments.join("/"));
        if let Some(items) = &import.items {
            self.push(".{");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    self.push(", ");
                }
                if item.is_type {
                    self.push("type ");
                }
                self.push_name_or_uname(&item.name);
                if let Some(alias) = &item.alias {
                    self.push(" as ");
                    self.push_name_or_uname(alias);
                }
            }
            self.push("}");
        }
        if let Some(alias) = &import.alias {
            self.push(" as ");
            self.push(&alias.text);
        }
        self.push(";");
        self.emit_trailing_comments();
    }

    fn push_name_or_uname(&mut self, n: &NameOrUName) {
        match n {
            NameOrUName::Name(n) => self.push(&n.text),
            NameOrUName::UName(n) => self.push(&n.text),
        }
    }

    fn fmt_const(&mut self, c: &ConstDef) {
        self.emit_trivia_before(c.span);
        if c.public {
            self.push("pub ");
        }
        self.push("const ");
        self.push(&c.name.text);
        if let Some(ty) = &c.ty {
            self.push(": ");
            self.fmt_type(ty);
        }
        self.push(" = ");
        self.fmt_expr(&c.value, 0);
        self.push(";");
        self.emit_trailing_comments();
    }

    fn fmt_fn(&mut self, f: &FnDef) {
        self.emit_trivia_before(f.span);
        if f.public {
            self.push("pub ");
        }
        self.push("fn ");
        self.push(&f.name.text);
        self.push("(");
        self.fmt_params(&f.params);
        self.push(")");
        if let Some(ret) = &f.return_type {
            self.push(" -> ");
            self.fmt_type(ret);
        }
        self.space();
        self.fmt_block(&f.body);
    }

    fn fmt_params(&mut self, params: &[Param]) {
        for (i, p) in params.iter().enumerate() {
            if i > 0 {
                self.push(", ");
            }
            if let Some(label) = &p.label {
                self.push(&label.text);
                self.space();
            }
            self.push(&p.name.text);
            if let Some(ty) = &p.ty {
                self.push(": ");
                self.fmt_type(ty);
            }
        }
    }

    fn fmt_type_def(&mut self, t: &TypeDef) {
        self.emit_trivia_before(t.span);
        if t.public {
            self.push("pub ");
        }
        if t.opaque {
            self.push("opaque ");
        }
        self.push("type ");
        self.push(&t.name.text);
        if !t.tvars.is_empty() {
            self.push("(");
            for (i, v) in t.tvars.iter().enumerate() {
                if i > 0 {
                    self.push(", ");
                }
                self.push(&v.text);
            }
            self.push(")");
        }
        match &t.body {
            TypeDefBody::Alias(ty) => {
                self.push(" = ");
                self.fmt_type(ty);
            }
            TypeDefBody::Adt(variants) => {
                self.space();
                self.push("{");
                self.newline();
                self.indent += 1;
                for v in variants {
                    self.emit_trivia_before(v.span);
                    self.push(&v.name.text);
                    if let Some(fields) = &v.fields {
                        self.push("(");
                        for (i, field) in fields.iter().enumerate() {
                            if i > 0 {
                                self.push(", ");
                            }
                            if let Some(label) = &field.label {
                                self.push(&label.text);
                                self.push(": ");
                            }
                            self.fmt_type(&field.ty);
                        }
                        self.push(")");
                    }
                    self.newline();
                }
                self.emit_leading_comments_for_rbrace();
                self.indent -= 1;
                self.ensure_line_for_rbrace();
                self.push("}");
            }
        }
    }

    fn ensure_line_for_rbrace(&mut self) {
        while self.out.ends_with("\n\n") {
            self.out.pop();
        }
        if !self.out.ends_with('\n') {
            self.newline();
        }
        self.at_line_start = true;
        self.col = 0;
    }

    fn fmt_block(&mut self, block: &Block) {
        self.push("{");
        if block.statements.is_empty() {
            self.emit_leading_comments_for_rbrace();
            self.push("}");
            return;
        }
        self.newline();
        self.indent += 1;
        for stmt in &block.statements {
            self.fmt_stmt(stmt);
            if !self.at_line_start {
                self.newline();
            }
        }
        self.emit_leading_comments_for_rbrace();
        self.indent -= 1;
        self.ensure_line_for_rbrace();
        self.push("}");
    }

    fn fmt_stmt(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Fn(f) => self.fmt_fn(f),
            Statement::Let(l) => {
                self.emit_trivia_before(l.span);
                self.push("let ");
                if l.assert {
                    self.push("assert ");
                }
                self.fmt_pattern(&l.pattern);
                if let Some(ty) = &l.ty {
                    self.push(": ");
                    self.fmt_type(ty);
                }
                self.push(" = ");
                self.fmt_expr(&l.value, 0);
                if let Some(msg) = &l.message {
                    self.push(" as ");
                    self.push(&msg.raw);
                }
                self.push(";");
                self.emit_trailing_comments();
            }
            Statement::Use(u) => {
                self.emit_trivia_before(u.span);
                self.push("use");
                if !u.patterns.is_empty() {
                    self.push(" ");
                    for (i, p) in u.patterns.iter().enumerate() {
                        if i > 0 {
                            self.push(", ");
                        }
                        self.fmt_pattern(p);
                    }
                }
                self.push(" <- ");
                self.fmt_expr(&u.value, 0);
                self.push(";");
                self.emit_trailing_comments();
            }
            Statement::Expr(e) => {
                self.emit_trivia_before(e.span);
                self.fmt_expr(e, 0);
                self.push(";");
                self.emit_trailing_comments();
            }
        }
    }

    fn fmt_expr(&mut self, expr: &Expr, parent_prec: u8) {
        self.emit_trivia_before(expr.span);
        let prec = expr_prec(expr);
        let wrap = match &expr.kind {
            ExprKind::Paren(_) => false,
            ExprKind::Binary { op, .. } if op.is_comparison_or_eq() => {
                // Only parenthesise under another comparison/equality (levels 6–7).
                matches!(parent_prec, 6 | 7)
            }
            _ => prec < parent_prec,
        };
        if wrap {
            self.push("(");
        }
        match &expr.kind {
            ExprKind::Paren(inner) => {
                self.push("(");
                self.fmt_expr(inner, 0);
                self.push(")");
            }
            ExprKind::Int(i) => self.push(&i.raw),
            ExprKind::Float(f) => self.push(&f.raw),
            ExprKind::String(s) => self.push(&s.raw),
            ExprKind::Var(n) => self.push(&n.text),
            ExprKind::Constructor(c) => self.fmt_ctor(c),
            ExprKind::Tuple(elems) => {
                self.push("#(");
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.fmt_expr(e, 0);
                }
                self.push(")");
            }
            ExprKind::List { items, spread } => {
                let flat = approx_list_width(items, spread.as_deref());
                let multiline =
                    (!items.is_empty() || spread.is_some()) && self.col + flat > SOFT_LIMIT;
                self.push("[");
                if multiline {
                    self.newline();
                    self.indent += 1;
                    for e in items {
                        self.fmt_expr(e, 0);
                        self.push(",");
                        self.newline();
                    }
                    if let Some(s) = spread {
                        self.push("..");
                        self.fmt_expr(s, 0);
                        self.push(",");
                        self.newline();
                    }
                    self.indent -= 1;
                    self.push("]");
                } else {
                    for (i, e) in items.iter().enumerate() {
                        if i > 0 {
                            self.push(", ");
                        }
                        self.fmt_expr(e, 0);
                    }
                    if let Some(s) = spread {
                        if !items.is_empty() {
                            self.push(", ");
                        }
                        self.push("..");
                        self.fmt_expr(s, 0);
                    }
                    self.push("]");
                }
            }
            ExprKind::BitArray(segs) => {
                self.push("<<");
                for (i, seg) in segs.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.fmt_expr(&seg.value, 0);
                    if !seg.options.is_empty() {
                        self.push(":");
                        for (j, opt) in seg.options.iter().enumerate() {
                            if j > 0 {
                                self.push("-");
                            }
                            self.fmt_bit_opt(opt);
                        }
                    }
                }
                self.push(">>");
            }
            ExprKind::RecordUpdate {
                constructor,
                base,
                fields,
            } => {
                self.fmt_ctor(constructor);
                self.push("(..");
                self.fmt_expr(base, 0);
                for (name, value) in fields {
                    self.push(", ");
                    self.push(&name.text);
                    self.push(": ");
                    self.fmt_expr(value, 0);
                }
                self.push(")");
            }
            ExprKind::Call { callee, args } => {
                let flat = approx_call_width(callee, args);
                let multiline = !args.is_empty() && self.col + flat > SOFT_LIMIT;
                self.fmt_expr(callee, 11);
                self.push("(");
                if multiline {
                    self.newline();
                    self.indent += 1;
                    for arg in args {
                        if let Some(label) = &arg.label {
                            self.push(&label.text);
                            self.push(": ");
                        }
                        match &arg.value {
                            ArgValue::Hole => self.push("_"),
                            ArgValue::Expr(e) => self.fmt_expr(e, 0),
                        }
                        self.push(",");
                        self.newline();
                    }
                    self.indent -= 1;
                    self.push(")");
                } else {
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            self.push(", ");
                        }
                        if let Some(label) = &arg.label {
                            self.push(&label.text);
                            self.push(": ");
                        }
                        match &arg.value {
                            ArgValue::Hole => self.push("_"),
                            ArgValue::Expr(e) => self.fmt_expr(e, 0),
                        }
                    }
                    self.push(")");
                }
            }
            ExprKind::Field { base, field } => {
                self.fmt_expr(base, 12);
                self.push(".");
                match field {
                    FieldName::Name(n) => self.push(&n.text),
                    FieldName::UName(n) => self.push(&n.text),
                }
            }
            ExprKind::Binary { .. } => self.fmt_binary_chain(expr, prec),
            ExprKind::Pipe { .. } => self.fmt_pipe_chain(expr),
            ExprKind::Unary { op, expr } => {
                match op {
                    UnaryOp::Neg => self.push("-"),
                    UnaryOp::Not => self.push("!"),
                }
                self.fmt_expr(expr, 10);
            }
            ExprKind::Fn {
                params,
                return_type,
                body,
            } => {
                self.push("fn(");
                self.fmt_params(params);
                self.push(")");
                if let Some(ret) = return_type {
                    self.push(" -> ");
                    self.fmt_type(ret);
                }
                self.space();
                self.fmt_block(body);
            }
            ExprKind::Case { subjects, clauses } => {
                self.push("case ");
                for (i, s) in subjects.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.fmt_expr(s, 0);
                }
                self.push(" {");
                self.newline();
                self.indent += 1;
                for clause in clauses {
                    self.fmt_clause(clause);
                    self.newline();
                }
                self.emit_leading_comments_for_rbrace();
                self.indent -= 1;
                self.ensure_line_for_rbrace();
                self.push("}");
            }
            ExprKind::Todo { message } => {
                self.push("todo");
                if let Some(m) = message {
                    self.push(" as ");
                    self.push(&m.raw);
                }
            }
            ExprKind::Panic { message } => {
                self.push("panic");
                if let Some(m) = message {
                    self.push(" as ");
                    self.push(&m.raw);
                }
            }
            ExprKind::Assert { expr, message } => {
                self.push("assert ");
                self.fmt_expr(expr, 0);
                if let Some(m) = message {
                    self.push(" as ");
                    self.push(&m.raw);
                }
            }
            ExprKind::Echo(e) => {
                self.push("echo ");
                self.fmt_expr(e, 0);
            }
            ExprKind::Block(b) => self.fmt_block(b),
        }
        if wrap {
            self.push(")");
        }
    }

    fn fmt_clause(&mut self, clause: &Clause) {
        self.emit_trivia_before(clause.span);
        for (i, row) in clause.patterns.iter().enumerate() {
            if i > 0 {
                self.push(" | ");
            }
            for (j, p) in row.patterns.iter().enumerate() {
                if j > 0 {
                    self.push(", ");
                }
                self.fmt_pattern(p);
            }
        }
        if let Some(guard) = &clause.guard {
            self.push(" if ");
            self.fmt_expr(guard, 0);
        }
        self.push(" -> ");
        self.fmt_expr(&clause.body, 0);
        self.push(";");
        self.emit_trailing_comments();
    }

    fn fmt_ctor(&mut self, c: &ConstructorRef) {
        if let Some(m) = &c.module {
            self.push(&m.text);
            self.push(".");
        }
        self.push(&c.name.text);
    }

    fn fmt_bit_opt(&mut self, opt: &BitOption) {
        match opt {
            BitOption::Size(e) => {
                self.push("size(");
                self.fmt_expr(e, 0);
                self.push(")");
            }
            BitOption::Named(n) => self.push(n),
        }
    }

    fn fmt_pattern(&mut self, pat: &Pattern) {
        match &pat.kind {
            PatternKind::Int(i) => self.push(&i.raw),
            PatternKind::NegatedInt(i) => {
                self.push("-");
                self.push(&i.raw);
            }
            PatternKind::Float(f) => self.push(&f.raw),
            PatternKind::String(s) => self.push(&s.raw),
            PatternKind::Var(n) | PatternKind::UnderscoreName(n) => self.push(&n.text),
            PatternKind::Discard => self.push("_"),
            PatternKind::Constructor { constructor, args } => {
                self.fmt_ctor(constructor);
                if let Some(args) = args {
                    self.push("(");
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            self.push(", ");
                        }
                        if arg.spread {
                            self.push("..");
                        } else {
                            if let Some(label) = &arg.label {
                                self.push(&label.text);
                                self.push(": ");
                            }
                            if let Some(p) = &arg.pattern {
                                self.fmt_pattern(p);
                            }
                        }
                    }
                    self.push(")");
                }
            }
            PatternKind::Tuple(elems) => {
                self.push("#(");
                for (i, p) in elems.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.fmt_pattern(p);
                }
                self.push(")");
            }
            PatternKind::List { items, spread } => {
                self.push("[");
                for (i, p) in items.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.fmt_pattern(p);
                }
                if let Some(s) = spread {
                    if !items.is_empty() {
                        self.push(", ");
                    }
                    self.push("..");
                    self.fmt_pattern(s);
                }
                self.push("]");
            }
            PatternKind::BitArray(segs) => {
                self.push("<<");
                for (i, seg) in segs.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.fmt_pattern(&seg.pattern);
                    if !seg.options.is_empty() {
                        self.push(":");
                        for (j, opt) in seg.options.iter().enumerate() {
                            if j > 0 {
                                self.push("-");
                            }
                            self.fmt_bit_opt(opt);
                        }
                    }
                }
                self.push(">>");
            }
            PatternKind::StringPrefix { prefix, rest } => {
                self.push(&prefix.raw);
                self.push(" <> ");
                self.fmt_pattern(rest);
            }
            PatternKind::Alias { pattern, name } => {
                self.fmt_pattern(pattern);
                self.push(" as ");
                self.push(&name.text);
            }
        }
    }

    fn fmt_type(&mut self, ty: &TypeExpr) {
        match &ty.kind {
            TypeKind::Named { name, args } => {
                match name {
                    TypeName::Unqualified(n) => self.push(&n.text),
                    TypeName::Qualified { module, name } => {
                        self.push(&module.text);
                        self.push(".");
                        self.push(&name.text);
                    }
                }
                if !args.is_empty() {
                    self.push("(");
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            self.push(", ");
                        }
                        self.fmt_type(a);
                    }
                    self.push(")");
                }
            }
            TypeKind::Var(n) => self.push(&n.text),
            TypeKind::Fn { params, ret } => {
                self.push("fn(");
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.fmt_type(p);
                }
                self.push(") -> ");
                self.fmt_type(ret);
            }
            TypeKind::Tuple(elems) => {
                self.push("#(");
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.fmt_type(e);
                }
                self.push(")");
            }
        }
    }

    /// Format a left-associative binary chain without recursing down the left spine.
    fn fmt_binary_chain(&mut self, expr: &Expr, prec: u8) {
        // Collect (op, right) pairs from the left spine while ops share `prec`.
        // Comparisons/equalities are non-associative and never flatten.
        let mut parts: Vec<(BinOp, &Expr)> = Vec::new();
        let mut cur = expr;
        loop {
            match &cur.kind {
                ExprKind::Binary { left, op, right }
                    if binop_prec(*op) == prec && !op.is_comparison_or_eq() =>
                {
                    parts.push((*op, right));
                    cur = left;
                }
                ExprKind::Binary { left, op, right } => {
                    parts.push((*op, right));
                    cur = left;
                    break;
                }
                _ => break,
            }
        }
        self.fmt_expr(cur, prec);
        for (op, right) in parts.into_iter().rev() {
            let op_str = op.as_str();
            let right_prec = if op.is_comparison_or_eq() {
                prec
            } else {
                prec + 1
            };
            let right_w = approx_expr_width(right);
            let needed = 1 + op_str.len() + 1 + right_w;
            if self.col + needed > SOFT_LIMIT && !self.at_line_start {
                self.newline();
                self.indent += 1;
                self.push(op_str);
                self.push(" ");
                self.fmt_expr(right, right_prec);
                self.indent -= 1;
            } else {
                self.push(" ");
                self.push(op_str);
                self.push(" ");
                self.fmt_expr(right, right_prec);
            }
        }
    }

    /// Format a pipe chain without recursing down the left spine.
    fn fmt_pipe_chain(&mut self, expr: &Expr) {
        let mut rights: Vec<&Expr> = Vec::new();
        let mut cur = expr;
        while let ExprKind::Pipe { left, right } = &cur.kind {
            rights.push(right);
            cur = left;
        }
        self.fmt_expr(cur, 1);
        for right in rights.into_iter().rev() {
            let right_w = approx_expr_width(right);
            let needed = 4 + right_w;
            if self.col + needed > SOFT_LIMIT && !self.at_line_start {
                self.newline();
                self.indent += 1;
                self.push("|> ");
                self.fmt_expr(right, 2);
                self.indent -= 1;
            } else {
                self.push(" |> ");
                self.fmt_expr(right, 2);
            }
        }
    }
}

fn binop_prec(op: BinOp) -> u8 {
    match op {
        BinOp::Or => 4,
        BinOp::And => 5,
        BinOp::Eq | BinOp::NotEq => 6,
        BinOp::Lt
        | BinOp::LtEq
        | BinOp::Gt
        | BinOp::GtEq
        | BinOp::LtFloat
        | BinOp::LtEqFloat
        | BinOp::GtFloat
        | BinOp::GtEqFloat => 7,
        BinOp::Add | BinOp::Sub | BinOp::AddFloat | BinOp::SubFloat | BinOp::Concat => 8,
        BinOp::Mul | BinOp::Div | BinOp::Rem | BinOp::MulFloat | BinOp::DivFloat => 9,
    }
}

fn expr_prec(expr: &Expr) -> u8 {
    match &expr.kind {
        ExprKind::Pipe { .. } => 1,
        ExprKind::Binary { op, .. } => match op {
            BinOp::Or => 4,
            BinOp::And => 5,
            BinOp::Eq | BinOp::NotEq => 6,
            BinOp::Lt
            | BinOp::LtEq
            | BinOp::Gt
            | BinOp::GtEq
            | BinOp::LtFloat
            | BinOp::LtEqFloat
            | BinOp::GtFloat
            | BinOp::GtEqFloat => 7,
            BinOp::Add | BinOp::Sub | BinOp::AddFloat | BinOp::SubFloat | BinOp::Concat => 8,
            BinOp::Mul | BinOp::Div | BinOp::Rem | BinOp::MulFloat | BinOp::DivFloat => 9,
        },
        ExprKind::Unary { .. } => 10,
        ExprKind::Call { .. } => 11,
        ExprKind::Field { .. } => 12,
        ExprKind::Paren(inner) => expr_prec(inner),
        _ => 100,
    }
}

/// Approximate flat width of a simple expression (idents, literals, short calls).
fn approx_expr_width(expr: &Expr) -> usize {
    match &expr.kind {
        ExprKind::Paren(inner) => 2 + approx_expr_width(inner),
        ExprKind::Int(i) => i.raw.len(),
        ExprKind::Float(f) => f.raw.len(),
        ExprKind::String(s) => s.raw.len(),
        ExprKind::Var(n) => n.text.len(),
        ExprKind::Constructor(c) => {
            let mut w = c.name.text.len();
            if let Some(m) = &c.module {
                w += m.text.len() + 1;
            }
            w
        }
        ExprKind::Call { callee, args } => approx_call_width(callee, args),
        ExprKind::List { items, spread } => approx_list_width(items, spread.as_deref()),
        ExprKind::Tuple(elems) => {
            let mut w = 3; // #()
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    w += 2;
                }
                w += approx_expr_width(e);
            }
            w
        }
        ExprKind::Binary { .. } => {
            let mut w = 0usize;
            let mut cur = expr;
            loop {
                match &cur.kind {
                    ExprKind::Binary { left, op, right } => {
                        w += 1 + op.as_str().len() + 1 + approx_expr_width(right);
                        cur = left;
                    }
                    _ => {
                        w += approx_expr_width(cur);
                        break;
                    }
                }
            }
            w
        }
        ExprKind::Pipe { .. } => {
            let mut w = 0usize;
            let mut cur = expr;
            while let ExprKind::Pipe { left, right } = &cur.kind {
                w += 4 + approx_expr_width(right);
                cur = left;
            }
            w + approx_expr_width(cur)
        }
        ExprKind::Unary { expr, .. } => 1 + approx_expr_width(expr),
        ExprKind::Field { base, field } => {
            let flen = match field {
                FieldName::Name(n) => n.text.len(),
                FieldName::UName(n) => n.text.len(),
            };
            approx_expr_width(base) + 1 + flen
        }
        ExprKind::Todo { message } | ExprKind::Panic { message } => {
            4 + message.as_ref().map(|m| 4 + m.raw.len()).unwrap_or(0)
        }
        ExprKind::Assert { expr, message } => {
            7 + approx_expr_width(expr) + message.as_ref().map(|m| 4 + m.raw.len()).unwrap_or(0)
        }
        ExprKind::Echo(e) => 5 + approx_expr_width(e),
        _ => 40,
    }
}

fn approx_call_width(callee: &Expr, args: &[Arg]) -> usize {
    let mut w = approx_expr_width(callee) + 2; // ()
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            w += 2;
        }
        if let Some(label) = &arg.label {
            w += label.text.len() + 2;
        }
        match &arg.value {
            ArgValue::Hole => w += 1,
            ArgValue::Expr(e) => w += approx_expr_width(e),
        }
    }
    w
}

fn approx_list_width(items: &[Expr], spread: Option<&Expr>) -> usize {
    let mut w = 2; // []
    for (i, e) in items.iter().enumerate() {
        if i > 0 {
            w += 2;
        }
        w += approx_expr_width(e);
    }
    if let Some(s) = spread {
        if !items.is_empty() {
            w += 2;
        }
        w += 2 + approx_expr_width(s);
    }
    w
}

#[cfg(test)]
mod comment_tests {
    use crate::{format_source, parse_module};

    #[test]
    fn trailing_and_before_close_comments() {
        let src = r#"fn f() {
  let x = 1; // trailing
  case x {
    _ -> 3; // after arm
  };
  // before close
}
"#;
        let formatted = format_source(src).expect("format");
        assert!(
            formatted.contains("; // trailing") || formatted.contains("1; // trailing"),
            "trailing comment should stay on same line:\n{formatted}"
        );
        assert!(
            formatted.contains("// after arm"),
            "arm comment missing:\n{formatted}"
        );
        // Comment before close must remain inside the function.
        let close = formatted.rfind('}').unwrap();
        let before_close = formatted[..close].rfind("// before close");
        assert!(
            before_close.is_some(),
            "comment before close escaped the function:\n{formatted}"
        );
        let _ = parse_module(&formatted);
    }
}
