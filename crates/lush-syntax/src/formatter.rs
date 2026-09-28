//! Opinionated Lush formatter (`spec.md` §11.5).
//!
//! Zero options, 2-space indent, 80-column soft limit, idempotent, preserves comments.

use crate::ast::*;
use crate::error::SyntaxError;
use crate::lexer::{lex_normalized, Token};
use crate::parser::parse_module;
use crate::span::Span;

const INDENT: &str = "  ";
// Soft column limit from §11.5; used as guidance when expanding lists later.
#[allow(dead_code)]
const SOFT_COLUMN: usize = 80;

/// Format `source` into canonical Lush text.
pub fn format_source(source: &str) -> Result<String, Vec<SyntaxError>> {
    let normalized = source.replace("\r\n", "\n");
    let module = parse_module(&normalized)?;
    let comments = collect_comments(&normalized)?;
    let mut printer = Printer::new(comments);
    printer.module(&module);
    printer.emit_remaining_comments();
    let mut out = printer.buffer;
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

#[derive(Debug, Clone)]
struct Comment {
    span: Span,
    text: String,
}

fn collect_comments(source: &str) -> Result<Vec<Comment>, Vec<SyntaxError>> {
    let tokens = lex_normalized(source)?;
    Ok(tokens
        .into_iter()
        .filter(|t| {
            matches!(
                t.kind,
                Token::LineComment | Token::DocComment | Token::ModuleDocComment
            )
        })
        .map(|t| Comment {
            span: t.span,
            text: t.span.slice(source).to_string(),
        })
        .collect())
}

struct Printer {
    buffer: String,
    indent: usize,
    comments: Vec<Comment>,
    comment_idx: usize,
    at_line_start: bool,
}

impl Printer {
    fn new(comments: Vec<Comment>) -> Self {
        Self {
            buffer: String::new(),
            indent: 0,
            comments,
            comment_idx: 0,
            at_line_start: true,
        }
    }

    fn emit_comments_before(&mut self, span: Span) {
        while self.comment_idx < self.comments.len()
            && self.comments[self.comment_idx].span.start < span.start
        {
            let comment = self.comments[self.comment_idx].text.clone();
            self.comment_idx += 1;
            if !self.at_line_start {
                self.newline();
            }
            self.write_indent();
            self.buffer.push_str(&comment);
            self.newline();
        }
    }

    fn emit_remaining_comments(&mut self) {
        while self.comment_idx < self.comments.len() {
            let comment = self.comments[self.comment_idx].text.clone();
            self.comment_idx += 1;
            if !self.at_line_start {
                self.newline();
            }
            self.write_indent();
            self.buffer.push_str(&comment);
            self.newline();
        }
    }

    fn write_indent(&mut self) {
        if self.at_line_start {
            for _ in 0..self.indent {
                self.buffer.push_str(INDENT);
            }
            self.at_line_start = false;
        }
    }

    fn push(&mut self, s: &str) {
        self.write_indent();
        self.buffer.push_str(s);
        self.at_line_start = s.ends_with('\n');
    }

    fn space(&mut self) {
        if !self.at_line_start && !self.buffer.ends_with(' ') && !self.buffer.ends_with('\n') {
            self.buffer.push(' ');
        }
    }

    fn newline(&mut self) {
        if !self.buffer.ends_with('\n') {
            self.buffer.push('\n');
        }
        self.at_line_start = true;
    }

    fn module(&mut self, module: &Module) {
        for import in &module.imports {
            self.emit_comments_before(import.span);
            self.import(import);
            self.newline();
        }
        if !module.imports.is_empty() && !module.definitions.is_empty() {
            self.newline();
        }
        for (i, def) in module.definitions.iter().enumerate() {
            let span = def_span(def);
            self.emit_comments_before(span);
            self.definition(def);
            if i + 1 != module.definitions.len() {
                self.newline();
            }
        }
    }

    fn import(&mut self, import: &Import) {
        self.push("import ");
        self.push(&import.path.join("/"));
        if let Some(items) = &import.items {
            self.push(".{");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    self.push(", ");
                }
                if item.is_type {
                    self.push("type ");
                }
                self.push(&item.name);
                if let Some(alias) = &item.alias {
                    self.push(" as ");
                    self.push(alias);
                }
            }
            self.push("}");
        }
        if let Some(alias) = &import.alias {
            self.push(" as ");
            self.push(alias);
        }
        self.push(";");
    }

    fn definition(&mut self, def: &Definition) {
        match def {
            Definition::Fn(f) => self.fn_def(f),
            Definition::Type(t) => self.type_def(t),
            Definition::Const(c) => self.const_def(c),
        }
    }

    fn fn_def(&mut self, f: &FnDef) {
        self.emit_comments_before(f.span);
        if f.is_pub {
            self.push("pub ");
        }
        self.push("fn ");
        self.push(&f.name);
        self.push("(");
        for (i, p) in f.params.iter().enumerate() {
            if i > 0 {
                self.push(", ");
            }
            self.param(p);
        }
        self.push(")");
        if let Some(ret) = &f.return_type {
            self.push(" -> ");
            self.ty(ret);
        }
        self.space();
        self.block(&f.body);
        self.newline();
    }

    fn param(&mut self, p: &Param) {
        if let Some(label) = &p.label {
            self.push(label);
            self.space();
        }
        self.push(&p.name);
        if let Some(ty) = &p.ty {
            self.push(": ");
            self.ty(ty);
        }
    }

    fn type_def(&mut self, t: &TypeDef) {
        self.emit_comments_before(t.span);
        if t.is_pub {
            self.push("pub ");
        }
        if t.is_opaque {
            self.push("opaque ");
        }
        self.push("type ");
        self.push(&t.name);
        if !t.params.is_empty() {
            self.push("(");
            self.push(&t.params.join(", "));
            self.push(")");
        }
        match &t.body {
            TypeDefBody::Alias(ty) => {
                self.push(" = ");
                self.ty(ty);
                self.newline();
            }
            TypeDefBody::Adt(variants) => {
                self.space();
                self.push("{");
                self.newline();
                self.indent += 1;
                for v in variants {
                    self.write_indent();
                    self.push(&v.name);
                    if !v.fields.is_empty() {
                        self.push("(");
                        for (i, field) in v.fields.iter().enumerate() {
                            if i > 0 {
                                self.push(", ");
                            }
                            if let Some(label) = &field.label {
                                self.push(label);
                                self.push(": ");
                            }
                            self.ty(&field.ty);
                        }
                        self.push(")");
                    }
                    self.newline();
                }
                self.indent -= 1;
                self.push("}");
                self.newline();
            }
        }
    }

    fn const_def(&mut self, c: &ConstDef) {
        self.emit_comments_before(c.span);
        if c.is_pub {
            self.push("pub ");
        }
        self.push("const ");
        self.push(&c.name);
        if let Some(ty) = &c.ty {
            self.push(": ");
            self.ty(ty);
        }
        self.push(" = ");
        self.expr(&c.value);
        self.push(";");
        self.newline();
    }

    fn block(&mut self, block: &Block) {
        self.push("{");
        if block.statements.is_empty() {
            self.push("}");
            return;
        }
        self.newline();
        self.indent += 1;
        for stmt in &block.statements {
            self.statement(stmt);
        }
        self.indent -= 1;
        self.write_indent();
        self.push("}");
    }

    fn statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Fn(f) => {
                // Local functions are not pub in practice.
                self.emit_comments_before(f.span);
                self.push("fn ");
                self.push(&f.name);
                self.push("(");
                for (i, p) in f.params.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.param(p);
                }
                self.push(")");
                if let Some(ret) = &f.return_type {
                    self.push(" -> ");
                    self.ty(ret);
                }
                self.space();
                self.block(&f.body);
                self.newline();
            }
            Statement::Let(l) => {
                self.emit_comments_before(l.span);
                self.write_indent();
                self.push("let ");
                if l.is_assert {
                    self.push("assert ");
                }
                self.pattern(&l.pattern);
                if let Some(ty) = &l.ty {
                    self.push(": ");
                    self.ty(ty);
                }
                self.push(" = ");
                self.expr(&l.value);
                if let Some(msg) = &l.message {
                    self.push(" as ");
                    self.expr(msg);
                }
                self.push(";");
                self.newline();
            }
            Statement::Use(u) => {
                self.emit_comments_before(u.span);
                self.write_indent();
                self.push("use ");
                for (i, p) in u.patterns.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.pattern(p);
                }
                if !u.patterns.is_empty() {
                    self.space();
                }
                self.push("<- ");
                self.expr(&u.value);
                self.push(";");
                self.newline();
            }
            Statement::Expr(e) => {
                self.emit_comments_before(e.span);
                self.write_indent();
                self.expr(e);
                self.push(";");
                self.newline();
            }
        }
    }

    fn ty(&mut self, ty: &TypeExpr) {
        match &ty.kind {
            TypeKind::Var(name) => self.push(name),
            TypeKind::Named { module, name, args } => {
                if let Some(m) = module {
                    self.push(m);
                    self.push(".");
                }
                self.push(name);
                if !args.is_empty() {
                    self.push("(");
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            self.push(", ");
                        }
                        self.ty(a);
                    }
                    self.push(")");
                }
            }
            TypeKind::Fn { params, ret } => {
                self.push("fn(");
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.ty(p);
                }
                self.push(") -> ");
                self.ty(ret);
            }
            TypeKind::Tuple(elems) => {
                self.push("#(");
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.ty(e);
                }
                self.push(")");
            }
        }
    }

    fn expr(&mut self, expr: &Expr) {
        self.expr_prec(expr, 0);
    }

    fn expr_prec(&mut self, expr: &Expr, parent_bp: u8) {
        match &expr.kind {
            ExprKind::Int(s) | ExprKind::Float(s) | ExprKind::String(s) => self.push(s),
            ExprKind::Ident(s) | ExprKind::Constructor(s) => self.push(s),
            ExprKind::Field { base, field, .. } => {
                self.expr_prec(base, 12);
                self.push(".");
                self.push(field);
            }
            ExprKind::Call { callee, args } => {
                self.expr_prec(callee, 11);
                self.push("(");
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    if let Some(label) = &arg.label {
                        self.push(label);
                        self.push(": ");
                    }
                    match &arg.value {
                        ArgValue::Hole => self.push("_"),
                        ArgValue::Expr(e) => self.expr(e),
                    }
                }
                self.push(")");
            }
            ExprKind::Unary { op, expr } => {
                self.push(match op {
                    UnaryOp::Neg => "-",
                    UnaryOp::Not => "!",
                });
                self.expr_prec(expr, 10);
            }
            ExprKind::Binary { left, op, right } => {
                let (l_bp, r_bp) = bin_bp(*op);
                let need_paren = l_bp < parent_bp;
                if need_paren {
                    self.push("(");
                }
                self.expr_prec(left, l_bp);
                self.space();
                self.push(bin_str(*op));
                self.space();
                self.expr_prec(right, r_bp);
                if need_paren {
                    self.push(")");
                }
            }
            ExprKind::Pipe { left, right } => {
                let need_paren = parent_bp > 1;
                if need_paren {
                    self.push("(");
                }
                self.expr_prec(left, 1);
                self.push(" |> ");
                self.expr_prec(right, 2);
                if need_paren {
                    self.push(")");
                }
            }
            ExprKind::Fn {
                params,
                return_type,
                body,
            } => {
                self.push("fn(");
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.param(p);
                }
                self.push(")");
                if let Some(ret) = return_type {
                    self.push(" -> ");
                    self.ty(ret);
                }
                self.space();
                self.block(body);
            }
            ExprKind::Case { subjects, clauses } => {
                self.push("case ");
                for (i, s) in subjects.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.expr(s);
                }
                self.space();
                self.push("{");
                self.newline();
                self.indent += 1;
                for clause in clauses {
                    self.write_indent();
                    for (i, row) in clause.patterns.iter().enumerate() {
                        if i > 0 {
                            self.push(" | ");
                        }
                        for (j, p) in row.patterns.iter().enumerate() {
                            if j > 0 {
                                self.push(", ");
                            }
                            self.pattern(p);
                        }
                    }
                    if let Some(guard) = &clause.guard {
                        self.push(" if ");
                        self.expr(guard);
                    }
                    self.push(" -> ");
                    self.expr(&clause.body);
                    self.push(";");
                    self.newline();
                }
                self.indent -= 1;
                self.write_indent();
                self.push("}");
            }
            ExprKind::Todo { message } => {
                self.push("todo");
                if let Some(msg) = message {
                    self.push(" as ");
                    self.expr(msg);
                }
            }
            ExprKind::Panic { message } => {
                self.push("panic");
                if let Some(msg) = message {
                    self.push(" as ");
                    self.expr(msg);
                }
            }
            ExprKind::Assert { condition, message } => {
                self.push("assert ");
                self.expr(condition);
                if let Some(msg) = message {
                    self.push(" as ");
                    self.expr(msg);
                }
            }
            ExprKind::Echo { value } => {
                self.push("echo ");
                self.expr(value);
            }
            ExprKind::Block(b) => self.block(b),
            ExprKind::List { items, spread } => {
                self.push("[");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.expr(item);
                }
                if let Some(s) = spread {
                    if !items.is_empty() {
                        self.push(", ");
                    }
                    self.push("..");
                    self.expr(s);
                }
                self.push("]");
            }
            ExprKind::Tuple(elems) => {
                self.push("#(");
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.expr(e);
                }
                self.push(")");
            }
            ExprKind::BitArray(segs) => {
                self.push("<<");
                for (i, seg) in segs.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.bit_segment(seg);
                }
                self.push(">>");
            }
            ExprKind::RecordUpdate {
                constructor,
                base,
                fields,
            } => {
                self.push(constructor);
                self.push("(..");
                self.expr(base);
                for (name, value) in fields {
                    self.push(", ");
                    self.push(name);
                    self.push(": ");
                    self.expr(value);
                }
                self.push(")");
            }
        }
    }

    fn pattern(&mut self, pattern: &Pattern) {
        match &pattern.kind {
            PatternKind::Discard => self.push("_"),
            PatternKind::Var(n)
            | PatternKind::Int(n)
            | PatternKind::Float(n)
            | PatternKind::String(n) => self.push(n),
            PatternKind::Constructor {
                module,
                name,
                fields,
                with_spread,
            } => {
                if let Some(m) = module {
                    self.push(m);
                    self.push(".");
                }
                self.push(name);
                if !fields.is_empty() || *with_spread {
                    self.push("(");
                    for (i, f) in fields.iter().enumerate() {
                        if i > 0 {
                            self.push(", ");
                        }
                        if let Some(label) = &f.label {
                            self.push(label);
                            self.push(": ");
                        }
                        self.pattern(&f.pattern);
                    }
                    if *with_spread {
                        if !fields.is_empty() {
                            self.push(", ");
                        }
                        self.push("..");
                    }
                    self.push(")");
                }
            }
            PatternKind::Tuple(elems) => {
                self.push("#(");
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.pattern(e);
                }
                self.push(")");
            }
            PatternKind::List { items, rest } => {
                self.push("[");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.pattern(item);
                }
                if let Some(r) = rest {
                    if !items.is_empty() {
                        self.push(", ");
                    }
                    self.push("..");
                    if !matches!(r.kind, PatternKind::Discard) {
                        self.pattern(r);
                    }
                }
                self.push("]");
            }
            PatternKind::StringPrefix { literal, rest } => {
                self.push(literal);
                self.push(" <> ");
                self.pattern(rest);
            }
            PatternKind::BitArray(segs) => {
                self.push("<<");
                for (i, seg) in segs.iter().enumerate() {
                    if i > 0 {
                        self.push(", ");
                    }
                    self.bit_segment(seg);
                }
                self.push(">>");
            }
            PatternKind::As { pattern, name } => {
                self.pattern(pattern);
                self.push(" as ");
                self.push(name);
            }
        }
    }

    fn bit_segment(&mut self, seg: &BitSegment) {
        match &seg.value {
            BitSegmentValue::Expr(e) => self.expr(e),
            BitSegmentValue::Pattern(p) => self.pattern(p),
        }
        if !seg.options.is_empty() {
            self.push(":");
            self.push(&seg.options.join("-"));
        }
    }
}

