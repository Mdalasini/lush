//! Abstract syntax tree for Lush (§5, §12).
//!
//! Spans are retained for diagnostics. Trivia is kept on tokens for formatting
//! and reattached during pretty-printing from the original token stream when
//! formatting from source; the formatter also walks the AST.

use crate::span::Span;
use crate::token::{FloatLit, IntLit, StringLit};

/// Stable identity for an expression or pattern node, assigned after desugaring
/// for the typed-AST handoff (§15.3 step 3). Parser-produced nodes use
/// [`NodeId::NONE`] (`u32::MAX`) so a dense numbering that starts at 0 never
/// collides with the sentinel. Excluded from [`PartialEq`] so formatter
/// round-trips stay span/structure based.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl Default for NodeId {
    fn default() -> Self {
        NodeId::NONE
    }
}

impl NodeId {
    /// Sentinel for nodes that have not yet been numbered. Never equal to a
    /// post-desugar id (those are dense starting at 0).
    pub const NONE: NodeId = NodeId(u32::MAX);

    pub fn is_none(self) -> bool {
        self == Self::NONE
    }
}

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
    Let(Box<LetStmt>),
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

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
    pub id: NodeId,
}

impl PartialEq for Expr {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.span == other.span
    }
}

impl Expr {
    /// Build an expression. `span` is taken first so callers can read spans from
    /// values that are then moved into `kind`.
    pub fn new(span: Span, kind: ExprKind) -> Self {
        Self {
            kind,
            span,
            id: NodeId::NONE,
        }
    }
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

#[derive(Clone, Debug)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
    pub id: NodeId,
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.span == other.span
    }
}

impl Pattern {
    /// Build a pattern. `span` is taken first so callers can read spans from
    /// values that are then moved into `kind`.
    pub fn new(span: Span, kind: PatternKind) -> Self {
        Self {
            kind,
            span,
            id: NodeId::NONE,
        }
    }
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

/// Structural equality ignoring spans and parentheses, for formatter round-trips.
pub mod equiv {
    use super::*;

    /// Structural equality ignoring spans and parentheses, for formatter round-trips.
    pub fn modules_eq(a: &Module, b: &Module) -> bool {
        strip_module(a) == strip_module(b)
    }

    fn strip_module(m: &Module) -> Module {
        Module {
            items: m.items.iter().map(strip_module_item).collect(),
            span: Span::default(),
        }
    }

    fn strip_module_item(item: &ModuleItem) -> ModuleItem {
        match item {
            ModuleItem::Import(i) => ModuleItem::Import(strip_import(i)),
            ModuleItem::Const(c) => ModuleItem::Const(strip_const(c)),
            ModuleItem::Fn(f) => ModuleItem::Fn(strip_fn(f)),
            ModuleItem::Type(t) => ModuleItem::Type(strip_type_def(t)),
        }
    }

    fn strip_import(i: &Import) -> Import {
        Import {
            path: ImportPath {
                segments: i.path.segments.clone(),
                span: Span::default(),
            },
            items: i
                .items
                .as_ref()
                .map(|items| items.iter().map(strip_import_item).collect()),
            alias: i.alias.as_ref().map(strip_name),
            span: Span::default(),
        }
    }

    fn strip_import_item(item: &ImportItem) -> ImportItem {
        ImportItem {
            is_type: item.is_type,
            name: strip_name_or_uname(&item.name),
            alias: item.alias.as_ref().map(strip_name_or_uname),
            span: Span::default(),
        }
    }

    fn strip_name_or_uname(n: &NameOrUName) -> NameOrUName {
        match n {
            NameOrUName::Name(n) => NameOrUName::Name(strip_name(n)),
            NameOrUName::UName(n) => NameOrUName::UName(strip_uname(n)),
        }
    }

    fn strip_name(n: &Name) -> Name {
        Name {
            text: n.text.clone(),
            span: Span::default(),
        }
    }

    fn strip_uname(n: &UName) -> UName {
        UName {
            text: n.text.clone(),
            span: Span::default(),
        }
    }

