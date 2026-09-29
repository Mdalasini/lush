//! Hand-written lexer for Lush (§3).
//!
//! Normalises `\r\n` to `\n` (bare `\r` is left alone), preserves comments as
//! trivia for the formatter, and attaches a span to every token.

use std::borrow::Cow;

use crate::codes;
use crate::diagnostic::{escape_for_message, Diagnostic, DiagnosticKind, DiagnosticSink, Severity};
use crate::span::{Span, MAX_SOURCE_LEN};
use crate::token::{
    FloatLit, IntBase, IntLit, SpannedToken, StringLit, TokenKind, Trivia, TriviaKind,
};

/// Lex `source` into a token stream with trailing EOF.
pub fn lex(source: &str) -> LexResult {
    if source.len() > MAX_SOURCE_LEN {
        let mut sink = DiagnosticSink::new();
        sink.error(
            codes::E0001_UNEXPECTED_CHAR,
            format!(
                "source file is too large ({} bytes; maximum is {MAX_SOURCE_LEN})",
                source.len()
            ),
            Span::empty(0),
            Some("split the file or raise the implementation limit".into()),
            DiagnosticKind::Lexer,
        );
        return LexResult {
            source: String::new(),
            tokens: vec![SpannedToken {
                kind: TokenKind::Eof,
                span: Span::empty(0),
                leading: vec![],
            }],
            diagnostics: sink.into_diagnostics(),
            casing_warnings: vec![],
        };
    }
    let normalised = normalise_newlines(source);
    let (tokens, diagnostics, casing_warnings) = {
        let mut lexer = Lexer::new(&normalised);
        let tokens = lexer.lex_all();
        (
            tokens,
            lexer.diagnostics.into_diagnostics(),
            lexer.casing_warnings,
        )
    };
    LexResult {
        source: normalised.into_owned(),
        tokens,
        diagnostics,
        casing_warnings,
    }
}

#[derive(Debug)]
pub struct LexResult {
    /// Source after `\r\n` → `\n` normalisation (bare `\r` preserved).
    pub source: String,
    pub tokens: Vec<SpannedToken>,
    pub diagnostics: Vec<Diagnostic>,
    /// Wrong-casing identifier warnings (§3). Never rejects.
    pub casing_warnings: Vec<Diagnostic>,
}

