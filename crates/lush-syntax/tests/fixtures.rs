//! Fixture-driven parser, formatter, and diagnostic tests for build step 1.

use lush_syntax::ast::equiv;
use lush_syntax::{format_round_trip, format_source, parse_module};
use lush_test_support::{
    assert_fence_lines_preserved, expected_codes, extract_lush_fences, read_lush_files,
    read_spec_md, spec_fence_inventory, wrap_as_expr, wrap_mixed, SpecFenceKind,
};
use std::fs;
use std::path::{Path, PathBuf};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
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
        let expected = expected_codes(&src);
        assert!(
            !expected.is_empty(),
            "negative fixture `{name}` needs at least one `// expect: CODE` line"
        );
        let outcome = parse_module(&src);
        assert!(
            !outcome.ok(),
            "negative fixture `{name}` unexpectedly parsed successfully"
        );
        let codes: Vec<_> = outcome
            .diagnostics
            .iter()
            .filter(|d| d.severity == lush_syntax::diagnostic::Severity::Error)
            .map(|d| d.code.as_str())
            .collect();
        for code in &expected {
            assert!(
                codes.iter().any(|c| c == code),
                "negative fixture `{name}` expected error `{code}`, got {codes:?}"
            );
        }
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
                .map(|tr| {
                    let text = &outcome.source[tr.span.range()];
                    format!("{:?}:{}", tr.kind, text)
                })
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
    // Debug builds use more stack per recursive-descent frame than release;
    // run on a larger stack so the depth limit (not the OS stack) is what fires.
    let handle = std::thread::Builder::new()
        .name("deep-nesting".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
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
            assert!(!outcome.ok());
        })
        .expect("spawn deep-nesting thread");
    handle.join().expect("deep-nesting thread panicked");
}

#[test]
fn junk_input_caps_diagnostics() {
    // 4 MB of `$` used to emit millions of diagnostics; the shared sink caps at 101.
    let junk = "$ ".repeat(2_000_000);
    let outcome = parse_module(&junk);
    let errors = outcome
        .diagnostics
        .iter()
        .filter(|d| d.severity == lush_syntax::diagnostic::Severity::Error)
        .count();
    assert!(
        errors <= lush_syntax::diagnostic::MAX_ERRORS + 1,
        "expected capped errors, got {errors}"
    );
    assert!(outcome
        .diagnostics
        .iter()
        .any(|d| d.code == lush_syntax::codes::E0191_TOO_MANY_ERRORS));
}

#[test]
fn long_binary_chain_is_bounded() {
    let handle = std::thread::Builder::new()
        .name("long-chain".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let terms = "a + ".repeat(8_000);
            let src = format!("pub fn f(a) {{ {terms}a; }}\n");
            let outcome = parse_module(&src);
            assert!(
                outcome
                    .diagnostics
                    .iter()
                    .any(|d| d.code == lush_syntax::codes::E0190_TOO_DEEP),
                "expected E0190 for long chain, got: {:?}",
                outcome
                    .diagnostics
                    .iter()
                    .map(|d| &d.code)
                    .collect::<Vec<_>>()
            );
            // Formatting must not stack-overflow either: parse fails first.
            assert!(format_source(&src).is_err());
        })
        .expect("spawn long-chain thread");
    handle.join().expect("long-chain thread panicked");
}

#[test]
fn mixed_comparison_equality_rejected() {
    let src = "pub fn bad(a, b, c) -> Bool { a < b == c; }\n";
    let outcome = parse_module(src);
    assert!(!outcome.ok());
    let e0110 = outcome
        .diagnostics
        .iter()
        .filter(|d| d.code == lush_syntax::codes::E0110_CHAINED_CMP)
        .count();
    assert_eq!(e0110, 1, "expected exactly one E0110, got {e0110}");
}

#[test]
fn spec_md_lush_fences_parse() {
    let spec = read_spec_md(Path::new(env!("CARGO_MANIFEST_DIR")));
    let fences = extract_lush_fences(&spec);
    let inventory = spec_fence_inventory();
    assert_eq!(
        fences.len(),
        inventory.len(),
        "update spec_fence_inventory when adding ```lush fences to spec.md"
    );

    for (i, (fence, kind)) in fences.iter().zip(inventory.iter()).enumerate() {
        let src = match kind {
            SpecFenceKind::Module => fence.clone(),
            SpecFenceKind::WrapMixed => wrap_mixed(fence),
            SpecFenceKind::WrapExpr => wrap_as_expr(fence),
            SpecFenceKind::Pseudocode | SpecFenceKind::Placeholder => continue,
        };
        assert_fence_lines_preserved(fence, &src, i);
        let outcome = parse_module(&src);
        let errors: Vec<_> = outcome
            .diagnostics
            .iter()
            .filter(|d| d.severity == lush_syntax::diagnostic::Severity::Error)
            .map(|d| d.to_string())
            .collect();
        assert!(
            outcome.ok(),
            "spec.md lush fence #{i} failed to parse:\n{}\n--- source ---\n{src}",
            errors.join("\n")
        );
    }
}
