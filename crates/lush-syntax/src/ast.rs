//! Abstract syntax tree for Lush modules.

use crate::span::Span;

/// A complete source module (one file).
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    /// Optional top-of-file module doc comment text (without `////` prefix).
    pub module_docs: Vec<String>,
    /// Imports in source order.
    pub imports: Vec<Import>,
    /// Top-level definitions.
    pub definitions: Vec<Definition>,
    /// Span covering the whole module.
    pub span: Span,
}

/// `import path[. { items }] [as name];`
#[derive(Debug, Clone, PartialEq)]
pub struct Import {
    /// Slash-separated path components (e.g. `lush`, `list`).
    pub path: Vec<String>,
    /// Optional selective import list.
    pub items: Option<Vec<ImportItem>>,
    /// Optional `as` binding for the module.
    pub alias: Option<String>,
    /// Source span.
    pub span: Span,
}

/// One selective import item.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportItem {
    /// True when prefixed with `type`.
    pub is_type: bool,
    /// Imported name.
    pub name: String,
    /// Optional rename target.
    pub alias: Option<String>,
    /// Source span.
    pub span: Span,
}

/// Top-level definition.
#[derive(Debug, Clone, PartialEq)]
pub enum Definition {
    /// Function definition.
    Fn(FnDef),
    /// Type definition (ADT or alias).
    Type(TypeDef),
    /// Constant definition.
    Const(ConstDef),
}

/// `pub? fn name(params) -> ret? body`
#[derive(Debug, Clone, PartialEq)]
pub struct FnDef {
    /// Whether the function is public.
    pub is_pub: bool,
    /// Function name.
    pub name: String,
    /// Parameters.
    pub params: Vec<Param>,
    /// Optional return type.
    pub return_type: Option<TypeExpr>,
    /// Function body.
    pub body: Block,
    /// Source span.
    pub span: Span,
}

/// Function parameter: optional external label, internal name, optional type.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    /// External label when distinct from the binding name.
    pub label: Option<String>,
    /// Internal binding name.
    pub name: String,
    /// Optional type annotation.
    pub ty: Option<TypeExpr>,
    /// Source span.
    pub span: Span,
}

/// `pub? opaque? type Name(tvars)? { variants } | = type`
#[derive(Debug, Clone, PartialEq)]
pub struct TypeDef {
    /// Whether the type is public.
    pub is_pub: bool,
    /// Whether constructors are opaque outside the module.
    pub is_opaque: bool,
    /// Type name.
    pub name: String,
    /// Type parameters.
    pub params: Vec<String>,
    /// Body: ADT variants or alias.
    pub body: TypeDefBody,
    /// Source span.
    pub span: Span,
}

/// Type definition body.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeDefBody {
    /// Algebraic data type with variants.
    Adt(Vec<Variant>),
    /// Transparent alias.
    Alias(TypeExpr),
}

/// One ADT variant.
#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    /// Constructor name.
    pub name: String,
    /// Fields (empty for nullary).
    pub fields: Vec<Field>,
    /// Source span.
    pub span: Span,
}

/// Variant field: optional label and type.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// Optional field label.
    pub label: Option<String>,
    /// Field type.
    pub ty: TypeExpr,
    /// Source span.
    pub span: Span,
}

/// `const name: ty? = expr;`
#[derive(Debug, Clone, PartialEq)]
pub struct ConstDef {
    /// Whether the constant is public.
    pub is_pub: bool,
    /// Constant name.
    pub name: String,
    /// Optional type annotation.
    pub ty: Option<TypeExpr>,
    /// Initializer expression.
    pub value: Expr,
    /// Source span.
    pub span: Span,
}

/// `{ statements }`
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// Statements in order.
    pub statements: Vec<Statement>,
    /// Source span.
    pub span: Span,
}

/// Statement inside a block.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // AST nodes are heap-allocated at the block level.
pub enum Statement {
    /// Local function definition (no trailing `;`).
    Fn(FnDef),
    /// `let` / `let assert` binding terminated by `;`.
    Let(LetStmt),
    /// `use` callback-flattening binding terminated by `;`.
    Use(UseStmt),
    /// Expression statement terminated by `;`.
    Expr(Expr),
}

