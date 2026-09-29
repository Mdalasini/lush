use std::fs;
use std::path::PathBuf;

use lush_vm::{compile_sources, Vm, VmConfig};

fn e2e_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/e2e")
}

fn run_named(name: &str) {
    let root = e2e_root();
    let src = fs::read_to_string(root.join(format!("{name}.lush"))).unwrap();
    let expect_out = fs::read_to_string(root.join(format!("{name}.stdout"))).unwrap();
    let expect_status: i32 = fs::read_to_string(root.join(format!("{name}.status")))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let expect_err_path = root.join(format!("{name}.stderr"));
    let expect_err = if expect_err_path.exists() {
        Some(fs::read_to_string(expect_err_path).unwrap())
    } else {
        None
    };

    let program = compile_sources(&[(name.into(), src)], name).unwrap_or_else(|d| {
        panic!(
            "{name} compile: {:?}",
            d.iter()
                .map(|x| format!("{}:{}", x.code, x.message))
                .collect::<Vec<_>>()
        )
    });
    let mut vm = Vm::new(program, VmConfig::default());
    let exit = vm.run_to_completion();
    assert_eq!(
        exit.status,
        expect_status,
        "{name} status; stderr={}",
        String::from_utf8_lossy(&exit.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&exit.stdout),
        expect_out,
        "{name} stdout"
    );
    if let Some(err) = expect_err {
        assert_eq!(String::from_utf8_lossy(&exit.stderr), err, "{name} stderr");
    } else {
        assert!(
            String::from_utf8_lossy(&exit.stderr).starts_with("panic:"),
            "{name} expected panic stderr, got {:?}",
            String::from_utf8_lossy(&exit.stderr)
        );
    }
}

#[test]
fn fib_e2e() {
    run_named("fib");
}

#[test]
fn list_length_e2e() {
    run_named("list_length");
}

#[test]
fn panic_div_e2e() {
    run_named("panic_div");
}

#[test]
fn tail_depth_e2e() {
    run_named("tail_depth");
}
