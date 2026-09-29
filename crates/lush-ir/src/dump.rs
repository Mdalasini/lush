//! Deterministic bytecode disassembler.

use crate::bytecode::{Constant, Op, Program};

pub fn dump_program(program: &Program) -> String {
    let mut out = String::new();
    out.push_str(&format!("entry: {}\n", program.entry));
    out.push_str("constants:\n");
    for (i, c) in program.constants.iter().enumerate() {
        out.push_str(&format!("  {i}: {}\n", dump_const(c)));
    }
    for (i, f) in program.functions.iter().enumerate() {
        out.push_str(&format!(
            "\nfun {i} {}/{} arity={} regs={}\n",
            f.module, f.name, f.arity, f.regs
        ));
        for (pc, op) in f.code.iter().enumerate() {
            let (line, col) = f.lines.get(pc).copied().unwrap_or((0, 0));
            out.push_str(&format!("  {pc:04}  {:<40} ; {line}:{col}\n", dump_op(op)));
        }
    }
    out
}

fn dump_const(c: &Constant) -> String {
    match c {
        Constant::Int(n) => format!("int {n}"),
        Constant::Float(f) => format!("float {f:?}"),
        Constant::String(s) => format!("string {:?}", s),
        Constant::Nullary { type_tag, variant } => {
            format!("nullary type={type_tag} variant={variant}")
        }
    }
}

fn dump_op(op: &Op) -> String {
    format!("{op:?}")
}
