//! Register-bytecode representation and verifier for Lush.

use std::fmt;

use lush_syntax::diagnostic::{Diagnostic, DiagnosticKind};
use lush_syntax::span::Span;

pub mod codes;

pub const MAX_REGISTERS: u16 = 256;
pub const MAX_ERRORS: usize = lush_syntax::diagnostic::MAX_ERRORS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceLoc {
    pub line: u32,
    pub column: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Instruction {
    Move { dst: u8, src: u8 },
    LoadInt { dst: u8, value: i64 },
    LoadBool { dst: u8, value: bool },
    AddInt { dst: u8, lhs: u8, rhs: u8 },
    SubInt { dst: u8, lhs: u8, rhs: u8 },
    MulInt { dst: u8, lhs: u8, rhs: u8 },
    DivInt { dst: u8, lhs: u8, rhs: u8 },
    RemInt { dst: u8, lhs: u8, rhs: u8 },
    EqInt { dst: u8, lhs: u8, rhs: u8 },
    LtInt { dst: u8, lhs: u8, rhs: u8 },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegisterRole {
    Destination,
    Source,
    LeftOperand,
    RightOperand,
    Condition,
    ReturnSource,
}

impl fmt::Display for RegisterRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Destination => "destination",
            Self::Source => "source",
            Self::LeftOperand => "left operand",
            Self::RightOperand => "right operand",
            Self::Condition => "condition",
            Self::ReturnSource => "return source",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyErrorKind {
    EntryOutOfRange,
    RegisterLimit,
    ArityExceedsRegisters,
    EmptyFunction,
    DebugInfoMismatch,
    RegisterOutOfRange { role: RegisterRole, register: u8 },
    JumpTargetOutOfRange,
    MissingTerminator,
    TooManyErrors,
}

impl VerifyErrorKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::EntryOutOfRange => codes::E2000_ENTRY_OUT_OF_RANGE,
            Self::RegisterLimit => codes::E2001_REGISTER_LIMIT,
            Self::ArityExceedsRegisters => codes::E2002_ARITY_EXCEEDS_REGISTERS,
            Self::EmptyFunction => codes::E2003_EMPTY_FUNCTION,
            Self::DebugInfoMismatch => codes::E2004_DEBUG_INFO_MISMATCH,
            Self::RegisterOutOfRange { .. } => codes::E2005_REGISTER_OUT_OF_RANGE,
            Self::JumpTargetOutOfRange => codes::E2006_JUMP_TARGET_OUT_OF_RANGE,
            Self::MissingTerminator => codes::E2007_MISSING_TERMINATOR,
            Self::TooManyErrors => codes::E2099_TOO_MANY_ERRORS,
        }
    }
}

impl fmt::Display for VerifyErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntryOutOfRange => f.write_str("entry function index out of range"),
            Self::RegisterLimit => f.write_str("register count exceeds MAX_REGISTERS"),
            Self::ArityExceedsRegisters => f.write_str("function arity exceeds register count"),
            Self::EmptyFunction => f.write_str("function has no instructions"),
            Self::DebugInfoMismatch => {
                f.write_str("debug location count does not match instruction count")
            }
            Self::RegisterOutOfRange { role, register } => {
                write!(f, "{role} register {register} is out of range")
            }
            Self::JumpTargetOutOfRange => f.write_str("jump target out of range"),
            Self::MissingTerminator => f.write_str("function is missing a terminator"),
            Self::TooManyErrors => write!(f, "too many verifier errors (limit {MAX_ERRORS})"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerifyError {
    pub function: Option<usize>,
    pub instruction: Option<usize>,
    pub kind: VerifyErrorKind,
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.function, self.instruction) {
            (Some(function), Some(pc)) => {
                write!(f, "function {function} instruction {pc}: {}", self.kind)
            }
            (Some(function), None) => write!(f, "function {function}: {}", self.kind),
            (None, _) => self.kind.fmt(f),
        }
    }
}

impl VerifyError {
    /// Convert this verifier failure to the shared diagnostic representation.
    /// `span` is supplied by the caller because bytecode source locations are
    /// not byte offsets into the original source text.
    pub fn to_diagnostic(self, span: Span) -> Diagnostic {
        Diagnostic::error(
            self.kind.code(),
            self.to_string(),
            span,
            None,
            DiagnosticKind::Bytecode,
        )
    }
}

