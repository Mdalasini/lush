//! Type-check documentation fixtures classified for parse checking (`spec.md` §11.4 / §15.4).

use lush_syntax::ast::*;
use lush_syntax::doc::{extract_fences, spec_inventory, FenceClass};
use lush_syntax::Span;
use lush_types::{
    typecheck_module, typecheck_source, typecheck_source_with_warnings, TypeError, TypeWarning,
};
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
fn rejects_non_exhaustive_option() {
    let err = typecheck_source(
        r#"
pub fn main(x: Option(Int)) -> Int {
  case x {
    Some(n) -> n;
  };
}
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("non-exhaustive")
        )),
        "expected non-exhaustive error, got {err:?}"
    );
}

#[test]
fn rejects_non_exhaustive_adt() {
    let err = typecheck_source(
        r#"
pub type Shape {
  Circle(Float)
  Rectangle(Float, Float)
  Point
}
pub fn main(s: Shape) -> Int {
  case s {
    Circle(_) -> 1;
    Point -> 0;
  };
}
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("Rectangle")
        )),
        "expected missing Rectangle, got {err:?}"
    );
}

#[test]
fn int_case_requires_wildcard() {
    let err = typecheck_source(
        r#"
pub fn main(n: Int) -> Int {
  case n {
    0 -> 0;
    1 -> 1;
  };
}
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("non-exhaustive")
        )),
        "expected non-exhaustive Int case, got {err:?}"
    );
}

#[test]
fn guarded_arm_does_not_establish_exhaustiveness() {
    let err = typecheck_source(
        r#"
pub fn main(x: Option(Int)) -> Int {
  case x {
    Some(n) if n > 0 -> n;
    None -> 0;
  };
}
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("non-exhaustive")
        )),
        "guarded Some must not count as covering, got {err:?}"
    );
}

#[test]
fn warns_on_redundant_pattern() {
    let warns = typecheck_source_with_warnings(
        r#"
pub fn main(x: Option(Int)) -> Int {
  case x {
    Some(n) -> n;
    None -> 0;
    Some(_) -> 1;
  };
}
"#,
    )
    .expect("exhaustive with redundant arm should type-check");
    assert!(
        warns.iter().any(|w| matches!(
            w,
            TypeWarning::RedundantPattern { message, .. } if message.contains("unreachable")
        )),
        "expected redundant pattern warning, got {warns:?}"
    );
}

#[test]
fn accepts_exhaustive_bool() {
    typecheck_source(
        r#"
pub fn main(b: Bool) -> Int {
  case b {
    True -> 1;
    False -> 0;
  };
}
"#,
    )
    .expect("Bool True/False is exhaustive");
}

#[test]
fn rejects_partial_constructor_fields() {
    let err = typecheck_source(
        r#"
pub fn main(x: Option(Int)) -> Int {
  case x {
    Some(0) -> 0;
    None -> 1;
  };
}
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("non-exhaustive")
        )),
        "Some(0) must not cover all Some values, got {err:?}"
    );
}

#[test]
fn accepts_list_rest_only_pattern() {
    typecheck_source(
        r#"
pub fn main(xs: List(Int)) -> List(Int) {
  case xs {
    [..rest] -> rest;
  };
}
"#,
    )
    .expect("[..rest] covers every list");
}

#[test]
fn warns_wildcard_after_exhaustive_bool() {
    let warns = typecheck_source_with_warnings(
        r#"
pub fn main(b: Bool) -> Int {
  case b {
    True -> 1;
    False -> 0;
    _ -> 2;
  };
}
"#,
    )
    .expect("exhaustive bool with extra wildcard");
    assert!(
        warns
            .iter()
            .any(|w| matches!(w, TypeWarning::RedundantPattern { .. })),
        "expected unreachable wildcard warning, got {warns:?}"
    );
}

#[test]
fn warns_redundant_alternative_in_or_pattern() {
    let warns = typecheck_source_with_warnings(
        r#"
pub fn main(b: Bool) -> Int {
  case b {
    True -> 1;
    True | False -> 0;
  };
}
"#,
    )
    .expect("or-pattern with redundant True alternative");
    assert!(
        warns
            .iter()
            .any(|w| matches!(w, TypeWarning::RedundantPattern { .. })),
        "expected unreachable True alternative warning, got {warns:?}"
    );
}

#[test]
fn warns_identical_string_prefix() {
    let warns = typecheck_source_with_warnings(
        r#"
pub fn main(s: String) -> Int {
  case s {
    "api/" <> rest -> 1;
    "api/" <> other -> 2;
    _ -> 0;
  };
}
"#,
    )
    .expect("string prefix case");
    assert!(
        warns
            .iter()
            .any(|w| matches!(w, TypeWarning::RedundantPattern { .. })),
        "expected unreachable second string-prefix warning, got {warns:?}"
    );
}

