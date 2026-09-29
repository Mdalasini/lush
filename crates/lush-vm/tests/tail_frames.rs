use lush_vm::{compile_source, Vm, VmConfig};

#[test]
fn tail_call_keeps_constant_depth() {
    let src = r#"
import lush/io;
import lush/int;

pub fn countdown(n) {
  case n {
    0 -> 0;
    _ -> countdown(n - 1);
  };
}

pub fn main() {
  io.println(int.to_string(countdown(100000)));
}
"#;
    let prog = compile_source("main", src).unwrap();
    let mut vm = Vm::new(prog, VmConfig::default());
    let exit = vm.run_to_completion();
    assert_eq!(
        exit.status,
        0,
        "stderr={}",
        String::from_utf8_lossy(&exit.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&exit.stdout), "0\n");
    // entry frame + at most one reused countdown frame
    assert!(
        vm.max_frame_depth <= 2,
        "max_frame_depth={} (expected <= 2 for proper TailCall)",
        vm.max_frame_depth
    );
    assert!(vm.had_tail_call);
}
