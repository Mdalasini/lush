//! Structured diagnostics with codes, snippets, and hints (§11.6).

use crate::span::Span;
use ariadne::{Color, Label, Report, ReportKind, Source};
use std::fmt;
use std::io::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    Lexer,
    Parser,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    pub span: Span,
    pub severity: Severity,
    pub hint: Option<String>,
    pub kind: DiagnosticKind,
}

impl Diagnostic {
    pub fn error(
        code: impl Into<String>,
        message: impl Into<String>,
        span: Span,
        hint: Option<String>,
        kind: DiagnosticKind,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            span,
            severity: Severity::Error,
            hint,
            kind,
        }
    }

    /// Render with ariadne into a string (for golden snapshot tests).
    pub fn render(&self, filename: &str, source: &str) -> String {
        let kind = match self.severity {
            Severity::Error => ReportKind::Error,
            Severity::Warning => ReportKind::Warning,
        };
        let mut report = Report::build(kind, (filename, self.span.range()))
            .with_code(&self.code)
            .with_message(&self.message)
            .with_label(
                Label::new((filename, self.span.range()))
                    .with_message(&self.message)
                    .with_color(Color::Red),
            );
        if let Some(hint) = &self.hint {
            report = report.with_help(hint);
        }
        let mut buf = Vec::new();
        let _ = report
            .finish()
            .write((filename, Source::from(source)), &mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    }

    pub fn write<W: Write>(&self, filename: &str, source: &str, w: &mut W) -> std::io::Result<()> {
        w.write_all(self.render(filename, source).as_bytes())
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}[{}]: {}",
            severity_tag(self.severity),
            self.code,
            self.message
        )?;
        if let Some(hint) = &self.hint {
            write!(f, " (hint: {hint})")?;
        }
        Ok(())
    }
}

fn severity_tag(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}
