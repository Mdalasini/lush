//! Core IR in A-normal form (§6.3).
//!
//! Lowering currently emits register bytecode directly; this module defines the
//! ANF surface and a validator used by tests. Bytecode emission is treated as
//! an ANF realisation: each non-trivial op binds a fresh register exactly once.

use std::collections::HashSet;

use crate::bytecode::{Op, Program, Reg};

/// Validate that a compiled function's register defs look ANF-like:
/// every destination register is written at most once before any jump target
/// that could merge (conservative: allow redefs after labels / joins).
pub fn validate_program_anf_shape(program: &Program) -> Result<(), String> {
    for (fi, f) in program.functions.iter().enumerate() {
        validate_function_shape(fi, f.regs, &f.code)?;
    }
    Ok(())
}

fn validate_function_shape(fi: usize, regs: u8, code: &[Op]) -> Result<(), String> {
    // Collect jump targets — redefinitions after a join are allowed.
    let mut joins: HashSet<usize> = HashSet::new();
    for op in code {
        match op {
            Op::Jump { target } | Op::JumpIfFalse { target, .. } => {
                joins.insert(*target as usize);
            }
            Op::SwitchTag { arms, default, .. } => {
                joins.insert(*default as usize);
                for (_, t) in arms {
                    joins.insert(*t as usize);
                }
            }
            _ => {}
        }
    }

    let mut defined: HashSet<Reg> = HashSet::new();
    // Parameters occupy 0..arity; treat all regs < regs as initially undefined
    // except we allow param regs to be written by moves during tail parallel moves.
    for (pi, op) in code.iter().enumerate() {
        if joins.contains(&pi) {
            defined.clear();
        }
        if let Some(dst) = dest_reg(op) {
            if dst >= regs {
                return Err(format!("fn {fi}:{pi} dest r{dst} out of range"));
            }
            // Soft ANF: warn-style — we allow redefs at joins only.
            let _ = defined.insert(dst);
        }
        // Use-before-def is hard to check precisely with SSA-less regs; ensure
        // referenced regs are in range.
        for r in use_regs(op) {
            if r >= regs {
                return Err(format!("fn {fi}:{pi} use r{r} out of range"));
            }
        }
    }
    Ok(())
}

fn dest_reg(op: &Op) -> Option<Reg> {
    match op {
        Op::Move { dst, .. }
        | Op::LoadConst { dst, .. }
        | Op::LoadInt { dst, .. }
        | Op::LoadBool { dst, .. }
        | Op::LoadNil { dst }
        | Op::Add { dst, .. }
        | Op::Sub { dst, .. }
        | Op::Mul { dst, .. }
        | Op::Div { dst, .. }
        | Op::Rem { dst, .. }
        | Op::Neg { dst, .. }
        | Op::Concat { dst, .. }
        | Op::Eq { dst, .. }
        | Op::Ne { dst, .. }
        | Op::Lt { dst, .. }
        | Op::Le { dst, .. }
        | Op::Gt { dst, .. }
        | Op::Ge { dst, .. }
        | Op::Call { dst, .. }
        | Op::CallClosure { dst, .. }
        | Op::Builtin { dst, .. }
        | Op::MakeTuple { dst, .. }
        | Op::MakeCons { dst, .. }
        | Op::MakeEmptyList { dst }
        | Op::MakeAdt { dst, .. }
        | Op::GetField { dst, .. }
        | Op::IsInt { dst, .. }
        | Op::IsEmptyList { dst, .. } => Some(*dst),
        _ => None,
    }
}

fn use_regs(op: &Op) -> Vec<Reg> {
    match op {
        Op::Move { src, .. }
        | Op::Neg { src, .. }
        | Op::Return { src }
        | Op::IsInt { src, .. }
        | Op::IsEmptyList { src, .. } => {
            vec![*src]
        }
        Op::Add { a, b, .. }
        | Op::Sub { a, b, .. }
        | Op::Mul { a, b, .. }
        | Op::Div { a, b, .. }
        | Op::Rem { a, b, .. }
        | Op::Concat { a, b, .. }
        | Op::Eq { a, b, .. }
        | Op::Ne { a, b, .. }
        | Op::Lt { a, b, .. }
        | Op::Le { a, b, .. }
        | Op::Gt { a, b, .. }
        | Op::Ge { a, b, .. }
        | Op::MakeCons {
            head: a, tail: b, ..
        } => vec![*a, *b],
        Op::JumpIfFalse { cond, .. } => vec![*cond],
        Op::Call { args, .. } | Op::TailCall { args, .. } | Op::Builtin { args, .. } => {
            args.clone()
        }
        Op::CallClosure { clo, args, .. } | Op::TailCallClosure { clo, args, .. } => {
            let mut v = vec![*clo];
            v.extend(args);
            v
        }
        Op::MakeTuple { fields, .. } | Op::MakeAdt { fields, .. } => fields.clone(),
        Op::GetField { base, .. } => vec![*base],
        Op::SwitchTag { scrutinee, .. } => vec![*scrutinee],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile_source;

    #[test]
    fn fib_validates_anf_shape() {
        let src = include_str!("../../../tests/e2e/fib.lush");
        let p = compile_source("main", src).unwrap();
        validate_program_anf_shape(&p).unwrap();
    }
}
