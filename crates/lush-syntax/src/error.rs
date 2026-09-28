//! Syntax frontend errors.

use crate::span::Span;

/// A lexer or parser diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SyntaxError {
    /// Unexpected character or malformed token.
    #[error("unexpected token at bytes {}..{}: {message}", span.start, span.end)]
    Lex {
        /// Location of the bad input.
        span: Span,
        /// Human-readable detail.
        message: String,
    },
    /// Parser could not continue.
    #[error("parse error at bytes {}..{}: {message}", span.start, span.end)]
    Parse {
        /// Location of the bad input.
        span: Span,
        /// Human-readable detail.
        message: String,
    },
}

impl SyntaxError {
    /// Span associated with this error.
    pub fn span(&self) -> Span {
        match self {
            SyntaxError::Lex { span, .. } | SyntaxError::Parse { span, .. } => *span,
        }
    }
}
