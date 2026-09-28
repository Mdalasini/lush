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
    let rendered = err.render("bad.lush", &outcome.source, true);
    // Strip ANSI for a stable snapshot.
    let plain = strip_ansi(&rendered);
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
        .map(|d| strip_ansi(&d.render("bad.lush", &outcome.source, true)))
        .collect::<Vec<_>>()
        .join("\n---\n");
    insta::assert_snapshot!("render_missing_semi", rendered);
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(n) = chars.next() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}
