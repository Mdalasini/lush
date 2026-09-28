//! Type-check documentation fixtures classified for parse checking (`spec.md` §11.4 / §15.4).

use lush_syntax::doc::{extract_fences, spec_inventory, wrap_snippet, FenceClass};
use lush_types::typecheck_source;
use std::path::PathBuf;

fn spec_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec.md")
}

#[test]
fn doc_modules_and_snippets_typecheck() {
    let markdown = std::fs::read_to_string(spec_path()).expect("read spec.md");
    let fences = extract_fences(&markdown);
    let inventory = spec_inventory();
    let mut checked = 0usize;

    for (fence, (_, class)) in fences.iter().zip(inventory.iter()) {
        let source = match class {
            FenceClass::Module => fence.body.clone(),
            FenceClass::WrappedSnippet => wrap_snippet(&fence.body),
            _ => continue,
        };
        typecheck_source(&source).unwrap_or_else(|e| {
            panic!(
                "fence {} (line {}) failed typecheck: {e:?}\n{source}",
                fence.index, fence.line
            )
        });
        checked += 1;
    }

    assert!(checked > 0);
}

#[test]
fn simple_module_typechecks() {
    typecheck_source(
        r#"
pub fn add(a: Int, b: Int) -> Int {
  a + b;
}
"#,
    )
    .expect("add should typecheck");
}

#[test]
fn rejects_int_plus_float() {
    let err = typecheck_source(
        r#"
pub fn main() -> Float {
  1 + 1.0;
}
"#,
    )
    .unwrap_err();
    assert!(!err.is_empty());
}