/// Convert `\r\n` to `\n`. Bare `\r` is left unchanged.
///
/// Returns [`Cow::Borrowed`] when the source contains no `\r\n` sequences.
fn normalise_newlines(source: &str) -> Cow<'_, str> {
    if !source.contains("\r\n") {
        return Cow::Borrowed(source);
    }
    let mut out = String::with_capacity(source.len());
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
            out.push('\n');
            i += 2;
        } else {
            // Safe: we only skip whole `\r\n` pairs; other bytes stay UTF-8 aligned.
            let ch = source[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    Cow::Owned(out)
}

struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    diagnostics: DiagnosticSink,
    casing_warnings: Vec<Diagnostic>,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
            diagnostics: DiagnosticSink::new(),
            casing_warnings: Vec::new(),
        }
    }

    fn lex_all(&mut self) -> Vec<SpannedToken> {
        let mut tokens = Vec::new();
        loop {
            if self.diagnostics.is_capped() {
                tokens.push(SpannedToken {
                    kind: TokenKind::Eof,
                    span: Span::empty(self.pos),
                    leading: vec![],
                });
                break;
            }
            let leading = self.consume_trivia();
            if self.pos >= self.bytes.len() {
                tokens.push(SpannedToken {
                    kind: TokenKind::Eof,
                    span: Span::empty(self.pos),
                    leading,
                });
                break;
            }
            let start = self.pos;
            let kind = self.next_token();
            let end = self.pos;
            tokens.push(SpannedToken {
                kind,
                span: Span::new(start, end),
                leading,
            });
        }
        tokens
    }

    fn consume_trivia(&mut self) -> Vec<Trivia> {
        let mut trivia = Vec::new();
        loop {
            if self.pos >= self.bytes.len() {
                break;
            }
            let b = self.bytes[self.pos];
            if is_whitespace_byte(b) {
                let start = self.pos;
                while self.pos < self.bytes.len() && is_whitespace_byte(self.bytes[self.pos]) {
                    self.pos += 1;
                }
                trivia.push(Trivia {
                    kind: TriviaKind::Whitespace,
                    span: Span::new(start, self.pos),
                });
                continue;
            }
            if b == b'/' && self.peek(1) == Some(b'/') {
                let start = self.pos;
                self.pos += 2;
                // Count extra slashes for doc vs module-doc.
                let mut slash_count = 2usize;
                while self.pos < self.bytes.len() && self.bytes[self.pos] == b'/' {
                    slash_count += 1;
                    self.pos += 1;
                }
                while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                    let ch = self.src[self.pos..].chars().next().unwrap();
                    self.pos += ch.len_utf8();
                }
                let kind = match slash_count {
                    2 => TriviaKind::LineComment,
                    3 => TriviaKind::DocComment,
                    _ => TriviaKind::ModuleDocComment, // 4+
                };
                trivia.push(Trivia {
                    kind,
                    span: Span::new(start, self.pos),
                });
                continue;
            }
            break;
        }
        trivia
    }

    fn next_token(&mut self) -> TokenKind {
        let b = self.bytes[self.pos];
        match b {
            b'(' => {
                self.pos += 1;
                TokenKind::LParen
            }
            b')' => {
                self.pos += 1;
                TokenKind::RParen
            }
            b'{' => {
                self.pos += 1;
                TokenKind::LBrace
            }
            b'}' => {
                self.pos += 1;
                TokenKind::RBrace
            }
            b'[' => {
                self.pos += 1;
                TokenKind::LBracket
            }
            b']' => {
                self.pos += 1;
                TokenKind::RBracket
            }
            b',' => {
                self.pos += 1;
                TokenKind::Comma
            }
            b';' => {
                self.pos += 1;
                TokenKind::Semicolon
            }
            b':' => {
                self.pos += 1;
                TokenKind::Colon
            }
            b'#' => {
                if self.peek(1) == Some(b'(') {
                    self.pos += 2;
                    TokenKind::HashLParen
                } else {
                    self.unexpected(self.pos, 1);
                    self.pos += 1;
                    TokenKind::Ident("#".into())
                }
            }
            b'.' => {
                if self.peek(1) == Some(b'.') {
                    self.pos += 2;
                    TokenKind::DotDot
                } else {
                    self.pos += 1;
                    TokenKind::Dot
                }
            }
            b'|' => {
                let next = self.peek(1);
                if next == Some(b'>') {
                    self.pos += 2;
                    TokenKind::PipeArrow
                } else if next == Some(b'|') {
                    self.pos += 2;
                    TokenKind::PipePipe
                } else {
                    self.pos += 1;
                    TokenKind::Pipe
                }
            }
            b'&' => {
                if self.peek(1) == Some(b'&') {
                    self.pos += 2;
                    TokenKind::AmpAmp
                } else {
                    self.unexpected(self.pos, 1);
                    self.pos += 1;
                    TokenKind::Ident("&".into())
                }
            }
            b'!' => {
                if self.peek(1) == Some(b'=') {
                    self.pos += 2;
                    TokenKind::NotEq
                } else {
                    self.pos += 1;
                    TokenKind::Bang
                }
            }
            b'=' => {
                if self.peek(1) == Some(b'=') {
                    self.pos += 2;
                    TokenKind::EqEq
                } else {
                    self.pos += 1;
                    TokenKind::Eq
                }
            }
            b'<' => self.lex_lt(),
            b'>' => self.lex_gt(),
            b'+' => {
                if self.peek(1) == Some(b'.') {
                    self.pos += 2;
                    TokenKind::PlusDot
                } else {
                    self.pos += 1;
                    TokenKind::Plus
                }
            }
            b'-' => {
                let next = self.peek(1);
                if next == Some(b'>') {
                    self.pos += 2;
                    TokenKind::Arrow
                } else if next == Some(b'.') {
                    self.pos += 2;
                    TokenKind::MinusDot
                } else {
                    self.pos += 1;
                    TokenKind::Minus
                }
            }
            b'*' => {
                if self.peek(1) == Some(b'.') {
                    self.pos += 2;
                    TokenKind::StarDot
                } else {
                    self.pos += 1;
                    TokenKind::Star
                }
            }
            b'/' => {
                // Comments already handled in trivia; `/` or `/.` here.
                if self.peek(1) == Some(b'.') {
                    self.pos += 2;
                    TokenKind::SlashDot
                } else {
                    self.pos += 1;
                    TokenKind::Slash
                }
            }
            b'%' => {
                self.pos += 1;
                TokenKind::Percent
            }
            b'"' => self.lex_string(),
            b'0'..=b'9' => self.lex_number(),
            b'a'..=b'z' | b'_' => self.lex_lower_ident(),
            b'A'..=b'Z' => self.lex_upper_ident(),
            _ => {
                let ch = self.src[self.pos..].chars().next().unwrap();
                let len = ch.len_utf8();
                self.unexpected(self.pos, len);
                self.pos += len;
                TokenKind::Ident(ch.to_string())
            }
        }
    }

    fn lex_lt(&mut self) -> TokenKind {
        // Possible: <<  <-  <>  <=.  <=  <.  <
        let b1 = self.peek(1);
        let b2 = self.peek(2);
        if b1 == Some(b'<') {
            self.pos += 2;
            TokenKind::LShift
        } else if b1 == Some(b'-') {
            self.pos += 2;
            TokenKind::LeftArrow
        } else if b1 == Some(b'>') {
            self.pos += 2;
            TokenKind::LtGt
        } else if b1 == Some(b'=') && b2 == Some(b'.') {
            self.pos += 3;
            TokenKind::LtEqDot
        } else if b1 == Some(b'=') {
            self.pos += 2;
            TokenKind::LtEq
        } else if b1 == Some(b'.') {
            self.pos += 2;
            TokenKind::LtDot
        } else {
            self.pos += 1;
            TokenKind::Lt
        }
    }

    fn lex_gt(&mut self) -> TokenKind {
        // Possible: >>  >=.  >=  >.  >
        let b1 = self.peek(1);
        let b2 = self.peek(2);
        if b1 == Some(b'>') {
            self.pos += 2;
            TokenKind::RShift
        } else if b1 == Some(b'=') && b2 == Some(b'.') {
            self.pos += 3;
            TokenKind::GtEqDot
        } else if b1 == Some(b'=') {
            self.pos += 2;
            TokenKind::GtEq
        } else if b1 == Some(b'.') {
            self.pos += 2;
            TokenKind::GtDot
        } else {
            self.pos += 1;
            TokenKind::Gt
        }
    }

    fn lex_lower_ident(&mut self) -> TokenKind {
        let start = self.pos;
        // `_` alone is Discard; `_name` is a normal binding.
        if self.bytes[self.pos] == b'_' {
            self.pos += 1;
            if self.pos >= self.bytes.len() || !is_ident_continue(self.bytes[self.pos]) {
                return TokenKind::Discard;
            }
        } else {
            self.pos += 1;
        }
        while self.pos < self.bytes.len() && is_ident_continue(self.bytes[self.pos]) {
            self.pos += 1;
        }
        let text = &self.src[start..self.pos];
        if let Some(kw) = keyword(text) {
            return kw;
        }
        self.warn_casing_lower(text, start);
        TokenKind::Ident(text.to_string())
    }

    fn lex_upper_ident(&mut self) -> TokenKind {
        let start = self.pos;
        self.pos += 1;
        while self.pos < self.bytes.len() && is_ident_continue_upper(self.bytes[self.pos]) {
            self.pos += 1;
        }
        let text = &self.src[start..self.pos];
        self.warn_casing_upper(text, start);
        TokenKind::UIdent(text.to_string())
    }

    fn lex_number(&mut self) -> TokenKind {
        let start = self.pos;
        // Bases: 0x / 0o / 0b
        if self.bytes[self.pos] == b'0' {
            match self.peek(1) {
                Some(b'x') | Some(b'X') => {
                    self.pos += 2;
                    let digits_start = self.pos;
                    self.consume_digits(|c| c.is_ascii_hexdigit() || c == b'_');
                    let digits: String = self.src[digits_start..self.pos]
                        .chars()
                        .filter(|c| *c != '_')
                        .collect();
                    if digits.is_empty() {
                        self.error(
                            Span::new(start, self.pos),
                            codes::E0002_BAD_NUMBER,
                            "hexadecimal literal needs at least one digit",
                            Some("write something like `0xFF`"),
                        );
                    }
                    self.reject_number_suffix(start, false);
                    let raw = self.src[start..self.pos].to_string();
                    return TokenKind::Int(IntLit {
                        digits,
                        base: IntBase::Hex,
                        raw,
                    });
                }
                Some(b'o') | Some(b'O') => {
                    self.pos += 2;
                    let digits_start = self.pos;
                    self.consume_digits(|c| matches!(c, b'0'..=b'7' | b'_'));
                    let digits: String = self.src[digits_start..self.pos]
                        .chars()
                        .filter(|c| *c != '_')
                        .collect();
                    if digits.is_empty() {
                        self.error(
                            Span::new(start, self.pos),
                            codes::E0002_BAD_NUMBER,
                            "octal literal needs at least one digit",
                            Some("write something like `0o17`"),
                        );
                    }
                    self.reject_number_suffix(start, false);
                    let raw = self.src[start..self.pos].to_string();
                    return TokenKind::Int(IntLit {
                        digits,
                        base: IntBase::Octal,
                        raw,
                    });
                }
                Some(b'b') | Some(b'B') => {
                    self.pos += 2;
                    let digits_start = self.pos;
                    self.consume_digits(|c| matches!(c, b'0' | b'1' | b'_'));
                    let digits: String = self.src[digits_start..self.pos]
                        .chars()
                        .filter(|c| *c != '_')
                        .collect();
                    if digits.is_empty() {
                        self.error(
                            Span::new(start, self.pos),
                            codes::E0002_BAD_NUMBER,
                            "binary literal needs at least one digit",
                            Some("write something like `0b1010`"),
                        );
                    }
                    self.reject_number_suffix(start, false);
                    let raw = self.src[start..self.pos].to_string();
                    return TokenKind::Int(IntLit {
                        digits,
                        base: IntBase::Binary,
                        raw,
                    });
                }
                _ => {}
            }
        }

        // Decimal int or float. A `.` followed by a digit starts a float.
        self.consume_digits(|c| c.is_ascii_digit() || c == b'_');
        let is_float =
            self.peek(0) == Some(b'.') && self.peek(1).map(|c| c.is_ascii_digit()).unwrap_or(false);
        if is_float {
            self.pos += 1; // '.'
            self.consume_digits(|c| c.is_ascii_digit() || c == b'_');
            if matches!(self.peek(0), Some(b'e') | Some(b'E')) {
                self.pos += 1;
                if matches!(self.peek(0), Some(b'+') | Some(b'-')) {
                    self.pos += 1;
                }
                let exp_start = self.pos;
                self.consume_digits(|c| c.is_ascii_digit() || c == b'_');
                if exp_start == self.pos {
                    self.error(
                        Span::new(start, self.pos),
                        codes::E0003_BAD_FLOAT,
                        "float exponent needs at least one digit",
                        Some("write something like `1.5e10`"),
                    );
                }
            }
            self.reject_number_suffix(start, true);
            let raw = self.src[start..self.pos].to_string();
            return TokenKind::Float(FloatLit { raw });
        }

        let digits: String = self.src[start..self.pos]
            .chars()
            .filter(|c| *c != '_')
            .collect();
        self.reject_number_suffix(start, false);
        let raw = self.src[start..self.pos].to_string();
        TokenKind::Int(IntLit {
            digits,
            base: IntBase::Decimal,
            raw,
        })
    }

    /// After a number, reject a glued identifier run (`123abc`, `1e10`).
    fn reject_number_suffix(&mut self, start: usize, is_float: bool) {
        if self.pos >= self.bytes.len() || !is_ident_continue(self.bytes[self.pos]) {
            return;
        }
        let hint = if !is_float && matches!(self.bytes[self.pos], b'e' | b'E') {
            Some("float literals need a `.`, e.g. `1.0e10`")
        } else {
            Some("separate the number and the following name with whitespace or an operator")
        };
        while self.pos < self.bytes.len() && is_ident_continue(self.bytes[self.pos]) {
            self.pos += 1;
        }
        let text = &self.src[start..self.pos];
        let code = if is_float {
            codes::E0003_BAD_FLOAT
        } else {
            codes::E0002_BAD_NUMBER
        };
        self.error(
            Span::new(start, self.pos),
            code,
            format!("invalid numeric literal `{text}`"),
            hint,
        );
    }

    fn lex_string(&mut self) -> TokenKind {
        let start = self.pos;
        self.pos += 1; // opening "
        let mut value = String::new();
        while self.pos < self.bytes.len() {
            let ch = self.src[self.pos..].chars().next().unwrap();
            if ch == '"' {
                self.pos += 1;
                let raw = self.src[start..self.pos].to_string();
                return TokenKind::String(StringLit { value, raw });
            }
            if ch == '\\' {
                self.pos += 1;
                if self.pos >= self.bytes.len() {
                    break;
                }
                let esc = self.src[self.pos..].chars().next().unwrap();
                match esc {
                    'n' => {
                        value.push('\n');
                        self.pos += 1;
                    }
                    'r' => {
                        value.push('\r');
                        self.pos += 1;
                    }
                    't' => {
                        value.push('\t');
                        self.pos += 1;
                    }
                    '\\' => {
                        value.push('\\');
                        self.pos += 1;
                    }
                    '"' => {
                        value.push('"');
                        self.pos += 1;
                    }
                    'u' => {
                        self.pos += 1;
                        if self.peek(0) != Some(b'{') {
                            self.error(
                                Span::new(self.pos.saturating_sub(2), self.pos),
                                codes::E0004_BAD_ESCAPE,
                                "invalid unicode escape; expected `\\u{...}`",
                                Some("write a code point like `\\u{1F600}`"),
                            );
                            continue;
                        }
                        self.pos += 1;
                        let hex_start = self.pos;
                        while self.pos < self.bytes.len()
                            && self.bytes[self.pos].is_ascii_hexdigit()
                        {
                            self.pos += 1;
                        }
                        let hex = &self.src[hex_start..self.pos];
                        if self.peek(0) != Some(b'}') {
                            self.error(
                                Span::new(hex_start.saturating_sub(3), self.pos),
                                codes::E0004_BAD_ESCAPE,
                                "unterminated unicode escape",
                                Some("close the escape with `}`"),
                            );
                            continue;
                        }
                        self.pos += 1;
                        match u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
                            Some(c) => value.push(c),
                            None => self.error(
                                Span::new(hex_start.saturating_sub(3), self.pos),
                                codes::E0004_BAD_ESCAPE,
                                "invalid unicode code point in string escape",
                                Some("use a valid scalar value such as `\\u{1F600}`"),
                            ),
                        }
                    }
                    other => {
                        self.error(
                            Span::new(self.pos - 1, self.pos + other.len_utf8()),
                            codes::E0004_BAD_ESCAPE,
                            format!("unknown string escape `\\{other}`"),
                            Some("supported escapes are `\\n \\r \\t \\\\ \\\" \\u{...}`"),
                        );
                        value.push(other);
                        self.pos += other.len_utf8();
                    }
                }
            } else {
                value.push(ch);
                self.pos += ch.len_utf8();
            }
        }
        self.error(
            Span::new(start, self.pos),
            codes::E0005_UNTERMINATED_STRING,
            "unterminated string literal",
            Some("add a closing `\"`"),
        );
        let raw = self.src[start..self.pos].to_string();
        TokenKind::String(StringLit { value, raw })
    }

    fn consume_digits(&mut self, mut pred: impl FnMut(u8) -> bool) {
        while self.pos < self.bytes.len() && pred(self.bytes[self.pos]) {
            self.pos += 1;
        }
    }

    fn peek(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn unexpected(&mut self, start: usize, len: usize) {
        let ch = self.src[start..].chars().next().unwrap_or('\0');
        self.error(
            Span::new(start, start + len),
            codes::E0001_UNEXPECTED_CHAR,
            format!("unexpected character `{}`", escape_for_message(ch)),
            Some("remove or escape this character"),
        );
    }

    fn error(
        &mut self,
        span: Span,
        code: impl Into<String>,
        message: impl Into<String>,
        hint: Option<&str>,
    ) {
        self.diagnostics.error(
            code,
            message,
            span,
            hint.map(str::to_string),
            DiagnosticKind::Lexer,
        );
    }

    fn warn_casing_lower(&mut self, text: &str, start: usize) {
        // Conventional: snake_case — ASCII uppercase letters are wrong casing.
        if text.chars().any(|c| c.is_ascii_uppercase()) {
            self.casing_warnings.push(Diagnostic {
                code: codes::W0001_CASING.into(),
                message: format!(
                    "identifier `{text}` should be `snake_case`; wrong casing is accepted but warned"
                ),
                span: Span::new(start, start + text.len()),
                severity: Severity::Warning,
                hint: Some("rename to snake_case, or keep the name and silence unused warnings with a `_` prefix".into()),
                kind: DiagnosticKind::Lexer,
                secondary: Vec::new(),
            });
        }
    }

    fn warn_casing_upper(&mut self, text: &str, start: usize) {
        // Conventional PascalCase: after the leading uppercase, underscores are unusual.
        if text.contains('_') {
            self.casing_warnings.push(Diagnostic {
                code: codes::W0001_CASING.into(),
                message: format!(
                    "identifier `{text}` should be `PascalCase`; wrong casing is accepted but warned"
                ),
                span: Span::new(start, start + text.len()),
                severity: Severity::Warning,
                hint: Some("rename to PascalCase without underscores".into()),
                kind: DiagnosticKind::Lexer,
                secondary: Vec::new(),
            });
        }
    }
}

