use lush_ir::compile_source;

#[test]
fn dump() {
    let src = include_str!("../../lush-syntax/tests/fixtures/examples/fib.lush");
    // Bypass verify by calling lower pieces — or just print errors and also
    // try to inspect via a hack: compile with a patched verifier.
    match compile_source("app/main", src) {
        Ok(p) => {
            for (i, f) in p.functions.iter().enumerate() {
                println!(
                    "=== [{i}] {}::{} arity={} regs={} ===",
                    f.module, f.name, f.arity, f.regs
                );
                for (j, op) in f.code.iter().enumerate() {
                    println!("  {j}: {op:?}");
                }
            }
        }
        Err(d) => {
            for x in &d {
                println!("{}: {}", x.code, x.message);
            }
            // Still try emit without verify
            panic!("compile failed");
        }
    }
}
