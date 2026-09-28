//! Lexer for Lush source (`spec.md` §3).

use logos::Logos;

use crate::error::SyntaxError;
use crate::span::Span;

/// A lexed token with its source span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSpan {
    /// Token kind.
    pub kind: Token,
    /// Byte span in the source.
    pub span: Span,
}

/// Token kinds produced by the lexer.
#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[logos(skip r"[ \t\r\n]+")]
pub enum Token {
    // Comments (longest match: module doc, doc, line)
    /// `////` module documentation comment.
    #[regex(r"////[^\n]*")]
    ModuleDocComment,
    /// `///` documentation comment.
    #[regex(r"///[^\n]*")]
    DocComment,
    /// `//` line comment.
    #[regex(r"//[^\n]*")]
    LineComment,

    // Keywords
    /// `as`
    #[token("as")]
    As,
    /// `assert`
    #[token("assert")]
    Assert,
    /// `case`
    #[token("case")]
    Case,
    /// `const`
    #[token("const")]
    Const,
    /// `else`
    #[token("else")]
    Else,
    /// `fn`
    #[token("fn")]
    Fn,
    /// `if`
    #[token("if")]
    If,
    /// `import`
    #[token("import")]
    Import,
    /// `let`
    #[token("let")]
    Let,
    /// `opaque`
    #[token("opaque")]
    Opaque,
    /// `panic`
    #[token("panic")]
    Panic,
    /// `pub`
    #[token("pub")]
    Pub,
    /// `todo`
    #[token("todo")]
    Todo,
    /// `type`
    #[token("type")]
    Type,
    /// `use`
    #[token("use")]
    Use,
    /// `echo`
    #[token("echo")]
    Echo,

    // Identifiers
    /// Discard placeholder `_`.
    #[token("_", priority = 3)]
    Discard,
    /// Lowercase / underscore identifier (values, functions, modules, labels).
    #[regex(r"[a-z_][A-Za-z0-9_]*", priority = 1)]
    Ident,
    /// PascalCase identifier (types and constructors).
    #[regex(r"[A-Z][A-Za-z0-9]*")]
    UIdent,

    // Literals
    /// Integer literal (`123`, `0xFF`, `0o17`, `0b1010`, with optional `_`).
    #[regex(r"0[xX][0-9a-fA-F][0-9a-fA-F_]*")]
    #[regex(r"0[oO][0-7][0-7_]*")]
    #[regex(r"0[bB][01][01_]*")]
    #[regex(r"[0-9][0-9_]*")]
    Int,
    /// Float literal; a `.` is required (`1.0`, `1.5e10`, `2_000.5`).
    #[regex(r"[0-9][0-9_]*\.[0-9][0-9_]*([eE][+-]?[0-9][0-9_]*)?")]
    Float,
    /// String literal with escapes.
    #[regex(r#""([^"\\]|\\.)*""#)]
    String,

    // Multi-character operators (before single-char)
    /// `|>`
    #[token("|>")]
    Pipe,
    /// `->`
    #[token("->")]
    Arrow,
    /// `<-`
    #[token("<-")]
    LeftArrow,
    /// `<<`
    #[token("<<")]
    LessLess,
    /// `>>`
    #[token(">>")]
    GreaterGreater,
    /// `..`
    #[token("..")]
    DotDot,
    /// `==`
    #[token("==")]
    EqEq,
    /// `!=`
    #[token("!=")]
    NotEq,
    /// `&&`
    #[token("&&")]
    AndAnd,
    /// `||`
    #[token("||")]
    OrOr,
    /// `<=.`
    #[token("<=.")]
    LessEqDot,
    /// `>=.`
    #[token(">=.")]
    GreaterEqDot,
    /// `<.`
    #[token("<.")]
    LessDot,
    /// `>.`
    #[token(">.")]
    GreaterDot,
    /// `<=`
    #[token("<=")]
    LessEq,
    /// `>=`
    #[token(">=")]
    GreaterEq,
    /// `*.`
    #[token("*.")]
    StarDot,
    /// `/.`
    #[token("/.")]
    SlashDot,
    /// `+.`
    #[token("+.")]
    PlusDot,
    /// `-.`
    #[token("-.")]
    MinusDot,
    /// `<>`
    #[token("<>")]
    LessGreater,
    /// `#(`
    #[token("#(")]
    HashLParen,

    // Single-character punctuation / operators
    /// `(`
    #[token("(")]
    LParen,
    /// `)`
    #[token(")")]
    RParen,
    /// `{`
    #[token("{")]
    LBrace,
    /// `}`
    #[token("}")]
    RBrace,
    /// `[`
    #[token("[")]
    LBracket,
    /// `]`
    #[token("]")]
    RBracket,
    /// `,`
    #[token(",")]
    Comma,
    /// `;`
    #[token(";")]
    Semicolon,
    /// `:`
    #[token(":")]
    Colon,
    /// `.`
    #[token(".")]
    Dot,
    /// `=`
    #[token("=")]
    Eq,
    /// `|`
    #[token("|")]
    PipeBar,
    /// `+`
    #[token("+")]
    Plus,
    /// `-`
    #[token("-")]
    Minus,
    /// `*`
    #[token("*")]
    Star,
    /// `/`
    #[token("/")]
    Slash,
    /// `%`
    #[token("%")]
    Percent,
    /// `<`
    #[token("<")]
    Less,
    /// `>`
    #[token(">")]
    Greater,
    /// `!`
    #[token("!")]
    Bang,
}

