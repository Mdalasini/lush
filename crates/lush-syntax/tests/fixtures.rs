//! Fixture-driven parser, formatter, and diagnostic tests for build step 1.

use lush_syntax::ast::equiv;
use lush_syntax::{format_round_trip, format_source, parse_module};
use std::fs;
use std::path::{Path, PathBuf};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read_lush_files(dir: &Path) -> Vec<(String, String)> {
    let mut files = Vec::new();
    if !dir.exists() {
        return files;
    }
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "lush"))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let src = fs::read_to_string(&path).unwrap();
        files.push((name, src));
    }
    files
}

#[test]
fn positive_fixtures_parse() {
    let root = fixture_root().join("positive");
    for (name, src) in read_lush_files(&root) {
        let outcome = parse_module(&src);
        let errors: Vec<_> = outcome
            .diagnostics
            .iter()
            .filter(|d| d.severity == lush_syntax::diagnostic::Severity::Error)
            .map(|d| d.to_string())
            .collect();
        assert!(
            outcome.ok(),
            "positive fixture `{name}` failed to parse:\n{}",
            errors.join("\n")
        );
    }
}

#[test]
fn example_fixtures_parse() {
    let root = fixture_root().join("examples");
    for (name, src) in read_lush_files(&root) {
        let outcome = parse_module(&src);
        let errors: Vec<_> = outcome
            .diagnostics
            .iter()
            .filter(|d| d.severity == lush_syntax::diagnostic::Severity::Error)
            .map(|d| d.to_string())
            .collect();
        assert!(
            outcome.ok(),
            "example fixture `{name}` failed to parse:\n{}",
            errors.join("\n")
        );
    }
}

#[test]
fn negative_fixtures_reject() {
    let root = fixture_root().join("negative");
    for (name, src) in read_lush_files(&root) {
        let outcome = parse_module(&src);
        assert!(
            !outcome.ok(),
            "negative fixture `{name}` unexpectedly parsed successfully"
        );
    }
}

#[test]
fn negative_diagnostic_snapshots() {
    let root = fixture_root().join("negative");
    for (name, src) in read_lush_files(&root) {
        let outcome = parse_module(&src);
        let rendered: String = outcome
            .diagnostics
            .iter()
            .filter(|d| d.severity == lush_syntax::diagnostic::Severity::Error)
            .map(|d| {
                // Strip ANSI / unstable paths for stable snapshots: use Display form + code.
                format!("{}:{}: {}", d.code, d.span, d.message)
            })
            .collect::<Vec<_>>()
            .join("\n");
        insta::assert_snapshot!(format!("diag_{name}"), rendered);
    }
}

#[test]
fn formatter_idempotent_and_round_trip_positive() {
    let mut roots = vec![
        fixture_root().join("positive"),
        fixture_root().join("examples"),
    ];
    for root in roots.drain(..) {
        for (name, src) in read_lush_files(&root) {
            match format_round_trip(&src) {
                Ok(_) => {}
                Err(err) => panic!("formatter round-trip failed for `{name}`: {err}"),
            }
        }
    }
}

#[test]
fn formatter_output_snapshots() {
    let root = fixture_root().join("positive");
    for (name, src) in read_lush_files(&root) {
        let formatted = format_source(&src).expect("format positive fixture");
        insta::assert_snapshot!(format!("fmt_{name}"), formatted);
    }
}

#[test]
fn token_stream_snapshot_literals() {
    let src = fs::read_to_string(fixture_root().join("examples/literals.lush")).unwrap();
    let outcome = parse_module(&src);
    let tokens: Vec<String> = outcome
        .tokens
        .iter()
        .filter(|t| !matches!(t.kind, lush_syntax::token::TokenKind::Eof))
        .map(|t| {
            let trivia: Vec<_> = t
                .leading
                .iter()
                .filter(|tr| !matches!(tr.kind, lush_syntax::token::TriviaKind::Whitespace))
                .map(|tr| format!("{:?}:{}", tr.kind, tr.text))
                .collect();
            format!("{:?} @{} trivia={:?}", t.kind, t.span, trivia)
        })
        .collect();
    insta::assert_snapshot!("tokens_literals", tokens.join("\n"));
}

#[test]
fn ast_snapshot_record_update() {
    let src =
        fs::read_to_string(fixture_root().join("positive/conformance_record_update.lush")).unwrap();
    let outcome = parse_module(&src);
    assert!(outcome.ok());
    insta::assert_debug_snapshot!("ast_record_update", outcome.module);
}

#[test]
fn mixed_level_comparisons_parse() {
    let src = r#"
pub fn ok(a: Int, b: Int, c: Bool, d: Bool) -> Bool {
  a == b && c == d;
}
"#;
    let outcome = parse_module(src);
    assert!(outcome.ok(), "{:?}", outcome.diagnostics);
}

#[test]
fn domain_prefixed_import_parses() {
    let src = "import github.com/user/repo;\n";
    let outcome = parse_module(src);
    assert!(outcome.ok(), "{:?}", outcome.diagnostics);
    let module = outcome.module.unwrap();
    match &module.items[0] {
        lush_syntax::ast::ModuleItem::Import(imp) => {
            assert_eq!(imp.path.segments, vec!["github.com", "user", "repo"]);
        }
        other => panic!("expected import, got {other:?}"),
    }
}

#[test]
fn casing_warning_does_not_reject() {
    // Lower/underscore-start names may contain ASCII uppercase; warn, don't reject (§3).
    let src = "pub fn badName() -> Int { 1; }\n";
    let outcome = parse_module(src);
    assert!(outcome.ok(), "{:?}", outcome.diagnostics);
    assert!(outcome.diagnostics.iter().any(|d| d.code == "W0001"));
}

#[test]
fn formatter_preserves_comments() {
    let src = "//// mod\n/// doc\n// line\npub fn f() -> Int {\n  1;\n}\n";
    let formatted = format_source(src).unwrap();
    assert!(formatted.contains("//// mod"));
    assert!(formatted.contains("/// doc"));
    assert!(formatted.contains("// line"));
    let again = format_source(&formatted).unwrap();
    assert_eq!(formatted, again);
}

#[test]
fn round_trip_equivalence_helper() {
    let src = include_str!("fixtures/examples/fib.lush");
    let a = parse_module(src).module.unwrap();
    let formatted = format_source(src).unwrap();
    let b = parse_module(&formatted).module.unwrap();
    assert!(equiv::modules_eq(&a, &b));
}

#[test]
fn deep_nesting_does_not_abort() {
    let inner = format!("{}1{}", "(".repeat(300), ")".repeat(300));
    let src = format!("pub fn f() {{ {inner}; }}\n");
    let outcome = parse_module(&src);
    assert!(
        outcome
            .diagnostics
            .iter()
            .any(|d| d.code == lush_syntax::codes::E0190_TOO_DEEP),
        "expected E0190 for deep nesting, got: {:?}",
        outcome
            .diagnostics
            .iter()
            .map(|d| &d.code)
            .collect::<Vec<_>>()
    );
    // Must complete without aborting / panicking.
    assert!(!outcome.ok());
}