/// `let [assert] pattern [: ty] = expr [as msg]`
#[derive(Debug, Clone, PartialEq)]
pub struct LetStmt {
    /// True for `let assert`.
    pub is_assert: bool,
    /// Bound pattern.
    pub pattern: Pattern,
    /// Optional type annotation.
    pub ty: Option<TypeExpr>,
    /// Right-hand side.
    pub value: Expr,
    /// Optional custom panic message for `let assert`.
    pub message: Option<Expr>,
    /// Source span.
    pub span: Span,
}

/// `use [patterns] <- expr`
#[derive(Debug, Clone, PartialEq)]
pub struct UseStmt {
    /// Callback parameter patterns (may be empty).
    pub patterns: Vec<Pattern>,
    /// Right-hand side call or function reference.
    pub value: Expr,
    /// Source span.
    pub span: Span,
}

/// Expression node.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    /// Expression kind.
    pub kind: ExprKind,
    /// Source span.
    pub span: Span,
}

/// Expression variants.
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// Integer literal text (underscores preserved).
    Int(String),
    /// Float literal text.
    Float(String),
    /// String literal including quotes; escapes unresolved at parse time.
    String(String),
    /// Variable or module-qualified value reference chain head.
    Ident(String),
    /// Constructor / type name.
    Constructor(String),
    /// Qualified access: `expr.name` or `expr.Name`.
    Field {
        /// Base expression.
        base: Box<Expr>,
        /// Field or constructor segment.
        field: String,
        /// True when the segment is PascalCase.
        is_constructor: bool,
    },
    /// Call or capture: `callee(args)`.
    Call {
        /// Callee.
        callee: Box<Expr>,
        /// Arguments.
        args: Vec<Arg>,
    },
    /// Binary operator application.
    Binary {
        /// Left operand.
        left: Box<Expr>,
        /// Operator.
        op: BinOp,
        /// Right operand.
        right: Box<Expr>,
    },
    /// Unary operator application.
    Unary {
        /// Operator.
        op: UnaryOp,
        /// Operand.
        expr: Box<Expr>,
    },
    /// Pipe `left |> right`.
    Pipe {
        /// Left operand (evaluated once).
        left: Box<Expr>,
        /// Right-hand callee or call.
        right: Box<Expr>,
    },
    /// Anonymous or local-style function expression.
    Fn {
        /// Parameters.
        params: Vec<Param>,
        /// Optional return type.
        return_type: Option<TypeExpr>,
        /// Body.
        body: Block,
    },
    /// `case subjects { clauses }`
    Case {
        /// Subjects (one or more).
        subjects: Vec<Expr>,
        /// Arms.
        clauses: Vec<Clause>,
    },
    /// `todo [as msg]`
    Todo {
        /// Optional message.
        message: Option<Box<Expr>>,
    },
    /// `panic [as msg]`
    Panic {
        /// Optional message.
        message: Option<Box<Expr>>,
    },
    /// `assert expr [as msg]`
    Assert {
        /// Condition.
        condition: Box<Expr>,
        /// Optional message.
        message: Option<Box<Expr>>,
    },
    /// `echo expr`
    Echo {
        /// Value to print.
        value: Box<Expr>,
    },
    /// Explicit parentheses `(expr)` — preserved so non-associative re-association stays valid.
    Group(Box<Expr>),
    /// Nested block.
    Block(Block),
    /// List literal.
    List {
        /// Element expressions.
        items: Vec<Expr>,
        /// Optional tail spread `..expr`.
        spread: Option<Box<Expr>>,
    },
    /// Tuple `#(...)` arity 2+.
    Tuple(Vec<Expr>),
    /// Bit array `<< segments >>`.
    BitArray(Vec<BitSegment>),
    /// Record update `Ctor(..base, field: value, ...)` or `mod.Ctor(..base, ...)`.
    RecordUpdate {
        /// Optional module qualifier.
        module: Option<String>,
        /// Constructor name.
        constructor: String,
        /// Base record.
        base: Box<Expr>,
        /// Updated fields.
        fields: Vec<(String, Expr)>,
    },
}

/// Call argument, optionally labelled; may be `_` capture placeholder.
#[derive(Debug, Clone, PartialEq)]
pub struct Arg {
    /// Optional label.
    pub label: Option<String>,
    /// Argument value or placeholder.
    pub value: ArgValue,
    /// Source span.
    pub span: Span,
}

/// Call argument payload.
#[derive(Debug, Clone, PartialEq)]
pub enum ArgValue {
    /// Ordinary expression argument.
    Expr(Expr),
    /// Capture placeholder `_`.
    Hole,
}

