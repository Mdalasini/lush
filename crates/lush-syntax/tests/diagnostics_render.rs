//! Golden renderings of parse-error diagnostics via ariadne (§11.6).

use lush_syntax::parse_module;

#[test]
fn chained_comparison_renders_snippet() {
    let src = "pub fn bad(a, b, c) -> Bool {\n  a < b < c;\n}\n";
    let outcome = parse_module(src);
    let err = outcome
        .diagnostics
        .iter()
        .find(|d| d.code == "E0110")
        .expect("expected E0110");
    // colour=false for stable, pipe-friendly snapshots (§11.6 review).
    let plain = err.render("bad.lush", &outcome.source, false);
    insta::assert_snapshot!("render_chained_comparison", plain);
}

#[test]
fn missing_semicolon_renders_hint() {
    let src = "pub fn bad() -> Int {\n  let x = 1\n  x;\n}\n";
    let outcome = parse_module(src);
    assert!(!outcome.ok());
    let rendered: String = outcome
        .diagnostics
        .iter()
        .filter(|d| d.severity == lush_syntax::diagnostic::Severity::Error)
        .map(|d| d.render("bad.lush", &outcome.source, false))
        .collect::<Vec<_>>()
        .join("\n---\n");
    insta::assert_snapshot!("render_missing_semi", rendered);
}