impl std::error::Error for VerifyError {}

fn push_error(errors: &mut Vec<VerifyError>, error: VerifyError) -> bool {
    if errors.len() < MAX_ERRORS {
        errors.push(error);
        true
    } else if errors.len() == MAX_ERRORS {
        errors.push(VerifyError {
            function: error.function,
            instruction: error.instruction,
            kind: VerifyErrorKind::TooManyErrors,
        });
        false
    } else {
        false
    }
}

/// Verify all indexes and control-flow terminators before a program is run.
pub fn verify(program: &Program) -> Result<(), Vec<VerifyError>> {
    let mut errors = Vec::new();
    if (program.entry as usize) >= program.functions.len()
        && !push_error(
            &mut errors,
            VerifyError {
                function: None,
                instruction: None,
                kind: VerifyErrorKind::EntryOutOfRange,
            },
        )
    {
        return Err(errors);
    }

    'functions: for (fi, function) in program.functions.iter().enumerate() {
        let mut report = |instruction, kind| {
            push_error(
                &mut errors,
                VerifyError {
                    function: Some(fi),
                    instruction,
                    kind,
                },
            )
        };

        if function.register_count > MAX_REGISTERS && !report(None, VerifyErrorKind::RegisterLimit)
        {
            break;
        }
        if (function.arity as u16) > function.register_count
            && !report(None, VerifyErrorKind::ArityExceedsRegisters)
        {
            break;
        }
        if function.code.is_empty() {
            if !report(None, VerifyErrorKind::EmptyFunction) {
                break;
            }
            continue;
        }
        if function.locations.len() != function.code.len()
            && !report(None, VerifyErrorKind::DebugInfoMismatch)
        {
            break;
        }

        for (pc, instruction) in function.code.iter().enumerate() {
            let mut operand_errors = [None; 3];
            let register = |r: u8, role| {
                ((r as u16) >= function.register_count)
                    .then_some(VerifyErrorKind::RegisterOutOfRange { role, register: r })
            };
            match instruction {
                Instruction::Move { dst, src } => {
                    operand_errors[0] = register(*dst, RegisterRole::Destination);
                    operand_errors[1] = register(*src, RegisterRole::Source);
                }
                Instruction::LoadInt { dst, .. } | Instruction::LoadBool { dst, .. } => {
                    operand_errors[0] = register(*dst, RegisterRole::Destination);
                }
                Instruction::AddInt { dst, lhs, rhs }
                | Instruction::SubInt { dst, lhs, rhs }
                | Instruction::MulInt { dst, lhs, rhs }
                | Instruction::DivInt { dst, lhs, rhs }
                | Instruction::RemInt { dst, lhs, rhs }
                | Instruction::EqInt { dst, lhs, rhs }
                | Instruction::LtInt { dst, lhs, rhs } => {
                    operand_errors[0] = register(*dst, RegisterRole::Destination);
                    operand_errors[1] = register(*lhs, RegisterRole::LeftOperand);
                    operand_errors[2] = register(*rhs, RegisterRole::RightOperand);
                }
                Instruction::Jump { target } => {
                    if (*target as usize) >= function.code.len()
                        && !report(Some(pc), VerifyErrorKind::JumpTargetOutOfRange)
                    {
                        break 'functions;
                    }
                }
                Instruction::JumpIfFalse { condition, target } => {
                    operand_errors[0] = register(*condition, RegisterRole::Condition);
                    if (*target as usize) >= function.code.len()
                        && !report(Some(pc), VerifyErrorKind::JumpTargetOutOfRange)
                    {
                        break 'functions;
                    }
                }
                Instruction::Return { src } => {
                    operand_errors[0] = register(*src, RegisterRole::ReturnSource);
                }
            }
            for kind in operand_errors.into_iter().flatten() {
                if !report(Some(pc), kind) {
                    break 'functions;
                }
            }
        }
        if !matches!(
            function.code.last(),
            Some(Instruction::Return { .. } | Instruction::Jump { .. })
        ) && !report(
            Some(function.code.len() - 1),
            VerifyErrorKind::MissingTerminator,
        ) {
            break;
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

    fn func(code: Vec<Instruction>, register_count: u16) -> Function {
        let locations = vec![SourceLoc { line: 1, column: 1 }; code.len()];
        Function {
            name: "main".into(),
            arity: 0,
            register_count,
            code,
            locations,
        }
    }

    #[test]
    fn verifier_accepts_well_formed_function() {
        let p = Program {
            functions: vec![func(
                vec![
                    Instruction::LoadInt { dst: 0, value: 42 },
                    Instruction::Return { src: 0 },
                ],
                2,
            )],
            entry: 0,
        };
        assert_eq!(verify(&p), Ok(()));
    }

    #[test]
    fn verifier_rejects_bad_register_and_jump() {
        let p = Program {
            functions: vec![func(
                vec![
                    Instruction::Jump { target: 9 },
                    Instruction::Return { src: 3 },
                ],
                2,
            )],
            entry: 0,
        };
        let errors = verify(&p).unwrap_err();
        assert_eq!(errors.len(), 2);
        assert!(errors
            .iter()
            .any(|e| e.kind == VerifyErrorKind::JumpTargetOutOfRange));
        assert!(errors
            .iter()
            .any(|e| matches!(e.kind, VerifyErrorKind::RegisterOutOfRange { .. })));
    }

    fn invalid_loads(count: usize) -> Program {
        let code = (0..count)
            .map(|_| Instruction::LoadInt {
                dst: u8::MAX,
                value: 0,
            })
            .chain([Instruction::Return { src: 0 }])
            .collect();
        Program {
            functions: vec![func(code, 2)],
            entry: 0,
        }
    }

    #[test]
    fn verifier_error_limit_boundary_is_exact() {
        let at_limit = verify(&invalid_loads(MAX_ERRORS)).unwrap_err();
        assert_eq!(at_limit.len(), MAX_ERRORS);
        assert!(at_limit
            .iter()
            .all(|error| error.kind != VerifyErrorKind::TooManyErrors));

        let over_limit = verify(&invalid_loads(MAX_ERRORS + 1)).unwrap_err();
        assert_eq!(over_limit.len(), MAX_ERRORS + 1);
        assert_eq!(
            over_limit.last().unwrap().kind,
            VerifyErrorKind::TooManyErrors
        );
    }

    #[test]
    fn verifier_rejection_paths_report_exact_error_locations_and_roles() {
        let mut over_limit = func(vec![Instruction::Return { src: 0 }], MAX_REGISTERS + 1);
        let register_limit = Program {
            functions: vec![over_limit.clone()],
            entry: 0,
        };
        let mut expected_register_limit = VerifyError {
            function: Some(0),
            instruction: None,
            kind: VerifyErrorKind::RegisterLimit,
        };
        assert_eq!(
            verify(&register_limit).unwrap_err(),
            vec![expected_register_limit]
        );

        over_limit.register_count = 1;
        over_limit.arity = 2;
        let arity = Program {
            functions: vec![over_limit],
            entry: 0,
        };
        expected_register_limit.kind = VerifyErrorKind::ArityExceedsRegisters;
        assert_eq!(verify(&arity).unwrap_err(), vec![expected_register_limit]);

        let empty = Program {
            functions: vec![func(vec![], 2)],
            entry: 0,
        };
        assert_eq!(
            verify(&empty).unwrap_err(),
            vec![VerifyError {
                function: Some(0),
                instruction: None,
                kind: VerifyErrorKind::EmptyFunction,
            }]
        );

        let mut no_debug = func(vec![Instruction::Return { src: 0 }], 2);
        no_debug.locations.clear();
        let mismatch = Program {
            functions: vec![no_debug],
            entry: 0,
        };
        assert_eq!(
            verify(&mismatch).unwrap_err(),
            vec![VerifyError {
                function: Some(0),
                instruction: None,
                kind: VerifyErrorKind::DebugInfoMismatch,
            }]
        );

        let unterminated = Program {
            functions: vec![func(
                vec![Instruction::LoadBool {
                    dst: 0,
                    value: true,
                }],
                2,
            )],
            entry: 0,
        };
        assert_eq!(
            verify(&unterminated).unwrap_err(),
            vec![VerifyError {
                function: Some(0),
                instruction: Some(0),
                kind: VerifyErrorKind::MissingTerminator,
            }]
        );

        let bad_if = Program {
            functions: vec![func(
                vec![
                    Instruction::JumpIfFalse {
                        condition: 9,
                        target: 99,
                    },
                    Instruction::Return { src: 0 },
                ],
                2,
            )],
            entry: 0,
        };
        assert_eq!(
            verify(&bad_if).unwrap_err(),
            vec![
                VerifyError {
                    function: Some(0),
                    instruction: Some(0),
                    kind: VerifyErrorKind::JumpTargetOutOfRange
                },
                VerifyError {
                    function: Some(0),
                    instruction: Some(0),
                    kind: VerifyErrorKind::RegisterOutOfRange {
                        role: RegisterRole::Condition,
                        register: 9
                    }
                },
            ]
        );

        let bad_operands = Program {
            functions: vec![func(
                vec![
                    Instruction::AddInt {
                        dst: 9,
                        lhs: 8,
                        rhs: 7,
                    },
                    Instruction::Return { src: 0 },
                ],
                2,
            )],
            entry: 0,
        };
        assert_eq!(
            verify(&bad_operands).unwrap_err(),
            vec![
                VerifyError {
                    function: Some(0),
                    instruction: Some(0),
                    kind: VerifyErrorKind::RegisterOutOfRange {
                        role: RegisterRole::Destination,
                        register: 9
                    }
                },
                VerifyError {
                    function: Some(0),
                    instruction: Some(0),
                    kind: VerifyErrorKind::RegisterOutOfRange {
                        role: RegisterRole::LeftOperand,
                        register: 8
                    }
                },
                VerifyError {
                    function: Some(0),
                    instruction: Some(0),
                    kind: VerifyErrorKind::RegisterOutOfRange {
                        role: RegisterRole::RightOperand,
                        register: 7
                    }
                },
            ]
        );
    }

    #[test]
    fn verifier_diagnostic_registry_covers_every_error_kind() {
        let cases = [
            (
                VerifyErrorKind::EntryOutOfRange,
                codes::E2000_ENTRY_OUT_OF_RANGE,
            ),
            (VerifyErrorKind::RegisterLimit, codes::E2001_REGISTER_LIMIT),
            (
                VerifyErrorKind::ArityExceedsRegisters,
                codes::E2002_ARITY_EXCEEDS_REGISTERS,
            ),
            (VerifyErrorKind::EmptyFunction, codes::E2003_EMPTY_FUNCTION),
            (
                VerifyErrorKind::DebugInfoMismatch,
                codes::E2004_DEBUG_INFO_MISMATCH,
            ),
            (
                VerifyErrorKind::RegisterOutOfRange {
                    role: RegisterRole::Destination,
                    register: 1,
                },
                codes::E2005_REGISTER_OUT_OF_RANGE,
            ),
            (
                VerifyErrorKind::JumpTargetOutOfRange,
                codes::E2006_JUMP_TARGET_OUT_OF_RANGE,
            ),
            (
                VerifyErrorKind::MissingTerminator,
                codes::E2007_MISSING_TERMINATOR,
            ),
            (VerifyErrorKind::TooManyErrors, codes::E2099_TOO_MANY_ERRORS),
        ];

        assert_eq!(codes::all_codes().len(), cases.len());
        for (kind, code) in cases {
            assert_eq!(kind.code(), code);
            assert!(codes::all_codes().contains(&code));
            let diagnostic = VerifyError {
                function: Some(2),
                instruction: Some(4),
                kind,
            }
            .to_diagnostic(Span::new(10, 11));
            assert_eq!(diagnostic.code, code);
            assert_eq!(diagnostic.kind, DiagnosticKind::Bytecode);
            assert_eq!(diagnostic.span, Span::new(10, 11));
            assert_eq!(
                diagnostic.message,
                format!("function 2 instruction 4: {kind}")
            );
        }
    }

    #[test]
    fn missing_entry_has_no_fake_function_index() {
        let p = Program {
            functions: vec![],
            entry: 0,
        };
        let error = verify(&p).unwrap_err()[0];
        assert_eq!(error.function, None);
        assert_eq!(error.kind, VerifyErrorKind::EntryOutOfRange);
    }
}
