use lush_vm::{compile_source, Vm, VmConfig};

#[test]
fn fib_50() {
    let src = include_str!("../../../crates/lush-syntax/tests/fixtures/examples/fib.lush");
    let prog = compile_source("app/main", src).unwrap_or_else(|d| {
        panic!(
            "compile failed: {:?}",
            d.iter().map(|x| format!("{}:{}", x.code, x.message)).collect::<Vec<_>>()
        )
    });
    let mut vm = Vm::new(prog, VmConfig::default());
    let exit = vm.run_to_completion();
    assert_eq!(exit.status, 0, "stderr={}", String::from_utf8_lossy(&exit.stderr));
    assert_eq!(
        String::from_utf8_lossy(&exit.stdout),
        "fib(50) = 12586269025\n"
    );
}
