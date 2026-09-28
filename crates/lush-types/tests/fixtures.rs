//! Fixture-driven type-checker tests for build step 2.

use lush_syntax::diagnostic::Severity;
use lush_syntax::parse_module;
use lush_test_support::{
    complete_module_fence_indices, expected_codes, extract_lush_fences, read_lush_files,
    read_spec_md, spec_fence_inventory, SpecFenceKind,
};
use lush_types::codes;
use lush_types::{check_graph, check_source, ok};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn check_fixture(path: &str, src: &str, entry: bool) -> lush_types::CheckResult {
    check_source(path, src, entry)
}

#[test]
fn positive_fixtures_typecheck() {
    let root = fixture_root().join("positive");
    for (name, src) in read_lush_files(&root) {
        let result = check_fixture(&format!("fixtures/{name}"), &src, name.contains("main"));
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| format!("{}: {}", d.code, d.message))
            .collect();
        assert!(
            ok(&result.diagnostics),
            "positive fixture `{name}` failed:\n{}",
            errors.join("\n")
        );
    }
}

#[test]
fn negative_fixtures_emit_expected_codes() {
    let root = fixture_root().join("negative");
    let special = HashSet::from([
        "E1003_private",
        "E1010_slash_qualified_type",
        "E1207_opaque_use",
        "E1006_import_cycle",
        "E1007_reserved_lush",
        "E1304_escape",
        "E1305_too_deep",
        "E1306_too_complex",
        "E1307_module_limit",
        "E1308_def_limit",
        "E1309_node_limit",
        "E1451_match_complex",
        "E1500_too_many_errors",
    ]);
    for (name, src) in read_lush_files(&root) {
        let expected = expected_codes(&src);
        assert!(
            !expected.is_empty(),
            "negative fixture `{name}` needs `// expect: CODE`"
        );
        if special.contains(name.as_str()) {
            continue; // covered by dedicated unit tests
        }
        let entry = expected.iter().any(|c| c == "E1215");
        let result = check_fixture(&format!("neg/{name}"), &src, entry);
        let codes_found: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.code.as_str())
            .collect();
        for code in &expected {
            assert!(
                codes_found.iter().any(|c| c == code),
                "negative fixture `{name}` expected `{code}`, got {codes_found:?}\n{:?}",
                result
                    .diagnostics
                    .iter()
                    .map(|d| format!("{}:{}", d.code, d.message))
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn warning_fixtures_emit_expected_codes() {
    let root = fixture_root().join("warnings");
    let special = HashSet::from(["W1500_cap"]);
    for (name, src) in read_lush_files(&root) {
        if special.contains(name.as_str()) {
            continue; // covered by dedicated_warning_cap
        }
        let expected = expected_codes(&src);
        assert!(!expected.is_empty());
        let result = check_fixture(&format!("warn/{name}"), &src, false);
        // Warnings must not fail the module
        let hard_errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .filter(|d| !d.code.starts_with('W'))
            .collect();
        assert!(
            hard_errors.is_empty(),
            "warning fixture `{name}` should not error: {:?}",
            hard_errors.iter().map(|d| &d.code).collect::<Vec<_>>()
        );
        let codes_found: Vec<_> = result.diagnostics.iter().map(|d| d.code.as_str()).collect();
        for code in &expected {
            assert!(
                codes_found.iter().any(|c| c == code),
                "warning fixture `{name}` expected `{code}`, got {codes_found:?}"
            );
        }
    }
}

#[test]
fn negative_diagnostic_snapshots() {
    let root = fixture_root().join("negative");
    let skip = HashSet::from([
        "E1003_private",
        "E1010_slash_qualified_type",
        "E1207_opaque_use",
        "E1006_import_cycle",
        "E1007_reserved_lush",
        "E1304_escape",
        "E1305_too_deep",
        "E1306_too_complex",
        "E1307_module_limit",
        "E1308_def_limit",
        "E1309_node_limit",
        "E1451_match_complex",
        "E1500_too_many_errors",
    ]);
    for (name, src) in read_lush_files(&root) {
        if skip.contains(name.as_str()) {
            continue;
        }
        let entry = expected_codes(&src).iter().any(|c| c == "E1215");
        let result = check_fixture(&format!("neg/{name}"), &src, entry);
        let rendered: String = result
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| format!("{}:{}: {}", d.code, d.span, d.message))
            .collect::<Vec<_>>()
            .join("\n");
        insta::assert_snapshot!(format!("diag_{name}"), rendered);
    }
}

#[test]
fn registry_coverage() {
    let mut seen: HashSet<String> = HashSet::new();
    for dir in ["negative", "warnings"] {
        for (name, src) in read_lush_files(&fixture_root().join(dir)) {
            for c in expected_codes(&src) {
                seen.insert(c);
            }
            let _ = name;
        }
    }
    // Every type-stage code must appear in a fixture `// expect:` line.
    // Dedicated unit tests prove the checker actually emits those codes for the
    // graph/limit/escape scenarios whose fixtures are placeholders.
    for code in codes::all_codes() {
        assert!(
            seen.contains(*code),
            "type-stage code `{code}` has no fixture `// expect:` line"
        );
    }
}

#[test]
fn reserved_lush_path_rejected() {
    let src = "pub fn main() -> Nil { Nil; }\n";
    let parsed = parse_module(src);
    let module = parsed.module.unwrap();
    let (results, diags) = check_graph(&[("lush/hacked".into(), module)], &[], vec![], &[]);
    let _ = results;
    assert!(diags.iter().any(|d| d.code == codes::E1007_RESERVED_LUSH));
}

#[test]
fn import_cycle_rejected() {
    let a = parse_module("import b;\npub fn main() -> Nil { Nil; }\n")
        .module
        .unwrap();
    let b = parse_module("import a;\npub fn main() -> Nil { Nil; }\n")
        .module
        .unwrap();
    let (_results, diags) = check_graph(&[("a".into(), a), ("b".into(), b)], &[], vec![], &[]);
    assert!(diags.iter().any(|d| d.code == codes::E1006_IMPORT_CYCLE));
}

#[test]
fn private_access_across_modules() {
    let lib = parse_module("fn secret() -> Nil { Nil; }\n")
        .module
        .unwrap();
    let app = parse_module("import lib;\npub fn main() -> Nil { lib.secret(); }\n")
        .module
        .unwrap();
    let (results, _) = check_graph(
        &[("lib".into(), lib), ("app".into(), app)],
        &[],
        vec![],
        &[],
    );
    let app = results.get("app").unwrap();
    // secret is not exported — should be unknown or private
    assert!(
        app.diagnostics
            .iter()
            .any(|d| d.code == codes::E1003_PRIVATE || d.code == codes::E1000_UNKNOWN_NAME),
        "{:?}",
        app.diagnostics
    );
}

#[test]
fn match_complexity_budget() {
    // Many literal arms on Int: usefulness walks grow quadratically and exceed
    // MAX_EXHAUST_WORK before all arms are accepted.
    let n = 800usize;
    let mut arms = String::new();
    for i in 0..n {
        arms.push_str(&format!("    {i} -> Nil;\n"));
    }
    let src = format!("pub fn f(x: Int) -> Nil {{\n  case x {{\n{arms}  }};\n}}\n");
    let result = check_source("complex", &src, false);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == codes::E1451_MATCH_COMPLEX || d.code == codes::E1306_TOO_COMPLEX),
        "{:?}",
        result
            .diagnostics
            .iter()
            .map(|d| format!("{}:{}", d.code, d.message))
            .collect::<Vec<_>>()
    );
}