fn def_span(def: &Definition) -> Span {
    match def {
        Definition::Fn(f) => f.span,
        Definition::Type(t) => t.span,
        Definition::Const(c) => c.span,
    }
}

fn bin_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::MulFloat => "*.",
        BinOp::DivFloat => "/.",
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::AddFloat => "+.",
        BinOp::SubFloat => "-.",
        BinOp::Concat => "<>",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::LtFloat => "<.",
        BinOp::LeFloat => "<=.",
        BinOp::GtFloat => ">.",
        BinOp::GeFloat => ">=.",
        BinOp::Eq => "==",
        BinOp::NotEq => "!=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}

fn bin_bp(op: BinOp) -> (u8, u8) {
    match op {
        BinOp::Mul | BinOp::Div | BinOp::Rem | BinOp::MulFloat | BinOp::DivFloat => (9, 10),
        BinOp::Add | BinOp::Sub | BinOp::AddFloat | BinOp::SubFloat | BinOp::Concat => (8, 9),
        BinOp::Lt
        | BinOp::Le
        | BinOp::Gt
        | BinOp::Ge
        | BinOp::LtFloat
        | BinOp::LeFloat
        | BinOp::GtFloat
        | BinOp::GeFloat => (7, 7),
        BinOp::Eq | BinOp::NotEq => (6, 6),
        BinOp::And => (5, 6),
        BinOp::Or => (4, 5),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_function_and_is_idempotent() {
        let src = r#"pub fn add(a:Int,b:Int)->Int{a+b;}"#;
        let once = format_source(src).unwrap();
        let twice = format_source(&once).unwrap();
        assert_eq!(once, twice);
        assert!(once.contains("pub fn add"));
        assert!(once.contains("a + b;"));
    }

    #[test]
    fn preserves_line_comments() {
        let src = "\
pub fn main() -> Nil {
  // keep me
  echo \"hi\";
}
";
        let formatted = format_source(src).unwrap();
        assert!(formatted.contains("// keep me"));
        let again = format_source(&formatted).unwrap();
        assert_eq!(formatted, again);
    }

    #[test]
    fn round_trip_parse_format_parse() {
        let src = r#"
import lush/list;
pub type Shape {
  Circle(radius: Float)
  Point
}
pub fn main() -> Int {
  let xs = [1, 2, 3];
  case xs {
    [] -> 0;
    [x, ..] -> x;
  };
}
"#;
        let formatted = format_source(src).unwrap();
        let module = parse_module(&formatted).expect("formatted source must parse");
        assert_eq!(module.definitions.len(), 2);
        assert_eq!(format_source(&formatted).unwrap(), formatted);
    }
}
