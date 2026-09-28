//! Golden type-error / warning message fixtures (`spec.md` §15.3 step 2).

use lush_types::{typecheck_source, typecheck_source_with_warnings};
use std::fs;
use std::path::PathBuf;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn normalize(s: &str) -> String {
    // Drop brittle byte offsets from Display output.
    let re_bytes = regex_lite_replace_bytes(s);
    re_bytes
}

fn regex_lite_replace_bytes(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            // Collapse digit runs that look like span offsets in messages.
            while chars.peek().is_some_and(|p| p.is_ascii_digit()) {
                chars.next();
            }
            out.push('N');
        } else {
            out.push(c);
        }
    }
    out
}

fn render_errors(source: &str) -> String {
    match typecheck_source_with_warnings(source) {
        Ok(warnings) => {
            if warnings.is_empty() {
                "ok\n".into()
            } else {
                let mut lines: Vec<String> =
                    warnings.iter().map(|w| format!("warning: {w}")).collect();
                lines.sort();
                normalize(&lines.join("\n")) + "\n"
            }
        }
        Err(errors) => {
            let mut lines: Vec<String> = errors.iter().map(|e| format!("error: {e}")).collect();
            lines.sort();
            normalize(&lines.join("\n")) + "\n"
        }
    }
}

#[test]
fn golden_type_diagnostics() {
    let dir = fixture_dir();
    let mut files: Vec<_> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "lush"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "expected .lush fixtures in {}",
        dir.display()
    );

    for path in files {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let source = fs::read_to_string(&path).expect("read fixture");
        let rendered = render_errors(&source);
        // Ensure negative fixtures actually fail (or warn) unless named `ok_*`.
        if !name.starts_with("ok_") {
            assert!(
                rendered != "ok\n",
                "fixture {name} unexpectedly type-checked cleanly"
            );
        }
        insta::assert_snapshot!(name, rendered);
    }
}

#[test]
fn typecheck_source_still_errors_only() {
    let err = typecheck_source("pub fn main() { 1 + 1.0; }\n").unwrap_err();
    assert!(!err.is_empty());
}
