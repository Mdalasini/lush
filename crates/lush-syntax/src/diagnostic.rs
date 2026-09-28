//! Structured diagnostics with codes, snippets, and hints (§11.6).

use crate::span::Span;
use ariadne::{Color, Config, Label, Report, ReportKind, Source};
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

    /// Render with ariadne into a string.
    ///
    /// When `color` is false, ANSI colours are omitted (tests, pipes, files).
    /// Control characters in the source snippet other than tab are sanitised so
    /// a malicious file cannot inject terminal escapes through diagnostics.
    pub fn render(&self, filename: &str, source: &str, color: bool) -> String {
        let kind = match self.severity {
            Severity::Error => ReportKind::Error,
            Severity::Warning => ReportKind::Warning,
        };
        let label_color = match self.severity {
            Severity::Error => Color::Red,
            Severity::Warning => Color::Yellow,
        };
        let safe_source = sanitise_source_for_display(source);
        let mut report = Report::build(kind, (filename, self.span.range()))
            .with_config(Config::default().with_color(color))
            .with_code(&self.code)
            .with_message(&self.message)
            .with_label(
                Label::new((filename, self.span.range()))
                    .with_message(&self.message)
                    .with_color(label_color),
            );
        if let Some(hint) = &self.hint {
            report = report.with_help(hint);
        }
        let mut buf = Vec::new();
        let _ = report
            .finish()
            .write((filename, Source::from(safe_source.as_str())), &mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    }

    pub fn write<W: Write>(
        &self,
        filename: &str,
        source: &str,
        color: bool,
        w: &mut W,
    ) -> std::io::Result<()> {
        w.write_all(self.render(filename, source, color).as_bytes())
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

/// Escape a character for inclusion in a diagnostic message.
pub fn escape_for_message(ch: char) -> String {
    ch.escape_default().to_string()
}

/// Escape a short slice for inclusion in a diagnostic message.
pub fn escape_str_for_message(s: &str) -> String {
    s.escape_default().to_string()
}

/// Replace control characters other than tab so ariadne snippets cannot inject
/// terminal escapes. Newlines are kept so line numbering stays accurate.
fn sanitise_source_for_display(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for ch in source.chars() {
        match ch {
            '\n' | '\t' => out.push(ch),
            c if c.is_control() => {
                for esc in c.escape_default() {
                    out.push(esc);
                }
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::DiagnosticKind;

    #[test]
    fn escape_injects_no_raw_esc() {
        let msg = format!("unexpected character `{}`", escape_for_message('\u{1b}'));
        assert!(!msg.as_bytes().contains(&0x1b));
        assert!(msg.contains("\\u{1b}") || msg.contains("\\x1b"));
    }

    #[test]
    fn render_sanitises_source_control_chars() {
        let src = "let x = \u{1b};";
        let d = Diagnostic::error(
            "E0001",
            format!("unexpected character `{}`", escape_for_message('\u{1b}')),
            Span::new(8, 9),
            None,
            DiagnosticKind::Lexer,
        );
        let rendered = d.render("t.lush", src, false);
        assert!(
            !rendered.as_bytes().contains(&0x1b),
            "raw ESC leaked into render: {rendered:?}"
        );
    }
}
