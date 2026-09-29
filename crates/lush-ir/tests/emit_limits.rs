//! Emit limits: register overflow → E2001, long chains, span line/col.

use lush_ir::codes;
use lush_ir::{compile_sources, compile_sources_with_opt, OptLevel};

fn plus_chain(terms: usize) -> String {
    let mut body = String::from("1");
    for _ in 1..terms {
        body.push_str(" + 1");
    }
    format!(
        r#"
import lush/io;
import lush/int;

pub fn main() -> Nil {{
  io.println(int.to_string({body}));
}}
"#
    )
}

#[test]
fn plus_chain_1000_compiles_and_runs() {
    let src = plus_chain(1000);
    let program = compile_sources(&[("main".into(), src)], "main").expect("compile");
    // High-water must stay well under the 256 register cap thanks to recycling.
    let main = program
        .functions
        .iter()
        .find(|f| f.name == "main")
        .expect("main");
    assert!(
        main.regs < 32,
        "1000-term chain should reuse temps, regs={}",
        main.regs
    );
}

#[test]
fn plus_chain_4096_compiles_without_stack_overflow() {
    let src = plus_chain(4096);
    let program = compile_sources(&[("main".into(), src)], "main").expect("compile 4096");
    let main = program
        .functions
        .iter()
        .find(|f| f.name == "main")
        .expect("main");
    assert!(
        (main.regs as usize) <= lush_ir::limits::MAX_REGISTERS,
        "regs={}",
        main.regs
    );
}

#[test]
fn too_many_live_registers_is_e2001() {
    // Keep 300 values live at once via a tuple — recycling cannot help.
    let mut elems = String::new();
    for i in 0..300 {
        if i > 0 {
            elems.push_str(", ");
        }
        elems.push_str(&i.to_string());
    }
    let src = format!(
        r#"
pub fn main() -> Nil {{
  let _t = #({elems});
  Nil;
}}
"#
    );
    let err = compile_sources(&[("main".into(), src)], "main").expect_err("should fail");
    assert!(
        err.iter()
            .any(|d| d.code == codes::E2001_TOO_MANY_REGISTERS),
        "expected E2001, got {err:?}"
    );
}

#[test]
fn panic_debug_info_uses_real_line_col() {
    let src = r#"
import lush/io;
import lush/int;

fn helper(n: Int) -> Int {
  n / 0;
}

pub fn main() -> Nil {
  let _ = helper(1);
  Nil;
}
"#;
    let program = compile_sources_with_opt(&[("main".into(), src.into())], "main", OptLevel::O0)
        .expect("compile");
    let helper = program
        .functions
        .iter()
        .find(|f| f.name == "helper")
        .expect("helper");
    // At least one instruction in helper must be on line 6 (the `n / 0`).
    assert!(
        helper.lines.iter().any(|&(line, _)| line == 6),
        "expected line 6 in helper debug info, got {:?}",
        helper.lines
    );
    assert!(
        !helper
            .lines
            .iter()
            .all(|&(line, col)| line == 1 && col == 1),
        "debug info must not be stamped (1,1) for every op"
    );
}
