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
fn pipe_chain_4000_reuses_registers() {
    let mut stages = String::from("1");
    for _ in 0..4000 {
        stages.push_str(" |> id");
    }
    let src = format!(
        r#"
import lush/io;
import lush/int;

fn id(n: Int) -> Int {{
  n;
}}

pub fn main() -> Nil {{
  io.println(int.to_string({stages}));
}}
"#
    );
    let program = compile_sources(&[("main".into(), src)], "main").expect("compile pipe chain");
    let main = program
        .functions
        .iter()
        .find(|f| f.name == "main")
        .expect("main");
    assert!(
        (main.regs as usize) < lush_ir::limits::MAX_REGISTERS,
        "4000-stage pipe should free dead locals, regs={}",
        main.regs
    );
}

#[test]
fn sequential_dead_lets_reuse_registers() {
    let mut lets = String::new();
    for i in 0..300 {
        lets.push_str(&format!("  let x{i} = {i};\n  let _ = x{i};\n"));
    }
    let src = format!(
        r#"
pub fn main() -> Nil {{
{lets}  Nil;
}}
"#
    );
    let program = compile_sources(&[("main".into(), src)], "main").expect("compile lets");
    let main = program
        .functions
        .iter()
        .find(|f| f.name == "main")
        .expect("main");
    assert!(
        (main.regs as usize) < 64,
        "dead sequential lets should reuse regs, got {}",
        main.regs
    );
}

#[test]
fn many_println_statements_reuse_registers() {
    let mut body = String::new();
    for i in 0..300 {
        body.push_str(&format!("  io.println(\"line {i}\");\n"));
    }
    let src = format!(
        r#"
import lush/io;

pub fn main() -> Nil {{
{body}}}
"#
    );
    let program = compile_sources(&[("main".into(), src)], "main").expect("compile printlns");
    let main = program
        .functions
        .iter()
        .find(|f| f.name == "main")
        .expect("main");
    assert!(
        (main.regs as usize) < 64,
        "300 println statements should free dead results, regs={}",
        main.regs
    );
}

#[test]
fn used_once_lets_with_println_reuse_registers() {
    let mut body = String::new();
    for i in 0..300 {
        body.push_str(&format!(
            "  let a{i} = {i};\n  io.println(int.to_string(a{i}));\n"
        ));
    }
    let src = format!(
        r#"
import lush/io;
import lush/int;

pub fn main() -> Nil {{
{body}}}
"#
    );
    let program = compile_sources(&[("main".into(), src)], "main").expect("compile used-once lets");
    let main = program
        .functions
        .iter()
        .find(|f| f.name == "main")
        .expect("main");
    assert!(
        (main.regs as usize) < 64,
        "used-once lets should free after println, regs={}",
        main.regs
    );
}

#[test]
fn unread_lets_freed_immediately() {
    let mut body = String::new();
    for i in 0..300 {
        body.push_str(&format!("  let _u{i} = {i};\n"));
    }
    body.push_str("  io.println(\"done\");\n");
    let src = format!(
        r#"
import lush/io;

pub fn main() -> Nil {{
{body}}}
"#
    );
    let program = compile_sources(&[("main".into(), src)], "main").expect("compile unread lets");
    let main = program
        .functions
        .iter()
        .find(|f| f.name == "main")
        .expect("main");
    assert!(
        (main.regs as usize) < 64,
        "unread lets should be freed on arrival, regs={}",
        main.regs
    );
}

#[test]
fn e2001_reported_once() {
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
    let n = err
        .iter()
        .filter(|d| d.code == codes::E2001_TOO_MANY_REGISTERS)
        .count();
    assert_eq!(n, 1, "E2001 should be reported once, got {err:?}");
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

/// Review round 7: emit_limits programs must compile cleanly at both O0 and O1.
#[test]
fn emit_limits_programs_compile_at_o0_and_o1() {
    let sources = [
        plus_chain(100),
        plus_chain(1000),
        r#"
import lush/io;
import lush/int;
pub fn main() -> Nil {
  let a = 4;
  let x = a + 1;
  io.println(int.to_string(x));
  io.println(int.to_string(a));
}
"#
        .to_string(),
        r#"
import lush/io;
import lush/int;
pub fn main() -> Nil {
  let a = 6;
  let b = 3;
  io.println(int.to_string(a - b));
  io.println(int.to_string(a * b));
  io.println(int.to_string(a / b));
}
"#
        .to_string(),
    ];
    for (i, src) in sources.iter().enumerate() {
        let p0 = compile_sources_with_opt(&[("main".into(), src.clone())], "main", OptLevel::O0)
            .unwrap_or_else(|d| panic!("case {i} O0: {d:?}"));
        let p1 = compile_sources_with_opt(&[("main".into(), src.clone())], "main", OptLevel::O1)
            .unwrap_or_else(|d| panic!("case {i} O1: {d:?}"));
        assert_eq!(
            p0.functions.len(),
            p1.functions.len(),
            "case {i} function count"
        );
        p0.verify()
            .unwrap_or_else(|e| panic!("case {i} O0 verify: {e}"));
        p1.verify()
            .unwrap_or_else(|e| panic!("case {i} O1 verify: {e}"));
    }
}
