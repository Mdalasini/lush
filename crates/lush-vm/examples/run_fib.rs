fn main() {
    let src = std::fs::read_to_string("tests/e2e/fib.lush").unwrap();
    match lush_vm::compile_source("main", &src) {
        Ok(prog) => {
            println!("funcs={}", prog.functions.len());
            for (i, f) in prog.functions.iter().enumerate() {
                println!(
                    "fn {i} {}/{} arity={} regs={} code={}",
                    f.module,
                    f.name,
                    f.arity,
                    f.regs,
                    f.code.len()
                );
                for (j, op) in f.code.iter().enumerate() {
                    println!("  {j}: {op:?}");
                }
            }
            let mut vm = lush_vm::Vm::new(prog, lush_vm::VmConfig::default());
            let r = vm.run_to_completion();
            println!("status={}", r.status);
            println!("stdout={:?}", String::from_utf8_lossy(&r.stdout));
            println!("stderr=\n{}", String::from_utf8_lossy(&r.stderr));
        }
        Err(d) => {
            for x in d {
                println!("diag {}: {}", x.code, x.message);
            }
        }
    }
}
