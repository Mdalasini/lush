//! Recursive-descent + Pratt parser for Lush (`spec.md` §§5, 12).

use crate::ast::*;
use crate::error::SyntaxError;
use crate::lexer::{lex_normalized, Token, TokenSpan};
use crate::span::Span;

/// Parse a complete module from source text.
pub fn parse_module(source: &str) -> Result<Module, Vec<SyntaxError>> {
    let normalized = source.replace("\r\n", "\n");
    let tokens = match lex_normalized(&normalized) {
        Ok(t) => t,
        Err(errors) => return Err(errors),
    };
    let mut parser = Parser::new(&normalized, tokens);
    match parser.parse_module() {
        Ok(module) => {
            if parser.errors.is_empty() {
                Ok(module)
            } else {
                Err(parser.errors)
            }
        }
        Err(()) => Err(parser.errors),
    }
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<TokenSpan>,
    pos: usize,
    errors: Vec<SyntaxError>,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str, tokens: Vec<TokenSpan>) -> Self {
        // Keep comments out of the parse stream; formatter will re-lex.
        let tokens = tokens
            .into_iter()
            .filter(|t| {
                !matches!(
                    t.kind,
                    Token::LineComment | Token::DocComment | Token::ModuleDocComment
                )
            })
            .collect();
        // Re-collect module docs from full lex for Module.module_docs.
        Self {
            source,
            tokens,
            pos: 0,
            errors: Vec::new(),
        }
    }

    fn peek(&self) -> Option<&TokenSpan> {
        self.tokens.get(self.pos)
    }

    fn peek_kind(&self) -> Option<Token> {
        self.peek().map(|t| t.kind)
    }

    fn at(&self, kind: Token) -> bool {
        self.peek_kind() == Some(kind)
    }

    fn bump(&mut self) -> Option<TokenSpan> {
        let tok = self.tokens.get(self.pos).cloned();
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, kind: Token, message: &str) -> Result<TokenSpan, ()> {
        match self.peek() {
            Some(t) if t.kind == kind => Ok(self.bump().unwrap()),
            Some(t) => {
                self.error(t.span, message);
                Err(())
            }
            None => {
                let span = Span::point(self.source.len());
                self.error(span, message);
                Err(())
            }
        }
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.errors.push(SyntaxError::Parse {
            span,
            message: message.into(),
        });
    }

    fn text(&self, span: Span) -> String {
        span.slice(self.source).to_string()
    }

    fn parse_module(&mut self) -> Result<Module, ()> {
        // Module docs: scan original source lines at top.
        let module_docs = extract_module_docs(self.source);
        let start = self.peek().map(|t| t.span.start).unwrap_or(0);
        let mut imports = Vec::new();
        let mut definitions = Vec::new();

        while self.peek().is_some() {
            if self.at(Token::Import) {
                imports.push(self.parse_import()?);
            } else {
                definitions.push(self.parse_definition()?);
            }
        }

        let end = self
            .tokens
            .last()
            .map(|t| t.span.end)
            .unwrap_or(self.source.len());
        Ok(Module {
            module_docs,
            imports,
            definitions,
            span: Span::new(start, end),
        })
    }

    fn parse_import(&mut self) -> Result<Import, ()> {
        let start = self.expect(Token::Import, "expected `import`")?.span.start;
        let mut path = vec![self.expect_ident_text()?];
        while self.at(Token::Slash) {
            self.bump();
            // Path segments may be idents or domain-like pieces; allow Ident/UIdent/Int for versions? Spec: module components. Use Ident primarily; also allow dotted domains as idents like github.com via Ident Dot Ident — actually path is slash-separated. `github.com/user/repo` has dots inside a segment.
            path.push(self.expect_path_segment()?);
        }

        let items = if self.at(Token::Dot) {
            self.bump();
            self.expect(Token::LBrace, "expected `{` after import path")?;
            let mut items = Vec::new();
            if !self.at(Token::RBrace) {
                loop {
                    items.push(self.parse_import_item()?);
                    if self.at(Token::Comma) {
                        self.bump();
                        if self.at(Token::RBrace) {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
            self.expect(Token::RBrace, "expected `}` in import list")?;
            Some(items)
        } else {
            None
        };

        let alias = if self.at(Token::As) {
            self.bump();
            Some(self.expect_ident_text()?)
        } else {
            None
        };

        let end = self
            .expect(Token::Semicolon, "expected `;` after import")?
            .span
            .end;
        Ok(Import {
            path,
            items,
            alias,
            span: Span::new(start, end),
        })
    }

    fn parse_import_item(&mut self) -> Result<ImportItem, ()> {
        let start_tok = self.peek().cloned();
        let is_type = if self.at(Token::Type) {
            self.bump();
            true
        } else {
            false
        };
        let name_tok = self
            .bump()
            .filter(|t| matches!(t.kind, Token::Ident | Token::UIdent))
            .ok_or_else(|| {
                let span = start_tok
                    .as_ref()
                    .map(|t| t.span)
                    .unwrap_or(Span::point(self.source.len()));
                self.error(span, "expected import item name");
            })?;
        let name = self.text(name_tok.span);
        let alias = if self.at(Token::As) {
            self.bump();
            let a = self
                .bump()
                .filter(|t| matches!(t.kind, Token::Ident | Token::UIdent))
                .ok_or_else(|| self.error(name_tok.span, "expected alias name"))?;
            Some(self.text(a.span))
        } else {
            None
        };
        let end = alias
            .as_ref()
            .map(|_| self.tokens[self.pos - 1].span.end)
            .unwrap_or(name_tok.span.end);
        Ok(ImportItem {
            is_type,
            name,
            alias,
            span: Span::new(name_tok.span.start, end),
        })
    }

    fn parse_definition(&mut self) -> Result<Definition, ()> {
        let is_pub = if self.at(Token::Pub) {
            self.bump();
            true
        } else {
            false
        };

        if self.at(Token::Fn) {
            Ok(Definition::Fn(self.parse_fn_def(is_pub)?))
        } else if self.at(Token::Opaque) || self.at(Token::Type) {
            Ok(Definition::Type(self.parse_type_def(is_pub)?))
        } else if self.at(Token::Const) {
            Ok(Definition::Const(self.parse_const_def(is_pub)?))
        } else {
            let span = self
                .peek()
                .map(|t| t.span)
                .unwrap_or(Span::point(self.source.len()));
            self.error(span, "expected `fn`, `type`, or `const` definition");
            Err(())
        }
    }

    fn parse_fn_def(&mut self, is_pub: bool) -> Result<FnDef, ()> {
        let start = if is_pub {
            // `pub` already consumed; approximate start from previous token.
            self.tokens[self.pos.saturating_sub(1)].span.start
        } else {
            self.peek().map(|t| t.span.start).unwrap_or(0)
        };
        self.expect(Token::Fn, "expected `fn`")?;
        let name = self.expect_ident_text()?;
        self.expect(Token::LParen, "expected `(` after function name")?;
        let params = self.parse_params()?;
        self.expect(Token::RParen, "expected `)` after parameters")?;
        let return_type = if self.at(Token::Arrow) {
            self.bump();
            Some(self.parse_type()?)
        } else {
            None
        };
        let body = self.parse_block()?;
        let span = Span::new(start, body.span.end);
        Ok(FnDef {
            is_pub,
            name,
            params,
            return_type,
            body,
            span,
        })
    }

    fn parse_params(&mut self) -> Result<Vec<Param>, ()> {
        let mut params = Vec::new();
        if self.at(Token::RParen) {
            return Ok(params);
        }
        loop {
            params.push(self.parse_param()?);
            if self.at(Token::Comma) {
                self.bump();
                if self.at(Token::RParen) {
                    break;
                }
            } else {
                break;
            }
        }
        Ok(params)
    }

    fn parse_param(&mut self) -> Result<Param, ()> {
        let first = self.expect(Token::Ident, "expected parameter name")?;
        let start = first.span.start;
        let (label, name) = if self.at(Token::Ident) {
            let second = self.bump().unwrap();
            (Some(self.text(first.span)), self.text(second.span))
        } else {
            (None, self.text(first.span))
        };
        let ty = if self.at(Token::Colon) {
            self.bump();
            Some(self.parse_type()?)
        } else {
            None
        };
        let end = ty
            .as_ref()
            .map(|t| t.span.end)
            .unwrap_or_else(|| self.tokens[self.pos - 1].span.end);
        Ok(Param {
            label,
            name,
            ty,
            span: Span::new(start, end),
        })
    }

    fn parse_type_def(&mut self, is_pub: bool) -> Result<TypeDef, ()> {
        let start = self.peek().map(|t| t.span.start).unwrap_or(0);
        let is_opaque = if self.at(Token::Opaque) {
            self.bump();
            true
        } else {
            false
        };
        self.expect(Token::Type, "expected `type`")?;
        let name = self.expect_uident_text()?;
        let mut params = Vec::new();
        if self.at(Token::LParen) {
            self.bump();
            if !self.at(Token::RParen) {
                loop {
                    params.push(self.expect_ident_text()?);
                    if self.at(Token::Comma) {
                        self.bump();
                        if self.at(Token::RParen) {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
            self.expect(Token::RParen, "expected `)` after type parameters")?;
        }

        let body = if self.at(Token::Eq) {
            self.bump();
            TypeDefBody::Alias(self.parse_type()?)
        } else {
            self.expect(Token::LBrace, "expected `{` or `=` in type definition")?;
            let mut variants = Vec::new();
            while !self.at(Token::RBrace) {
                variants.push(self.parse_variant()?);
            }
            self.expect(Token::RBrace, "expected `}` after variants")?;
            TypeDefBody::Adt(variants)
        };

        let end = self.tokens[self.pos - 1].span.end;
        Ok(TypeDef {
            is_pub,
            is_opaque,
            name,
            params,
            body,
            span: Span::new(start, end),
        })
    }

    fn parse_variant(&mut self) -> Result<Variant, ()> {
        let name_tok = self.expect(Token::UIdent, "expected variant constructor")?;
        let start = name_tok.span.start;
        let name = self.text(name_tok.span);
        let mut fields = Vec::new();
        if self.at(Token::LParen) {
            self.bump();
            if !self.at(Token::RParen) {
                loop {
                    fields.push(self.parse_field()?);
                    if self.at(Token::Comma) {
                        self.bump();
                        if self.at(Token::RParen) {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
            self.expect(Token::RParen, "expected `)` after variant fields")?;
        }
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Variant {
            name,
            fields,
            span: Span::new(start, end),
        })
    }

    fn parse_field(&mut self) -> Result<Field, ()> {
        // labelled: ident : type  OR bare type
        if self.at(Token::Ident)
            && self
                .tokens
                .get(self.pos + 1)
                .map(|t| t.kind)
                .unwrap_or(Token::Comma)
                == Token::Colon
        {
            let label_tok = self.bump().unwrap();
            self.bump(); // colon
            let ty = self.parse_type()?;
            return Ok(Field {
                label: Some(self.text(label_tok.span)),
                ty: ty.clone(),
                span: Span::new(label_tok.span.start, ty.span.end),
            });
        }
        let ty = self.parse_type()?;
        Ok(Field {
            label: None,
            ty: ty.clone(),
            span: ty.span,
        })
    }

    fn parse_const_def(&mut self, is_pub: bool) -> Result<ConstDef, ()> {
        let start = self.expect(Token::Const, "expected `const`")?.span.start;
        let name = self.expect_ident_text()?;
        let ty = if self.at(Token::Colon) {
            self.bump();
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(Token::Eq, "expected `=` in const")?;
        let value = self.parse_expr(0)?;
        let end = self
            .expect(Token::Semicolon, "expected `;` after const")?
            .span
            .end;
        Ok(ConstDef {
            is_pub,
            name,
            ty,
            value,
            span: Span::new(start, end),
        })
    }

    fn parse_block(&mut self) -> Result<Block, ()> {
        let start = self.expect(Token::LBrace, "expected `{`")?.span.start;
        let mut statements = Vec::new();
        while !self.at(Token::RBrace) {
            if self.peek().is_none() {
                self.error(Span::point(self.source.len()), "unclosed block");
                return Err(());
            }
            statements.push(self.parse_statement()?);
        }
        let end = self.expect(Token::RBrace, "expected `}`")?.span.end;
        Ok(Block {
            statements,
            span: Span::new(start, end),
        })
    }

    fn parse_statement(&mut self) -> Result<Statement, ()> {
        if self.at(Token::Fn) {
            // Local function: no pub, no trailing semicolon.
            Ok(Statement::Fn(self.parse_fn_def(false)?))
        } else if self.at(Token::Let) {
            let stmt = self.parse_let_stmt()?;
            self.expect(Token::Semicolon, "expected `;` after let")?;
            Ok(Statement::Let(stmt))
        } else if self.at(Token::Use) {
            // Core parser rejects use for now? PR4 adds it — but AST has it.
            // For PR3, parse use as error or implement basic use.
            // Implement basic use here so structure is ready.
            let stmt = self.parse_use_stmt()?;
            self.expect(Token::Semicolon, "expected `;` after use")?;
            Ok(Statement::Use(stmt))
        } else {
            let expr = self.parse_expr(0)?;
            self.expect(Token::Semicolon, "expected `;` after expression")?;
            Ok(Statement::Expr(expr))
        }
    }

    fn parse_let_stmt(&mut self) -> Result<LetStmt, ()> {
        let start = self.expect(Token::Let, "expected `let`")?.span.start;
        let is_assert = if self.at(Token::Assert) {
            self.bump();
            true
        } else {
            false
        };
        let pattern = self.parse_pattern()?;
        let ty = if self.at(Token::Colon) {
            self.bump();
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(Token::Eq, "expected `=` in let")?;
        let value = self.parse_expr(0)?;
        let message = if is_assert && self.at(Token::As) {
            self.bump();
            Some(self.parse_expr(0)?)
        } else {
            None
        };
        let end = message
            .as_ref()
            .map(|m| m.span.end)
            .unwrap_or(value.span.end);
        Ok(LetStmt {
            is_assert,
            pattern,
            ty,
            value,
            message,
            span: Span::new(start, end),
        })
    }

    fn parse_use_stmt(&mut self) -> Result<UseStmt, ()> {
        let start = self.expect(Token::Use, "expected `use`")?.span.start;
        let mut patterns = Vec::new();
        if !self.at(Token::LeftArrow) {
            loop {
                patterns.push(self.parse_pattern()?);
                if self.at(Token::Comma) {
                    self.bump();
                } else {
                    break;
                }
            }
        }
        self.expect(Token::LeftArrow, "expected `<-` in use")?;
        let value = self.parse_expr(0)?;
        Ok(UseStmt {
            patterns,
            value: value.clone(),
            span: Span::new(start, value.span.end),
        })
    }

    // --- Types ---

    fn parse_type(&mut self) -> Result<TypeExpr, ()> {
        if self.at(Token::Fn) {
            let start = self.bump().unwrap().span.start;
            self.expect(Token::LParen, "expected `(` in function type")?;
            let mut params = Vec::new();
            if !self.at(Token::RParen) {
                loop {
                    params.push(self.parse_type()?);
                    if self.at(Token::Comma) {
                        self.bump();
                        if self.at(Token::RParen) {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
            self.expect(Token::RParen, "expected `)` in function type")?;
            self.expect(Token::Arrow, "expected `->` in function type")?;
            let ret = self.parse_type()?;
            return Ok(TypeExpr {
                span: Span::new(start, ret.span.end),
                kind: TypeKind::Fn {
                    params,
                    ret: Box::new(ret),
                },
            });
        }
        if self.at(Token::HashLParen) {
            let start = self.bump().unwrap().span.start;
            let mut elems = Vec::new();
            loop {
                elems.push(self.parse_type()?);
                if self.at(Token::Comma) {
                    self.bump();
                    if self.at(Token::RParen) {
                        break;
                    }
                } else {
                    break;
                }
            }
            let end = self
                .expect(Token::RParen, "expected `)` in tuple type")?
                .span
                .end;
            if elems.len() < 2 {
                self.error(Span::new(start, end), "tuple types require arity 2+");
                return Err(());
            }
            return Ok(TypeExpr {
                kind: TypeKind::Tuple(elems),
                span: Span::new(start, end),
            });
        }

        if self.at(Token::Ident) {
            let first = self.bump().unwrap();
            // qualified: ident . UIdent
            if self.at(Token::Dot) {
                self.bump();
                let name_tok = self.expect(Token::UIdent, "expected type name after `.`")?;
                let module = self.text(first.span);
                let name = self.text(name_tok.span);
                return self.finish_named_type(Some(module), name, first.span, false);
            }
            return self.finish_named_type(None, self.text(first.span), first.span, true);
        }

        if self.at(Token::UIdent) {
            let name_tok = self.bump().unwrap();
            return self.finish_named_type(None, self.text(name_tok.span), name_tok.span, false);
        }

        let span = self
            .peek()
            .map(|t| t.span)
            .unwrap_or(Span::point(self.source.len()));
        self.error(span, "expected type");
        Err(())
    }

    fn finish_named_type(
        &mut self,
        module: Option<String>,
        name: String,
        start_span: Span,
        is_var_style: bool,
    ) -> Result<TypeExpr, ()> {
        let mut args = Vec::new();
        let mut end = start_span.end;
        if self.at(Token::LParen) {
            self.bump();
            if !self.at(Token::RParen) {
                loop {
                    args.push(self.parse_type()?);
                    if self.at(Token::Comma) {
                        self.bump();
                        if self.at(Token::RParen) {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
            end = self
                .expect(Token::RParen, "expected `)` after type arguments")?
                .span
                .end;
        } else if is_var_style && module.is_none() && args.is_empty() {
            return Ok(TypeExpr {
                kind: TypeKind::Var(name),
                span: start_span,
            });
        }
        // Module-qualified always Named; bare UIdent Named; bare lowercase with args Named.
        if is_var_style && module.is_none() && args.is_empty() {
            unreachable!();
        }
        let span_start = start_span.start;
        Ok(TypeExpr {
            kind: TypeKind::Named { module, name, args },
            span: Span::new(span_start, end),
        })
    }

    // --- Patterns (core) ---

    fn parse_pattern(&mut self) -> Result<Pattern, ()> {
        let mut pat = self.parse_pattern_atom()?;
        if self.at(Token::As) {
            self.bump();
            let name = self.expect_ident_text()?;
            let span = Span::new(pat.span.start, self.tokens[self.pos - 1].span.end);
            pat = Pattern {
                kind: PatternKind::As {
                    pattern: Box::new(pat),
                    name,
                },
                span,
            };
        }
        // String prefix: "lit" <> pattern
        if matches!(pat.kind, PatternKind::String(_)) && self.at(Token::LessGreater) {
            let PatternKind::String(literal) = pat.kind.clone() else {
                unreachable!()
            };
            self.bump();
            let rest = self.parse_pattern()?;
            let span = Span::new(pat.span.start, rest.span.end);
            return Ok(Pattern {
                kind: PatternKind::StringPrefix {
                    literal,
                    rest: Box::new(rest),
                },
                span,
            });
        }
        Ok(pat)
    }

    fn parse_pattern_atom(&mut self) -> Result<Pattern, ()> {
        let Some(tok) = self.peek().cloned() else {
            self.error(Span::point(self.source.len()), "expected pattern");
            return Err(());
        };
        match tok.kind {
            Token::Discard => {
                self.bump();
                Ok(Pattern {
                    kind: PatternKind::Discard,
                    span: tok.span,
                })
            }
            Token::Ident => {
                self.bump();
                Ok(Pattern {
                    kind: PatternKind::Var(self.text(tok.span)),
                    span: tok.span,
                })
            }
            Token::Int => {
                self.bump();
                Ok(Pattern {
                    kind: PatternKind::Int(self.text(tok.span)),
                    span: tok.span,
                })
            }
            Token::Float => {
                self.bump();
                Ok(Pattern {
                    kind: PatternKind::Float(self.text(tok.span)),
                    span: tok.span,
                })
            }
            Token::String => {
                self.bump();
                Ok(Pattern {
                    kind: PatternKind::String(self.text(tok.span)),
                    span: tok.span,
                })
            }
            Token::UIdent => self.parse_constructor_pattern(None),
            Token::HashLParen => {
                let start = self.bump().unwrap().span.start;
                let mut elems = Vec::new();
                loop {
                    elems.push(self.parse_pattern()?);
                    if self.at(Token::Comma) {
                        self.bump();
                        if self.at(Token::RParen) {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                let end = self
                    .expect(Token::RParen, "expected `)` in tuple pattern")?
                    .span
                    .end;
                if elems.len() < 2 {
                    self.error(Span::new(start, end), "tuple patterns require arity 2+");
                    return Err(());
                }
                Ok(Pattern {
                    kind: PatternKind::Tuple(elems),
                    span: Span::new(start, end),
                })
            }
            Token::LBracket => {
                let start = self.bump().unwrap().span.start;
                let mut items = Vec::new();
                let mut rest = None;
                if !self.at(Token::RBracket) {
                    if self.at(Token::DotDot) {
                        self.bump();
                        rest = Some(Box::new(self.parse_pattern()?));
                    } else {
                        loop {
                            if self.at(Token::DotDot) {
                                self.bump();
                                rest = Some(Box::new(self.parse_pattern()?));
                                if self.at(Token::Comma) {
                                    self.bump();
                                }
                                break;
                            }
                            items.push(self.parse_pattern()?);
                            if self.at(Token::Comma) {
                                self.bump();
                                if self.at(Token::RBracket) {
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                    }
                }
                let end = self
                    .expect(Token::RBracket, "expected `]` in list pattern")?
                    .span
                    .end;
                Ok(Pattern {
                    kind: PatternKind::List { items, rest },
                    span: Span::new(start, end),
                })
            }
            Token::LessLess => self.parse_bit_array_pattern(),
            _ => {
                // qualified constructor: ident . UIdent
                if self.at(Token::Ident) {
                    // handled above
                }
                self.error(tok.span, "expected pattern");
                Err(())
            }
        }
    }

    fn parse_constructor_pattern(&mut self, module: Option<String>) -> Result<Pattern, ()> {
        let name_tok = self.expect(Token::UIdent, "expected constructor")?;
        let start = module
            .as_ref()
            .map(|_| self.tokens[self.pos.saturating_sub(3)].span.start)
            .unwrap_or(name_tok.span.start);
        let name = self.text(name_tok.span);
        if !self.at(Token::LParen) {
            return Ok(Pattern {
                kind: PatternKind::Constructor {
                    module,
                    name,
                    fields: Vec::new(),
                    with_spread: false,
                },
                span: name_tok.span,
            });
        }
        self.bump();
        let mut fields = Vec::new();
        let mut with_spread = false;
        if !self.at(Token::RParen) {
            loop {
                if self.at(Token::DotDot) {
                    self.bump();
                    with_spread = true;
                    if self.at(Token::Comma) {
                        self.bump();
                    }
                    break;
                }
                // labelled or positional
                if self.at(Token::Ident) {
                    let maybe = self.bump().unwrap();
                    if self.at(Token::Colon) {
                        self.bump();
                        let pattern = self.parse_pattern()?;
                        fields.push(PatternField {
                            label: Some(self.text(maybe.span)),
                            pattern: pattern.clone(),
                            span: Span::new(maybe.span.start, pattern.span.end),
                        });
                    } else {
                        // variable pattern — we already consumed Ident
                        let pattern = Pattern {
                            kind: PatternKind::Var(self.text(maybe.span)),
                            span: maybe.span,
                        };
                        fields.push(PatternField {
                            label: None,
                            pattern,
                            span: maybe.span,
                        });
                    }
                } else {
                    let pattern = self.parse_pattern()?;
                    fields.push(PatternField {
                        label: None,
                        pattern: pattern.clone(),
                        span: pattern.span,
                    });
                }
                if self.at(Token::Comma) {
                    self.bump();
                    if self.at(Token::RParen) {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
        let end = self
            .expect(Token::RParen, "expected `)` in constructor pattern")?
            .span
            .end;
        Ok(Pattern {
            kind: PatternKind::Constructor {
                module,
                name,
                fields,
                with_spread,
            },
            span: Span::new(start, end),
        })
    }

    fn parse_bit_array_pattern(&mut self) -> Result<Pattern, ()> {
        let start = self.expect(Token::LessLess, "expected `<<`")?.span.start;
        let mut segments = Vec::new();
        if !self.at(Token::GreaterGreater) {
            loop {
                segments.push(self.parse_bit_segment_pattern()?);
                if self.at(Token::Comma) {
                    self.bump();
                    if self.at(Token::GreaterGreater) {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
        let end = self
            .expect(Token::GreaterGreater, "expected `>>`")?
            .span
            .end;
        Ok(Pattern {
            kind: PatternKind::BitArray(segments),
            span: Span::new(start, end),
        })
    }

    fn parse_bit_segment_pattern(&mut self) -> Result<BitSegment, ()> {
        let value_pat = self.parse_pattern_atom()?;
        let start = value_pat.span.start;
        let mut options = Vec::new();
        let mut end = value_pat.span.end;
        if self.at(Token::Colon) {
            self.bump();
            options.push(self.parse_bit_option()?);
            while self.at(Token::Minus) {
                self.bump();
                options.push(self.parse_bit_option()?);
            }
            end = self.tokens[self.pos - 1].span.end;
        }
        Ok(BitSegment {
            value: BitSegmentValue::Pattern(value_pat),
            options,
            span: Span::new(start, end),
        })
    }

    fn parse_bit_option(&mut self) -> Result<String, ()> {
        // size(n) or bare ident like utf8, bytes, signed, ...
        let tok = self
            .bump()
            .filter(|t| matches!(t.kind, Token::Ident | Token::UIdent))
            .ok_or_else(|| {
                let span = self
                    .peek()
                    .map(|t| t.span)
                    .unwrap_or(Span::point(self.source.len()));
                self.error(span, "expected bit segment option");
            })?;
        let mut text = self.text(tok.span);
        if self.at(Token::LParen) {
            self.bump();
            let inner = self.parse_expr(0)?;
            self.expect(Token::RParen, "expected `)` in bit option")?;
            text = format!("{text}({})", self.text(inner.span));
        }
        Ok(text)
    }

    // --- Expressions (Pratt) ---

    fn parse_expr(&mut self, min_bp: u8) -> Result<Expr, ()> {
        let mut lhs = self.parse_prefix()?;

        loop {
            // postfix: call, field
            if self.at(Token::LParen) {
                lhs = self.parse_call(lhs)?;
                continue;
            }
            if self.at(Token::Dot) {
                self.bump();
                let field_tok = self
                    .bump()
                    .filter(|t| matches!(t.kind, Token::Ident | Token::UIdent))
                    .ok_or_else(|| {
                        self.error(lhs.span, "expected field name after `.`");
                    })?;
                let is_constructor = field_tok.kind == Token::UIdent;
                let span = Span::new(lhs.span.start, field_tok.span.end);
                lhs = Expr {
                    kind: ExprKind::Field {
                        base: Box::new(lhs),
                        field: self.text(field_tok.span),
                        is_constructor,
                    },
                    span,
                };
                continue;
            }

            let Some(op_tok) = self.peek().cloned() else {
                break;
            };
            if let Some((l_bp, r_bp, op, non_assoc)) = bin_info(op_tok.kind) {
                if l_bp < min_bp {
                    break;
                }
                if non_assoc && l_bp == min_bp {
                    self.error(
                        op_tok.span,
                        "chained comparisons/equality require parentheses",
                    );
                    return Err(());
                }
                self.bump();
                if op_tok.kind == Token::Pipe {
                    let right = self.parse_expr(r_bp)?;
                    let span = Span::new(lhs.span.start, right.span.end);
                    lhs = Expr {
                        kind: ExprKind::Pipe {
                            left: Box::new(lhs),
                            right: Box::new(right),
                        },
                        span,
                    };
                } else {
                    let right = self.parse_expr(r_bp)?;
                    let span = Span::new(lhs.span.start, right.span.end);
                    lhs = Expr {
                        kind: ExprKind::Binary {
                            left: Box::new(lhs),
                            op,
                            right: Box::new(right),
                        },
                        span,
                    };
                }
                continue;
            }
            break;
        }
        Ok(lhs)
    }

    fn parse_prefix(&mut self) -> Result<Expr, ()> {
        if self.at(Token::Minus) || self.at(Token::Bang) {
            let op_tok = self.bump().unwrap();
            let op = if op_tok.kind == Token::Minus {
                UnaryOp::Neg
            } else {
                UnaryOp::Not
            };
            let expr = self.parse_expr(prefix_bp())?;
            let span = Span::new(op_tok.span.start, expr.span.end);
            return Ok(Expr {
                kind: ExprKind::Unary {
                    op,
                    expr: Box::new(expr),
                },
                span,
            });
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, ()> {
        let Some(tok) = self.peek().cloned() else {
            self.error(Span::point(self.source.len()), "expected expression");
            return Err(());
        };
        match tok.kind {
            Token::Int => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Int(self.text(tok.span)),
                    span: tok.span,
                })
            }
            Token::Float => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Float(self.text(tok.span)),
                    span: tok.span,
                })
            }
            Token::String => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::String(self.text(tok.span)),
                    span: tok.span,
                })
            }
            Token::Ident => {
                self.bump();
                Ok(Expr {
                    kind: ExprKind::Ident(self.text(tok.span)),
                    span: tok.span,
                })
            }
            Token::UIdent => {
                self.bump();
                // Record update: Name(..expr, ...)
                if self.at(Token::LParen) {
                    // Lookahead for `..`
                    if self.tokens.get(self.pos + 1).map(|t| t.kind) == Some(Token::DotDot) {
                        return self.parse_record_update(self.text(tok.span), tok.span.start);
                    }
                }
                Ok(Expr {
                    kind: ExprKind::Constructor(self.text(tok.span)),
                    span: tok.span,
                })
            }
            Token::LParen => {
                self.bump();
                let expr = self.parse_expr(0)?;
                self.expect(Token::RParen, "expected `)`")?;
                Ok(expr)
            }
            Token::LBrace => {
                let block = self.parse_block()?;
                Ok(Expr {
                    span: block.span,
                    kind: ExprKind::Block(block),
                })
            }
            Token::LBracket => self.parse_list(),
            Token::HashLParen => self.parse_tuple(),
            Token::LessLess => self.parse_bit_array_expr(),
            Token::Fn => self.parse_fn_expr(),
            Token::Case => self.parse_case(),
            Token::Todo => self.parse_todo_panic(true),
            Token::Panic => self.parse_todo_panic(false),
            Token::Assert => {
                let start = self.bump().unwrap().span.start;
                let condition = self.parse_expr(0)?;
                let message = if self.at(Token::As) {
                    self.bump();
                    Some(Box::new(self.parse_expr(0)?))
                } else {
                    None
                };
                let end = message
                    .as_ref()
                    .map(|m| m.span.end)
                    .unwrap_or(condition.span.end);
                Ok(Expr {
                    kind: ExprKind::Assert {
                        condition: Box::new(condition),
                        message,
                    },
                    span: Span::new(start, end),
                })
            }
            Token::Echo => {
                let start = self.bump().unwrap().span.start;
                let value = self.parse_expr(0)?;
                Ok(Expr {
                    span: Span::new(start, value.span.end),
                    kind: ExprKind::Echo {
                        value: Box::new(value),
                    },
                })
            }
            _ => {
                self.error(tok.span, "expected expression");
                Err(())
            }
        }
    }

    fn parse_call(&mut self, callee: Expr) -> Result<Expr, ()> {
        self.expect(Token::LParen, "expected `(`")?;
        let mut args = Vec::new();
        if !self.at(Token::RParen) {
            loop {
                args.push(self.parse_arg()?);
                if self.at(Token::Comma) {
                    self.bump();
                    if self.at(Token::RParen) {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
        let end = self.expect(Token::RParen, "expected `)`")?.span.end;
        Ok(Expr {
            span: Span::new(callee.span.start, end),
            kind: ExprKind::Call {
                callee: Box::new(callee),
                args,
            },
        })
    }

    fn parse_arg(&mut self) -> Result<Arg, ()> {
        // label: expr / label: _ / expr / _
        if self.at(Token::Ident) {
            if self.tokens.get(self.pos + 1).map(|t| t.kind) == Some(Token::Colon) {
                let label_tok = self.bump().unwrap();
                self.bump(); // colon
                let start = label_tok.span.start;
                if self.at(Token::Discard) {
                    let hole = self.bump().unwrap();
                    return Ok(Arg {
                        label: Some(self.text(label_tok.span)),
                        value: ArgValue::Hole,
                        span: Span::new(start, hole.span.end),
                    });
                }
                let expr = self.parse_expr(0)?;
                return Ok(Arg {
                    label: Some(self.text(label_tok.span)),
                    value: ArgValue::Expr(expr.clone()),
                    span: Span::new(start, expr.span.end),
                });
            }
        }
        if self.at(Token::Discard) {
            let hole = self.bump().unwrap();
            return Ok(Arg {
                label: None,
                value: ArgValue::Hole,
                span: hole.span,
            });
        }
        let expr = self.parse_expr(0)?;
        Ok(Arg {
            label: None,
            value: ArgValue::Expr(expr.clone()),
            span: expr.span,
        })
    }

    fn parse_list(&mut self) -> Result<Expr, ()> {
        let start = self.expect(Token::LBracket, "expected `[`")?.span.start;
        let mut items = Vec::new();
        let mut spread = None;
        if !self.at(Token::RBracket) {
            if self.at(Token::DotDot) {
                self.bump();
                spread = Some(Box::new(self.parse_expr(0)?));
            } else {
                loop {
                    if self.at(Token::DotDot) {
                        self.bump();
                        spread = Some(Box::new(self.parse_expr(0)?));
                        if self.at(Token::Comma) {
                            self.bump();
                        }
                        break;
                    }
                    items.push(self.parse_expr(0)?);
                    if self.at(Token::Comma) {
                        self.bump();
                        if self.at(Token::RBracket) {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
        }
        let end = self.expect(Token::RBracket, "expected `]`")?.span.end;
        Ok(Expr {
            kind: ExprKind::List { items, spread },
            span: Span::new(start, end),
        })
    }

    fn parse_tuple(&mut self) -> Result<Expr, ()> {
        let start = self.expect(Token::HashLParen, "expected `#(`")?.span.start;
        let mut elems = Vec::new();
        loop {
            elems.push(self.parse_expr(0)?);
            if self.at(Token::Comma) {
                self.bump();
                if self.at(Token::RParen) {
                    break;
                }
            } else {
                break;
            }
        }
        let end = self
            .expect(Token::RParen, "expected `)` in tuple")?
            .span
            .end;
        if elems.len() < 2 {
            self.error(Span::new(start, end), "tuples require arity 2+");
            return Err(());
        }
        Ok(Expr {
            kind: ExprKind::Tuple(elems),
            span: Span::new(start, end),
        })
    }

    fn parse_bit_array_expr(&mut self) -> Result<Expr, ()> {
        let start = self.expect(Token::LessLess, "expected `<<`")?.span.start;
        let mut segments = Vec::new();
        if !self.at(Token::GreaterGreater) {
            loop {
                let value = self.parse_expr(0)?;
                let seg_start = value.span.start;
                let mut options = Vec::new();
                let mut end = value.span.end;
                if self.at(Token::Colon) {
                    self.bump();
                    options.push(self.parse_bit_option()?);
                    while self.at(Token::Minus) {
                        self.bump();
                        options.push(self.parse_bit_option()?);
                    }
                    end = self.tokens[self.pos - 1].span.end;
                }
                segments.push(BitSegment {
                    value: BitSegmentValue::Expr(value),
                    options,
                    span: Span::new(seg_start, end),
                });
                if self.at(Token::Comma) {
                    self.bump();
                    if self.at(Token::GreaterGreater) {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
        let end = self
            .expect(Token::GreaterGreater, "expected `>>`")?
            .span
            .end;
        Ok(Expr {
            kind: ExprKind::BitArray(segments),
            span: Span::new(start, end),
        })
    }

    fn parse_record_update(&mut self, constructor: String, start: usize) -> Result<Expr, ()> {
        self.expect(Token::LParen, "expected `(`")?;
        self.expect(Token::DotDot, "expected `..` in record update")?;
        let base = self.parse_expr(0)?;
        let mut fields = Vec::new();
        while self.at(Token::Comma) {
            self.bump();
            if self.at(Token::RParen) {
                break;
            }
            let label = self.expect_ident_text()?;
            self.expect(Token::Colon, "expected `:` in record update field")?;
            let value = self.parse_expr(0)?;
            fields.push((label, value));
        }
        let end = self
            .expect(Token::RParen, "expected `)` in record update")?
            .span
            .end;
        Ok(Expr {
            kind: ExprKind::RecordUpdate {
                constructor,
                base: Box::new(base),
                fields,
            },
            span: Span::new(start, end),
        })
    }

    fn parse_fn_expr(&mut self) -> Result<Expr, ()> {
        let start = self.expect(Token::Fn, "expected `fn`")?.span.start;
        self.expect(Token::LParen, "expected `(`")?;
        let params = self.parse_params()?;
        self.expect(Token::RParen, "expected `)`")?;
        let return_type = if self.at(Token::Arrow) {
            self.bump();
            Some(self.parse_type()?)
        } else {
            None
        };
        let body = self.parse_block()?;
        Ok(Expr {
            span: Span::new(start, body.span.end),
            kind: ExprKind::Fn {
                params,
                return_type,
                body,
            },
        })
    }

    fn parse_case(&mut self) -> Result<Expr, ()> {
        let start = self.expect(Token::Case, "expected `case`")?.span.start;
        let mut subjects = vec![self.parse_expr(0)?];
        while self.at(Token::Comma) {
            self.bump();
            subjects.push(self.parse_expr(0)?);
        }
        self.expect(Token::LBrace, "expected `{` after case subjects")?;
        let mut clauses = Vec::new();
        while !self.at(Token::RBrace) {
            clauses.push(self.parse_clause(subjects.len())?);
        }
        let end = self.expect(Token::RBrace, "expected `}`")?.span.end;
        Ok(Expr {
            kind: ExprKind::Case { subjects, clauses },
            span: Span::new(start, end),
        })
    }

    fn parse_clause(&mut self, subject_count: usize) -> Result<Clause, ()> {
        let start = self.peek().map(|t| t.span.start).unwrap_or(0);
        let mut patterns = vec![self.parse_pattern_row(subject_count)?];
        while self.at(Token::PipeBar) {
            self.bump();
            patterns.push(self.parse_pattern_row(subject_count)?);
        }
        let guard = if self.at(Token::If) {
            self.bump();
            Some(self.parse_expr(0)?)
        } else {
            None
        };
        self.expect(Token::Arrow, "expected `->` in case clause")?;
        let body = self.parse_expr(0)?;
        let end = self
            .expect(Token::Semicolon, "expected `;` after case arm")?
            .span
            .end;
        Ok(Clause {
            patterns,
            guard,
            body,
            span: Span::new(start, end),
        })
    }

    fn parse_pattern_row(&mut self, subject_count: usize) -> Result<PatternRow, ()> {
        let start = self.peek().map(|t| t.span.start).unwrap_or(0);
        let mut patterns = vec![self.parse_pattern()?];
        while self.at(Token::Comma) {
            // Ambiguous with case subject commas only inside rows; rows use commas between patterns.
            self.bump();
            patterns.push(self.parse_pattern()?);
        }
        if patterns.len() != subject_count {
            let end = patterns.last().map(|p| p.span.end).unwrap_or(start);
            self.error(
                Span::new(start, end),
                format!(
                    "case arm has {} pattern(s) but case has {subject_count} subject(s)",
                    patterns.len()
                ),
            );
            return Err(());
        }
        let end = patterns.last().map(|p| p.span.end).unwrap_or(start);
        Ok(PatternRow {
            patterns,
            span: Span::new(start, end),
        })
    }

    fn parse_todo_panic(&mut self, is_todo: bool) -> Result<Expr, ()> {
        let tok = self.bump().unwrap();
        let message = if self.at(Token::As) {
            self.bump();
            Some(Box::new(self.parse_expr(0)?))
        } else {
            None
        };
        let end = message.as_ref().map(|m| m.span.end).unwrap_or(tok.span.end);
        Ok(Expr {
            span: Span::new(tok.span.start, end),
            kind: if is_todo {
                ExprKind::Todo { message }
            } else {
                ExprKind::Panic { message }
            },
        })
    }

    fn expect_ident_text(&mut self) -> Result<String, ()> {
        let tok = self.expect(Token::Ident, "expected identifier")?;
        Ok(self.text(tok.span))
    }

    fn expect_uident_text(&mut self) -> Result<String, ()> {
        let tok = self.expect(Token::UIdent, "expected type/constructor name")?;
        Ok(self.text(tok.span))
    }

    fn expect_path_segment(&mut self) -> Result<String, ()> {
        // Allow `github.com` style by joining Ident Dot Ident within a slash segment.
        let first = self
            .bump()
            .filter(|t| matches!(t.kind, Token::Ident | Token::UIdent | Token::Int))
            .ok_or_else(|| {
                let span = self
                    .peek()
                    .map(|t| t.span)
                    .unwrap_or(Span::point(self.source.len()));
                self.error(span, "expected import path segment");
            })?;
        let mut text = self.text(first.span);
        while self.at(Token::Dot)
            && self
                .tokens
                .get(self.pos + 1)
                .map(|t| matches!(t.kind, Token::Ident | Token::UIdent))
                .unwrap_or(false)
            && self.tokens.get(self.pos + 1).map(|t| t.kind) != Some(Token::LBrace)
        {
            self.bump(); // dot
            let part = self
                .bump()
                .filter(|t| matches!(t.kind, Token::Ident | Token::UIdent))
                .ok_or_else(|| self.error(first.span, "expected path segment after `.`"))?;
            text.push('.');
            text.push_str(&self.text(part.span));
        }
        Ok(text)
    }
}

fn extract_module_docs(source: &str) -> Vec<String> {
    let mut docs = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("////") {
            docs.push(trimmed.trim_start_matches('/').trim_start().to_string());
        } else if trimmed.is_empty() || trimmed.starts_with("//") {
            continue;
        } else {
            break;
        }
    }
    docs
}

fn prefix_bp() -> u8 {
    10
}

fn bin_info(kind: Token) -> Option<(u8, u8, BinOp, bool)> {
    // returns (left_bp, right_bp, op, non_associative)
    // Precedence from §5.7; comparisons/equality non-associative.
    match kind {
        Token::Star => Some((9, 10, BinOp::Mul, false)),
        Token::Slash => Some((9, 10, BinOp::Div, false)),
        Token::Percent => Some((9, 10, BinOp::Rem, false)),
        Token::StarDot => Some((9, 10, BinOp::MulFloat, false)),
        Token::SlashDot => Some((9, 10, BinOp::DivFloat, false)),
        Token::Plus => Some((8, 9, BinOp::Add, false)),
        Token::Minus => Some((8, 9, BinOp::Sub, false)),
        Token::PlusDot => Some((8, 9, BinOp::AddFloat, false)),
        Token::MinusDot => Some((8, 9, BinOp::SubFloat, false)),
        Token::LessGreater => Some((8, 9, BinOp::Concat, false)),
        Token::Less => Some((7, 7, BinOp::Lt, true)),
        Token::LessEq => Some((7, 7, BinOp::Le, true)),
        Token::Greater => Some((7, 7, BinOp::Gt, true)),
        Token::GreaterEq => Some((7, 7, BinOp::Ge, true)),
        Token::LessDot => Some((7, 7, BinOp::LtFloat, true)),
        Token::LessEqDot => Some((7, 7, BinOp::LeFloat, true)),
        Token::GreaterDot => Some((7, 7, BinOp::GtFloat, true)),
        Token::GreaterEqDot => Some((7, 7, BinOp::GeFloat, true)),
        Token::EqEq => Some((6, 6, BinOp::Eq, true)),
        Token::NotEq => Some((6, 6, BinOp::NotEq, true)),
        Token::AndAnd => Some((5, 6, BinOp::And, false)),
        Token::OrOr => Some((4, 5, BinOp::Or, false)),
        Token::Pipe => Some((1, 2, BinOp::Add /* dummy */, false)), // special-cased
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(source: &str) -> Module {
        parse_module(source).unwrap_or_else(|e| panic!("parse failed: {e:?}\nsource:\n{source}"))
    }

    #[test]
    fn parses_simple_function() {
        let m = parse_ok(
            r#"
pub fn add(a: Int, b: Int) -> Int {
  a + b;
}
"#,
        );
        assert_eq!(m.definitions.len(), 1);
        match &m.definitions[0] {
            Definition::Fn(f) => {
                assert!(f.is_pub);
                assert_eq!(f.name, "add");
                assert_eq!(f.params.len(), 2);
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn parses_import_and_const() {
        let m = parse_ok(
            r#"
import lush/list;
import lush/result.{try, map as result_map};
const max_size = 1024;
"#,
        );
        assert_eq!(m.imports.len(), 2);
        assert_eq!(m.imports[0].path, vec!["lush", "list"]);
        assert!(m.imports[1].items.is_some());
        assert_eq!(m.definitions.len(), 1);
    }

    #[test]
    fn parses_type_and_list_tuple() {
        let m = parse_ok(
            r#"
pub type Shape {
  Circle(radius: Float)
  Point
}

pub fn demo() -> #(Int, List(Int)) {
  let xs = [1, 2, ..rest];
  #(1, xs);
}
"#,
        );
        assert_eq!(m.definitions.len(), 2);
    }

    #[test]
    fn parses_operators_and_precedence() {
        let m = parse_ok(
            r#"
pub fn main() -> Int {
  1 + 2 * 3;
}
"#,
        );
        let Definition::Fn(f) = &m.definitions[0] else {
            panic!();
        };
        let Statement::Expr(expr) = &f.body.statements[0] else {
            panic!();
        };
        match &expr.kind {
            ExprKind::Binary {
                op: BinOp::Add,
                right,
                ..
            } => match &right.kind {
                ExprKind::Binary { op: BinOp::Mul, .. } => {}
                other => panic!("expected mul on right, got {other:?}"),
            },
            other => panic!("expected add, got {other:?}"),
        }
    }

    #[test]
    fn rejects_missing_semicolon() {
        let err = parse_module(
            r#"
pub fn main() -> Nil {
  1
}
"#,
        )
        .unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn rejects_chained_comparison() {
        let err = parse_module(
            r#"
pub fn main() -> Bool {
  a < b < c;
}
"#,
        )
        .unwrap_err();
        assert!(err.iter().any(|e| e.to_string().contains("parentheses")));
    }

    #[test]
    fn parses_pipe_and_case() {
        let m = parse_ok(
            r#"
pub fn main() -> Int {
  let x = 1 |> add(2, _);
  case x {
    0 -> 0;
    n -> n;
  };
}
"#,
        );
        assert_eq!(m.definitions.len(), 1);
    }

    #[test]
    fn snapshot_module_ast_shape() {
        let m = parse_ok(
            r#"
import lush/list;
pub fn main() -> Nil {
  echo "hi";
}
"#,
        );
        insta::assert_debug_snapshot!(m.definitions.len());
        insta::assert_debug_snapshot!("import_path", m.imports[0].path.clone());
    }
}
