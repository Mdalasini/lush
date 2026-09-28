//! Type checker diagnostics.

use lush_syntax::Span;

/// A type checking error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TypeError {
    /// Unification failure.
    #[error("type mismatch at bytes {}..{}: {message}", span.start, span.end)]
    Mismatch {
        /// Location.
        span: Span,
        /// Detail.
        message: String,
    },
    /// Unbound name.
    #[error("unknown name `{name}` at bytes {}..{}", span.start, span.end)]
    Unbound {
        /// Location.
        span: Span,
        /// Name.
        name: String,
    },
    /// Other checking failure.
    #[error("type error at bytes {}..{}: {message}", span.start, span.end)]
    Other {
        /// Location.
        span: Span,
        /// Detail.
        message: String,
    },
}

/// A non-fatal type checker warning.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TypeWarning {
    /// Unreachable / redundant pattern.
    #[error("warning at bytes {}..{}: {message}", span.start, span.end)]
    RedundantPattern {
        /// Location.
        span: Span,
        /// Detail.
        message: String,
    },
}
