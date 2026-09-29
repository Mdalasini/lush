use std::fs;
use std::path::PathBuf;

use lush_vm::{compile_sources, Vm, VmConfig};

fn e2e_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/e2e")
}

#[test]
fn fib_e2e() {
    let root = e2e_root();
    let src = fs::read_to_string(root.join("fib.lush")).unwrap();
    let expect_out = fs::read_to_string(root.join("fib.stdout")).unwrap();
    let expect_err = fs::read_to_string(root.join("fib.stderr")).unwrap();
    let expect_status: i32 = fs::read_to_string(root.join("fib.status"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    let program = compile_sources(&[("main".into(), src)], "main").expect("compile fib");
    let mut vm = Vm::new(program, VmConfig::default());
    let exit = vm.run_to_completion();
    assert_eq!(exit.status, expect_status);
    assert_eq!(String::from_utf8_lossy(&exit.stdout), expect_out);
    assert_eq!(String::from_utf8_lossy(&exit.stderr), expect_err);
}
