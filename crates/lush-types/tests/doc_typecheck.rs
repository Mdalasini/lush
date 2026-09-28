//! Type-check documentation fixtures classified for parse checking (`spec.md` §11.4 / §15.4).

use lush_syntax::ast::*;
use lush_syntax::doc::{extract_fences, spec_inventory, FenceClass};
use lush_syntax::Span;
use lush_types::{typecheck_module, typecheck_source, TypeError};
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

#[test]
fn rejects_cyclic_alias() {
    let err = typecheck_source(
        r#"
type A = A
pub fn main(x: A) { x; }
"#,
    )
    .unwrap_err();
    assert!(
        err.iter()
            .any(|e| matches!(e, TypeError::Other { message, .. } if message.contains("cyclic"))),
        "expected cyclic alias error, got {err:?}"
    );
}

#[test]
fn rejects_refutable_let() {
    let err = typecheck_source(
        r#"
pub fn main(x: Option(Int)) {
  let Some(y) = x;
  y;
}
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("let assert")
        )),
        "expected refutable let error, got {err:?}"
    );
}

#[test]
fn labelled_call_uses_param_names() {
    typecheck_source(
        r#"
pub fn pair(s: String, n: Int) -> Int { n; }
pub fn main() {
  pair(n: 1, s: "x");
}
"#,
    )
    .expect("param names are external labels");
}

#[test]
fn labelled_constructor_call_reorders() {
    typecheck_source(
        r#"
pub type User {
  User(name: String, age: Int)
}
pub fn main() -> User {
  User(age: 1, name: "n");
}
"#,
    )
    .expect("labelled constructor fields reorder");
}

#[test]
fn accepts_min_int_literal() {
    typecheck_source(
        r#"
pub fn main() -> Int {
  -9223372036854775808;
}
"#,
    )
    .expect("direct negation of 2^63 is MIN_INT");
}

#[test]
fn rejects_positive_min_int_magnitude() {
    let err = typecheck_source(
        r#"
pub fn main() -> Int {
  9223372036854775808;
}
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("Int range")
        )),
        "expected Int range error, got {err:?}"
    );
}

#[test]
fn rejects_closure_constant() {
    let err = typecheck_source(
        r#"
const callback = fn() { Nil; };
pub fn main() { Nil; }
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("constant")
        )),
        "expected constant expression error, got {err:?}"
    );
}

#[test]
fn case_pattern_shadows_outer_binding() {
    typecheck_source(
        r#"
pub fn main(x: String) -> Int {
  case Some(1) {
    Some(x) -> x;
    None -> 0;
  };
}
"#,
    )
    .expect("pattern x should shadow parameter x");
}

#[test]
fn rejects_case_pattern_count_mismatch() {
    // Parser already rejects this shape; exercise the typechecker path on a
    // hand-built AST where subjects and pattern arity disagree.
    let s = Span { start: 0, end: 1 };
    let mut module = Module {
        module_docs: vec![],
        imports: vec![],
        definitions: vec![Definition::Fn(FnDef {
            is_pub: true,
            name: "main".into(),
            params: vec![],
            return_type: None,
            body: Block {
                statements: vec![Statement::Expr(Expr {
                    kind: ExprKind::Case {
                        subjects: vec![
                            Expr {
                                kind: ExprKind::Int("1".into()),
                                span: s,
                            },
                            Expr {
                                kind: ExprKind::Int("2".into()),
                                span: s,
                            },
                        ],
                        clauses: vec![Clause {
                            patterns: vec![PatternRow {
                                patterns: vec![Pattern {
                                    kind: PatternKind::Var("a".into()),
                                    span: s,
                                }],
                                span: s,
                            }],
                            guard: None,
                            body: Expr {
                                kind: ExprKind::Ident("a".into()),
                                span: s,
                            },
                            span: s,
                        }],
                    },
                    span: s,
                })],
                span: s,
            },
            span: s,
        })],
        span: s,
    };
    let err = typecheck_module(&mut module).unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("pattern count")
        )),
        "expected pattern count error, got {err:?}"
    );
}

#[test]
fn rejects_duplicate_constructor() {
    let err = typecheck_source(
        r#"
pub type First { Value }
pub type Second { Value }
pub fn main() { Nil; }
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("duplicate constructor")
        )),
        "expected duplicate constructor error, got {err:?}"
    );
}

