use lush_ir::OptLevel;
use lush_vm::{compile_sources_with_opt, Vm, VmConfig};

mod common;

#[test]
fn fib_e2e() {
    common::assert_case("fib");
}

#[test]
fn fib_differential_opt() {
    common::assert_differential("fib");
}

#[test]
fn fib_quantum_invariance() {
    common::assert_quantum_invariant("fib");
}

#[test]
fn arithmetic_e2e() {
    common::assert_case("arith");
}

#[test]
fn lists_e2e() {
    common::assert_case("lists");
}

#[test]
fn result_case_e2e() {
    common::assert_case("result_case");
}

#[test]
fn result_error_e2e() {
    common::assert_case("result_error");
}

#[test]
fn list_length_e2e() {
    common::assert_case("list_length");
}

#[test]
fn tail_depth_e2e() {
    common::assert_case("tail_depth");
}

#[test]
fn panic_div_zero() {
    let (src, _, _, expect_status) = common::read_case("panic_div");
    let exit = common::run_source("main", &src, OptLevel::default());
    assert_eq!(exit.status, expect_status);
    assert!(exit.stdout.is_empty(), "stdout should be empty");
    let err = String::from_utf8_lossy(&exit.stderr);
    assert!(err.starts_with("panic:"), "{err}");
    assert!(
        err.contains("division by zero") || err.contains("div"),
        "{err}"
    );
}

#[test]
fn spec_fib_extracted() {
    let spec = lush_test_support::read_spec_md(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
    let fences = lush_test_support::extract_lush_fences(&spec);
    let inv = lush_test_support::spec_fence_inventory();
    assert_eq!(fences.len(), inv.len(), "fence inventory drift");
    // Fence 15 is §13 fib (Module).
    let fib = &fences[15];
    assert!(fib.contains("fib(50)"), "expected §13 fib fence");
    let exit = common::run_source("main", fib, OptLevel::default());
    assert_eq!(exit.status, 0);
    assert_eq!(
        String::from_utf8_lossy(&exit.stdout),
        "fib(50) = 12586269025
"
    );
}

#[test]
fn unavailable_builtin_is_compile_error() {
    let src = r#"
import lush/list;
pub fn main() {
  let _ = list.length([]);
  Nil;
}
"#;
    let err = compile_sources_with_opt(&[("main".into(), src.into())], "main", OptLevel::O0)
        .expect_err("must fail");
    assert!(
        err.iter().any(|d| d.code == "E2000"),
        "expected E2000, got {:?}",
        err.iter()
            .map(|d| format!("{}:{}", d.code, d.message))
            .collect::<Vec<_>>()
    );
}

#[test]
fn stack_overflow_panics() {
    let src = r#"
fn boom(n) {
  let x = boom(n + 1);
  x;
}
pub fn main() {
  let _ = boom(0);
  Nil;
}
"#;
    let program = compile_sources_with_opt(&[("main".into(), src.into())], "main", OptLevel::O0)
        .unwrap_or_else(|d| {
            panic!(
                "compile: {:?}",
                d.iter()
                    .map(|x| format!("{}:{}", x.code, x.message))
                    .collect::<Vec<_>>()
            )
        });
    let mut vm = Vm::new(
        program,
        VmConfig {
            max_stack_frames: 64,
            ..VmConfig::default()
        },
    );
    let exit = vm.run_to_completion();
    assert_eq!(exit.status, 1);
    let err = String::from_utf8_lossy(&exit.stderr);
    assert!(err.contains("stack overflow"), "{err}");
    assert!(err.contains("panic:"), "{err}");
}

#[test]
fn registry_codes_defined() {
    assert!(lush_ir::codes::all_codes().contains(&"E2000"));
    assert!(lush_ir::codes::all_codes().len() >= 12);
}

#[test]
fn panic_overflow_e2e() {
    let (src, _, _, expect_status) = common::read_case("panic_overflow");
    let exit = common::run_source("main", &src, OptLevel::default());
    assert_eq!(exit.status, expect_status);
    let err = String::from_utf8_lossy(&exit.stderr);
    assert!(err.contains("integer overflow"), "{err}");
    assert!(err.starts_with("panic:"), "{err}");
}

#[test]
fn panic_assert_e2e() {
    let (src, _, _, expect_status) = common::read_case("panic_assert");
    let exit = common::run_source("main", &src, OptLevel::default());
    assert_eq!(exit.status, expect_status);
    let err = String::from_utf8_lossy(&exit.stderr);
    assert!(err.contains("nope"), "{err}");
    assert!(err.starts_with("panic:"), "{err}");
}