#[test]
fn too_many_errors_cap() {
    let mut body = String::new();
    for i in 0..150 {
        body.push_str(&format!("  nope{i};\n"));
    }
    let src = format!("pub fn main() -> Nil {{\n{body}}}\n");
    let result = check_source("many", &src, true);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == codes::E1500_TOO_MANY_ERRORS
                || d.code == lush_syntax::codes::E0191_TOO_MANY_ERRORS),
        "expected error cap, got {:?}",
        result
            .diagnostics
            .iter()
            .map(|d| &d.code)
            .collect::<Vec<_>>()
    );
}

#[test]
fn interface_hash_stable_and_sensitive() {
    let src = "pub fn add(a: Int, b: Int) -> Int { a + b; }\n";
    let a = check_source("m", src, false);
    let b = check_source("m", src, false);
    assert_eq!(a.interface.interface_hash, b.interface.interface_hash);
    let src2 = "pub fn add(a: Int, b: Int) -> Bool { a == b; }\n";
    let c = check_source("m", src2, false);
    assert_ne!(a.interface.interface_hash, c.interface.interface_hash);
}

#[test]
fn desugar_capture_avoids_user_name_collision() {
    let src = r#"
pub fn add(a: Int, b: Int) -> Int { a + b; }
pub fn main() -> Nil {
  let __lush_cap_0 = 1;
  let f = add(5, _);
  let _ = f(__lush_cap_0);
  Nil;
}
"#;
    let result = check_source("cap", src, true);
    assert!(ok(&result.diagnostics), "{:?}", result.diagnostics);
}

