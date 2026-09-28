//! Lush syntax crate: lexer, parser, AST, formatter, and diagnostics.
//!
//! Covers build step 1 from `spec.md` §15.3 — the full §12 grammar and the
//! lexical forms in §3.

pub mod ast;
pub mod codes;
pub mod diagnostic;
pub mod formatter;
pub mod lexer;
pub mod parser;
pub mod span;
pub mod token;

use ast::Module;
use diagnostic::Diagnostic;
use token::SpannedToken;

/// Fully analysed source: normalised text, tokens, AST, and diagnostics.
#[derive(Debug)]
pub struct ParseOutcome {
    pub source: String,
    pub tokens: Vec<SpannedToken>,
    pub module: Option<Module>,
    pub diagnostics: Vec<Diagnostic>,
}

impl ParseOutcome {
    pub fn ok(&self) -> bool {
        self.diagnostics
            .iter()
            .all(|d| d.severity != diagnostic::Severity::Error)
            && self.module.is_some()
    }
}

/// Lex and parse a Lush module.
pub fn parse_module(source: &str) -> ParseOutcome {
    let lexed = lexer::lex(source);
    let mut diagnostics = lexed.diagnostics;
    diagnostics.extend(lexed.casing_warnings);
    let parsed = parser::parse(&lexed.tokens);
    diagnostics.extend(parsed.diagnostics);
    ParseOutcome {
        source: lexed.source,
        tokens: lexed.tokens,
        module: parsed.module,
        diagnostics,
    }
}

/// Format a module. Returns an error diagnostic list when the input does not parse.
pub fn format_source(source: &str) -> Result<String, Vec<Diagnostic>> {
    let outcome = parse_module(source);
    if !outcome.ok() {
        return Err(outcome
            .diagnostics
            .into_iter()
            .filter(|d| d.severity == diagnostic::Severity::Error)
            .collect());
    }
    let module = outcome.module.expect("ok implies module");
    Ok(formatter::format_module(&module, &outcome.tokens))
}

/// Format and verify idempotence + round-trip equivalence (ignoring spans).
pub fn format_round_trip(source: &str) -> Result<String, String> {
    let formatted = format_source(source).map_err(|ds| {
        ds.into_iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let again = format_source(&formatted).map_err(|ds| {
        ds.into_iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    if formatted != again {
        return Err("formatter is not idempotent".into());
    }
    let a = parse_module(source);
    let b = parse_module(&formatted);
    let ma = a.module.as_ref().ok_or("original failed to parse")?;
    let mb = b.module.as_ref().ok_or("formatted failed to parse")?;
    if !ast::ignore_spans::modules_eq(ma, mb) {
        return Err("formatted output does not reparse to an equivalent AST".into());
    }
    Ok(formatted)
}
