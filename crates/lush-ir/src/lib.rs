//! Core and bytecode compiler infrastructure for Lush (build step 3).
//!
//! The bytecode representation is deliberately independent of the VM so it can
//! be validated and serialized before execution.

use std::fmt;

pub const MAX_REGISTERS: u16 = 256;
pub const FORMAT_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceLoc {
    pub line: u32,
    pub column: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Instruction {
    Move { dst: u8, src: u8 },
    LoadInt { dst: u8, value: i64 },
    AddInt { dst: u8, lhs: u8, rhs: u8 },
    SubInt { dst: u8, lhs: u8, rhs: u8 },
    MulInt { dst: u8, lhs: u8, rhs: u8 },
    DivInt { dst: u8, lhs: u8, rhs: u8 },
    RemInt { dst: u8, lhs: u8, rhs: u8 },
    Jump { target: u32 },
    JumpIfFalse { condition: u8, target: u32 },
    Return { src: u8 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    pub arity: u8,
    pub register_count: u16,
    pub code: Vec<Instruction>,
    /// One source location per instruction.
    pub locations: Vec<SourceLoc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub functions: Vec<Function>,
    pub entry: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyError {
    pub function: usize,
    pub instruction: Option<usize>,
    pub message: String,
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(pc) = self.instruction {
            write!(
                f,
                "function {} instruction {pc}: {}",
                self.function, self.message
            )
        } else {
            write!(f, "function {}: {}", self.function, self.message)
        }
    }
}

impl std::error::Error for VerifyError {}

/// Verify all indexes and control-flow terminators before a program is run.
pub fn verify(program: &Program) -> Result<(), Vec<VerifyError>> {
    let mut errors = Vec::new();
    if program.entry as usize >= program.functions.len() {
        errors.push(VerifyError {
            function: 0,
            instruction: None,
            message: "entry function index out of range".into(),
        });
    }
    for (fi, function) in program.functions.iter().enumerate() {
        let fail = |pc, message: &str| VerifyError {
            function: fi,
            instruction: pc,
            message: message.into(),
        };
        if function.register_count > MAX_REGISTERS {
            errors.push(fail(None, "register count exceeds MAX_REGISTERS"));
        }
        if function.arity as u16 > function.register_count {
            errors.push(fail(None, "function arity exceeds register count"));
        }
        if function.code.is_empty() {
            errors.push(fail(None, "function has no instructions"));
            continue;
        }
        if function.locations.len() != function.code.len() {
            errors.push(fail(
                None,
                "debug location count does not match instruction count",
            ));
        }
        for (pc, instruction) in function.code.iter().enumerate() {
            let reg = |r: u8| (r as u16) < function.register_count;
            let check_reg = |r: u8, role: &str| {
                if reg(r) {
                    None
                } else {
                    Some(format!("{role} register {r} is out of range"))
                }
            };
            let mut bad = Vec::new();
            match instruction {
                Instruction::Move { dst, src } => {
                    bad.extend(check_reg(*dst, "destination"));
                    bad.extend(check_reg(*src, "source"));
                }
                Instruction::LoadInt { dst, .. } => bad.extend(check_reg(*dst, "destination")),
                Instruction::AddInt { dst, lhs, rhs }
                | Instruction::SubInt { dst, lhs, rhs }
                | Instruction::MulInt { dst, lhs, rhs }
                | Instruction::DivInt { dst, lhs, rhs }
                | Instruction::RemInt { dst, lhs, rhs } => {
                    bad.extend(check_reg(*dst, "destination"));
                    bad.extend(check_reg(*lhs, "left operand"));
                    bad.extend(check_reg(*rhs, "right operand"));
                }
                Instruction::Jump { target } => {
                    if *target as usize >= function.code.len() {
                        bad.push("jump target out of range".into());
                    }
                }
                Instruction::JumpIfFalse { condition, target } => {
                    bad.extend(check_reg(*condition, "condition"));
                    if *target as usize >= function.code.len() {
                        bad.push("jump target out of range".into());
                    }
                }
                Instruction::Return { src } => bad.extend(check_reg(*src, "return source")),
            }
            for message in bad {
                errors.push(fail(Some(pc), &message));
            }
        }
        if !matches!(
            function.code.last(),
            Some(Instruction::Return { .. } | Instruction::Jump { .. })
        ) {
            errors.push(fail(
                Some(function.code.len() - 1),
                "function is missing a terminator",
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn func(code: Vec<Instruction>) -> Function {
        let locations = vec![SourceLoc { line: 1, column: 1 }; code.len()];
        Function {
            name: "main".into(),
            arity: 0,
            register_count: 2,
            code,
            locations,
        }
    }

    #[test]
    fn verifier_accepts_well_formed_function() {
        let p = Program {
            functions: vec![func(vec![
                Instruction::LoadInt { dst: 0, value: 42 },
                Instruction::Return { src: 0 },
            ])],
            entry: 0,
        };
        assert_eq!(verify(&p), Ok(()));
    }

    #[test]
    fn verifier_rejects_bad_register_and_jump() {
        let p = Program {
            functions: vec![func(vec![
                Instruction::Jump { target: 9 },
                Instruction::Return { src: 3 },
            ])],
            entry: 0,
        };
        let errors = verify(&p).unwrap_err();
        assert_eq!(errors.len(), 2);
        assert!(errors.iter().any(|e| e.message.contains("jump target")));
        assert!(errors.iter().any(|e| e.message.contains("register")));
    }
}