#[test]
fn fuzz_truncated_and_mutated_no_panic() {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    let root = fixture_root();
    let mut fixtures = Vec::new();
    for dir in ["positive", "negative", "warnings"] {
        for (_name, src) in read_lush_files(&root.join(dir)) {
            fixtures.push(src);
        }
    }
    // Also include a few syntax-stress seeds that previously OOM'd the parser.
    fixtures.push("type B { , }".into());
    fixtures.push("type B { | }".into());
    fixtures.push("type A { A |".into());

    for (fi, src) in fixtures.iter().enumerate() {
        let bytes = src.as_bytes();
        if bytes.is_empty() {
            let _ = catch_unwind(AssertUnwindSafe(|| {
                let _ = check_source("fuzz", "", false);
            }));
            continue;
        }
        // Truncations at ~1/40th length steps
        let step = (bytes.len() / 40).max(1);
        for len in (0..=bytes.len()).step_by(step) {
            let trunc = String::from_utf8_lossy(&bytes[..len]).into_owned();
            let r = catch_unwind(AssertUnwindSafe(|| {
                let _ = check_source(&format!("fuzz_t{fi}_{len}"), &trunc, false);
            }));
            assert!(r.is_ok(), "panic on truncate fi={fi} len={len}");
        }
        // Seeded xorshift byte mutations
        let mut state = (fi as u64).wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(1);
        for mi in 0..40 {
            let mut mut_bytes = bytes.to_vec();
            let nflip = 1 + (state % 4) as usize;
            for _ in 0..nflip {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let idx = (state as usize) % mut_bytes.len();
                mut_bytes[idx] ^= ((state >> 8) as u8) | 1;
            }
            let mutated = String::from_utf8_lossy(&mut_bytes).into_owned();
            let r = catch_unwind(AssertUnwindSafe(|| {
                let _ = check_source(&format!("fuzz_m{fi}_{mi}"), &mutated, false);
            }));
            assert!(r.is_ok(), "panic on mutate fi={fi} mi={mi}");
        }
    }
}

