//! Positive/negative grammar conformance fixtures from `spec.md` §12.

use lush_syntax::parse_module;

fn wrap_expr(body: &str) -> String {
    format!("pub fn main() -> Nil {{\n  {body}\n}}\n")
}

fn wrap_block(body: &str) -> String {
    format!("pub fn main() -> Nil {{\n{body}\n}}\n")
}

fn assert_parses(label: &str, source: &str) {
    if let Err(errors) = parse_module(source) {
        panic!("{label}: expected parse success, got {errors:?}\n{source}");
    }
}

fn assert_rejects(label: &str, source: &str) {
    if parse_module(source).is_ok() {
        panic!("{label}: expected parse failure\n{source}");
    }
}

#[test]
fn multiple_subjects_accept() {
    assert_parses(
        "multiple subjects",
        &wrap_expr("case a, b { 1, 2 -> True; _, _ -> False; };"),
    );
}

#[test]
fn multiple_subjects_reject_arity() {
    assert_rejects(
        "arm with only one pattern",
        &wrap_expr("case a, b { 1 -> True; _, _ -> False; };"),
    );
}

#[test]
fn list_spread_accept() {
    assert_parses("list spread elems", &wrap_expr("let xs = [x, ..ys]; Nil;"));
    assert_parses("list spread only", &wrap_expr("let xs = [..ys]; Nil;"));
}

#[test]
fn list_spread_reject() {
    // Missing comma between element and spread.
    assert_rejects(
        "list spread without comma",
        &wrap_expr("let xs = [x ..ys]; Nil;"),
    );
}

#[test]
fn tuple_arity_accept_reject() {
    assert_parses("tuple arity 2", &wrap_expr("let t = #(a, b,); Nil;"));
    assert_rejects("empty tuple", &wrap_expr("let t = #(); Nil;"));
    assert_rejects("unary tuple", &wrap_expr("let t = #(a); Nil;"));
}

#[test]
fn local_function_accept_reject() {
    assert_parses(
        "local function",
        &wrap_block("  fn id(x) { x; }\n  id(1);\n"),
    );
    assert_rejects(
        "bodyless local signature",
        &wrap_block("  fn id(x);\n  id(1);\n"),
    );
}

#[test]
fn qualified_type_accept_reject() {
    assert_parses(
        "qualified type",
        "pub fn main() -> actor.Next(Int) {\n  todo;\n}\n",
    );
    assert_rejects(
        "slash-qualified type",
        "pub fn main() -> actor/Next(Int) {\n  todo;\n}\n",
    );
}

#[test]
fn comparison_associativity() {
    assert_parses(
        "parenthesized comparison equality",
        &wrap_expr("let x = (a < b) == True; Nil;"),
    );
    assert_rejects("chained comparison", &wrap_expr("let x = a < b < c; Nil;"));
}

#[test]
fn trailing_commas() {
    assert_parses("trailing comma call", &wrap_expr("f(a,);"));
    assert_parses(
        "trailing comma params",
        &wrap_block("  fn g(a,) { a; }\n  g(1);\n"),
    );
    assert_rejects(
        "comma instead of case-arm semicolon",
        &wrap_expr("case x { 1 -> True, _ -> False; };"),
    );
}

#[test]
fn record_update_accept() {
    assert_parses(
        "record update",
        r#"
pub type User {
  User(name: String, age: Int)
}
pub fn main() -> User {
  User(..user, age: 31);
}
"#,
    );
}

#[test]
fn semicolon_enforcement() {
    assert_rejects("missing expr semicolon", "pub fn main() -> Int {\n  1\n}\n");
    assert_rejects(
        "missing import semicolon",
        "import lush/list\npub fn main() -> Nil { Nil; }\n",
    );
    assert_rejects(
        "missing case arm semicolon",
        &wrap_expr("case x { 1 -> True _ -> False; };"),
    );
    assert_parses(
        "final case arm semicolon required and accepted",
        &wrap_expr("case x { 1 -> True; _ -> False; };"),
    );
}

#[test]
fn pipes_labels_captures() {
    assert_parses("pipe with capture", &wrap_expr("1 |> add(2, _);"));
    assert_parses(
        "labelled call",
        &wrap_expr(r#"replace(in: "a,b", each: ",", with: " ");"#),
    );
}