    fn strip_const(c: &ConstDef) -> ConstDef {
        ConstDef {
            public: c.public,
            name: strip_name(&c.name),
            ty: c.ty.as_ref().map(strip_type),
            value: strip_expr(&c.value),
            span: Span::default(),
        }
    }

    fn strip_fn(f: &FnDef) -> FnDef {
        FnDef {
            public: f.public,
            name: strip_name(&f.name),
            params: f.params.iter().map(strip_param).collect(),
            return_type: f.return_type.as_ref().map(strip_type),
            body: strip_block(&f.body),
            span: Span::default(),
        }
    }

    fn strip_param(p: &Param) -> Param {
        Param {
            label: p.label.as_ref().map(strip_name),
            name: strip_name(&p.name),
            ty: p.ty.as_ref().map(strip_type),
            span: Span::default(),
        }
    }

    fn strip_type_def(t: &TypeDef) -> TypeDef {
        TypeDef {
            public: t.public,
            opaque: t.opaque,
            name: strip_uname(&t.name),
            tvars: t.tvars.iter().map(strip_name).collect(),
            body: strip_type_def_body(&t.body),
            span: Span::default(),
        }
    }

    fn strip_type_def_body(body: &TypeDefBody) -> TypeDefBody {
        match body {
            TypeDefBody::Adt(variants) => {
                TypeDefBody::Adt(variants.iter().map(strip_variant).collect())
            }
            TypeDefBody::Alias(ty) => TypeDefBody::Alias(strip_type(ty)),
        }
    }

    fn strip_variant(v: &Variant) -> Variant {
        Variant {
            name: strip_uname(&v.name),
            fields: v
                .fields
                .as_ref()
                .map(|fields| fields.iter().map(strip_field).collect()),
            span: Span::default(),
        }
    }

    fn strip_field(f: &Field) -> Field {
        Field {
            label: f.label.as_ref().map(strip_name),
            ty: strip_type(&f.ty),
            span: Span::default(),
        }
    }

    fn strip_block(b: &Block) -> Block {
        Block {
            statements: b.statements.iter().map(strip_statement).collect(),
            span: Span::default(),
        }
    }

    fn strip_statement(s: &Statement) -> Statement {
        match s {
            Statement::Fn(f) => Statement::Fn(strip_fn(f)),
            Statement::Let(l) => Statement::Let(Box::new(strip_let(l))),
            Statement::Use(u) => Statement::Use(strip_use(u)),
            Statement::Expr(e) => Statement::Expr(strip_expr(e)),
        }
    }

    fn strip_let(l: &LetStmt) -> LetStmt {
        LetStmt {
            assert: l.assert,
            pattern: strip_pattern(&l.pattern),
            ty: l.ty.as_ref().map(strip_type),
            value: strip_expr(&l.value),
            message: l.message.clone(),
            span: Span::default(),
        }
    }

    fn strip_use(u: &UseStmt) -> UseStmt {
        UseStmt {
            patterns: u.patterns.iter().map(strip_pattern).collect(),
            value: strip_expr(&u.value),
            span: Span::default(),
        }
    }

    fn strip_expr(e: &Expr) -> Expr {
        // Unwrap parentheses so they do not affect equivalence.
        if let ExprKind::Paren(inner) = &e.kind {
            return strip_expr(inner);
        }
        Expr::new(Span::default(), strip_expr_kind(&e.kind))
    }