/// Lex `source` into tokens. `\r` is skipped as whitespace, so CRLF keeps
/// original byte offsets for diagnostics.
pub fn lex(source: &str) -> Result<Vec<TokenSpan>, Vec<SyntaxError>> {
    lex_normalized(source)
}

/// Lex source text (same as [`lex`]; name kept for call sites).
pub fn lex_normalized(source: &str) -> Result<Vec<TokenSpan>, Vec<SyntaxError>> {
    let mut lexer = Token::lexer(source);
    let mut tokens = Vec::new();
    let mut errors = Vec::new();

    while let Some(result) = lexer.next() {
        let span = Span::new(lexer.span().start, lexer.span().end);
        match result {
            Ok(kind) => tokens.push(TokenSpan { kind, span }),
            Err(()) => errors.push(SyntaxError::Lex {
                span,
                message: format!("unexpected character {:?}", span.slice(source)),
            }),
        }
    }

    if errors.is_empty() {
        Ok(tokens)
    } else {
        Err(errors)
    }
}

/// Lex and omit comment tokens (kept available for the formatter).
pub fn lex_significant(source: &str) -> Result<Vec<TokenSpan>, Vec<SyntaxError>> {
    let tokens = lex(source)?;
    Ok(tokens
        .into_iter()
        .filter(|t| {
            !matches!(
                t.kind,
                Token::LineComment | Token::DocComment | Token::ModuleDocComment
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<Token> {
        lex(source)
            .unwrap_or_else(|e| panic!("lex errors: {e:?}"))
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    #[test]
    fn keywords_and_idents() {
        assert_eq!(
            kinds("pub fn add_one(x) { x; }"),
            vec![
                Token::Pub,
                Token::Fn,
                Token::Ident,
                Token::LParen,
                Token::Ident,
                Token::RParen,
                Token::LBrace,
                Token::Ident,
                Token::Semicolon,
                Token::RBrace,
            ]
        );
    }

    #[test]
    fn integer_and_float_literals() {
        assert_eq!(
            kinds("123 1_000 0xFF 0o17 0b1010 1.0 1.5e10 2_000.5"),
            vec![
                Token::Int,
                Token::Int,
                Token::Int,
                Token::Int,
                Token::Int,
                Token::Float,
                Token::Float,
                Token::Float,
            ]
        );
    }

    #[test]
    fn negative_float_is_minus_then_float() {
        assert_eq!(kinds("-1.5"), vec![Token::Minus, Token::Float]);
    }

    #[test]
    fn comments_and_strings() {
        let toks = kinds("//// mod\n/// doc\n// line\n\"hi\\n\";");
        assert_eq!(
            toks,
            vec![
                Token::ModuleDocComment,
                Token::DocComment,
                Token::LineComment,
                Token::String,
                Token::Semicolon,
            ]
        );
    }

    #[test]
    fn operators_and_bit_array() {
        assert_eq!(
            kinds("a |> b <<1>> == != && || <- -> .. <>"),
            vec![
                Token::Ident,
                Token::Pipe,
                Token::Ident,
                Token::LessLess,
                Token::Int,
                Token::GreaterGreater,
                Token::EqEq,
                Token::NotEq,
                Token::AndAnd,
                Token::OrOr,
                Token::LeftArrow,
                Token::Arrow,
                Token::DotDot,
                Token::LessGreater,
            ]
        );
    }

    #[test]
    fn float_ops_before_dot() {
        assert_eq!(
            kinds("1.0 +. 2.0 *. 3.0"),
            vec![
                Token::Float,
                Token::PlusDot,
                Token::Float,
                Token::StarDot,
                Token::Float,
            ]
        );
    }

    #[test]
    fn discard_and_unused_binding() {
        assert_eq!(kinds("_ _name"), vec![Token::Discard, Token::Ident]);
    }

    #[test]
    fn crlf_normalized() {
        let tokens = lex("let x = 1;\r\n").unwrap();
        assert!(tokens.iter().any(|t| t.kind == Token::Let));
        assert!(tokens.iter().any(|t| t.kind == Token::Semicolon));
    }

    #[test]
    fn unexpected_character_errors() {
        let err = lex("let x = $;").unwrap_err();
        assert!(!err.is_empty());
        assert!(matches!(err[0], SyntaxError::Lex { .. }));
    }

    #[test]
    fn snapshot_sample_module() {
        let source = "\
//// Sample
import lush/list;
pub fn main() -> Nil {
  // greet
  echo \"hi\";
}
";
        let tokens: Vec<_> = lex_normalized(source)
            .unwrap()
            .into_iter()
            .map(|t| format!("{:?} `{}`", t.kind, t.span.slice(source)))
            .collect();
        insta::assert_debug_snapshot!(tokens);
    }
}
