//! Recursive-descent parser for the full §12 grammar.

use crate::ast::*;
use crate::codes;
use crate::diagnostic::{Diagnostic, DiagnosticKind, DiagnosticSink, Severity};
use crate::span::Span;
use crate::token::{SpannedToken, StringLit, TokenKind};

const MAX_DEPTH: u32 = 256;
/// Maximum postfix/binary chain length within one expression (protects later recursive walks).
const MAX_CHAIN: u32 = 4096;

pub struct ParseResult {
    pub module: Option<Module>,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn parse(tokens: &[SpannedToken]) -> ParseResult {
    let mut parser = Parser::new(tokens);
    let module = parser.parse_module();
    ParseResult {
        module: Some(module),
        diagnostics: parser.diagnostics.into_diagnostics(),
    }
}

pub fn parse_expression_only(tokens: &[SpannedToken]) -> (Option<Expr>, Vec<Diagnostic>) {
    let mut parser = Parser::new(tokens);
    let expr = parser.parse_expression(0);
    let diagnostics = parser.diagnostics.into_diagnostics();
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        (None, diagnostics)
    } else {
        (Some(expr), diagnostics)
    }
}

struct Parser<'a> {
    tokens: &'a [SpannedToken],
    pos: usize,
    diagnostics: DiagnosticSink,
    depth: u32,
    stopped: bool,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [SpannedToken]) -> Self {
        Self {
            tokens,
            pos: 0,
            diagnostics: DiagnosticSink::new(),
            depth: 0,
            stopped: false,
        }
    }

    fn current(&self) -> &SpannedToken {
        &self.tokens[self.pos.min(self.tokens.len().saturating_sub(1))]
    }

    fn kind(&self) -> &TokenKind {
        &self.current().kind
    }

    fn span(&self) -> Span {
        self.current().span
    }

    fn bump(&mut self) -> &SpannedToken {
        let tok = &self.tokens[self.pos.min(self.tokens.len().saturating_sub(1))];
        if self.pos < self.tokens.len() && !matches!(tok.kind, TokenKind::Eof) {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, expected: TokenKind, hint: &str) -> Span {
        if std::mem::discriminant(self.kind()) == std::mem::discriminant(&expected) {
            let span = self.span();
            self.bump();
            span
        } else {
            let span = self.span();
            self.error(
                span,
                codes::E0100_EXPECTED_TOKEN,
                format!(
                    "expected `{}`, found {}",
                    expected.as_str(),
                    self.kind().describe()
                ),
                Some(hint.to_string()),
            );
            span
        }
    }

    fn error(&mut self, span: Span, code: &str, message: impl Into<String>, hint: Option<String>) {
        if self.stopped {
            return;
        }
        let before_capped = self.diagnostics.is_capped();
        self.diagnostics
            .error(code, message, span, hint, DiagnosticKind::Parser);
        if self.diagnostics.is_capped() && !before_capped {
            self.stopped = true;
        }
    }

    fn enter_depth(&mut self) -> bool {
        if self.depth >= MAX_DEPTH {
            self.error(
                self.span(),
                codes::E0190_TOO_DEEP,
                "expression nesting is too deep",
                Some(format!("maximum nesting depth is {MAX_DEPTH}")),
            );
            return false;
        }
        self.depth += 1;
        true
    }

    fn exit_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn dummy_expr(&self) -> Expr {
        Expr {
            kind: ExprKind::Todo { message: None },
            span: self.span(),
        }
    }

    fn dummy_pattern(&self) -> Pattern {
        Pattern {
            kind: PatternKind::Discard,
            span: self.span(),
        }
    }

    fn dummy_type(&self) -> TypeExpr {
        TypeExpr {
            kind: TypeKind::Var(Name {
                text: "_".into(),
                span: self.span(),
            }),
            span: self.span(),
        }
    }

    fn dummy_block(&self) -> Block {
        Block {
            statements: vec![],
            span: self.span(),
        }
    }

    /// Sync to the next module-level item keyword after an unexpected token.
    fn sync_module_item(&mut self) {
        if !matches!(self.kind(), TokenKind::Eof) {
            self.bump();
        }
        while !matches!(
            self.kind(),
            TokenKind::Import
                | TokenKind::Pub
                | TokenKind::Fn
                | TokenKind::Type
                | TokenKind::Opaque
                | TokenKind::Const
                | TokenKind::Eof
        ) {
            self.bump();
        }
    }

    /// Sync to the end of a statement (`;` consumed, or `}` left for the block).
    fn sync_statement(&mut self) {
        while !matches!(
            self.kind(),
            TokenKind::Semicolon | TokenKind::RBrace | TokenKind::Eof
        ) {
            if self.stopped {
                return;
            }
            self.bump();
        }
        if matches!(self.kind(), TokenKind::Semicolon) {
            self.bump();
        }
    }

    fn expect_semicolon_or_sync(&mut self, hint: &str) {
        if matches!(self.kind(), TokenKind::Semicolon) {
            self.bump();
        } else {
            let span = self.span();
            self.error(
                span,
                codes::E0100_EXPECTED_TOKEN,
                format!("expected `;`, found {}", self.kind().describe()),
                Some(hint.to_string()),
            );
            self.sync_statement();
        }
    }

    fn parse_module(&mut self) -> Module {
        let start = self.span();
        let mut items = Vec::new();
        while !matches!(self.kind(), TokenKind::Eof) {
            if self.stopped {
                break;
            }
            match self.kind() {
                TokenKind::Import => items.push(ModuleItem::Import(self.parse_import())),
                TokenKind::Pub => {
                    self.bump();
                    match self.kind() {
                        TokenKind::Fn => {
                            let mut f = self.parse_fn_def();
                            f.public = true;
                            items.push(ModuleItem::Fn(f));
                        }
                        TokenKind::Const => {
                            let mut c = self.parse_const_def();
                            c.public = true;
                            items.push(ModuleItem::Const(c));
                        }
                        TokenKind::Opaque | TokenKind::Type => {
                            let mut t = self.parse_type_def();
                            t.public = true;
                            items.push(ModuleItem::Type(t));
                        }
                        _ => {
                            self.error(
                                self.span(),
                                codes::E0101_AFTER_PUB,
                                "expected `fn`, `const`, `type`, or `opaque` after `pub`",
                                Some("write `pub fn`, `pub const`, or `pub type`".into()),
                            );
                            self.sync_module_item();
                        }
                    }
                }
                TokenKind::Fn => items.push(ModuleItem::Fn(self.parse_fn_def())),
                TokenKind::Const => items.push(ModuleItem::Const(self.parse_const_def())),
                TokenKind::Opaque | TokenKind::Type => {
                    items.push(ModuleItem::Type(self.parse_type_def()))
                }
                _ => {
                    self.error(
                        self.span(),
                        codes::E0102_MODULE_ITEM,
                        format!("expected a module item, found {}", self.kind().describe()),
                        Some("modules contain `import`, `fn`, `type`, and `const` items".into()),
                    );
                    self.sync_module_item();
                }
            }
        }
        let end = self.tokens.last().map(|t| t.span).unwrap_or(start);
        Module {
            items,
            span: start.merge(end),
        }
    }

    fn parse_import(&mut self) -> Import {
        let start = self.expect(TokenKind::Import, "imports start with `import`");
        let path = self.parse_import_path();
        let items = if matches!(self.kind(), TokenKind::Dot) {
            self.bump();
            self.expect(TokenKind::LBrace, "selective imports use `{ ... }`");
            let mut items = Vec::new();
            if !matches!(self.kind(), TokenKind::RBrace) {
                loop {
                    items.push(self.parse_import_item());
                    if matches!(self.kind(), TokenKind::Comma) {
                        self.bump();
                        if matches!(self.kind(), TokenKind::RBrace) {
                            break;
                        }
                        continue;
                    }
                    break;
                }
            }
            self.expect(TokenKind::RBrace, "close the selective import with `}`");
            Some(items)
        } else {
            None
        };
        let alias = if matches!(self.kind(), TokenKind::As) {
            self.bump();
            Some(self.parse_name())
        } else {
            None
        };
        let end = self.expect(TokenKind::Semicolon, "every import ends with `;`");
        Import {
            path,
            items,
            alias,
            span: start.merge(end),
        }
    }

    fn parse_import_path(&mut self) -> ImportPath {
        let start = self.span();
        let mut segments = Vec::new();
        // Slash-separated path; a segment may itself contain dots (e.g. `github.com`).
        segments.push(self.parse_path_segment());
        while matches!(self.kind(), TokenKind::Slash) {
            self.bump();
            segments.push(self.parse_path_segment());
        }
        let end = self.tokens[self.pos.saturating_sub(1)].span;
        ImportPath {
            segments,
            span: start.merge(end),
        }
    }

    fn parse_path_segment(&mut self) -> String {
        // Segment may be `ident` or `ident.ident` (domain).
        let mut parts = Vec::new();
        match self.kind() {
            TokenKind::Ident(s) => {
                parts.push(s.clone());
                self.bump();
            }
            TokenKind::UIdent(s) => {
                // Unusual but accept for recovery.
                parts.push(s.clone());
                self.bump();
            }
            _ => {
                self.error(
                    self.span(),
                    codes::E0103_IMPORT_PATH,
                    "expected an import path segment",
                    Some("paths look like `lush/list` or `github.com/user/repo`".into()),
                );
                return String::new();
            }
        }
        // Allow dots inside a segment for domains: github.com
        while matches!(self.kind(), TokenKind::Dot) {
            // Lookahead: if next is `{`, this dot starts a selective import.
            if self.pos + 1 < self.tokens.len()
                && matches!(self.tokens[self.pos + 1].kind, TokenKind::LBrace)
            {
                break;
            }
            // If next is Ident that continues the domain.
            if self.pos + 1 < self.tokens.len()
                && matches!(self.tokens[self.pos + 1].kind, TokenKind::Ident(_))
            {
                self.bump(); // dot
                if let TokenKind::Ident(s) = self.kind() {
                    parts.push(s.clone());
                    self.bump();
                }
            } else {
                break;
            }
        }
        parts.join(".")
    }

    fn parse_import_item(&mut self) -> ImportItem {
        let start = self.span();
        let is_type = if matches!(self.kind(), TokenKind::Type) {
            self.bump();
            true
        } else {
            false
        };
        let name = self.parse_name_or_uname();
        let alias = if matches!(self.kind(), TokenKind::As) {
            self.bump();
            Some(self.parse_name_or_uname())
        } else {
            None
        };
        let end = alias
            .as_ref()
            .map(|a| match a {
                NameOrUName::Name(n) => n.span,
                NameOrUName::UName(n) => n.span,
            })
            .unwrap_or(match &name {
                NameOrUName::Name(n) => n.span,
                NameOrUName::UName(n) => n.span,
            });
        ImportItem {
            is_type,
            name,
            alias,
            span: start.merge(end),
        }
    }

    fn parse_const_def(&mut self) -> ConstDef {
        let start = self.expect(TokenKind::Const, "`const` definition");
        let name = self.parse_name();
        let ty = if matches!(self.kind(), TokenKind::Colon) {
            self.bump();
            Some(self.parse_type())
        } else {
            None
        };
        self.expect(TokenKind::Eq, "const values use `=`");
        let value = self.parse_expression(0);
        let end = self.expect(TokenKind::Semicolon, "every const ends with `;`");
        ConstDef {
            public: false,
            name,
            ty,
            value,
            span: start.merge(end),
        }
    }

    fn parse_fn_def(&mut self) -> FnDef {
        let start = self.expect(TokenKind::Fn, "function definitions start with `fn`");
        let name = self.parse_name();
        self.expect(
            TokenKind::LParen,
            "function parameters are wrapped in `(...)`",
        );
        let params = self.parse_params();
        self.expect(TokenKind::RParen, "close parameters with `)`");
        let return_type = if matches!(self.kind(), TokenKind::Arrow) {
            self.bump();
            Some(self.parse_type())
        } else {
            None
        };
        let body = self.parse_block();
        let span = start.merge(body.span);
        FnDef {
            public: false,
            name,
            params,
            return_type,
            body,
            span,
        }
    }

    fn parse_params(&mut self) -> Vec<Param> {
        let mut params = Vec::new();
        if matches!(self.kind(), TokenKind::RParen) {
            return params;
        }
        loop {
            params.push(self.parse_param());
            if matches!(self.kind(), TokenKind::Comma) {
                self.bump();
                if matches!(self.kind(), TokenKind::RParen) {
                    break;
                }
                continue;
            }
            break;
        }
        params
    }

    fn parse_param(&mut self) -> Param {
        let start = self.span();
        // [ident] ident [: type]
        // Could be: name | label name | name: Type | label name: Type
        // Parameter names may be `_` (unused); function/const names may not.
        let first = self.parse_name_allowing_discard();
        let (label, name) = if matches!(self.kind(), TokenKind::Ident(_) | TokenKind::Discard) {
            let second = self.parse_name_allowing_discard();
            (Some(first), second)
        } else {
            (None, first)
        };
        let ty = if matches!(self.kind(), TokenKind::Colon) {
            self.bump();
            Some(self.parse_type())
        } else {
            None
        };
        let end = ty.as_ref().map(|t| t.span).unwrap_or(name.span);
        Param {
            label,
            name,
            ty,
            span: start.merge(end),
        }
    }

    fn parse_type_def(&mut self) -> TypeDef {
        let start = self.span();
        let opaque = if matches!(self.kind(), TokenKind::Opaque) {
            self.bump();
            true
        } else {
            false
        };
        self.expect(TokenKind::Type, "type definitions start with `type`");
        let name = self.parse_uname();
        let tvars = if matches!(self.kind(), TokenKind::LParen) {
            self.bump();
            let mut tvars = Vec::new();
            if !matches!(self.kind(), TokenKind::RParen) {
                loop {
                    tvars.push(self.parse_name());
                    if matches!(self.kind(), TokenKind::Comma) {
                        self.bump();
                        if matches!(self.kind(), TokenKind::RParen) {
                            break;
                        }
                        continue;
                    }
                    break;
                }
            }
            self.expect(TokenKind::RParen, "close type parameters with `)`");
            tvars
        } else {
            Vec::new()
        };
        let body = if matches!(self.kind(), TokenKind::Eq) {
            self.bump();
            TypeDefBody::Alias(self.parse_type())
        } else {
            self.expect(TokenKind::LBrace, "ADT bodies use `{ ... }`");
            let mut variants = Vec::new();
            while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
                variants.push(self.parse_variant());
            }
            self.expect(TokenKind::RBrace, "close the type body with `}`");
            TypeDefBody::Adt(variants)
        };
        let end = match &body {
            TypeDefBody::Alias(t) => t.span,
            TypeDefBody::Adt(_) => self.tokens[self.pos.saturating_sub(1)].span,
        };
        TypeDef {
            public: false,
            opaque,
            name,
            tvars,
            body,
            span: start.merge(end),
        }
    }

    fn parse_variant(&mut self) -> Variant {
        let name = self.parse_uname();
        let fields = if matches!(self.kind(), TokenKind::LParen) {
            self.bump();
            let mut fields = Vec::new();
            if !matches!(self.kind(), TokenKind::RParen) {
                loop {
                    fields.push(self.parse_field());
                    if matches!(self.kind(), TokenKind::Comma) {
                        self.bump();
                        if matches!(self.kind(), TokenKind::RParen) {
                            break;
                        }
                        continue;
                    }
                    break;
                }
            }
            self.expect(TokenKind::RParen, "close variant fields with `)`");
            Some(fields)
        } else {
            None
        };
        let end = fields
            .as_ref()
            .and_then(|f| f.last().map(|x| x.span))
            .unwrap_or(name.span);
        Variant {
            name: name.clone(),
            fields,
            span: name.span.merge(end),
        }
    }

    fn parse_field(&mut self) -> Field {
        let start = self.span();
        // [ident :] type
        if matches!(self.kind(), TokenKind::Ident(_)) {
            // Could be label: Type OR a type var used as type (ident alone).
            // Lookahead for colon.
            if self.pos + 1 < self.tokens.len()
                && matches!(self.tokens[self.pos + 1].kind, TokenKind::Colon)
            {
                let label = self.parse_name();
                self.bump(); // colon
                let ty = self.parse_type();
                return Field {
                    label: Some(label),
                    span: start.merge(ty.span),
                    ty,
                };
            }
        }
        let ty = self.parse_type();
        Field {
            label: None,
            span: start.merge(ty.span),
            ty,
        }
    }

    fn parse_block(&mut self) -> Block {
        if !self.enter_depth() {
            return self.dummy_block();
        }
        let start = self.expect(TokenKind::LBrace, "blocks start with `{`");
        let mut statements = Vec::new();
        while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
            if self.stopped {
                break;
            }
            // `fn (` — anonymous function in statement position (expression + `;`).
            // `fn name` — named local function (no trailing semicolon).
            if matches!(self.kind(), TokenKind::Fn) {
                let anon = self.pos + 1 < self.tokens.len()
                    && matches!(self.tokens[self.pos + 1].kind, TokenKind::LParen);
                if anon {
                    let expr = self.parse_expression(0);
                    self.expect_semicolon_or_sync(
                        "every expression statement ends with `;`, even the last one and even across a newline",
                    );
                    statements.push(Statement::Expr(expr));
                } else {
                    statements.push(Statement::Fn(self.parse_fn_def()));
                }
                continue;
            }
            if matches!(self.kind(), TokenKind::Let) {
                let stmt = self.parse_let_stmt();
                self.expect_semicolon_or_sync("every `let` statement ends with `;`");
                statements.push(Statement::Let(Box::new(stmt)));
                continue;
            }
            if matches!(self.kind(), TokenKind::Use) {
                let stmt = self.parse_use_stmt();
                self.expect_semicolon_or_sync("every `use` statement ends with `;`");
                statements.push(Statement::Use(stmt));
                continue;
            }
            let expr = self.parse_expression(0);
            self.expect_semicolon_or_sync(
                "every expression statement ends with `;`, even the last one and even across a newline",
            );
            statements.push(Statement::Expr(expr));
        }
        let end = self.expect(TokenKind::RBrace, "close the block with `}`");
        self.exit_depth();
        Block {
            statements,
            span: start.merge(end),
        }
    }

    fn parse_let_stmt(&mut self) -> LetStmt {
        let start = self.expect(TokenKind::Let, "`let` binding");
        let assert = if matches!(self.kind(), TokenKind::Assert) {
            self.bump();
            true
        } else {
            false
        };
        let pattern = self.parse_pattern();
        let ty = if matches!(self.kind(), TokenKind::Colon) {
            self.bump();
            Some(self.parse_type())
        } else {
            None
        };
        self.expect(TokenKind::Eq, "`let` bindings use `=`");
        let value = self.parse_expression(0);
        let message = if matches!(self.kind(), TokenKind::As) {
            self.bump();
            Some(self.parse_string_lit())
        } else {
            None
        };
        let end = message
            .as_ref()
            .map(|_| self.tokens[self.pos.saturating_sub(1)].span)
            .unwrap_or(value.span);
        LetStmt {
            assert,
            pattern,
            ty,
            value,
            message,
            span: start.merge(end),
        }
    }

    fn parse_use_stmt(&mut self) -> UseStmt {
        let start = self.expect(TokenKind::Use, "`use` statement");
        let mut patterns = Vec::new();
        // Zero or more patterns before `<-`.
        if !matches!(self.kind(), TokenKind::LeftArrow) {
            loop {
                patterns.push(self.parse_pattern());
                if matches!(self.kind(), TokenKind::Comma) {
                    self.bump();
                    continue;
                }
                break;
            }
        }
        self.expect(TokenKind::LeftArrow, "`use` requires `<-` before the call");
        let value = self.parse_expression(0);
        UseStmt {
            patterns,
            span: start.merge(value.span),
            value,
        }
    }

    // ----- expressions (Pratt) -----

    fn parse_expression(&mut self, min_prec: u8) -> Expr {
        if !self.enter_depth() {
            return self.dummy_expr();
        }
        let mut left = self.parse_prefix();
        let mut chain_len = 0u32;
        loop {
            if self.stopped {
                break;
            }
            // Postfix: call and field access (prec 11/12), left-to-right chain.
            if matches!(self.kind(), TokenKind::LParen) && min_prec <= 11 {
                if !self.bump_chain(&mut chain_len) {
                    break;
                }
                left = self.parse_call(left);
                continue;
            }
            if matches!(self.kind(), TokenKind::Dot) && min_prec <= 12 {
                if !self.bump_chain(&mut chain_len) {
                    break;
                }
                left = self.parse_field_access(left);
                continue;
            }

            // Pipe `|>` — precedence 1, left-associative (§5.7).
            if matches!(self.kind(), TokenKind::PipeArrow) {
                if 1 < min_prec {
                    break;
                }
                if !self.bump_chain(&mut chain_len) {
                    break;
                }
                self.bump();
                let right = self.parse_expression(2);
                left = Expr {
                    span: left.span.merge(right.span),
                    kind: ExprKind::Pipe {
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                };
                continue;
            }

            let Some((op, prec, assoc)) = binop_info(self.kind()) else {
                break;
            };
            if prec < min_prec {
                break;
            }
            if !self.bump_chain(&mut chain_len) {
                break;
            }
            let next_min = match assoc {
                Assoc::Left | Assoc::None => prec + 1,
            };
            let op_span = self.span();
            self.bump();
            let right = self.parse_expression(next_min);
            // One diagnostic per chaining site: a bare comparison/equality on
            // either side covers both same-prec (`a < b < c`) and mixed-prec
            // (`a < b == c`) chains without a second lookahead report.
            if op.is_comparison_or_eq()
                && (expr_is_bare_comparison_or_eq(&left) || expr_is_bare_comparison_or_eq(&right))
            {
                self.error(
                    op_span,
                    codes::E0110_CHAINED_CMP,
                    "chained comparisons or equality are not allowed",
                    Some("add parentheses, for example `(a < b) == True`".into()),
                );
            }
            left = Expr {
                span: left.span.merge(right.span),
                kind: ExprKind::Binary {
                    left: Box::new(left),
                    op,
                    right: Box::new(right),
                },
            };
        }
        self.exit_depth();
        left
    }

    fn bump_chain(&mut self, chain_len: &mut u32) -> bool {
        *chain_len += 1;
        if *chain_len > MAX_CHAIN {
            self.error(
                self.span(),
                codes::E0190_TOO_DEEP,
                "expression chain is too long",
                Some(format!("maximum chain length is {MAX_CHAIN}")),
            );
            return false;
        }
        true
    }

    fn parse_prefix(&mut self) -> Expr {
        match self.kind() {
            TokenKind::Minus => {
                let start = self.span();
                self.bump();
                let expr = self.parse_expression(10); // prefix prec
                Expr {
                    span: start.merge(expr.span),
                    kind: ExprKind::Unary {
                        op: UnaryOp::Neg,
                        expr: Box::new(expr),
                    },
                }
            }
            TokenKind::Bang => {
                let start = self.span();
                self.bump();
                let expr = self.parse_expression(10);
                Expr {
                    span: start.merge(expr.span),
                    kind: ExprKind::Unary {
                        op: UnaryOp::Not,
                        expr: Box::new(expr),
                    },
                }
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Expr {
        match self.kind().clone() {
            TokenKind::Int(lit) => {
                let span = self.span();
                self.bump();
                Expr {
                    kind: ExprKind::Int(lit),
                    span,
                }
            }
            TokenKind::Float(lit) => {
                let span = self.span();
                self.bump();
                Expr {
                    kind: ExprKind::Float(lit),
                    span,
                }
            }
            TokenKind::String(lit) => {
                let span = self.span();
                self.bump();
                Expr {
                    kind: ExprKind::String(lit),
                    span,
                }
            }
            TokenKind::Discard => {
                let span = self.span();
                self.error(
                    span,
                    codes::E0180_DISCARD_EXPR,
                    "`_` cannot be used as an expression",
                    Some("`_` is only valid as a pattern discard or a call capture hole".into()),
                );
                self.bump();
                Expr {
                    kind: ExprKind::Todo { message: None },
                    span,
                }
            }
            TokenKind::Ident(text) => {
                let span = self.span();
                self.bump();
                // Qualified constructor: module.UIdent — handled via field/call path.
                // Bare ident.
                Expr {
                    kind: ExprKind::Var(Name { text, span }),
                    span,
                }
            }
            TokenKind::UIdent(text) => {
                let span = self.span();
                self.bump();
                let ctor = ConstructorRef {
                    module: None,
                    name: UName { text, span },
                    span,
                };
                // Record update: UIdent(..expr, field: expr)
                if matches!(self.kind(), TokenKind::LParen)
                    && self.pos + 1 < self.tokens.len()
                    && matches!(self.tokens[self.pos + 1].kind, TokenKind::DotDot)
                {
                    return self.parse_record_update(ctor);
                }
                Expr {
                    kind: ExprKind::Constructor(ctor),
                    span,
                }
            }
            TokenKind::LParen => {
                let start = self.span();
                self.bump();
                let expr = self.parse_expression(0);
                let end = self.expect(TokenKind::RParen, "close the group with `)`");
                Expr {
                    span: start.merge(end),
                    kind: ExprKind::Paren(Box::new(expr)),
                }
            }
            TokenKind::LBrace => {
                let block = self.parse_block();
                Expr {
                    span: block.span,
                    kind: ExprKind::Block(block),
                }
            }
            TokenKind::LBracket => self.parse_list_expr(),
            TokenKind::HashLParen => self.parse_tuple_expr(),
            TokenKind::LShift => self.parse_bit_array_expr(),
            TokenKind::Fn => self.parse_anon_fn(),
            TokenKind::Case => self.parse_case_expr(),
            TokenKind::Todo => {
                let start = self.span();
                self.bump();
                let message = self.parse_optional_as_string();
                let end = message
                    .as_ref()
                    .map(|_| self.tokens[self.pos.saturating_sub(1)].span)
                    .unwrap_or(start);
                Expr {
                    kind: ExprKind::Todo { message },
                    span: start.merge(end),
                }
            }
            TokenKind::Panic => {
                let start = self.span();
                self.bump();
                let message = self.parse_optional_as_string();
                let end = message
                    .as_ref()
                    .map(|_| self.tokens[self.pos.saturating_sub(1)].span)
                    .unwrap_or(start);
                Expr {
                    kind: ExprKind::Panic { message },
                    span: start.merge(end),
                }
            }
            TokenKind::Assert => {
                let start = self.span();
                self.bump();
                let expr = self.parse_expression(0);
                let message = self.parse_optional_as_string();
                let end = message
                    .as_ref()
                    .map(|_| self.tokens[self.pos.saturating_sub(1)].span)
                    .unwrap_or(expr.span);
                Expr {
                    span: start.merge(end),
                    kind: ExprKind::Assert {
                        expr: Box::new(expr),
                        message,
                    },
                }
            }
            TokenKind::Echo => {
                let start = self.span();
                self.bump();
                let expr = self.parse_expression(0);
                Expr {
                    span: start.merge(expr.span),
                    kind: ExprKind::Echo(Box::new(expr)),
                }
            }
            other => {
                let span = self.span();
                self.error(
                    span,
                    codes::E0120_EXPECTED_EXPR,
                    format!("expected an expression, found {}", other.describe()),
                    Some(
                        "expressions include literals, calls, `case`, blocks, and operators".into(),
                    ),
                );
                self.bump();
                Expr {
                    kind: ExprKind::Todo { message: None },
                    span,
                }
            }
        }
    }

    fn parse_optional_as_string(&mut self) -> Option<StringLit> {
        if matches!(self.kind(), TokenKind::As) {
            self.bump();
            Some(self.parse_string_lit())
        } else {
            None
        }
    }

    fn parse_string_lit(&mut self) -> StringLit {
        match self.kind().clone() {
            TokenKind::String(lit) => {
                self.bump();
                lit
            }
            _ => {
                self.error(
                    self.span(),
                    codes::E0121_EXPECTED_STRING,
                    "expected a string literal",
                    Some("write a message in double quotes".into()),
                );
                StringLit {
                    value: String::new(),
                    raw: "\"\"".into(),
                }
            }
        }
    }

    fn parse_call(&mut self, callee: Expr) -> Expr {
        self.expect(TokenKind::LParen, "calls use `(...)`");
        let mut args = Vec::new();
        if !matches!(self.kind(), TokenKind::RParen) {
            loop {
                args.push(self.parse_arg());
                if matches!(self.kind(), TokenKind::Comma) {
                    self.bump();
                    if matches!(self.kind(), TokenKind::RParen) {
                        break;
                    }
                    continue;
                }
                break;
            }
        }
        let hole_count = args
            .iter()
            .filter(|a| matches!(a.value, ArgValue::Hole))
            .count();
        if hole_count > 1 {
            self.error(
                callee.span.merge(self.span()),
                codes::E0181_MULTI_HOLE,
                "a call may contain at most one capture hole `_`",
                Some("write separate partial calls, or use a lambda".into()),
            );
        }
        let end = self.expect(TokenKind::RParen, "close the call with `)`");
        Expr {
            span: callee.span.merge(end),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                args,
            },
        }
    }

    fn parse_arg(&mut self) -> Arg {
        let start = self.span();
        // [ident :] (expr | _)
        if matches!(self.kind(), TokenKind::Ident(_))
            && self.pos + 1 < self.tokens.len()
            && matches!(self.tokens[self.pos + 1].kind, TokenKind::Colon)
        {
            let label = self.parse_name();
            self.bump(); // colon
            let value = if matches!(self.kind(), TokenKind::Discard) {
                self.bump();
                ArgValue::Hole
            } else {
                ArgValue::Expr(self.parse_expression(0))
            };
            let end = match &value {
                ArgValue::Expr(e) => e.span,
                ArgValue::Hole => self.tokens[self.pos.saturating_sub(1)].span,
            };
            return Arg {
                label: Some(label),
                value,
                span: start.merge(end),
            };
        }
        let value = if matches!(self.kind(), TokenKind::Discard) {
            // Hole — but only as a direct argument. `_` as expression is also Discard.
            // Spec: capture placeholder. Treat bare `_` in arg position as Hole.
            self.bump();
            ArgValue::Hole
        } else {
            ArgValue::Expr(self.parse_expression(0))
        };
        let end = match &value {
            ArgValue::Expr(e) => e.span,
            ArgValue::Hole => self.tokens[self.pos.saturating_sub(1)].span,
        };
        Arg {
            label: None,
            value,
            span: start.merge(end),
        }
    }

    fn parse_field_access(&mut self, base: Expr) -> Expr {
        self.expect(TokenKind::Dot, "field access uses `.`");
        let field = match self.kind().clone() {
            TokenKind::Ident(text) => {
                let span = self.span();
                self.bump();
                FieldName::Name(Name { text, span })
            }
            TokenKind::UIdent(text) => {
                let span = self.span();
                self.bump();
                FieldName::UName(UName { text, span })
            }
            _ => {
                self.error(
                    self.span(),
                    codes::E0122_EXPECTED_FIELD,
                    "expected a field or constructor name after `.`",
                    Some("write `value.field` or `module.Constructor`".into()),
                );
                FieldName::Name(Name {
                    text: String::new(),
                    span: self.span(),
                })
            }
        };
        let end = match &field {
            FieldName::Name(n) => n.span,
            FieldName::UName(n) => n.span,
        };
        // `module.Constructor` → ConstructorRef; `value.field` stays Field.
        if let (ExprKind::Var(module), FieldName::UName(name)) = (&base.kind, &field) {
            let ctor = ConstructorRef {
                module: Some(module.clone()),
                name: name.clone(),
                span: base.span.merge(end),
            };
            if matches!(self.kind(), TokenKind::LParen)
                && self.pos + 1 < self.tokens.len()
                && matches!(self.tokens[self.pos + 1].kind, TokenKind::DotDot)
            {
                return self.parse_record_update(ctor);
            }
            return Expr {
                span: base.span.merge(end),
                kind: ExprKind::Constructor(ctor),
            };
        }
        Expr {
            span: base.span.merge(end),
            kind: ExprKind::Field {
                base: Box::new(base),
                field,
            },
        }
    }

    fn parse_record_update(&mut self, constructor: ConstructorRef) -> Expr {
        let start = constructor.span;
        self.expect(TokenKind::LParen, "record updates use `(..base, fields)`");
        self.expect(TokenKind::DotDot, "record updates start with `..`");
        let base = self.parse_expression(0);
        let mut fields = Vec::new();
        while matches!(self.kind(), TokenKind::Comma) {
            self.bump();
            if matches!(self.kind(), TokenKind::RParen) {
                break;
            }
            let name = self.parse_name();
            self.expect(TokenKind::Colon, "record update fields use `name: value`");
            let value = self.parse_expression(0);
            fields.push((name, value));
        }
        let end = self.expect(TokenKind::RParen, "close the record update with `)`");
        Expr {
            span: start.merge(end),
            kind: ExprKind::RecordUpdate {
                constructor,
                base: Box::new(base),
                fields,
            },
        }
    }

    fn parse_list_expr(&mut self) -> Expr {
        let start = self.expect(TokenKind::LBracket, "lists start with `[`");
        if matches!(self.kind(), TokenKind::RBracket) {
            let end = self.span();
            self.bump();
            return Expr {
                span: start.merge(end),
                kind: ExprKind::List {
                    items: vec![],
                    spread: None,
                },
            };
        }
        // Spread-only: [..xs] or [..xs,]
        if matches!(self.kind(), TokenKind::DotDot) {
            self.bump();
            let spread = self.parse_expression(0);
            if matches!(self.kind(), TokenKind::Comma) {
                self.bump();
                // Reject [..xs, x]
                if !matches!(self.kind(), TokenKind::RBracket) {
                    self.error(
                        self.span(),
                        codes::E0130_SPREAD_FINAL,
                        "list spread must be the final element",
                        Some("write `[x, ..xs]` or `[..xs]`, not `[..xs, x]`".into()),
                    );
                    // recover: skip until ]
                    while !matches!(self.kind(), TokenKind::RBracket | TokenKind::Eof) {
                        self.bump();
                    }
                }
            }
            let end = self.expect(TokenKind::RBracket, "close the list with `]`");
            return Expr {
                span: start.merge(end),
                kind: ExprKind::List {
                    items: vec![],
                    spread: Some(Box::new(spread)),
                },
            };
        }
        let mut items = Vec::new();
        let mut spread = None;
        loop {
            if matches!(self.kind(), TokenKind::DotDot) {
                self.error(
                    self.span(),
                    codes::E0131_SPREAD_COMMA,
                    "missing comma before list spread",
                    Some("write `[x, ..xs]` with a comma before `..`".into()),
                );
                self.bump();
                spread = Some(Box::new(self.parse_expression(0)));
                if matches!(self.kind(), TokenKind::Comma) {
                    self.bump();
                }
                break;
            }
            items.push(self.parse_expression(0));
            if matches!(self.kind(), TokenKind::Comma) {
                self.bump();
                if matches!(self.kind(), TokenKind::RBracket) {
                    break;
                }
                if matches!(self.kind(), TokenKind::DotDot) {
                    self.bump();
                    spread = Some(Box::new(self.parse_expression(0)));
                    if matches!(self.kind(), TokenKind::Comma) {
                        self.bump();
                        if !matches!(self.kind(), TokenKind::RBracket) {
                            self.error(
                                self.span(),
                                codes::E0130_SPREAD_FINAL,
                                "list spread must be the final element",
                                Some("write `[x, ..xs]`; nothing may follow the spread".into()),
                            );
                        }
                    }
                    break;
                }
                continue;
            }
            break;
        }
        let end = self.expect(TokenKind::RBracket, "close the list with `]`");
        Expr {
            span: start.merge(end),
            kind: ExprKind::List { items, spread },
        }
    }

    fn parse_tuple_expr(&mut self) -> Expr {
        let start = self.expect(TokenKind::HashLParen, "tuples start with `#(`");
        let mut elems = Vec::new();
        if matches!(self.kind(), TokenKind::RParen) {
            let end = self.span();
            self.bump();
            self.error(
                start.merge(end),
                codes::E0132_TUPLE_ARITY,
                "tuples need at least two elements",
                Some("write `#(a, b)`; `#()` is not valid".into()),
            );
            return Expr {
                span: start.merge(end),
                kind: ExprKind::Tuple(vec![]),
            };
        }
        elems.push(self.parse_expression(0));
        if !matches!(self.kind(), TokenKind::Comma) {
            // #(a) — reject
            let end = self.expect(TokenKind::RParen, "close the tuple with `)`");
            self.error(
                start.merge(end),
                codes::E0132_TUPLE_ARITY,
                "tuples need at least two elements",
                Some("write `#(a, b)`; `#(a)` is not valid".into()),
            );
            return Expr {
                span: start.merge(end),
                kind: ExprKind::Tuple(elems),
            };
        }
        // Require at least one comma-separated element.
        while matches!(self.kind(), TokenKind::Comma) {
            self.bump();
            if matches!(self.kind(), TokenKind::RParen) {
                break;
            }
            elems.push(self.parse_expression(0));
        }
        let end = self.expect(TokenKind::RParen, "close the tuple with `)`");
        if elems.len() < 2 {
            self.error(
                start.merge(end),
                codes::E0132_TUPLE_ARITY,
                "tuples need at least two elements",
                Some("write `#(a, b)`".into()),
            );
        }
        Expr {
            span: start.merge(end),
            kind: ExprKind::Tuple(elems),
        }
    }

    fn parse_bit_array_expr(&mut self) -> Expr {
        let start = self.expect(TokenKind::LShift, "bit arrays start with `<<`");
        let mut segments = Vec::new();
        if !matches!(self.kind(), TokenKind::RShift) {
            loop {
                segments.push(self.parse_bit_segment());
                if matches!(self.kind(), TokenKind::Comma) {
                    self.bump();
                    if matches!(self.kind(), TokenKind::RShift) {
                        break;
                    }
                    continue;
                }
                break;
            }
        }
        let end = self.expect(TokenKind::RShift, "close the bit array with `>>`");
        Expr {
            span: start.merge(end),
            kind: ExprKind::BitArray(segments),
        }
    }

    fn parse_bit_segment(&mut self) -> BitSegment {
        let value = self.parse_expression(0);
        let mut options = Vec::new();
        if matches!(self.kind(), TokenKind::Colon) {
            self.bump();
            loop {
                options.push(self.parse_bit_option());
                if matches!(self.kind(), TokenKind::Minus) {
                    // option joiner `-` — but Minus is also unary. Here it's a separator.
                    // Spec: joined by `-`, e.g. size(16)-signed-little
                    self.bump();
                    continue;
                }
                break;
            }
        }
        let end = options
            .last()
            .map(|_| self.tokens[self.pos.saturating_sub(1)].span)
            .unwrap_or(value.span);
        BitSegment {
            span: value.span.merge(end),
            value,
            options,
        }
    }

    fn parse_bit_option(&mut self) -> BitOption {
        match self.kind().clone() {
            TokenKind::Ident(name) if name == "size" => {
                self.bump();
                self.expect(TokenKind::LParen, "`size` takes an argument in parentheses");
                let expr = self.parse_expression(0);
                self.expect(TokenKind::RParen, "close `size(...)` with `)`");
                BitOption::Size(expr)
            }
            TokenKind::Ident(name) => {
                let span = self.span();
                self.bump();
                const KNOWN: &[&str] = &[
                    "signed", "unsigned", "big", "little", "utf8", "bytes", "bits", "size",
                ];
                if !KNOWN.contains(&name.as_str()) {
                    self.error(
                        span,
                        codes::E0134_UNKNOWN_BIT_OPTION,
                        format!("unknown bit-array segment option `{name}`"),
                        Some(
                            "options are `size(n)`, `signed`, `unsigned`, `big`, `little`, `utf8`, `bytes`, `bits`"
                                .into(),
                        ),
                    );
                }
                BitOption::Named(name)
            }
            _ => {
                self.error(
                    self.span(),
                    codes::E0133_BIT_OPTION,
                    "expected a bit-array segment option",
                    Some("options include `size(n)`, `utf8`, `bytes`, `bits`, `signed`, `unsigned`, `big`, `little`".into()),
                );
                BitOption::Named(String::new())
            }
        }
    }

    fn parse_anon_fn(&mut self) -> Expr {
        let start = self.expect(TokenKind::Fn, "anonymous functions start with `fn`");
        self.expect(TokenKind::LParen, "anonymous functions need `(params)`");
        let params = self.parse_params();
        self.expect(TokenKind::RParen, "close parameters with `)`");
        let return_type = if matches!(self.kind(), TokenKind::Arrow) {
            self.bump();
            Some(self.parse_type())
        } else {
            None
        };
        let body = self.parse_block();
        Expr {
            span: start.merge(body.span),
            kind: ExprKind::Fn {
                params,
                return_type,
                body,
            },
        }
    }

    fn parse_case_expr(&mut self) -> Expr {
        let start = self.expect(TokenKind::Case, "`case` expression");
        let mut subjects = Vec::new();
        subjects.push(self.parse_expression(0));
        while matches!(self.kind(), TokenKind::Comma) {
            self.bump();
            subjects.push(self.parse_expression(0));
        }
        let subject_count = subjects.len();
        self.expect(TokenKind::LBrace, "`case` bodies use `{ ... }`");
        let mut clauses = Vec::new();
        while !matches!(self.kind(), TokenKind::RBrace | TokenKind::Eof) {
            let clause = self.parse_clause(subject_count);
            clauses.push(clause);
        }
        let end = self.expect(TokenKind::RBrace, "close the `case` with `}`");
        Expr {
            span: start.merge(end),
            kind: ExprKind::Case { subjects, clauses },
        }
    }

    fn parse_clause(&mut self, subject_count: usize) -> Clause {
        let start = self.span();
        let mut patterns = Vec::new();
        patterns.push(self.parse_pattern_row(subject_count));
        while matches!(self.kind(), TokenKind::Pipe) {
            self.bump();
            patterns.push(self.parse_pattern_row(subject_count));
        }
        let guard = if matches!(self.kind(), TokenKind::If) {
            self.bump();
            Some(self.parse_expression(0))
        } else {
            None
        };
        self.expect(TokenKind::Arrow, "case arms use `->` before the body");
        let body = self.parse_expression(0);
        let end = self.expect(
            TokenKind::Semicolon,
            "every case arm ends with `;`, including the last arm and block branches",
        );
        // Reject comma used instead of semicolon — already handled because we require `;`.
        Clause {
            patterns,
            guard,
            body,
            span: start.merge(end),
        }
    }

    fn parse_pattern_row(&mut self, subject_count: usize) -> PatternRow {
        let start = self.span();
        let mut patterns = Vec::new();
        patterns.push(self.parse_pattern());
        // Multi-subject rows: `1, 2` — keep reading patterns while commas remain,
        // stopping before `->`, `if`, `|`, or `}`.
        while matches!(self.kind(), TokenKind::Comma) {
            if self.pos + 1 < self.tokens.len()
                && matches!(
                    self.tokens[self.pos + 1].kind,
                    TokenKind::Arrow | TokenKind::If | TokenKind::Pipe | TokenKind::RBrace
                )
            {
                break;
            }
            self.bump();
            patterns.push(self.parse_pattern());
        }
        if patterns.len() != subject_count {
            self.error(
                start.merge(patterns.last().map(|p| p.span).unwrap_or(start)),
                codes::E0140_CASE_ARITY,
                format!(
                    "case arm has {} pattern(s) but there are {subject_count} subject(s)",
                    patterns.len()
                ),
                Some("each arm must provide one pattern per subject, separated by commas".into()),
            );
        }
        let end = patterns.last().map(|p| p.span).unwrap_or(start);
        PatternRow {
            patterns,
            span: start.merge(end),
        }
    }

    // ----- patterns -----

    fn parse_pattern(&mut self) -> Pattern {
        if !self.enter_depth() {
            return self.dummy_pattern();
        }
        let mut pat = self.parse_pattern_primary();
        // String prefix: "hello " <> name
        if matches!(self.kind(), TokenKind::LtGt) {
            if let PatternKind::String(prefix) = pat.kind.clone() {
                self.bump();
                let rest = self.parse_pattern();
                pat = Pattern {
                    span: pat.span.merge(rest.span),
                    kind: PatternKind::StringPrefix {
                        prefix,
                        rest: Box::new(rest),
                    },
                };
            }
        }
        // Alias: pattern as name
        if matches!(self.kind(), TokenKind::As) {
            self.bump();
            let name = self.parse_name();
            pat = Pattern {
                span: pat.span.merge(name.span),
                kind: PatternKind::Alias {
                    pattern: Box::new(pat),
                    name,
                },
            };
        }
        self.exit_depth();
        pat
    }

    fn parse_pattern_primary(&mut self) -> Pattern {
        match self.kind().clone() {
            TokenKind::Int(lit) => {
                let span = self.span();
                self.bump();
                Pattern {
                    kind: PatternKind::Int(lit),
                    span,
                }
            }
            TokenKind::Float(lit) => {
                let span = self.span();
                self.bump();
                Pattern {
                    kind: PatternKind::Float(lit),
                    span,
                }
            }
            TokenKind::String(lit) => {
                let span = self.span();
                self.bump();
                Pattern {
                    kind: PatternKind::String(lit),
                    span,
                }
            }
            TokenKind::Discard => {
                let span = self.span();
                self.bump();
                Pattern {
                    kind: PatternKind::Discard,
                    span,
                }
            }
            TokenKind::Ident(text) => {
                let span = self.span();
                self.bump();
                // Qualified constructor pattern: `module.Ctor`
                if matches!(self.kind(), TokenKind::Dot)
                    && self.pos + 1 < self.tokens.len()
                    && matches!(self.tokens[self.pos + 1].kind, TokenKind::UIdent(_))
                {
                    self.bump(); // dot
                    let name = self.parse_uname();
                    let ctor = ConstructorRef {
                        module: Some(Name { text, span }),
                        name: name.clone(),
                        span: span.merge(name.span),
                    };
                    let args = if matches!(self.kind(), TokenKind::LParen) {
                        Some(self.parse_pattern_args())
                    } else {
                        None
                    };
                    let end = args
                        .as_ref()
                        .and_then(|a| a.last().map(|x| x.span))
                        .unwrap_or(ctor.span);
                    return Pattern {
                        span: span.merge(end),
                        kind: PatternKind::Constructor {
                            constructor: ctor,
                            args,
                        },
                    };
                }
                let name = Name {
                    text: text.clone(),
                    span,
                };
                if text.starts_with('_') {
                    Pattern {
                        kind: PatternKind::UnderscoreName(name),
                        span,
                    }
                } else {
                    Pattern {
                        kind: PatternKind::Var(name),
                        span,
                    }
                }
            }
            TokenKind::UIdent(text) => {
                let span = self.span();
                self.bump();
                let ctor = ConstructorRef {
                    module: None,
                    name: UName { text, span },
                    span,
                };
                let args = if matches!(self.kind(), TokenKind::LParen) {
                    Some(self.parse_pattern_args())
                } else {
                    None
                };
                let end = args
                    .as_ref()
                    .and_then(|a| a.last().map(|x| x.span))
                    .unwrap_or(ctor.span);
                Pattern {
                    span: span.merge(end),
                    kind: PatternKind::Constructor {
                        constructor: ctor,
                        args,
                    },
                }
            }
            TokenKind::HashLParen => self.parse_tuple_pattern(),
            TokenKind::LBracket => self.parse_list_pattern(),
            TokenKind::LShift => self.parse_bit_array_pattern(),
            _ => {
                let span = self.span();
                self.error(
                    span,
                    codes::E0150_EXPECTED_PATTERN,
                    format!("expected a pattern, found {}", self.kind().describe()),
                    Some(
                        "patterns include literals, variables, constructors, lists, and tuples"
                            .into(),
                    ),
                );
                self.bump();
                Pattern {
                    kind: PatternKind::Discard,
                    span,
                }
            }
        }
    }

    fn parse_pattern_args(&mut self) -> Vec<PatternArg> {
        self.expect(TokenKind::LParen, "constructor patterns use `(...)`");
        let mut args = Vec::new();
        if !matches!(self.kind(), TokenKind::RParen) {
            loop {
                if matches!(self.kind(), TokenKind::DotDot) {
                    let span = self.span();
                    self.bump();
                    args.push(PatternArg {
                        label: None,
                        pattern: None,
                        spread: true,
                        span,
                    });
                } else if matches!(self.kind(), TokenKind::Ident(_))
                    && self.pos + 1 < self.tokens.len()
                    && matches!(self.tokens[self.pos + 1].kind, TokenKind::Colon)
                {
                    let label = self.parse_name();
                    self.bump();
                    let pattern = self.parse_pattern();
                    args.push(PatternArg {
                        span: label.span.merge(pattern.span),
                        label: Some(label),
                        pattern: Some(pattern),
                        spread: false,
                    });
                } else {
                    let pattern = self.parse_pattern();
                    args.push(PatternArg {
                        span: pattern.span,
                        label: None,
                        pattern: Some(pattern),
                        spread: false,
                    });
                }
                if matches!(self.kind(), TokenKind::Comma) {
                    self.bump();
                    if matches!(self.kind(), TokenKind::RParen) {
                        break;
                    }
                    continue;
                }
                break;
            }
        }
        self.expect(TokenKind::RParen, "close constructor pattern args with `)`");
        args
    }

    fn parse_tuple_pattern(&mut self) -> Pattern {
        let start = self.expect(TokenKind::HashLParen, "tuple patterns start with `#(`");
        let mut elems = Vec::new();
        if !matches!(self.kind(), TokenKind::RParen) {
            loop {
                elems.push(self.parse_pattern());
                if matches!(self.kind(), TokenKind::Comma) {
                    self.bump();
                    if matches!(self.kind(), TokenKind::RParen) {
                        break;
                    }
                    continue;
                }
                break;
            }
        }
        let end = self.expect(TokenKind::RParen, "close the tuple pattern with `)`");
        if elems.len() < 2 {
            self.error(
                start.merge(end),
                codes::E0132_TUPLE_ARITY,
                "tuples need at least two elements",
                Some("write `#(a, b)`".into()),
            );
        }
        Pattern {
            span: start.merge(end),
            kind: PatternKind::Tuple(elems),
        }
    }

    fn parse_list_pattern(&mut self) -> Pattern {
        let start = self.expect(TokenKind::LBracket, "list patterns start with `[`");
        if matches!(self.kind(), TokenKind::RBracket) {
            let end = self.span();
            self.bump();
            return Pattern {
                span: start.merge(end),
                kind: PatternKind::List {
                    items: vec![],
                    spread: None,
                },
            };
        }
        if matches!(self.kind(), TokenKind::DotDot) {
            self.bump();
            let spread = self.parse_pattern();
            if matches!(self.kind(), TokenKind::Comma) {
                self.bump();
                if !matches!(self.kind(), TokenKind::RBracket) {
                    self.error(
                        self.span(),
                        codes::E0130_SPREAD_FINAL,
                        "list spread must be the final element",
                        Some("write `[x, ..xs]` or `[..xs]`".into()),
                    );
                }
            }
            let end = self.expect(TokenKind::RBracket, "close the list pattern with `]`");
            return Pattern {
                span: start.merge(end),
                kind: PatternKind::List {
                    items: vec![],
                    spread: Some(Box::new(spread)),
                },
            };
        }
        let mut items = Vec::new();
        let mut spread = None;
        loop {
            if matches!(self.kind(), TokenKind::DotDot) {
                self.error(
                    self.span(),
                    codes::E0131_SPREAD_COMMA,
                    "missing comma before list spread",
                    Some("write `[x, ..xs]` with a comma before `..`".into()),
                );
                self.bump();
                spread = Some(Box::new(self.parse_pattern()));
                break;
            }
            items.push(self.parse_pattern());
            if matches!(self.kind(), TokenKind::Comma) {
                self.bump();
                if matches!(self.kind(), TokenKind::RBracket) {
                    break;
                }
                if matches!(self.kind(), TokenKind::DotDot) {
                    self.bump();
                    spread = Some(Box::new(self.parse_pattern()));
                    if matches!(self.kind(), TokenKind::Comma) {
                        self.bump();
                    }
                    break;
                }
                continue;
            }
            break;
        }
        let end = self.expect(TokenKind::RBracket, "close the list pattern with `]`");
        Pattern {
            span: start.merge(end),
            kind: PatternKind::List { items, spread },
        }
    }

    fn parse_bit_array_pattern(&mut self) -> Pattern {
        let start = self.expect(TokenKind::LShift, "bit-array patterns start with `<<`");
        let mut segments = Vec::new();
        if !matches!(self.kind(), TokenKind::RShift) {
            loop {
                let pat = self.parse_pattern_primary();
                let mut options = Vec::new();
                if matches!(self.kind(), TokenKind::Colon) {
                    self.bump();
                    loop {
                        options.push(self.parse_bit_option());
                        if matches!(self.kind(), TokenKind::Minus)
                            && self.pos + 1 < self.tokens.len()
                            && matches!(self.tokens[self.pos + 1].kind, TokenKind::Ident(_))
                        {
                            self.bump();
                            continue;
                        }
                        break;
                    }
                }
                // Allow alias after segment? Spec shows segments inside << >>.
                let end = options
                    .last()
                    .map(|_| self.tokens[self.pos.saturating_sub(1)].span)
                    .unwrap_or(pat.span);
                segments.push(BitSegmentPat {
                    span: pat.span.merge(end),
                    pattern: pat,
                    options,
                });
                if matches!(self.kind(), TokenKind::Comma) {
                    self.bump();
                    if matches!(self.kind(), TokenKind::RShift) {
                        break;
                    }
                    continue;
                }
                break;
            }
        }
        let end = self.expect(TokenKind::RShift, "close the bit-array pattern with `>>`");
        Pattern {
            span: start.merge(end),
            kind: PatternKind::BitArray(segments),
        }
    }

    // ----- types -----

    fn parse_type(&mut self) -> TypeExpr {
        if !self.enter_depth() {
            return self.dummy_type();
        }
        let result = self.parse_type_inner();
        self.exit_depth();
        result
    }

    fn parse_type_inner(&mut self) -> TypeExpr {
        match self.kind().clone() {
            TokenKind::Fn => {
                let start = self.span();
                self.bump();
                self.expect(TokenKind::LParen, "function types use `fn(...) -> T`");
                let mut params = Vec::new();
                if !matches!(self.kind(), TokenKind::RParen) {
                    loop {
                        params.push(self.parse_type());
                        if matches!(self.kind(), TokenKind::Comma) {
                            self.bump();
                            if matches!(self.kind(), TokenKind::RParen) {
                                break;
                            }
                            continue;
                        }
                        break;
                    }
                }
                self.expect(TokenKind::RParen, "close function type params with `)`");
                self.expect(
                    TokenKind::Arrow,
                    "function types use `->` before the return type",
                );
                let ret = self.parse_type();
                TypeExpr {
                    span: start.merge(ret.span),
                    kind: TypeKind::Fn {
                        params,
                        ret: Box::new(ret),
                    },
                }
            }
            TokenKind::HashLParen => {
                let start = self.span();
                self.bump();
                let mut elems = Vec::new();
                if !matches!(self.kind(), TokenKind::RParen) {
                    loop {
                        elems.push(self.parse_type());
                        if matches!(self.kind(), TokenKind::Comma) {
                            self.bump();
                            if matches!(self.kind(), TokenKind::RParen) {
                                break;
                            }
                            continue;
                        }
                        break;
                    }
                }
                let end = self.expect(TokenKind::RParen, "close the tuple type with `)`");
                if elems.len() < 2 {
                    self.error(
                        start.merge(end),
                        codes::E0132_TUPLE_ARITY,
                        "tuples need at least two elements",
                        Some("write `#(a, b)`; `#()` and `#(a)` are not valid".into()),
                    );
                }
                TypeExpr {
                    span: start.merge(end),
                    kind: TypeKind::Tuple(elems),
                }
            }
            TokenKind::Ident(text) => {
                let span = self.span();
                self.bump();
                // Qualified type: module.Type
                if matches!(self.kind(), TokenKind::Dot) {
                    self.bump();
                    let name = self.parse_uname();
                    let type_name = TypeName::Qualified {
                        module: Name { text, span },
                        name: name.clone(),
                    };
                    return self.finish_named_type(span, type_name, name.span);
                }
                // Type variable.
                TypeExpr {
                    kind: TypeKind::Var(Name { text, span }),
                    span,
                }
            }
            TokenKind::UIdent(text) => {
                let span = self.span();
                self.bump();
                let type_name = TypeName::Unqualified(UName { text, span });
                self.finish_named_type(span, type_name, span)
            }
            _ => {
                let span = self.span();
                self.error(
                    span,
                    codes::E0160_EXPECTED_TYPE,
                    format!("expected a type, found {}", self.kind().describe()),
                    Some("types include `Int`, `List(a)`, `fn(a) -> b`, and `#(a, b)`".into()),
                );
                self.bump();
                TypeExpr {
                    kind: TypeKind::Var(Name {
                        text: "_".into(),
                        span,
                    }),
                    span,
                }
            }
        }
    }

    fn finish_named_type(&mut self, start: Span, name: TypeName, name_end: Span) -> TypeExpr {
        if matches!(self.kind(), TokenKind::LParen) {
            self.bump();
            let mut args = Vec::new();
            if !matches!(self.kind(), TokenKind::RParen) {
                loop {
                    args.push(self.parse_type());
                    if matches!(self.kind(), TokenKind::Comma) {
                        self.bump();
                        if matches!(self.kind(), TokenKind::RParen) {
                            break;
                        }
                        continue;
                    }
                    break;
                }
            }
            let end = self.expect(TokenKind::RParen, "close type arguments with `)`");
            TypeExpr {
                span: start.merge(end),
                kind: TypeKind::Named { name, args },
            }
        } else {
            TypeExpr {
                span: start.merge(name_end),
                kind: TypeKind::Named { name, args: vec![] },
            }
        }
    }

    // ----- names -----

    fn parse_name(&mut self) -> Name {
        match self.kind().clone() {
            TokenKind::Ident(text) => {
                let span = self.span();
                self.bump();
                Name { text, span }
            }
            TokenKind::Discard => {
                let span = self.span();
                self.error(
                    span,
                    codes::E0172_DISCARD_NAME,
                    "`_` cannot be used as a binding name here",
                    Some("`_` is allowed in patterns and as an unused parameter, not as a definition name".into()),
                );
                self.bump();
                Name {
                    text: "_".into(),
                    span,
                }
            }
            _ => {
                let span = self.span();
                self.error(
                    span,
                    codes::E0170_EXPECTED_NAME,
                    format!("expected a name, found {}", self.kind().describe()),
                    Some("names start with a lowercase letter or underscore".into()),
                );
                Name {
                    text: String::new(),
                    span,
                }
            }
        }
    }

    /// Like `parse_name`, but accepts bare `_` (for unused parameters).
    fn parse_name_allowing_discard(&mut self) -> Name {
        match self.kind().clone() {
            TokenKind::Ident(text) => {
                let span = self.span();
                self.bump();
                Name { text, span }
            }
            TokenKind::Discard => {
                let span = self.span();
                self.bump();
                Name {
                    text: "_".into(),
                    span,
                }
            }
            _ => {
                let span = self.span();
                self.error(
                    span,
                    codes::E0170_EXPECTED_NAME,
                    format!("expected a name, found {}", self.kind().describe()),
                    Some("names start with a lowercase letter or underscore".into()),
                );
                Name {
                    text: String::new(),
                    span,
                }
            }
        }
    }

    fn parse_uname(&mut self) -> UName {
        match self.kind().clone() {
            TokenKind::UIdent(text) => {
                let span = self.span();
                self.bump();
                UName { text, span }
            }
            _ => {
                let span = self.span();
                self.error(
                    span,
                    codes::E0171_EXPECTED_UNAME,
                    format!(
                        "expected a type/constructor name, found {}",
                        self.kind().describe()
                    ),
                    Some("type and constructor names are PascalCase".into()),
                );
                UName {
                    text: String::new(),
                    span,
                }
            }
        }
    }

    fn parse_name_or_uname(&mut self) -> NameOrUName {
        match self.kind() {
            TokenKind::UIdent(_) => NameOrUName::UName(self.parse_uname()),
            _ => NameOrUName::Name(self.parse_name()),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Assoc {
    Left,
    None,
}

fn binop_info(kind: &TokenKind) -> Option<(BinOp, u8, Assoc)> {
    // Precedence from §5.7. Pipe `|>` is handled separately.
    Some(match kind {
        TokenKind::Star => (BinOp::Mul, 9, Assoc::Left),
        TokenKind::Slash => (BinOp::Div, 9, Assoc::Left),
        TokenKind::Percent => (BinOp::Rem, 9, Assoc::Left),
        TokenKind::StarDot => (BinOp::MulFloat, 9, Assoc::Left),
        TokenKind::SlashDot => (BinOp::DivFloat, 9, Assoc::Left),
        TokenKind::Plus => (BinOp::Add, 8, Assoc::Left),
        TokenKind::Minus => (BinOp::Sub, 8, Assoc::Left),
        TokenKind::PlusDot => (BinOp::AddFloat, 8, Assoc::Left),
        TokenKind::MinusDot => (BinOp::SubFloat, 8, Assoc::Left),
        TokenKind::LtGt => (BinOp::Concat, 8, Assoc::Left),
        TokenKind::Lt => (BinOp::Lt, 7, Assoc::None),
        TokenKind::LtEq => (BinOp::LtEq, 7, Assoc::None),
        TokenKind::Gt => (BinOp::Gt, 7, Assoc::None),
        TokenKind::GtEq => (BinOp::GtEq, 7, Assoc::None),
        TokenKind::LtDot => (BinOp::LtFloat, 7, Assoc::None),
        TokenKind::LtEqDot => (BinOp::LtEqFloat, 7, Assoc::None),
        TokenKind::GtDot => (BinOp::GtFloat, 7, Assoc::None),
        TokenKind::GtEqDot => (BinOp::GtEqFloat, 7, Assoc::None),
        TokenKind::EqEq => (BinOp::Eq, 6, Assoc::None),
        TokenKind::NotEq => (BinOp::NotEq, 6, Assoc::None),
        TokenKind::AmpAmp => (BinOp::And, 5, Assoc::Left),
        TokenKind::PipePipe => (BinOp::Or, 4, Assoc::Left),
        _ => return None,
    })
}

/// True when `expr` is an unparenthesised comparison or equality (§5.7).
fn expr_is_bare_comparison_or_eq(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Paren(_) => false,
        ExprKind::Binary { op, .. } => op.is_comparison_or_eq(),
        _ => false,
    }
}