    fn strip_expr_kind(kind: &ExprKind) -> ExprKind {
        match kind {
            ExprKind::Int(i) => ExprKind::Int(i.clone()),
            ExprKind::Float(f) => ExprKind::Float(f.clone()),
            ExprKind::String(s) => ExprKind::String(s.clone()),
            ExprKind::Var(n) => ExprKind::Var(strip_name(n)),
            ExprKind::Constructor(c) => ExprKind::Constructor(strip_ctor(c)),
            ExprKind::Tuple(items) => ExprKind::Tuple(items.iter().map(strip_expr).collect()),
            ExprKind::List { items, spread } => ExprKind::List {
                items: items.iter().map(strip_expr).collect(),
                spread: spread.as_ref().map(|e| Box::new(strip_expr(e))),
            },
            ExprKind::BitArray(segs) => {
                ExprKind::BitArray(segs.iter().map(strip_bit_segment).collect())
            }
            ExprKind::RecordUpdate {
                constructor,
                base,
                fields,
            } => ExprKind::RecordUpdate {
                constructor: strip_ctor(constructor),
                base: Box::new(strip_expr(base)),
                fields: fields
                    .iter()
                    .map(|(n, e)| (strip_name(n), strip_expr(e)))
                    .collect(),
            },
            ExprKind::Call { callee, args } => ExprKind::Call {
                callee: Box::new(strip_expr(callee)),
                args: args.iter().map(strip_arg).collect(),
            },
            ExprKind::Field { base, field } => ExprKind::Field {
                base: Box::new(strip_expr(base)),
                field: strip_field_name(field),
            },
            ExprKind::Binary { left, op, right } => ExprKind::Binary {
                left: Box::new(strip_expr(left)),
                op: *op,
                right: Box::new(strip_expr(right)),
            },
            ExprKind::Unary { op, expr } => ExprKind::Unary {
                op: *op,
                expr: Box::new(strip_expr(expr)),
            },
            ExprKind::Pipe { left, right } => ExprKind::Pipe {
                left: Box::new(strip_expr(left)),
                right: Box::new(strip_expr(right)),
            },
            ExprKind::Fn {
                params,
                return_type,
                body,
            } => ExprKind::Fn {
                params: params.iter().map(strip_param).collect(),
                return_type: return_type.as_ref().map(strip_type),
                body: strip_block(body),
            },
            ExprKind::Case { subjects, clauses } => ExprKind::Case {
                subjects: subjects.iter().map(strip_expr).collect(),
                clauses: clauses.iter().map(strip_clause).collect(),
            },
            ExprKind::Todo { message } => ExprKind::Todo {
                message: message.clone(),
            },
            ExprKind::Panic { message } => ExprKind::Panic {
                message: message.clone(),
            },
            ExprKind::Assert { expr, message } => ExprKind::Assert {
                expr: Box::new(strip_expr(expr)),
                message: message.clone(),
            },
            ExprKind::Echo(e) => ExprKind::Echo(Box::new(strip_expr(e))),
            ExprKind::Block(b) => ExprKind::Block(strip_block(b)),
            // Handled by strip_expr before this match.
            ExprKind::Paren(inner) => strip_expr(inner).kind,
        }
    }

    fn strip_ctor(c: &ConstructorRef) -> ConstructorRef {
        ConstructorRef {
            module: c.module.as_ref().map(strip_name),
            name: strip_uname(&c.name),
            span: Span::default(),
        }
    }

    fn strip_field_name(f: &FieldName) -> FieldName {
        match f {
            FieldName::Name(n) => FieldName::Name(strip_name(n)),
            FieldName::UName(n) => FieldName::UName(strip_uname(n)),
        }
    }

    fn strip_arg(a: &Arg) -> Arg {
        Arg {
            label: a.label.as_ref().map(strip_name),
            value: match &a.value {
                ArgValue::Expr(e) => ArgValue::Expr(strip_expr(e)),
                ArgValue::Hole => ArgValue::Hole,
            },
            span: Span::default(),
        }
    }

    fn strip_clause(c: &Clause) -> Clause {
        Clause {
            patterns: c.patterns.iter().map(strip_pattern_row).collect(),
            guard: c.guard.as_ref().map(strip_expr),
            body: strip_expr(&c.body),
            span: Span::default(),
        }
    }

    fn strip_pattern_row(r: &PatternRow) -> PatternRow {
        PatternRow {
            patterns: r.patterns.iter().map(strip_pattern).collect(),
            span: Span::default(),
        }
    }