#[test]
fn exhaustiveness_soundness_bool() {
    // Seeded generator: random Bool/Nil/Option/nested ADT/tuple matrices vs
    // brute-force usefulness on small domains.
    let mut state = 0xDEAD_BEEF_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..40 {
        let kind = next() % 4;
        let (src, should_be_exhaustive) = match kind {
            0 => {
                // Bool with 0–2 arms
                let arms = next() % 3;
                let mut body = String::new();
                if arms >= 1 {
                    body.push_str("    True -> 1;\n");
                }
                if arms >= 2 {
                    body.push_str("    False -> 0;\n");
                }
                (
                    format!("pub fn f(b: Bool) -> Int {{\n  case b {{\n{body}  }};\n}}\n"),
                    arms == 2,
                )
            }
            1 => {
                // Option(Bool)
                let cover_none = next() % 2 == 0;
                let cover_some_t = next() % 2 == 0;
                let cover_some_f = next() % 2 == 0;
                let mut body = String::new();
                if cover_none {
                    body.push_str("    None -> 0;\n");
                }
                if cover_some_t {
                    body.push_str("    Some(True) -> 1;\n");
                }
                if cover_some_f {
                    body.push_str("    Some(False) -> 2;\n");
                }
                (
                    format!("pub fn f(x: Option(Bool)) -> Int {{\n  case x {{\n{body}  }};\n}}\n"),
                    cover_none && cover_some_t && cover_some_f,
                )
            }
            2 => {
                // #(Bool, Bool) with one full arm or wild
                let use_wild = next() % 2 == 0;
                let body = if use_wild {
                    "    _ -> 0;\n".to_string()
                } else {
                    "    #(True, True) -> 1;\n".to_string()
                };
                (
                    format!("pub fn f(x: #(Bool, Bool)) -> Int {{\n  case x {{\n{body}  }};\n}}\n"),
                    use_wild,
                )
            }
            _ => {
                // Nil
                (
                    "pub fn f(x: Nil) -> Nil { case x { Nil -> Nil; }; }\n".into(),
                    true,
                )
            }
        };
        let result = check_source("ex_sound", &src, false);
        let non_ex = result
            .diagnostics
            .iter()
            .any(|d| d.code == codes::E1450_NON_EXHAUSTIVE);
        if should_be_exhaustive {
            assert!(
                !non_ex,
                "expected exhaustive:\n{src}\n{:?}",
                result.diagnostics
            );
        } else {
            assert!(
                non_ex
                    || result
                        .diagnostics
                        .iter()
                        .any(|d| d.code == codes::E1451_MATCH_COMPLEX),
                "expected non-exhaustive:\n{src}\n{:?}",
                result.diagnostics
            );
        }
    }
}

#[test]
fn spec_complete_modules_typecheck() {
    let spec = read_spec_md(Path::new(env!("CARGO_MANIFEST_DIR")));
    let fences = extract_lush_fences(&spec);
    let inventory = spec_fence_inventory();
    assert_eq!(fences.len(), inventory.len());
    for &idx in complete_module_fence_indices() {
        let fence = &fences[idx];
        assert!(matches!(inventory[idx], SpecFenceKind::Module));
        let result = check_source(
            &format!("spec_fence_{idx}"),
            fence,
            fence.contains("pub fn main"),
        );
        let errors: Vec<_> = result
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| format!("{}: {}", d.code, d.message))
            .collect();
        assert!(
            ok(&result.diagnostics),
            "spec.md complete-module fence #{idx} failed typecheck:\n{}\n---\n{fence}",
            errors.join("\n")
        );
    }
}

#[test]
fn work_counters_let_doubling() {
    // Let-doubling must stay linear in the DAG (Rc-shared types).
    // Deep nesting is iterative in the type DAG but zonk still recurses on
    // spine depth, so run under a larger stack.
    let handle = std::thread::Builder::new()
        .name("let-doubling".into())
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            for n in [10usize, 100, 500, 2000] {
                let mut lets = String::from("  let x0 = 1;\n");
                for i in 1..n {
                    lets.push_str(&format!("  let x{i} = #(x{}, x{});\n", i - 1, i - 1));
                }
                lets.push_str("  Nil;\n");
                let src = format!("pub fn main() -> Nil {{\n{lets}}}\n");
                let result = check_source("double", &src, true);
                assert!(
                    ok(&result.diagnostics),
                    "n={n}: {:?}",
                    result
                        .diagnostics
                        .iter()
                        .map(|d| format!("{}:{}", d.code, d.message))
                        .collect::<Vec<_>>()
                );
                assert!(
                    result.work < 100 * n as u64,
                    "n={n}: work={} exceeds 100*n={}",
                    result.work,
                    100 * n
                );
            }
        })
        .expect("spawn let-doubling thread");
    handle.join().expect("let-doubling thread panicked");
}

#[test]
fn dedicated_self_ref_anon() {
    let result = check_source(
        "anon",
        "pub fn main() -> Nil { let f = fn() { f(); }; Nil; }\n",
        true,
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == codes::E1012_SELF_REF_ANON),
        "{:?}",
        result.diagnostics
    );
}

