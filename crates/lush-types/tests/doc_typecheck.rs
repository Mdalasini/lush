//! Type-check documentation fixtures classified for parse checking (`spec.md` §11.4 / §15.4).

use lush_syntax::doc::{extract_fences, spec_inventory, FenceClass};
use lush_types::{typecheck_source, TypeError};
use std::path::PathBuf;

fn spec_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec.md")
}

/// Complete `Module` fences must type-check. Wrapped snippets are parse-gated
/// only (`lush-syntax`); they often omit bindings by design.
#[test]
fn doc_modules_typecheck() {
    let markdown = std::fs::read_to_string(spec_path()).expect("read spec.md");
    let fences = extract_fences(&markdown);
    let inventory = spec_inventory();
    let mut checked = 0usize;

    for (fence, entry) in fences.iter().zip(inventory.iter()) {
        if !matches!(entry.class, FenceClass::Module) {
            continue;
        }
        let source = fence.body.clone();
        typecheck_source(&source).unwrap_or_else(|e| {
            panic!(
                "fence {} ({}, line {}) failed typecheck: {e:?}\n{source}",
                fence.index, entry.key, fence.line
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

#[test]
fn self_application_does_not_stack_overflow() {
    let err = typecheck_source(
        r#"
pub fn boom(x) {
  x(x);
}
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(e, TypeError::Mismatch { .. })),
        "expected occurs-check mismatch, got {err:?}"
    );
}

#[test]
fn unbound_name_is_reported() {
    let err = typecheck_source(
        r#"
pub fn main() {
  missing;
}
"#,
    )
    .unwrap_err();
    assert!(
        err.iter()
            .any(|e| matches!(e, TypeError::Unbound { name, .. } if name == "missing")),
        "expected Unbound(missing), got {err:?}"
    );
}

#[test]
fn identity_is_polymorphic() {
    typecheck_source(
        r#"
pub fn id(x) { x; }
pub fn main() {
  let a = id(1);
  let b = id("hi");
  a;
}
"#,
    )
    .expect("polymorphic id should typecheck at Int and String");
}