    fn strip_pattern(p: &Pattern) -> Pattern {
        Pattern::new(Span::default(), strip_pattern_kind(&p.kind))
    }

    fn strip_pattern_kind(kind: &PatternKind) -> PatternKind {
        match kind {
            PatternKind::Int(i) => PatternKind::Int(i.clone()),
            PatternKind::Float(f) => PatternKind::Float(f.clone()),
            PatternKind::String(s) => PatternKind::String(s.clone()),
            PatternKind::Var(n) => PatternKind::Var(strip_name(n)),
            PatternKind::Discard => PatternKind::Discard,
            PatternKind::UnderscoreName(n) => PatternKind::UnderscoreName(strip_name(n)),
            PatternKind::Constructor { constructor, args } => PatternKind::Constructor {
                constructor: strip_ctor(constructor),
                args: args
                    .as_ref()
                    .map(|args| args.iter().map(strip_pattern_arg).collect()),
            },
            PatternKind::Tuple(items) => {
                PatternKind::Tuple(items.iter().map(strip_pattern).collect())
            }
            PatternKind::List { items, spread } => PatternKind::List {
                items: items.iter().map(strip_pattern).collect(),
                spread: spread.as_ref().map(|p| Box::new(strip_pattern(p))),
            },
            PatternKind::BitArray(segs) => {
                PatternKind::BitArray(segs.iter().map(strip_bit_segment_pat).collect())
            }
            PatternKind::StringPrefix { prefix, rest } => PatternKind::StringPrefix {
                prefix: prefix.clone(),
                rest: Box::new(strip_pattern(rest)),
            },
            PatternKind::Alias { pattern, name } => PatternKind::Alias {
                pattern: Box::new(strip_pattern(pattern)),
                name: strip_name(name),
            },
        }
    }

    fn strip_pattern_arg(a: &PatternArg) -> PatternArg {
        PatternArg {
            label: a.label.as_ref().map(strip_name),
            pattern: a.pattern.as_ref().map(strip_pattern),
            spread: a.spread,
            span: Span::default(),
        }
    }

    fn strip_bit_segment(s: &BitSegment) -> BitSegment {
        BitSegment {
            value: strip_expr(&s.value),
            options: s.options.iter().map(strip_bit_option).collect(),
            span: Span::default(),
        }
    }

    fn strip_bit_segment_pat(s: &BitSegmentPat) -> BitSegmentPat {
        BitSegmentPat {
            pattern: strip_pattern(&s.pattern),
            options: s.options.iter().map(strip_bit_option).collect(),
            span: Span::default(),
        }
    }

    fn strip_bit_option(o: &BitOption) -> BitOption {
        match o {
            BitOption::Size(e) => BitOption::Size(strip_expr(e)),
            BitOption::Named(n) => BitOption::Named(n.clone()),
        }
    }

    fn strip_type(t: &TypeExpr) -> TypeExpr {
        TypeExpr {
            kind: strip_type_kind(&t.kind),
            span: Span::default(),
        }
    }

    fn strip_type_kind(kind: &TypeKind) -> TypeKind {
        match kind {
            TypeKind::Named { name, args } => TypeKind::Named {
                name: strip_type_name(name),
                args: args.iter().map(strip_type).collect(),
            },
            TypeKind::Var(n) => TypeKind::Var(strip_name(n)),
            TypeKind::Fn { params, ret } => TypeKind::Fn {
                params: params.iter().map(strip_type).collect(),
                ret: Box::new(strip_type(ret)),
            },
            TypeKind::Tuple(items) => TypeKind::Tuple(items.iter().map(strip_type).collect()),
        }
    }

    fn strip_type_name(n: &TypeName) -> TypeName {
        match n {
            TypeName::Unqualified(u) => TypeName::Unqualified(strip_uname(u)),
            TypeName::Qualified { module, name } => TypeName::Qualified {
                module: strip_name(module),
                name: strip_uname(name),
            },
        }
    }
}