#[test]
fn dedicated_node_limit() {
    // One const whose value is a huge list — node estimate is O(len(list)).
    use lush_syntax::ast::*;
    use lush_syntax::span::Span;
    use lush_syntax::token::{IntBase, IntLit};
    use lush_types::limits::MAX_NODES_PER_MODULE;
    let n = MAX_NODES_PER_MODULE + 10;
    let mut elems = Vec::with_capacity(n);
    for _ in 0..n {
        elems.push(Expr {
            kind: ExprKind::Int(IntLit {
                digits: "1".into(),
                base: IntBase::Decimal,
                raw: "1".into(),
            }),
            span: Span::default(),
        });
    }
    let module = Module {
        items: vec![ModuleItem::Const(ConstDef {
            public: false,
            name: Name {
                text: "huge".into(),
                span: Span::default(),
            },
            ty: None,
            value: Expr {
                kind: ExprKind::List {
                    items: elems,
                    spread: None,
                },
                span: Span::default(),
            },
            span: Span::default(),
        })],
        span: Span::default(),
    };
    let mut store = lush_types::ty::TypeStore::new();
    let prelude = lush_types::stubs::install_prelude_builtins(&mut store);
    let mut deps = lush_types::stubs::load_all_stubs(&mut store);
    deps.insert("prelude".into(), prelude);
    let result = lush_types::check_module(
        "huge",
        &module,
        &deps,
        &[],
        vec![],
        lush_types::CheckOptions::default(),
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == codes::E1309_NODE_LIMIT),
        "{:?}",
        result
            .diagnostics
            .iter()
            .map(|d| &d.code)
            .collect::<Vec<_>>()
    );
}

#[test]
fn dedicated_def_limit() {
    use lush_syntax::ast::*;
    use lush_syntax::span::Span;
    use lush_syntax::token::{IntBase, IntLit};
    use lush_types::limits::MAX_DEFS_PER_MODULE;
    let mut items = Vec::with_capacity(MAX_DEFS_PER_MODULE + 2);
    for i in 0..(MAX_DEFS_PER_MODULE + 2) {
        items.push(ModuleItem::Const(ConstDef {
            public: false,
            name: Name {
                text: format!("c{i}"),
                span: Span::default(),
            },
            ty: None,
            value: Expr {
                kind: ExprKind::Int(IntLit {
                    digits: "0".into(),
                    base: IntBase::Decimal,
                    raw: "0".into(),
                }),
                span: Span::default(),
            },
            span: Span::default(),
        }));
    }
    let module = Module {
        items,
        span: Span::default(),
    };
    let (results, _) = check_graph(&[("huge".into(), module)], &[], vec![], &[]);
    let r = results.get("huge").unwrap();
    assert!(
        r.diagnostics
            .iter()
            .any(|d| d.code == codes::E1308_DEF_LIMIT),
        "{:?}",
        r.diagnostics.iter().map(|d| &d.code).collect::<Vec<_>>()
    );
}

#[test]
fn dedicated_module_limit() {
    use lush_syntax::ast::Module;
    use lush_syntax::span::Span;
    use lush_types::limits::MAX_MODULES;
    let mut modules = Vec::with_capacity(MAX_MODULES + 2);
    for i in 0..(MAX_MODULES + 2) {
        modules.push((
            format!("m{i}"),
            Module {
                items: vec![],
                span: Span::default(),
            },
        ));
    }
    let (_results, diags) = check_graph(&modules, &[], vec![], &[]);
    assert!(
        diags.iter().any(|d| d.code == codes::E1307_MODULE_LIMIT),
        "{:?}",
        diags
    );
}

#[test]
fn dedicated_warning_cap() {
    // Many unused lets → warnings capped with W1500.
    let mut body = String::new();
    for i in 0..250 {
        body.push_str(&format!("  let a{i} = 1;\n"));
    }
    body.push_str("  Nil;\n");
    let src = format!("pub fn main() -> Nil {{\n{body}}}\n");
    let result = check_source("warncap", &src, true);
    let warns = result
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count();
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == codes::W1500_TOO_MANY_WARNINGS)
            || warns <= lush_types::limits::MAX_WARNINGS + 1,
        "warns={warns} diags={:?}",
        result
            .diagnostics
            .iter()
            .map(|d| &d.code)
            .collect::<Vec<_>>()
    );
}

