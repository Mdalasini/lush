//! Abstract syntax tree for Lush (§5, §12).
//!
//! Spans are retained for diagnostics. Trivia is kept on tokens for formatting
//! and reattached during pretty-printing from the original token stream when
//! formatting from source; the formatter also walks the AST.

use crate::span::Span;
use crate::token::{FloatLit, IntLit, StringLit};

#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub items: Vec<ModuleItem>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ModuleItem {
    Import(Import),
    Const(ConstDef),
    Fn(FnDef),
    Type(TypeDef),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Import {
    pub path: ImportPath,
    pub items: Option<Vec<ImportItem>>,
    pub alias: Option<Name>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImportPath {
    pub segments: Vec<String>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImportItem {
    pub is_type: bool,
    pub name: NameOrUName,
    pub alias: Option<NameOrUName>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NameOrUName {
    Name(Name),
    UName(UName),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Name {
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UName {
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConstDef {
    pub public: bool,
    pub name: Name,
    pub ty: Option<TypeExpr>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FnDef {
    pub public: bool,
    pub name: Name,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    /// Optional external label. When absent, the binding name is also the label.
    pub label: Option<Name>,
    pub name: Name,
    pub ty: Option<TypeExpr>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeDef {
    pub public: bool,
    pub opaque: bool,
    pub name: UName,
    pub tvars: Vec<Name>,
    pub body: TypeDefBody,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeDefBody {
    Adt(Vec<Variant>),
    Alias(TypeExpr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub name: UName,
    pub fields: Option<Vec<Field>>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub label: Option<Name>,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub statements: Vec<Statement>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Statement {
    Fn(FnDef),
    Let(LetStmt),
    Use(UseStmt),
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct LetStmt {
    pub assert: bool,
    pub pattern: Pattern,
    pub ty: Option<TypeExpr>,
    pub value: Expr,
    pub message: Option<StringLit>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UseStmt {
    pub patterns: Vec<Pattern>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Int(IntLit),
    Float(FloatLit),
    String(StringLit),
    /// Variable / module binding.
    Var(Name),
    /// Constructor or type name used as a value (nullary).
    Constructor(ConstructorRef),
    Discard,
    Tuple(Vec<Expr>),
    List {
        items: Vec<Expr>,
        /// Optional tail after `..`.
        spread: Option<Box<Expr>>,
    },
    BitArray(Vec<BitSegment>),
    RecordUpdate {
        constructor: ConstructorRef,
        base: Box<Expr>,
        fields: Vec<(Name, Expr)>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Arg>,
    },
    Field {
        base: Box<Expr>,
        field: FieldName,
    },
    Binary {
        left: Box<Expr>,
        op: BinOp,
        right: Box<Expr>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Pipe {
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Fn {
        params: Vec<Param>,
        return_type: Option<TypeExpr>,
        body: Block,
    },
    Case {
        subjects: Vec<Expr>,
        clauses: Vec<Clause>,
    },
    Todo {
        message: Option<StringLit>,
    },
    Panic {
        message: Option<StringLit>,
    },
    Assert {
        expr: Box<Expr>,
        message: Option<StringLit>,
    },
    Echo(Box<Expr>),
    Block(Block),
    /// Parenthesised expression (preserved for formatting fidelity).
    Paren(Box<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldName {
    Name(Name),
    UName(UName),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConstructorRef {
    /// Optional module qualifier (`actor.Next`).
    pub module: Option<Name>,
    pub name: UName,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    pub label: Option<Name>,
    pub value: ArgValue,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ArgValue {
    Expr(Expr),
    Hole,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Mul,
    Div,
    Rem,
    MulFloat,
    DivFloat,
    Add,
    Sub,
    AddFloat,
    SubFloat,
    Concat,
    Lt,
    LtEq,
    Gt,
    GtEq,
    LtFloat,
    LtEqFloat,
    GtFloat,
    GtEqFloat,
    Eq,
    NotEq,
    And,
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clause {
    pub patterns: Vec<PatternRow>,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

/// One alternative in `p1 | p2 | p3` — itself a comma-separated row for multi-subject case.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternRow {
    pub patterns: Vec<Pattern>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PatternKind {
    Int(IntLit),
    Float(FloatLit),
    String(StringLit),
    Var(Name),
    Discard,
    UnderscoreName(Name),
    Constructor {
        constructor: ConstructorRef,
        args: Option<Vec<PatternArg>>,
    },
    Tuple(Vec<Pattern>),
    List {
        items: Vec<Pattern>,
        spread: Option<Box<Pattern>>,
    },
    BitArray(Vec<BitSegmentPat>),
    StringPrefix {
        prefix: StringLit,
        rest: Box<Pattern>,
    },
    Alias {
        pattern: Box<Pattern>,
        name: Name,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PatternArg {
    pub label: Option<Name>,
    pub pattern: Option<Pattern>, // None means `..` alone in that position is invalid; use spread flag
    pub spread: bool,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BitSegment {
    pub value: Expr,
    pub options: Vec<BitOption>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BitSegmentPat {
    pub pattern: Pattern,
    pub options: Vec<BitOption>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BitOption {
    Size(Expr),
    Named(String), // utf8, bytes, bits, signed, unsigned, big, little
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeExpr {
    pub kind: TypeKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeKind {
    Named {
        name: TypeName,
        args: Vec<TypeExpr>,
    },
    Var(Name),
    Fn {
        params: Vec<TypeExpr>,
        ret: Box<TypeExpr>,
    },
    Tuple(Vec<TypeExpr>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeName {
    Unqualified(UName),
    Qualified { module: Name, name: UName },
}

impl BinOp {
    pub fn as_str(self) -> &'static str {
        match self {
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
            BinOp::LtEq => "<=",
            BinOp::Gt => ">",
            BinOp::GtEq => ">=",
            BinOp::LtFloat => "<.",
            BinOp::LtEqFloat => "<=.",
            BinOp::GtFloat => ">.",
            BinOp::GtEqFloat => ">=.",
            BinOp::Eq => "==",
            BinOp::NotEq => "!=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }

    pub fn is_comparison_or_eq(self) -> bool {
        matches!(
            self,
            BinOp::Lt
                | BinOp::LtEq
                | BinOp::Gt
                | BinOp::GtEq
                | BinOp::LtFloat
                | BinOp::LtEqFloat
                | BinOp::GtFloat
                | BinOp::GtEqFloat
                | BinOp::Eq
                | BinOp::NotEq
        )
    }
}

/// Structural equality ignoring spans (for formatter round-trips).
pub mod ignore_spans {
    use super::*;

    pub fn modules_eq(a: &Module, b: &Module) -> bool {
        a.items.len() == b.items.len() && a.items.iter().zip(&b.items).all(|(x, y)| items_eq(x, y))
    }

    fn items_eq(a: &ModuleItem, b: &ModuleItem) -> bool {
        match (a, b) {
            (ModuleItem::Import(a), ModuleItem::Import(b)) => {
                a.path.segments == b.path.segments
                    && a.alias.as_ref().map(|n| &n.text) == b.alias.as_ref().map(|n| &n.text)
                    && match (&a.items, &b.items) {
                        (None, None) => true,
                        (Some(x), Some(y)) => {
                            x.len() == y.len()
                                && x.iter().zip(y).all(|(i, j)| {
                                    i.is_type == j.is_type
                                        && name_or_uname_eq(&i.name, &j.name)
                                        && match (&i.alias, &j.alias) {
                                            (None, None) => true,
                                            (Some(a), Some(b)) => name_or_uname_eq(a, b),
                                            _ => false,
                                        }
                                })
                        }
                        _ => false,
                    }
            }
            (ModuleItem::Const(a), ModuleItem::Const(b)) => {
                a.public == b.public
                    && a.name.text == b.name.text
                    && type_opt_eq(&a.ty, &b.ty)
                    && expr_eq(&a.value, &b.value)
            }
            (ModuleItem::Fn(a), ModuleItem::Fn(b)) => fn_eq(a, b),
            (ModuleItem::Type(a), ModuleItem::Type(b)) => {
                a.public == b.public
                    && a.opaque == b.opaque
                    && a.name.text == b.name.text
                    && a.tvars
                        .iter()
                        .map(|t| &t.text)
                        .eq(b.tvars.iter().map(|t| &t.text))
                    && type_body_eq(&a.body, &b.body)
            }
            _ => false,
        }
    }

    fn fn_eq(a: &FnDef, b: &FnDef) -> bool {
        a.public == b.public
            && a.name.text == b.name.text
            && params_eq(&a.params, &b.params)
            && type_opt_eq(&a.return_type, &b.return_type)
            && block_eq(&a.body, &b.body)
    }

    fn params_eq(a: &[Param], b: &[Param]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|(x, y)| {
                x.label.as_ref().map(|n| &n.text) == y.label.as_ref().map(|n| &n.text)
                    && x.name.text == y.name.text
                    && type_opt_eq(&x.ty, &y.ty)
            })
    }

    fn type_body_eq(a: &TypeDefBody, b: &TypeDefBody) -> bool {
        match (a, b) {
            (TypeDefBody::Alias(a), TypeDefBody::Alias(b)) => type_eq(a, b),
            (TypeDefBody::Adt(a), TypeDefBody::Adt(b)) => {
                a.len() == b.len()
                    && a.iter().zip(b).all(|(x, y)| {
                        x.name.text == y.name.text
                            && match (&x.fields, &y.fields) {
                                (None, None) => true,
                                (Some(xf), Some(yf)) => {
                                    xf.len() == yf.len()
                                        && xf.iter().zip(yf).all(|(f1, f2)| {
                                            f1.label.as_ref().map(|n| &n.text)
                                                == f2.label.as_ref().map(|n| &n.text)
                                                && type_eq(&f1.ty, &f2.ty)
                                        })
                                }
                                _ => false,
                            }
                    })
            }
            _ => false,
        }
    }

    fn block_eq(a: &Block, b: &Block) -> bool {
        a.statements.len() == b.statements.len()
            && a.statements
                .iter()
                .zip(&b.statements)
                .all(|(x, y)| stmt_eq(x, y))
    }

    fn stmt_eq(a: &Statement, b: &Statement) -> bool {
        match (a, b) {
            (Statement::Fn(a), Statement::Fn(b)) => fn_eq(a, b),
            (Statement::Let(a), Statement::Let(b)) => {
                a.assert == b.assert
                    && pattern_eq(&a.pattern, &b.pattern)
                    && type_opt_eq(&a.ty, &b.ty)
                    && expr_eq(&a.value, &b.value)
                    && a.message.as_ref().map(|s| &s.value) == b.message.as_ref().map(|s| &s.value)
            }
            (Statement::Use(a), Statement::Use(b)) => {
                a.patterns.len() == b.patterns.len()
                    && a.patterns
                        .iter()
                        .zip(&b.patterns)
                        .all(|(x, y)| pattern_eq(x, y))
                    && expr_eq(&a.value, &b.value)
            }
            (Statement::Expr(a), Statement::Expr(b)) => expr_eq(a, b),
            _ => false,
        }
    }

    pub fn expr_eq(a: &Expr, b: &Expr) -> bool {
        // Strip paren wrappers for equivalence.
        let a = strip_paren(a);
        let b = strip_paren(b);
        match (&a.kind, &b.kind) {
            (ExprKind::Int(x), ExprKind::Int(y)) => x.digits == y.digits && x.base == y.base,
            (ExprKind::Float(x), ExprKind::Float(y)) => x.raw == y.raw,
            (ExprKind::String(x), ExprKind::String(y)) => x.value == y.value,
            (ExprKind::Var(x), ExprKind::Var(y)) => x.text == y.text,
            (ExprKind::Discard, ExprKind::Discard) => true,
            (ExprKind::Constructor(x), ExprKind::Constructor(y)) => ctor_eq(x, y),
            (ExprKind::Tuple(x), ExprKind::Tuple(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(a, b)| expr_eq(a, b))
            }
            (
                ExprKind::List {
                    items: ai,
                    spread: as_,
                },
                ExprKind::List {
                    items: bi,
                    spread: bs,
                },
            ) => {
                ai.len() == bi.len()
                    && ai.iter().zip(bi).all(|(a, b)| expr_eq(a, b))
                    && match (as_, bs) {
                        (None, None) => true,
                        (Some(a), Some(b)) => expr_eq(a, b),
                        _ => false,
                    }
            }
            (ExprKind::BitArray(a), ExprKind::BitArray(b)) => {
                a.len() == b.len()
                    && a.iter().zip(b).all(|(x, y)| {
                        expr_eq(&x.value, &y.value) && bit_opts_eq(&x.options, &y.options)
                    })
            }
            (
                ExprKind::RecordUpdate {
                    constructor: ac,
                    base: ab,
                    fields: af,
                },
                ExprKind::RecordUpdate {
                    constructor: bc,
                    base: bb,
                    fields: bf,
                },
            ) => {
                ctor_eq(ac, bc)
                    && expr_eq(ab, bb)
                    && af.len() == bf.len()
                    && af
                        .iter()
                        .zip(bf)
                        .all(|((n1, e1), (n2, e2))| n1.text == n2.text && expr_eq(e1, e2))
            }
            (
                ExprKind::Call {
                    callee: ac,
                    args: aa,
                },
                ExprKind::Call {
                    callee: bc,
                    args: ba,
                },
            ) => {
                expr_eq(ac, bc)
                    && aa.len() == ba.len()
                    && aa.iter().zip(ba).all(|(x, y)| {
                        x.label.as_ref().map(|n| &n.text) == y.label.as_ref().map(|n| &n.text)
                            && match (&x.value, &y.value) {
                                (ArgValue::Hole, ArgValue::Hole) => true,
                                (ArgValue::Expr(a), ArgValue::Expr(b)) => expr_eq(a, b),
                                _ => false,
                            }
                    })
            }
            (
                ExprKind::Field {
                    base: ab,
                    field: af,
                },
                ExprKind::Field {
                    base: bb,
                    field: bf,
                },
            ) => expr_eq(ab, bb) && field_name_eq(af, bf),
            (
                ExprKind::Binary {
                    left: al,
                    op: ao,
                    right: ar,
                },
                ExprKind::Binary {
                    left: bl,
                    op: bo,
                    right: br,
                },
            ) => ao == bo && expr_eq(al, bl) && expr_eq(ar, br),
            (ExprKind::Unary { op: ao, expr: ae }, ExprKind::Unary { op: bo, expr: be }) => {
                ao == bo && expr_eq(ae, be)
            }
            (
                ExprKind::Pipe {
                    left: al,
                    right: ar,
                },
                ExprKind::Pipe {
                    left: bl,
                    right: br,
                },
            ) => expr_eq(al, bl) && expr_eq(ar, br),
            (
                ExprKind::Fn {
                    params: ap,
                    return_type: ar,
                    body: ab,
                },
                ExprKind::Fn {
                    params: bp,
                    return_type: br,
                    body: bb,
                },
            ) => params_eq(ap, bp) && type_opt_eq(ar, br) && block_eq(ab, bb),
            (
                ExprKind::Case {
                    subjects: asub,
                    clauses: ac,
                },
                ExprKind::Case {
                    subjects: bsub,
                    clauses: bc,
                },
            ) => {
                asub.len() == bsub.len()
                    && asub.iter().zip(bsub).all(|(a, b)| expr_eq(a, b))
                    && ac.len() == bc.len()
                    && ac.iter().zip(bc).all(|(a, b)| {
                        a.patterns.len() == b.patterns.len()
                            && a.patterns.iter().zip(&b.patterns).all(|(r1, r2)| {
                                r1.patterns.len() == r2.patterns.len()
                                    && r1
                                        .patterns
                                        .iter()
                                        .zip(&r2.patterns)
                                        .all(|(p1, p2)| pattern_eq(p1, p2))
                            })
                            && match (&a.guard, &b.guard) {
                                (None, None) => true,
                                (Some(g1), Some(g2)) => expr_eq(g1, g2),
                                _ => false,
                            }
                            && expr_eq(&a.body, &b.body)
                    })
            }
            (ExprKind::Todo { message: a }, ExprKind::Todo { message: b })
            | (ExprKind::Panic { message: a }, ExprKind::Panic { message: b }) => {
                a.as_ref().map(|s| &s.value) == b.as_ref().map(|s| &s.value)
            }
            (
                ExprKind::Assert {
                    expr: ae,
                    message: am,
                },
                ExprKind::Assert {
                    expr: be,
                    message: bm,
                },
            ) => expr_eq(ae, be) && am.as_ref().map(|s| &s.value) == bm.as_ref().map(|s| &s.value),
            (ExprKind::Echo(a), ExprKind::Echo(b)) => expr_eq(a, b),
            (ExprKind::Block(a), ExprKind::Block(b)) => block_eq(a, b),
            _ => false,
        }
    }

    fn strip_paren(e: &Expr) -> &Expr {
        match &e.kind {
            ExprKind::Paren(inner) => strip_paren(inner),
            _ => e,
        }
    }

    fn ctor_eq(a: &ConstructorRef, b: &ConstructorRef) -> bool {
        a.module.as_ref().map(|n| &n.text) == b.module.as_ref().map(|n| &n.text)
            && a.name.text == b.name.text
    }

    fn field_name_eq(a: &FieldName, b: &FieldName) -> bool {
        match (a, b) {
            (FieldName::Name(a), FieldName::Name(b)) => a.text == b.text,
            (FieldName::UName(a), FieldName::UName(b)) => a.text == b.text,
            _ => false,
        }
    }

    fn bit_opts_eq(a: &[BitOption], b: &[BitOption]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|(x, y)| match (x, y) {
                (BitOption::Named(a), BitOption::Named(b)) => a == b,
                (BitOption::Size(a), BitOption::Size(b)) => expr_eq(a, b),
                _ => false,
            })
    }

    fn pattern_eq(a: &Pattern, b: &Pattern) -> bool {
        match (&a.kind, &b.kind) {
            (PatternKind::Int(x), PatternKind::Int(y)) => x.digits == y.digits && x.base == y.base,
            (PatternKind::Float(x), PatternKind::Float(y)) => x.raw == y.raw,
            (PatternKind::String(x), PatternKind::String(y)) => x.value == y.value,
            (PatternKind::Var(x), PatternKind::Var(y)) => x.text == y.text,
            (PatternKind::Discard, PatternKind::Discard) => true,
            (PatternKind::UnderscoreName(x), PatternKind::UnderscoreName(y)) => x.text == y.text,
            (
                PatternKind::Constructor {
                    constructor: ac,
                    args: aa,
                },
                PatternKind::Constructor {
                    constructor: bc,
                    args: ba,
                },
            ) => {
                ctor_eq(ac, bc)
                    && match (aa, ba) {
                        (None, None) => true,
                        (Some(a), Some(b)) => {
                            a.len() == b.len()
                                && a.iter().zip(b).all(|(x, y)| {
                                    x.spread == y.spread
                                        && x.label.as_ref().map(|n| &n.text)
                                            == y.label.as_ref().map(|n| &n.text)
                                        && match (&x.pattern, &y.pattern) {
                                            (None, None) => true,
                                            (Some(p1), Some(p2)) => pattern_eq(p1, p2),
                                            _ => false,
                                        }
                                })
                        }
                        _ => false,
                    }
            }
            (PatternKind::Tuple(a), PatternKind::Tuple(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| pattern_eq(x, y))
            }
            (
                PatternKind::List {
                    items: ai,
                    spread: as_,
                },
                PatternKind::List {
                    items: bi,
                    spread: bs,
                },
            ) => {
                ai.len() == bi.len()
                    && ai.iter().zip(bi).all(|(x, y)| pattern_eq(x, y))
                    && match (as_, bs) {
                        (None, None) => true,
                        (Some(a), Some(b)) => pattern_eq(a, b),
                        _ => false,
                    }
            }
            (PatternKind::BitArray(a), PatternKind::BitArray(b)) => {
                a.len() == b.len()
                    && a.iter().zip(b).all(|(x, y)| {
                        pattern_eq(&x.pattern, &y.pattern) && bit_opts_eq(&x.options, &y.options)
                    })
            }
            (
                PatternKind::StringPrefix {
                    prefix: ap,
                    rest: ar,
                },
                PatternKind::StringPrefix {
                    prefix: bp,
                    rest: br,
                },
            ) => ap.value == bp.value && pattern_eq(ar, br),
            (
                PatternKind::Alias {
                    pattern: ap,
                    name: an,
                },
                PatternKind::Alias {
                    pattern: bp,
                    name: bn,
                },
            ) => pattern_eq(ap, bp) && an.text == bn.text,
            _ => false,
        }
    }

    fn type_opt_eq(a: &Option<TypeExpr>, b: &Option<TypeExpr>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => type_eq(a, b),
            _ => false,
        }
    }

    fn type_eq(a: &TypeExpr, b: &TypeExpr) -> bool {
        match (&a.kind, &b.kind) {
            (TypeKind::Named { name: an, args: aa }, TypeKind::Named { name: bn, args: ba }) => {
                type_name_eq(an, bn)
                    && aa.len() == ba.len()
                    && aa.iter().zip(ba).all(|(x, y)| type_eq(x, y))
            }
            (TypeKind::Var(a), TypeKind::Var(b)) => a.text == b.text,
            (
                TypeKind::Fn {
                    params: ap,
                    ret: ar,
                },
                TypeKind::Fn {
                    params: bp,
                    ret: br,
                },
            ) => {
                ap.len() == bp.len()
                    && ap.iter().zip(bp).all(|(x, y)| type_eq(x, y))
                    && type_eq(ar, br)
            }
            (TypeKind::Tuple(a), TypeKind::Tuple(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| type_eq(x, y))
            }
            _ => false,
        }
    }

    fn type_name_eq(a: &TypeName, b: &TypeName) -> bool {
        match (a, b) {
            (TypeName::Unqualified(a), TypeName::Unqualified(b)) => a.text == b.text,
            (
                TypeName::Qualified {
                    module: am,
                    name: an,
                },
                TypeName::Qualified {
                    module: bm,
                    name: bn,
                },
            ) => am.text == bm.text && an.text == bn.text,
            _ => false,
        }
    }

    fn name_or_uname_eq(a: &NameOrUName, b: &NameOrUName) -> bool {
        match (a, b) {
            (NameOrUName::Name(a), NameOrUName::Name(b)) => a.text == b.text,
            (NameOrUName::UName(a), NameOrUName::UName(b)) => a.text == b.text,
            _ => false,
        }
    }
}