/// Binary operators (precedence in parser).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Rem,
    /// `*.`
    MulFloat,
    /// `/.`
    DivFloat,
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `+.`
    AddFloat,
    /// `-.`
    SubFloat,
    /// `<>`
    Concat,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `<.`
    LtFloat,
    /// `<=.`
    LeFloat,
    /// `>.`
    GtFloat,
    /// `>=.`
    GeFloat,
    /// `==`
    Eq,
    /// `!=`
    NotEq,
    /// `&&`
    And,
    /// `||`
    Or,
}

/// Prefix unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// `-`
    Neg,
    /// `!`
    Not,
}

/// `case` clause.
#[derive(Debug, Clone, PartialEq)]
pub struct Clause {
    /// Alternatives separated by `|`.
    pub patterns: Vec<PatternRow>,
    /// Optional guard.
    pub guard: Option<Expr>,
    /// Branch body expression.
    pub body: Expr,
    /// Source span.
    pub span: Span,
}

/// One pattern row (matches subjects left-to-right).
#[derive(Debug, Clone, PartialEq)]
pub struct PatternRow {
    /// Patterns for each subject.
    pub patterns: Vec<Pattern>,
    /// Source span.
    pub span: Span,
}

/// Pattern node.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    /// Pattern kind.
    pub kind: PatternKind,
    /// Source span.
    pub span: Span,
}

/// Pattern variants.
#[derive(Debug, Clone, PartialEq)]
pub enum PatternKind {
    /// Discard `_`.
    Discard,
    /// Variable binding.
    Var(String),
    /// Integer literal.
    Int(String),
    /// Float literal.
    Float(String),
    /// String literal.
    String(String),
    /// Constructor with optional fields and optional `..` catch-all.
    Constructor {
        /// Optional module qualifier.
        module: Option<String>,
        /// Constructor name.
        name: String,
        /// Field patterns.
        fields: Vec<PatternField>,
        /// Whether `..` is present.
        with_spread: bool,
    },
    /// Tuple pattern.
    Tuple(Vec<Pattern>),
    /// List pattern with optional rest.
    List {
        /// Element patterns.
        items: Vec<Pattern>,
        /// Optional rest binding `..rest`.
        rest: Option<Box<Pattern>>,
    },
    /// String prefix `"lit" <> name`.
    StringPrefix {
        /// Literal prefix including quotes.
        literal: String,
        /// Remainder binding.
        rest: Box<Pattern>,
    },
    /// Bit array pattern.
    BitArray(Vec<BitSegment>),
    /// `pattern as name`
    As {
        /// Inner pattern.
        pattern: Box<Pattern>,
        /// Alias binding.
        name: String,
    },
}

/// Constructor field pattern.
#[derive(Debug, Clone, PartialEq)]
pub struct PatternField {
    /// Optional field label.
    pub label: Option<String>,
    /// Field pattern.
    pub pattern: Pattern,
    /// Source span.
    pub span: Span,
}

/// Bit array segment (literal construction or pattern).
#[derive(Debug, Clone, PartialEq)]
pub struct BitSegment {
    /// Segment value / pattern expression textually as an expression or pattern.
    pub value: BitSegmentValue,
    /// Options after `:`, e.g. `size(16)`, `utf8`.
    pub options: Vec<String>,
    /// Source span.
    pub span: Span,
}

/// Payload of a bit segment.
#[derive(Debug, Clone, PartialEq)]
pub enum BitSegmentValue {
    /// Expression form (construction).
    Expr(Expr),
    /// Pattern form.
    Pattern(Pattern),
}

/// Type expression.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeExpr {
    /// Type kind.
    pub kind: TypeKind,
    /// Source span.
    pub span: Span,
}

/// Type expression variants.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeKind {
    /// Named type, optionally qualified and applied: `list.List(Int)`.
    Named {
        /// Optional module qualifier.
        module: Option<String>,
        /// Type name.
        name: String,
        /// Type arguments.
        args: Vec<TypeExpr>,
    },
    /// Type variable / lowercase name.
    Var(String),
    /// Function type.
    Fn {
        /// Parameter types.
        params: Vec<TypeExpr>,
        /// Return type.
        ret: Box<TypeExpr>,
    },
    /// Tuple type arity 2+.
    Tuple(Vec<TypeExpr>),
}