#[test]
fn warns_list_covered_by_shorter_rest() {
    let warns = typecheck_source_with_warnings(
        r#"
pub fn main(xs: List(Int)) -> Int {
  case xs {
    [x, ..xs] -> 1;
    [x, y, ..ys] -> 2;
    _ -> 0;
  };
}
"#,
    )
    .expect("list rest coverage");
    assert!(
        warns
            .iter()
            .any(|w| matches!(w, TypeWarning::RedundantPattern { .. })),
        "expected unreachable longer list pattern warning, got {warns:?}"
    );
}

#[test]
fn labelled_field_reorder_is_not_redundant() {
    let warns = typecheck_source_with_warnings(
        r#"
pub type User {
  User(a: Int, b: Int)
}
pub fn main(u: User) -> Int {
  case u {
    User(a: 1, b: _) -> 1;
    User(b: 1, a: _) -> 2;
    _ -> 0;
  };
}
"#,
    )
    .expect("labelled reorder case");
    assert!(
        !warns
            .iter()
            .any(|w| matches!(w, TypeWarning::RedundantPattern { .. })),
        "reordered labels must not warn as unreachable, got {warns:?}"
    );
}

#[test]
fn spread_does_not_false_cover_different_field() {
    let warns = typecheck_source_with_warnings(
        r#"
pub type User {
  User(name: String, age: Int)
}
pub fn main(u: User) -> Int {
  case u {
    User(name: "a", ..) -> 1;
    User(name: "b", ..) -> 2;
    _ -> 0;
  };
}
"#,
    )
    .expect("spread field cases");
    assert!(
        !warns
            .iter()
            .any(|w| matches!(w, TypeWarning::RedundantPattern { .. })),
        "different spread field values must both be useful, got {warns:?}"
    );
}

#[test]
fn accepts_exhaustive_list_nil_cons() {
    typecheck_source(
        r#"
pub fn main(xs: List(Int)) -> Int {
  case xs {
    [] -> 0;
    [head, ..tail] -> head;
  };
}
"#,
    )
    .expect("[] and [head, ..tail] cover every list");
}

#[test]
fn accepts_exhaustive_tuple_bool_pairs() {
    typecheck_source(
        r#"
pub fn main(p: #(Bool, Bool)) -> Int {
  case p {
    #(True, True) -> 1;
    #(True, False) -> 2;
    #(False, True) -> 3;
    #(False, False) -> 4;
  };
}
"#,
    )
    .expect("all Bool pair combinations are exhaustive");
}

#[test]
fn accepts_exhaustive_multi_subject_bool() {
    typecheck_source(
        r#"
pub fn main(a: Bool, b: Bool) -> Int {
  case a, b {
    True, True -> 1;
    True, False -> 2;
    False, True -> 3;
    False, False -> 4;
  };
}
"#,
    )
    .expect("multi-subject Bool matrix is exhaustive");
}

#[test]
fn rejects_partial_nested_adt_fields() {
    let err = typecheck_source(
        r#"
pub type Inner {
  A
  B
}
pub type Outer {
  O(Inner)
}
pub fn main(o: Outer) -> Int {
  case o {
    O(A) -> 1;
  };
}
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("non-exhaustive")
        )),
        "O(A) must not cover O(B), got {err:?}"
    );
}

#[test]
fn rejects_wrong_tuple_arity_as_cover() {
    let err = typecheck_source(
        r#"
pub fn main(p: #(Bool, Bool)) -> Int {
  case p {
    #(_, _, _) -> 0;
  };
}
"#,
    )
    .unwrap_err();
    assert!(
        err.iter().any(|e| matches!(
            e,
            TypeError::Other { message, .. } if message.contains("non-exhaustive")
        )),
        "#(_, _, _) must not cover #(Bool, Bool), got {err:?}"
    );
}

#[test]
fn warns_list_fixed_after_rest() {
    let warns = typecheck_source_with_warnings(
        r#"
pub fn main(xs: List(Int)) -> Int {
  case xs {
    [head, ..tail] -> 1;
    [x, y] -> 2;
    _ -> 0;
  };
}
"#,
    )
    .expect("list rest then fixed");
    assert!(
        warns
            .iter()
            .any(|w| matches!(w, TypeWarning::RedundantPattern { .. })),
        "expected [x, y] unreachable after [head, ..tail], got {warns:?}"
    );
}

#[test]
fn warns_equivalent_labelled_reorder() {
    let warns = typecheck_source_with_warnings(
        r#"
pub type User {
  User(first: Bool, last: Bool)
}
pub fn main(u: User) -> Int {
  case u {
    User(first: True, last: _) -> 1;
    User(last: _, first: True) -> 2;
    _ -> 0;
  };
}
"#,
    )
    .expect("equivalent labelled reorder");
    assert!(
        warns
            .iter()
            .any(|w| matches!(w, TypeWarning::RedundantPattern { .. })),
        "expected equivalent labelled reorder unreachable, got {warns:?}"
    );
}
