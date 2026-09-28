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

/// Leading `// expect: E0xxx` lines list required diagnostic codes.
fn expected_codes(src: &str) -> Vec<String> {
    src.lines()
        .take_while(|line| line.trim_start().starts_with("//"))
        .filter_map(|line| {
            let trimmed = line.trim_start().trim_start_matches("//").trim();
            trimmed
                .strip_prefix("expect:")
                .map(|rest| rest.trim().to_string())
        })
        .filter(|c| !c.is_empty())
        .collect()
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
    assert!(outcome
        .diagnostics
        .iter()
        .any(|d| d.code == lush_syntax::codes::E0110_CHAINED_CMP));
}

/// How a ` ```lush ` fence from `spec.md` is exercised by the syntax crate.
#[derive(Clone, Copy)]
enum SpecFenceKind {
    /// Complete module; parse unchanged.
    Module,
    /// Statement/expression fragment; wrap in a function body.
    WrapFn,
    /// Expression fragment; wrap as a single expression statement.
    WrapExpr,
    /// API pseudocode / signature-only illustrations (§11.4).
    Pseudocode,
    /// Contains illustrative placeholders (`...`) that are not source tokens.
    Placeholder,
}

fn spec_fence_inventory() -> &'static [SpecFenceKind] {
    use SpecFenceKind::*;
    &[
        Module,      // 0 types
        WrapFn,      // 1 bindings (const stays invalid inside fn — see note below)
        Placeholder, // 2 functions (`...;`)
        WrapExpr,    // 3 pipes
        WrapExpr,    // 4 case
        WrapFn,      // 5 use
        Module,      // 6 imports (+ trailing call — not a module item)
        Module,      // 7 Msg type
        WrapFn,      // 8 selector
        Pseudocode,  // 9 process API
        Pseudocode,  // 10 actor API
        Module,      // 11 actor example
        Pseudocode,  // 12 supervisor API
        WrapFn,      // 13 task await
        Pseudocode,  // 14 task API
        Module,      // 15 fib
        Module,      // 16 million processes
        Module,      // 17 supervised worker
    ]
}

fn extract_lush_fences(spec: &str) -> Vec<String> {
    let mut fences = Vec::new();
    let mut rest = spec;
    while let Some(start) = rest.find("```lush\n") {
        rest = &rest[start + "```lush\n".len()..];
        let Some(end) = rest.find("```") else {
            break;
        };
        fences.push(rest[..end].to_string());
        rest = &rest[end + 3..];
    }
    fences
}

fn wrap_as_fn(body: &str) -> String {
    // Drop module-level `const` lines when wrapping §5.1; they are covered by example fixtures.
    let body: String = body
        .lines()
        .filter(|line| !line.trim_start().starts_with("const "))
        .collect::<Vec<_>>()
        .join("\n");
    format!("pub fn __spec_snippet() {{\n{body}\n}}\n")
}

fn wrap_as_expr(expr: &str) -> String {
    let expr = expr.trim().trim_end_matches(';');
    format!("pub fn __spec_snippet() {{\n  {expr};\n}}\n")
}

#[test]
fn spec_md_lush_fences_parse() {
    let spec_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec.md");
    let spec = fs::read_to_string(&spec_path).expect("read spec.md");
    let fences = extract_lush_fences(&spec);
    let inventory = spec_fence_inventory();
    assert_eq!(
        fences.len(),
        inventory.len(),
        "update spec_fence_inventory when adding ```lush fences to spec.md"
    );

    for (i, (fence, kind)) in fences.iter().zip(inventory.iter()).enumerate() {
        let src = match kind {
            SpecFenceKind::Module => {
                // Fence 6 appends a call after imports; keep only import lines for module parse.
                if i == 6 {
                    fence
                        .lines()
                        .filter(|l| l.trim_start().starts_with("import "))
                        .collect::<Vec<_>>()
                        .join("\n")
                        + "\n"
                } else {
                    fence.clone()
                }
            }
            SpecFenceKind::WrapFn => wrap_as_fn(fence),
            SpecFenceKind::WrapExpr => wrap_as_expr(fence),
            SpecFenceKind::Pseudocode | SpecFenceKind::Placeholder => continue,
        };
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
