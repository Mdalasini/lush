//! Lexical tokens for Lush (§3, §5.7).

use crate::span::Span;

/// A token with its source span.
#[derive(Clone, Debug, PartialEq)]
pub struct SpannedToken {
    pub kind: TokenKind,
    pub span: Span,
    /// Leading trivia (whitespace and comments) immediately before this token.
    pub leading: Vec<Trivia>,
}

/// Non-significant lexical material preserved for formatting.
///
/// Text is recovered by slicing the normalised source with [`Span`]; storing
/// only the span avoids one allocation per whitespace/comment run.
#[derive(Clone, Debug, PartialEq)]
pub struct Trivia {
    pub kind: TriviaKind,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriviaKind {
    Whitespace,
    LineComment,
    DocComment,
    ModuleDocComment,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    // Keywords
    As,
    Assert,
    Case,
    Const,
    Else,
    Echo,
    Fn,
    If,
    Import,
    Let,
    Opaque,
    Panic,
    Pub,
    Todo,
    Type,
    Use,

    // Identifiers
    /// Lower/underscore-start name (values, functions, modules, labels).
    Ident(String),
    /// PascalCase / uppercase-start name (types and constructors).
    UIdent(String),
    /// Bare discard `_`.
    Discard,

    // Literals
    Int(IntLit),
    Float(FloatLit),
    String(StringLit),

    // Punctuation / operators
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    HashLParen, // #(
    LShift,     // <<
    RShift,     // >>
    Comma,
    Dot,
    DotDot, // ..
    Colon,
    Semicolon,
    Pipe,      // |
    PipeArrow, // |>
    Arrow,     // ->
    LeftArrow, // <-
    Eq,        // =
    EqEq,      // ==
    NotEq,     // !=
    Lt,
    LtEq,
    Gt,
    GtEq,
    LtDot,   // <.
    LtEqDot, // <=.
    GtDot,   // >.
    GtEqDot, // >=.
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    PlusDot,  // +.
    MinusDot, // -.
    StarDot,  // *.
    SlashDot, // /.
    Bang,     // !
    AmpAmp,   // &&
    PipePipe, // ||
    LtGt,     // <>
    At,       // unused but reserved for future; not in grammar
    Eof,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntLit {
    /// Digits as written (underscores stripped).
    pub digits: String,
    pub base: IntBase,
    /// Raw lexeme including prefixes and underscores.
    pub raw: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntBase {
    Decimal,
    Hex,
    Octal,
    Binary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloatLit {
    pub raw: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StringLit {
    /// Decoded string contents.
    pub value: String,
    /// Raw lexeme including quotes.
    pub raw: String,
}

impl TokenKind {
    pub fn is_keyword(&self) -> bool {
        matches!(
            self,
            TokenKind::As
                | TokenKind::Assert
                | TokenKind::Case
                | TokenKind::Const
                | TokenKind::Else
                | TokenKind::Echo
                | TokenKind::Fn
                | TokenKind::If
                | TokenKind::Import
                | TokenKind::Let
                | TokenKind::Opaque
                | TokenKind::Panic
                | TokenKind::Pub
                | TokenKind::Todo
                | TokenKind::Type
                | TokenKind::Use
        )
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            TokenKind::As => "as",
            TokenKind::Assert => "assert",
            TokenKind::Case => "case",
            TokenKind::Const => "const",
            TokenKind::Else => "else",
            TokenKind::Echo => "echo",
            TokenKind::Fn => "fn",
            TokenKind::If => "if",
            TokenKind::Import => "import",
            TokenKind::Let => "let",
            TokenKind::Opaque => "opaque",
            TokenKind::Panic => "panic",
            TokenKind::Pub => "pub",
            TokenKind::Todo => "todo",
            TokenKind::Type => "type",
            TokenKind::Use => "use",
            TokenKind::Discard => "_",
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::LBrace => "{",
            TokenKind::RBrace => "}",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::HashLParen => "#(",
            TokenKind::LShift => "<<",
            TokenKind::RShift => ">>",
            TokenKind::Comma => ",",
            TokenKind::Dot => ".",
            TokenKind::DotDot => "..",
            TokenKind::Colon => ":",
            TokenKind::Semicolon => ";",
            TokenKind::Pipe => "|",
            TokenKind::PipeArrow => "|>",
            TokenKind::Arrow => "->",
            TokenKind::LeftArrow => "<-",
            TokenKind::Eq => "=",
            TokenKind::EqEq => "==",
            TokenKind::NotEq => "!=",
            TokenKind::Lt => "<",
            TokenKind::LtEq => "<=",
            TokenKind::Gt => ">",
            TokenKind::GtEq => ">=",
            TokenKind::LtDot => "<.",
            TokenKind::LtEqDot => "<=.",
            TokenKind::GtDot => ">.",
            TokenKind::GtEqDot => ">=.",
            TokenKind::Plus => "+",
            TokenKind::Minus => "-",
            TokenKind::Star => "*",
            TokenKind::Slash => "/",
            TokenKind::Percent => "%",
            TokenKind::PlusDot => "+.",
            TokenKind::MinusDot => "-.",
            TokenKind::StarDot => "*.",
            TokenKind::SlashDot => "/.",
            TokenKind::Bang => "!",
            TokenKind::AmpAmp => "&&",
            TokenKind::PipePipe => "||",
            TokenKind::LtGt => "<>",
            TokenKind::At => "@",
            TokenKind::Eof => "<eof>",
            TokenKind::Ident(_)
            | TokenKind::UIdent(_)
            | TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::String(_) => "<value>",
        }
    }

    /// Human-readable description for diagnostics (§11.6).
    pub fn describe(&self) -> String {
        match self {
            TokenKind::Ident(s) => format!("identifier `{s}`"),
            TokenKind::UIdent(s) => format!("type name `{s}`"),
            TokenKind::Int(i) => format!("integer `{}`", i.raw),
            TokenKind::Float(f) => format!("float `{}`", f.raw),
            TokenKind::String(s) => format!("string `{}`", s.raw.escape_default()),
            TokenKind::Eof => "end of file".into(),
            other => format!("`{}`", other.as_str()),
        }
    }
}
