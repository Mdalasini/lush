//! Structured diagnostics with codes, snippets, and hints (§11.6).

use crate::codes;
use crate::span::Span;
use ariadne::{Color, Config, IndexType, Label, Report, ReportKind, Source};
use std::fmt;
use std::io::Write;

/// Maximum error diagnostics retained by the lexer and parser.
pub const MAX_ERRORS: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    Lexer,
    Parser,
    /// Name resolution, inference, exhaustiveness, and related type-stage checks.
    Type,
    /// Core IR, bytecode verification, and compiler-generated artifacts.
    Bytecode,
}

/// A labelled span attached to a diagnostic (primary or secondary).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticLabel {
    pub span: Span,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    pub span: Span,
    pub severity: Severity,
    pub hint: Option<String>,
    pub kind: DiagnosticKind,
    /// Secondary labels (e.g. where an expected type originated). The primary
    /// label always uses [`Self::span`] and [`Self::message`].
    pub secondary: Vec<DiagnosticLabel>,
}

/// Collects diagnostics with a shared error cap for lexer and parser.
#[derive(Debug, Default)]
pub struct DiagnosticSink {
    diagnostics: Vec<Diagnostic>,
    capped: bool,
}

impl DiagnosticSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, d: Diagnostic) {
        if d.severity == Severity::Warning {
            self.diagnostics.push(d);
            return;
        }
        if self.capped {
            return;
        }
        let errors = self
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        if errors >= MAX_ERRORS {
            self.capped = true;
            self.diagnostics.push(Diagnostic {
                code: codes::E0191_TOO_MANY_ERRORS.into(),
                message: format!(
                    "too many errors, stopping after {MAX_ERRORS} (further errors omitted)"
                ),
                span: d.span,
                severity: Severity::Error,
                hint: Some("fix earlier errors first".into()),
                kind: d.kind,
                secondary: Vec::new(),
            });
            return;
        }
        self.diagnostics.push(d);
    }

    pub fn error(
        &mut self,
        code: impl Into<String>,
        message: impl Into<String>,
        span: Span,
        hint: Option<String>,
        kind: DiagnosticKind,
    ) {
        self.push(Diagnostic::error(code, message, span, hint, kind));
    }

    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    pub fn as_slice(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn is_capped(&self) -> bool {
        self.capped
    }
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
            secondary: Vec::new(),
        }
    }

    pub fn warning(
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
            severity: Severity::Warning,
            hint,
            kind,
            secondary: Vec::new(),
        }
    }

    pub fn with_secondary(mut self, span: Span, message: impl Into<String>) -> Self {
        self.secondary.push(DiagnosticLabel {
            span,
            message: message.into(),
        });
        self
    }

    /// Render with ariadne into a string.
    ///
    /// When `color` is false, ANSI colours are omitted (tests, pipes, files).
    /// Control characters other than tab/newline are replaced with `?` so byte
    /// offsets stay stable. Spans are interpreted as byte ranges.
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
            .with_config(
                Config::default()
                    .with_color(color)
                    .with_index_type(IndexType::Byte),
            )
            .with_code(&self.code)
            .with_message(&self.message)
            .with_label(
                Label::new((filename, self.span.range()))
                    .with_message(&self.message)
                    .with_color(label_color),
            );
        for sec in &self.secondary {
            report = report.with_label(
                Label::new((filename, sec.span.range()))
                    .with_message(&sec.message)
                    .with_color(Color::Blue),
            );
        }
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
/// Uses `escape_debug` so printable Unicode stays readable.
pub fn escape_for_message(ch: char) -> String {
    ch.escape_debug().to_string()
}

/// Escape a short slice for inclusion in a diagnostic message.
pub fn escape_str_for_message(s: &str) -> String {
    s.escape_debug().to_string()
}

/// Replace control characters other than tab/newline with `?` (same UTF-8
/// byte length) so ariadne snippets cannot inject terminal escapes and so
/// byte spans remain valid.
fn sanitise_source_for_display(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for ch in source.chars() {
        match ch {
            '\n' | '\t' => out.push(ch),
            c if c.is_control() => {
                for _ in 0..c.len_utf8() {
                    out.push('?');
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

    #[test]
    fn escape_injects_no_raw_esc() {
        let msg = format!("unexpected character `{}`", escape_for_message('\u{1b}'));
        assert!(!msg.as_bytes().contains(&0x1b));
        assert!(msg.contains("\\u{1b}") || msg.contains("\\x1b"));
    }

    #[test]
    fn escape_keeps_printable_unicode() {
        assert_eq!(escape_for_message('é'), "é");
    }

    #[test]
    fn render_sanitises_without_shifting_later_spans() {
        // ESC (1 byte) then the error at `!`
        let src = "let x = \u{1b}!;";
        let bang = src.find('!').unwrap();
        let d = Diagnostic::error(
            "E0001",
            "unexpected character `!`",
            Span::new(bang, bang + 1),
            None,
            DiagnosticKind::Lexer,
        );
        let rendered = d.render("t.lush", src, false);
        assert!(!rendered.as_bytes().contains(&0x1b));
        // The labelled line should still show `!` under the error.
        assert!(rendered.contains('!'), "label lost the bang: {rendered:?}");
    }

    #[test]
    fn render_labels_after_multibyte_char() {
        let src = "let é = !;";
        let bang = src.find('!').unwrap();
        assert!(bang > 4); // past the multibyte é
        let d = Diagnostic::error(
            "E0001",
            "unexpected character `!`",
            Span::new(bang, bang + 1),
            None,
            DiagnosticKind::Lexer,
        );
        let rendered = d.render("t.lush", src, false);
        assert!(
            rendered.contains('!'),
            "byte-indexed label missed bang after é: {rendered:?}"
        );
    }

    #[test]
    fn sink_caps_errors() {
        let mut sink = DiagnosticSink::new();
        for i in 0..250 {
            sink.error(
                "E0001",
                format!("err {i}"),
                Span::new(i, i + 1),
                None,
                DiagnosticKind::Lexer,
            );
        }
        let diags = sink.into_diagnostics();
        let errors = diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        assert_eq!(errors, MAX_ERRORS + 1);
        assert!(diags.iter().any(|d| d.code == codes::E0191_TOO_MANY_ERRORS));
    }
}
