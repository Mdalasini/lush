//! Type-stage diagnostic helpers with graph-wide error capping (E1500).

use lush_syntax::diagnostic::{Diagnostic, DiagnosticKind, DiagnosticSink, Severity, MAX_ERRORS};
use lush_syntax::span::Span;

use crate::codes;

/// Collects type-stage diagnostics, honouring a shared error budget across the
/// module graph (seeded with parse/earlier errors).
#[derive(Debug)]
pub struct TypeSink {
    inner: DiagnosticSink,
    /// Errors already counted before this sink started (parse, prior modules).
    prior_errors: usize,
    capped: bool,
}

impl TypeSink {
    pub fn new(prior_errors: usize) -> Self {
        Self {
            inner: DiagnosticSink::new(),
            prior_errors,
            capped: prior_errors >= MAX_ERRORS,
        }
    }

    pub fn from_existing(existing: Vec<Diagnostic>) -> Self {
        let prior_errors = existing
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        let mut sink = Self::new(prior_errors);
        for d in existing {
            sink.inner_push(d);
        }
        sink
    }

    fn error_count(&self) -> usize {
        self.prior_errors
            + self
                .inner
                .as_slice()
                .iter()
                .filter(|d| d.severity == Severity::Error)
                .count()
    }

    fn inner_push(&mut self, d: Diagnostic) {
        // Bypass DiagnosticSink's E0191 cap; we emit E1500 ourselves.
        if d.severity == Severity::Warning {
            self.inner.push(d);
            return;
        }
        if self.capped {
            return;
        }
        if self.error_count() >= MAX_ERRORS {
            self.capped = true;
            self.inner.push(Diagnostic::error(
                codes::E1500_TOO_MANY_ERRORS,
                format!("too many errors, stopping after {MAX_ERRORS} (further errors omitted)"),
                d.span,
                Some("fix earlier errors first".into()),
                DiagnosticKind::Type,
            ));
            return;
        }
        self.inner.push(d);
    }

    pub fn push(&mut self, d: Diagnostic) {
        self.inner_push(d);
    }

    pub fn error(
        &mut self,
        code: impl Into<String>,
        message: impl Into<String>,
        span: Span,
        hint: Option<String>,
    ) {
        self.push(Diagnostic::error(
            code,
            message,
            span,
            hint,
            DiagnosticKind::Type,
        ));
    }

    pub fn error_with_secondary(
        &mut self,
        code: impl Into<String>,
        message: impl Into<String>,
        span: Span,
        hint: Option<String>,
        secondary_span: Span,
        secondary_msg: impl Into<String>,
    ) {
        self.push(
            Diagnostic::error(code, message, span, hint, DiagnosticKind::Type)
                .with_secondary(secondary_span, secondary_msg),
        );
    }

    pub fn warning(
        &mut self,
        code: impl Into<String>,
        message: impl Into<String>,
        span: Span,
        hint: Option<String>,
    ) {
        self.push(Diagnostic::warning(
            code,
            message,
            span,
            hint,
            DiagnosticKind::Type,
        ));
    }

    pub fn is_capped(&self) -> bool {
        self.capped
    }

    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        let mut diags = self.inner.into_diagnostics();
        // Deterministic order: module span, then code (spec §8).
        diags.sort_by(|a, b| {
            (a.span.start, a.span.end, a.code.as_str()).cmp(&(
                b.span.start,
                b.span.end,
                b.code.as_str(),
            ))
        });
        diags
    }

    pub fn as_slice(&self) -> &[Diagnostic] {
        self.inner.as_slice()
    }

    pub fn has_errors(&self) -> bool {
        self.as_slice()
            .iter()
            .any(|d| d.severity == Severity::Error)
    }
}