#[test]
fn rejects_pipe_duplicate_first_label() {
    let err = typecheck_source(
        r#"
pub fn f(a x: Int, b y: Int) -> Int { x + y; }
pub fn main() -> Int {
  1 |> f(a: 2);
}
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("duplicate argument label")
        )),
        "expected duplicate pipe label error, got {err:?}"
    );
}

#[test]
fn rejects_non_constant_ident_in_const() {
    let err = typecheck_source(
        r#"
pub fn f() -> Int { 1; }
const c = f;
pub fn main() { Nil; }
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("not a constant reference")
        )),
        "expected non-constant reference error, got {err:?}"
    );
}

#[test]
fn rejects_echo_in_const() {
    let err = typecheck_source(
        r#"
const c = echo 1;
pub fn main() { Nil; }
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("echo")
        )),
        "expected echo-in-constant error, got {err:?}"
    );
}

#[test]
fn accepts_constructor_pattern_with_spread() {
    typecheck_source(
        r#"
pub type User {
  User(name: String, age: Int)
}
pub fn main(u: User) -> String {
  case u {
    User(name: n, ..) -> n;
  };
}
"#,
    )
    .expect("spread constructor pattern should type-check");
}

#[test]
fn rejects_const_division_by_zero() {
    let err = typecheck_source(
        r#"
const x = 1 / 0;
pub fn main() { Nil; }
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("division")
        )),
        "expected constant division error, got {err:?}"
    );
}

#[test]
fn rejects_const_division_by_zero_constant() {
    let err = typecheck_source(
        r#"
const zero = 0;
const bad = 1 / zero;
pub fn main() { Nil; }
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("division")
        )),
        "expected cross-constant division error, got {err:?}"
    );
}

#[test]
fn rejects_cyclic_constants() {
    let err = typecheck_source(
        r#"
const a = b;
const b = a;
pub fn main() { Nil; }
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("reference cycle")
        )),
        "expected constant cycle error, got {err:?}"
    );
}

#[test]
fn rejects_unknown_constructor_label() {
    let err = typecheck_source(
        r#"
pub type User {
  User(name: String, age: Int)
}
pub fn main(u: User) -> Int {
  case u {
    User(typo: x, name: y) -> 1;
  };
}
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("unknown field label")
        )),
        "expected unknown field label error, got {err:?}"
    );
}

#[test]
fn rejects_out_of_range_int() {
    let err = typecheck_source(
        r#"
pub fn main() {
  999999999999999999999999999999;
}
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("Int range")
        )),
        "expected Int range error, got {err:?}"
    );
}

#[test]
fn rejects_function_equality() {
    let err = typecheck_source(
        r#"
pub fn main() {
  let f = fn(x) { x; };
  f == f;
}
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("Eq constraint")
        )),
        "expected Eq failure for functions, got {err:?}"
    );
}

#[test]
fn rejects_nested_function_equality() {
    let err = typecheck_source(
        r#"
pub fn main() {
  let xs = [fn(x) { x; }];
  xs == xs;
}
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("Eq constraint")
        )),
        "expected Eq failure for list of functions, got {err:?}"
    );
}

#[test]
fn accepts_structural_equality() {
    typecheck_source(
        r#"
pub type Pair(a) {
  Pair(a, a)
}
pub fn main() {
  1 == 1;
  "a" == "a";
  #(1, True) == #(1, True);
  [1, 2] == [1, 2];
  Pair(1, 1) == Pair(1, 1);
}
"#,
    )
    .expect("eligible structural values should support Eq");
}

#[test]
fn polymorphic_eq_helper_rejects_function_instantiation() {
    let err = typecheck_source(
        r#"
pub fn same(a, b) {
  a == b;
}
pub fn main() {
  let f = fn(x) { x; };
  same(f, f);
}
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("Eq constraint")
        )),
        "expected Eq failure via polymorphic helper, got {err:?}"
    );
}

#[test]
fn neg_constraint_accepts_int_and_float() {
    typecheck_source(
        r#"
pub fn main() {
  let a = -1;
  let b = -1.5;
  a;
  b;
}
"#,
    )
    .expect("Neg on Int and Float");
}

#[test]
fn neg_constraint_rejects_string() {
    let err = typecheck_source(
        r#"
pub fn main() {
  -"nope";
}
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("Neg constraint")
        )),
        "expected Neg failure for String, got {err:?}"
    );
}
