//! Type-stage diagnostic helpers with graph-wide error and warning capping.

use std::cell::Cell;
use std::rc::Rc;

use lush_syntax::diagnostic::{Diagnostic, DiagnosticKind, DiagnosticSink, Severity, MAX_ERRORS};
use lush_syntax::span::Span;

use crate::codes;
use crate::limits::MAX_WARNINGS;

/// Shared graph-wide error/warning budgets.
#[derive(Clone, Debug)]
pub struct DiagBudget {
    pub errors: Rc<Cell<usize>>,
    pub warnings: Rc<Cell<usize>>,
    pub errors_capped: Rc<Cell<bool>>,
    pub warnings_capped: Rc<Cell<bool>>,
}

impl DiagBudget {
    pub fn new(prior_errors: usize, prior_warnings: usize) -> Self {
        Self {
            errors: Rc::new(Cell::new(prior_errors)),
            warnings: Rc::new(Cell::new(prior_warnings)),
            errors_capped: Rc::new(Cell::new(prior_errors >= MAX_ERRORS)),
            warnings_capped: Rc::new(Cell::new(prior_warnings >= MAX_WARNINGS)),
        }
    }

    pub fn fresh() -> Self {
        Self::new(0, 0)
    }
}

/// Collects type-stage diagnostics, honouring a shared error/warning budget.
#[derive(Debug)]
pub struct TypeSink {
    inner: DiagnosticSink,
    budget: DiagBudget,
}

impl TypeSink {
    pub fn new(prior_errors: usize) -> Self {
        Self {
            inner: DiagnosticSink::new(),
            budget: DiagBudget::new(prior_errors, 0),
        }
    }

    pub fn from_existing(existing: Vec<Diagnostic>) -> Self {
        let prior_errors = existing
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        let prior_warnings = existing
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count();
        let mut sink = Self {
            inner: DiagnosticSink::new(),
            budget: DiagBudget::new(prior_errors, prior_warnings),
        };
        for d in existing {
            // Seed without re-charging the budget (already counted in prior_*).
            sink.inner_push_raw(d);
        }
        sink
    }

    pub fn from_existing_with_budget(existing: Vec<Diagnostic>, budget: DiagBudget) -> Self {
        let mut sink = Self {
            inner: DiagnosticSink::new(),
            budget,
        };
        for d in existing {
            // Module-local prior (e.g. def-limit) still goes through the shared budget.
            sink.inner_push(d);
        }
        sink
    }

    fn inner_push_raw(&mut self, d: Diagnostic) {
        self.inner.push(d);
    }

    fn inner_push(&mut self, d: Diagnostic) {
        match d.severity {
            Severity::Warning => {
                if self.budget.warnings_capped.get() {
                    return;
                }
                let n = self.budget.warnings.get();
                if n >= MAX_WARNINGS {
                    self.budget.warnings_capped.set(true);
                    self.inner.push(Diagnostic::warning(
                        codes::W1500_TOO_MANY_WARNINGS,
                        format!(
                            "too many warnings, stopping after {MAX_WARNINGS} (further warnings omitted)"
                        ),
                        d.span,
                        None,
                        DiagnosticKind::Type,
                    ));
                    self.budget.warnings.set(n + 1);
                    return;
                }
                self.budget.warnings.set(n + 1);
                self.inner.push(d);
            }
            Severity::Error => {
                if self.budget.errors_capped.get() {
                    return;
                }
                let n = self.budget.errors.get();
                if n >= MAX_ERRORS {
                    self.budget.errors_capped.set(true);
                    self.inner.push(Diagnostic::error(
                        codes::E1500_TOO_MANY_ERRORS,
                        format!(
                            "too many errors, stopping after {MAX_ERRORS} (further errors omitted)"
                        ),
                        d.span,
                        Some("fix earlier errors first".into()),
                        DiagnosticKind::Type,
                    ));
                    self.budget.errors.set(n + 1);
                    return;
                }
                self.budget.errors.set(n + 1);
                self.inner.push(d);
            }
        }
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
        self.budget.errors_capped.get()
    }

    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        let mut diags = self.inner.into_diagnostics();
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