fn is_whitespace_byte(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' || c == 0x0c
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_ident_continue_upper(b: u8) -> bool {
    // Types/constructors: [A-Za-z0-9]* after the leading uppercase (§3).
    b.is_ascii_alphanumeric() || b == b'_'
}

fn keyword(text: &str) -> Option<TokenKind> {
    Some(match text {
        "as" => TokenKind::As,
        "assert" => TokenKind::Assert,
        "case" => TokenKind::Case,
        "const" => TokenKind::Const,
        "else" => TokenKind::Else,
        "echo" => TokenKind::Echo,
        "fn" => TokenKind::Fn,
        "if" => TokenKind::If,
        "import" => TokenKind::Import,
        "let" => TokenKind::Let,
        "opaque" => TokenKind::Opaque,
        "panic" => TokenKind::Panic,
        "pub" => TokenKind::Pub,
        "todo" => TokenKind::Todo,
        "type" => TokenKind::Type,
        "use" => TokenKind::Use,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_crlf() {
        let result = lex("let x = 1;\r\n");
        assert!(!result.source.contains('\r'));
        assert!(result
            .tokens
            .iter()
            .any(|t| matches!(t.kind, TokenKind::Let)));
    }

    #[test]
    fn preserves_bare_cr_in_string() {
        // Bare CR (not part of CRLF) must survive newline normalisation.
        let result = lex("\"a\rb\"");
        assert!(result.source.contains('\r'));
        let TokenKind::String(s) = &result.tokens[0].kind else {
            panic!("expected string token");
        };
        assert_eq!(s.value, "a\rb");
    }

    #[test]
    fn bare_cr_is_whitespace_trivia() {
        let result = lex("let\rx");
        let x = result
            .tokens
            .iter()
            .find(|t| matches!(&t.kind, TokenKind::Ident(n) if n == "x"))
            .expect("ident x");
        assert!(x.leading.iter().any(|t| t.kind == TriviaKind::Whitespace));
        let ws = x
            .leading
            .iter()
            .find(|t| t.kind == TriviaKind::Whitespace)
            .unwrap();
        assert!(result.source[ws.span.start.as_usize()..ws.span.end.as_usize()].contains('\r'));
    }

    #[test]
    fn lexes_keywords_and_idents() {
        let result = lex("pub fn add_one(x) { x; }");
        let kinds: Vec<_> = result
            .tokens
            .iter()
            .map(|t| t.kind.as_str().to_string())
            .collect();
        assert!(kinds.contains(&"pub".to_string()));
        assert!(kinds.contains(&"fn".to_string()));
    }

    #[test]
    fn lexes_number_bases_and_float() {
        let result = lex("123 1_000 0xFF 0o17 0b1010 1.5 1.5e10");
        let ints = result
            .tokens
            .iter()
            .filter(|t| matches!(t.kind, TokenKind::Int(_)))
            .count();
        let floats = result
            .tokens
            .iter()
            .filter(|t| matches!(t.kind, TokenKind::Float(_)))
            .count();
        assert_eq!(ints, 5);
        assert_eq!(floats, 2);
    }

    #[test]
    fn number_glued_to_ident_is_error() {
        let result = lex("123abc");
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.code == codes::E0002_BAD_NUMBER),
            "expected E0002, got {:?}",
            result.diagnostics
        );
        // Trailing ident chars are consumed — no separate Ident token.
        assert!(
            !result
                .tokens
                .iter()
                .any(|t| matches!(&t.kind, TokenKind::Ident(n) if n == "abc")),
            "trailing ident should not be a separate token"
        );
    }

    #[test]
    fn scientific_without_dot_hints_float() {
        let result = lex("1e10");
        let d = result
            .diagnostics
            .iter()
            .find(|d| d.code == codes::E0002_BAD_NUMBER)
            .expect("E0002");
        assert!(
            d.hint.as_deref().is_some_and(|h| h.contains("1.0e10")),
            "hint was {:?}",
            d.hint
        );
    }

    #[test]
    fn preserves_comments_as_trivia() {
        let result = lex("// line\n/// doc\n//// module\nfn");
        let fn_tok = result
            .tokens
            .iter()
            .find(|t| matches!(t.kind, TokenKind::Fn))
            .unwrap();
        assert!(fn_tok
            .leading
            .iter()
            .any(|t| t.kind == TriviaKind::LineComment));
        assert!(fn_tok
            .leading
            .iter()
            .any(|t| t.kind == TriviaKind::DocComment));
        assert!(fn_tok
            .leading
            .iter()
            .any(|t| t.kind == TriviaKind::ModuleDocComment));
        // Trivia has only kind + span (text recovered via source[span]).
        for t in &fn_tok.leading {
            let _ = (t.kind, t.span);
        }
    }

    #[test]
    fn discard_and_underscore_name() {
        let result = lex("_ _name");
        assert!(matches!(result.tokens[0].kind, TokenKind::Discard));
        assert!(matches!(
            &result.tokens[1].kind,
            TokenKind::Ident(n) if n == "_name"
        ));
    }

    #[test]
    fn unexpected_control_char_is_escaped_in_message() {
        let result = lex("\u{1b}");
        let d = result
            .diagnostics
            .iter()
            .find(|d| d.code == codes::E0001_UNEXPECTED_CHAR)
            .expect("E0001");
        assert!(
            !d.message.as_bytes().contains(&0x1b),
            "raw ESC leaked into message: {:?}",
            d.message
        );
    }
}