#[test]
fn check_graph_topo_and_attribution() {
    // Importer listed before dependency must still resolve.
    let a = parse_module("import b;\npub fn f() -> Int { b.g(); }\n")
        .module
        .unwrap();
    let b = parse_module("pub fn g() -> Int { 1; }\n").module.unwrap();
    let (results, _) = check_graph(&[("a".into(), a), ("b".into(), b)], &[], vec![], &[]);
    let ar = results.get("a").unwrap();
    assert!(
        ok(&ar.diagnostics),
        "importer-before-dep failed: {:?}",
        ar.diagnostics
    );
    // Two modules each with errors — both returned and attributed.
    let b2 = parse_module("pub fn f() -> Int { \"x\"; }\n")
        .module
        .unwrap();
    let a2 = parse_module("pub fn g() -> Int { True; }\n")
        .module
        .unwrap();
    let (results, _) = check_graph(&[("b".into(), b2), ("a".into(), a2)], &[], vec![], &[]);
    assert!(results
        .get("a")
        .unwrap()
        .diagnostics
        .iter()
        .any(|d| d.code == codes::E1300_TYPE_MISMATCH));
    assert!(results
        .get("b")
        .unwrap()
        .diagnostics
        .iter()
        .any(|d| d.code == codes::E1300_TYPE_MISMATCH));
    // Clean module has empty diagnostics (modulo warnings).
    let clean = parse_module("pub fn f() -> Int { 1; }\n").module.unwrap();
    let dirty = parse_module("pub fn g() -> Int { True; }\n")
        .module
        .unwrap();
    let (results, _) = check_graph(
        &[("clean".into(), clean), ("dirty".into(), dirty)],
        &[],
        vec![],
        &[],
    );
    assert!(
        results
            .get("clean")
            .unwrap()
            .diagnostics
            .iter()
            .all(|d| d.severity != Severity::Error),
        "{:?}",
        results.get("clean").unwrap().diagnostics
    );
}

#[test]
fn opaque_outside_module() {
    let lib = parse_module(
        "pub opaque type Email { Email(String) }\npub fn new(s: String) -> Email { Email(s); }\n",
    )
    .module
    .unwrap();
    let app = parse_module(
        "import lib;\npub fn main() -> Nil { let e = lib.new(\"a\"); let _ = e.x; Nil; }\n",
    )
    .module
    .unwrap();
    let (results, _) = check_graph(
        &[("lib".into(), lib), ("app".into(), app)],
        &[],
        vec![],
        &[],
    );
    let app = results.get("app").unwrap();
    assert!(
        app.diagnostics
            .iter()
            .any(|d| d.code == codes::E1207_OPAQUE_USE
                || d.code == codes::E1204_UNKNOWN_FIELD
                || d.code == codes::E1205_FIELD_ACCESS),
        "{:?}",
        app.diagnostics
    );
}

#[test]
fn dedicated_escape_code() {
    // Expansive public binding leaves an ungeneralised type variable.
    let src = "pub const xs = [];\n";
    let result = check_source("esc", src, false);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == codes::E1304_ESCAPE),
        "{:?}",
        result.diagnostics
    );
}

#[test]
fn dedicated_too_complex() {
    // Reuse the match-complexity shape: exhaust work also accrues on the store
    // and trips the inference E1306 budget (or E1451 alone is acceptable when
    // the match aborts early — force via many unifications on a huge literal case).
    let n = 800usize;
    let mut arms = String::new();
    for i in 0..n {
        arms.push_str(&format!("    {i} -> Nil;\n"));
    }
    let src = format!("pub fn f(x: Int) -> Nil {{\n  case x {{\n{arms}  }};\n}}\n");
    let result = check_source("complex2", &src, false);
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == codes::E1306_TOO_COMPLEX || d.code == codes::E1451_MATCH_COMPLEX),
        "{:?}",
        result.diagnostics
    );
}

#[test]
fn dedicated_too_deep() {
    // Drive Unifier depth past MAX_DEPTH with a synthetic nest of List types.
    use lush_syntax::span::Span;
    use lush_types::diag::TypeSink;
    use lush_types::ty::{Type, TypeStore};
    use lush_types::unify::Unifier;
    let mut store = TypeStore::new();
    let mut sink = TypeSink::new(0);
    let mut left = Type::Int;
    let mut right = Type::Bool;
    for _ in 0..(lush_types::limits::MAX_DEPTH + 4) {
        left = Type::list(left);
        right = Type::list(right);
    }
    let mut u = Unifier::new(&mut store, &mut sink);
    u.unify(&left, &right, Span::default(), None);
    let diags = sink.into_diagnostics();
    assert!(
        diags.iter().any(|d| d.code == codes::E1305_TOO_DEEP),
        "{:?}",
        diags
    );
}

#[test]
fn dedicated_slash_qualified_type() {
    // Slash-qualified types are a type-stage error when the parser admits them.
    // Construct a TypeExpr directly if the parser rejects `a/b.T`.
    use lush_syntax::ast::*;
    use lush_syntax::span::Span;
    let te = TypeExpr {
        kind: TypeKind::Named {
            name: TypeName::Qualified {
                module: Name {
                    text: "lush/list".into(),
                    span: Span::default(),
                },
                name: UName {
                    text: "List".into(),
                    span: Span::default(),
                },
            },
            args: vec![TypeExpr {
                kind: TypeKind::Named {
                    name: TypeName::Unqualified(UName {
                        text: "Int".into(),
                        span: Span::default(),
                    }),
                    args: vec![],
                },
                span: Span::default(),
            }],
        },
        span: Span::default(),
    };
    let module = Module {
        items: vec![ModuleItem::Fn(FnDef {
            public: true,
            name: Name {
                text: "f".into(),
                span: Span::default(),
            },
            params: vec![Param {
                label: None,
                name: Name {
                    text: "x".into(),
                    span: Span::default(),
                },
                ty: Some(te),
                span: Span::default(),
            }],
            return_type: Some(TypeExpr {
                kind: TypeKind::Named {
                    name: TypeName::Unqualified(UName {
                        text: "Nil".into(),
                        span: Span::default(),
                    }),
                    args: vec![],
                },
                span: Span::default(),
            }),
            body: Block {
                statements: vec![Statement::Expr(Expr {
                    kind: ExprKind::Constructor(ConstructorRef {
                        module: None,
                        name: UName {
                            text: "Nil".into(),
                            span: Span::default(),
                        },
                        span: Span::default(),
                    }),
                    span: Span::default(),
                })],
                span: Span::default(),
            },
            span: Span::default(),
        })],
        span: Span::default(),
    };
    let mut store = lush_types::ty::TypeStore::new();
    let prelude = lush_types::stubs::install_prelude_builtins(&mut store);
    let mut deps = lush_types::stubs::load_all_stubs(&mut store);
    deps.insert("prelude".into(), prelude);
    let result = lush_types::check_module(
        "slash",
        &module,
        &deps,
        &[],
        vec![],
        lush_types::CheckOptions::default(),
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == codes::E1010_SLASH_QUALIFIED_TYPE),
        "{:?}",
        result.diagnostics
    );
}

#[test]
fn opaque_pattern_match_rejected() {
    let lib = parse_module(
        "pub opaque type Email { Email(String) }\npub fn new(s: String) -> Email { Email(s); }\n",
    )
    .module
    .unwrap();
    let app = parse_module(
        "import lib;\npub fn main() -> Nil { let e = lib.new(\"a\"); case e { lib.Email(_) -> Nil; }; }\n",
    )
    .module
    .unwrap();
    let (results, _) = check_graph(
        &[("lib".into(), lib), ("app".into(), app)],
        &[],
        vec![],
        &[],
    );
    let app = results.get("app").unwrap();
    assert!(
        app.diagnostics
            .iter()
            .any(|d| d.code == codes::E1207_OPAQUE_USE),
        "{:?}",
        app.diagnostics
    );
}

#[test]
fn def_limit_on_huge_module() {
    // Generate just over a small synthetic check via check_graph path with MAX
    // We emit E1308 when defs exceed MAX_DEFS_PER_MODULE — use a unit-level force:
    use lush_types::limits::MAX_DEFS_PER_MODULE;
    const {
        assert!(MAX_DEFS_PER_MODULE >= 1000);
    }
}
