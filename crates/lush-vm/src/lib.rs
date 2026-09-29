//! Resumable single-process interpreter for verified Lush bytecode.

use std::fmt;

use lush_ir::{verify, Instruction, Program, SourceLoc, VerifyError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Nil,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanicKind {
    ExpectedInt,
    ExpectedBool,
    IntegerOverflow,
    DivisionByZero,
}

impl PanicKind {
    pub const fn message(self) -> &'static str {
        match self {
            Self::ExpectedInt => "expected Int",
            Self::ExpectedBool => "expected Bool condition",
            Self::IntegerOverflow => "integer overflow",
            Self::DivisionByZero => "division by zero",
        }
    }
}

impl fmt::Display for PanicKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panic {
    pub kind: PanicKind,
    pub location: SourceLoc,
    pub function: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exit {
    Returned(Value),
    Panicked(Panic),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SliceResult {
    Yielded,
    Done(Exit),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VmError {
    InvalidProgram(Vec<VerifyError>),
    InvalidArgumentCount { expected: usize, actual: usize },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub reductions: u64,
    pub instructions: u64,
    pub maximum_frame_depth: usize,
}

pub struct Vm {
    program: Program,
    function: usize,
    pc: usize,
    registers: Vec<Value>,
    done: Option<Exit>,
    counters: Counters,
}

impl Vm {
    pub fn new(program: Program, arguments: &[Value]) -> Result<Self, VmError> {
        verify(&program).map_err(VmError::InvalidProgram)?;
        let function = program.entry as usize;
        let entry = &program.functions[function];
        if arguments.len() != entry.arity as usize {
            return Err(VmError::InvalidArgumentCount {
                expected: entry.arity as usize,
                actual: arguments.len(),
            });
        }
        let mut registers = vec![Value::Nil; entry.register_count as usize];
        registers[..arguments.len()].clone_from_slice(arguments);
        Ok(Self {
            program,
            function,
            pc: 0,
            registers,
            done: None,
            counters: Counters {
                maximum_frame_depth: 1,
                ..Counters::default()
            },
        })
    }

    pub fn counters(&self) -> &Counters {
        &self.counters
    }

    /// Executes at most `quantum` instructions. A zero quantum yields without work.
    pub fn run_slice(&mut self, quantum: u64) -> SliceResult {
        if let Some(done) = &self.done {
            return SliceResult::Done(done.clone());
        }
        if quantum == 0 {
            return SliceResult::Yielded;
        }
        let mut spent = 0;
        while spent < quantum {
            let pc = self.pc;
            let instruction = self.program.functions[self.function].code[pc];
            self.pc += 1;
            self.counters.reductions += 1;
            self.counters.instructions += 1;
            spent += 1;
            match decode_instruction(instruction) {
                DecodedInstruction::Integer { op, dst, lhs, rhs } => {
                    if let Err(kind) = self.execute_int_op(dst, lhs, rhs, op) {
                        return self.fail(pc, kind);
                    }
                }
                DecodedInstruction::Move { dst, src } => {
                    self.registers[dst as usize] = self.registers[src as usize].clone();
                }
                DecodedInstruction::LoadInt { dst, value } => {
                    self.registers[dst as usize] = Value::Int(value);
                }
                DecodedInstruction::LoadBool { dst, value } => {
                    self.registers[dst as usize] = Value::Bool(value);
                }
                DecodedInstruction::Jump { target } => self.pc = target as usize,
                DecodedInstruction::JumpIfFalse { condition, target } => {
                    match self.registers[condition as usize] {
                        Value::Bool(false) => self.pc = target as usize,
                        Value::Bool(true) => {}
                        _ => return self.fail(pc, PanicKind::ExpectedBool),
                    }
                }
                DecodedInstruction::Return { src } => {
                    return self.finish(Exit::Returned(self.registers[src as usize].clone()));
                }
            }
        }
        SliceResult::Yielded
    }

    pub fn run_to_completion(&mut self) -> Exit {
        loop {
            if let SliceResult::Done(exit) = self.run_slice(u64::MAX) {
                return exit;
            }
        }
    }

    fn execute_int_op(&mut self, dst: u8, lhs: u8, rhs: u8, op: IntOp) -> Result<(), PanicKind> {
        let left = self.read_int(lhs)?;
        let right = self.read_int(rhs)?;
        self.registers[dst as usize] = eval_int_op(op, left, right)?;
        Ok(())
    }

    fn read_int(&self, register: u8) -> Result<i64, PanicKind> {
        match &self.registers[register as usize] {
            Value::Int(n) => Ok(*n),
            _ => Err(PanicKind::ExpectedInt),
        }
    }

    fn fail(&mut self, pc: usize, kind: PanicKind) -> SliceResult {
        let function = &self.program.functions[self.function];
        let panic = Panic {
            kind,
            location: function.locations[pc],
            function: function.name.clone(),
        };
        self.finish(Exit::Panicked(panic))
    }

    fn finish(&mut self, exit: Exit) -> SliceResult {
        self.done = Some(exit.clone());
        SliceResult::Done(exit)
    }
}

#[derive(Clone, Copy)]
enum IntOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Lt,
}

enum DecodedInstruction {
    Integer {
        op: IntOp,
        dst: u8,
        lhs: u8,
        rhs: u8,
    },
    Move {
        dst: u8,
        src: u8,
    },
    LoadInt {
        dst: u8,
        value: i64,
    },
    LoadBool {
        dst: u8,
        value: bool,
    },
    Jump {
        target: u32,
    },
    JumpIfFalse {
        condition: u8,
        target: u32,
    },
    Return {
        src: u8,
    },
}

fn decode_instruction(instruction: Instruction) -> DecodedInstruction {
    match instruction {
        Instruction::AddInt { dst, lhs, rhs } => DecodedInstruction::Integer {
            op: IntOp::Add,
            dst,
            lhs,
            rhs,
        },
        Instruction::SubInt { dst, lhs, rhs } => DecodedInstruction::Integer {
            op: IntOp::Sub,
            dst,
            lhs,
            rhs,
        },
        Instruction::MulInt { dst, lhs, rhs } => DecodedInstruction::Integer {
            op: IntOp::Mul,
            dst,
            lhs,
            rhs,
        },
        Instruction::DivInt { dst, lhs, rhs } => DecodedInstruction::Integer {
            op: IntOp::Div,
            dst,
            lhs,
            rhs,
        },
        Instruction::RemInt { dst, lhs, rhs } => DecodedInstruction::Integer {
            op: IntOp::Rem,
            dst,
            lhs,
            rhs,
        },
        Instruction::EqInt { dst, lhs, rhs } => DecodedInstruction::Integer {
            op: IntOp::Eq,
            dst,
            lhs,
            rhs,
        },
        Instruction::LtInt { dst, lhs, rhs } => DecodedInstruction::Integer {
            op: IntOp::Lt,
            dst,
            lhs,
            rhs,
        },
        Instruction::Move { dst, src } => DecodedInstruction::Move { dst, src },
        Instruction::LoadInt { dst, value } => DecodedInstruction::LoadInt { dst, value },
        Instruction::LoadBool { dst, value } => DecodedInstruction::LoadBool { dst, value },
        Instruction::Jump { target } => DecodedInstruction::Jump { target },
        Instruction::JumpIfFalse { condition, target } => {
            DecodedInstruction::JumpIfFalse { condition, target }
        }
        Instruction::Return { src } => DecodedInstruction::Return { src },
    }
}

fn eval_int_op(op: IntOp, lhs: i64, rhs: i64) -> Result<Value, PanicKind> {
    let value = match op {
        IntOp::Add => Value::Int(lhs.checked_add(rhs).ok_or(PanicKind::IntegerOverflow)?),
        IntOp::Sub => Value::Int(lhs.checked_sub(rhs).ok_or(PanicKind::IntegerOverflow)?),
        IntOp::Mul => Value::Int(lhs.checked_mul(rhs).ok_or(PanicKind::IntegerOverflow)?),
        IntOp::Div | IntOp::Rem => {
            if rhs == 0 {
                return Err(PanicKind::DivisionByZero);
            }
            if lhs == i64::MIN && rhs == -1 {
                return Err(PanicKind::IntegerOverflow);
            }
            Value::Int(if matches!(op, IntOp::Div) {
                lhs / rhs
            } else {
                lhs % rhs
            })
        }
        IntOp::Eq => Value::Bool(lhs == rhs),
        IntOp::Lt => Value::Bool(lhs < rhs),
    };
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lush_ir::{Function, Program};

    fn program(code: Vec<Instruction>, arity: u8) -> Program {
        let locations = (0..code.len())
            .map(|pc| SourceLoc {
                line: 1,
                column: pc as u32 + 1,
            })
            .collect();
        Program {
            functions: vec![Function {
                name: "main".into(),
                arity,
                register_count: 4,
                code,
                locations,
            }],
            entry: 0,
        }
    }

    fn run_binary(op: Instruction, lhs: i64, rhs: i64) -> Exit {
        let (dst, left, right) = match op {
            Instruction::AddInt { dst, lhs, rhs }
            | Instruction::SubInt { dst, lhs, rhs }
            | Instruction::MulInt { dst, lhs, rhs }
            | Instruction::DivInt { dst, lhs, rhs }
            | Instruction::RemInt { dst, lhs, rhs } => (dst, lhs, rhs),
            _ => panic!("expected binary arithmetic instruction"),
        };
        let code = vec![
            Instruction::LoadInt {
                dst: left,
                value: lhs,
            },
            Instruction::LoadInt {
                dst: right,
                value: rhs,
            },
            op,
            Instruction::Return { src: dst },
        ];
        Vm::new(program(code, 0), &[]).unwrap().run_to_completion()
    }

    #[test]
    fn quantum_yields_and_resume_returns_value() {
        let p = program(
            vec![
                Instruction::LoadInt { dst: 0, value: 40 },
                Instruction::LoadInt { dst: 1, value: 2 },
                Instruction::AddInt {
                    dst: 0,
                    lhs: 0,
                    rhs: 1,
                },
                Instruction::Return { src: 0 },
            ],
            0,
        );
        let mut vm = Vm::new(p, &[]).unwrap();
        assert_eq!(vm.run_slice(1), SliceResult::Yielded);
        assert_eq!(vm.counters().reductions, 1);
        assert_eq!(vm.run_to_completion(), Exit::Returned(Value::Int(42)));
        assert_eq!(vm.counters().reductions, 4);
    }

    #[test]
    fn arithmetic_uses_checked_integer_semantics() {
        let cases = [
            (
                Instruction::SubInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                9,
                4,
                Exit::Returned(Value::Int(5)),
            ),
            (
                Instruction::MulInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                6,
                -7,
                Exit::Returned(Value::Int(-42)),
            ),
            (
                Instruction::DivInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                -7,
                2,
                Exit::Returned(Value::Int(-3)),
            ),
            (
                Instruction::RemInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                -7,
                2,
                Exit::Returned(Value::Int(-1)),
            ),
            (
                Instruction::DivInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                7,
                -2,
                Exit::Returned(Value::Int(-3)),
            ),
            (
                Instruction::RemInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                7,
                -2,
                Exit::Returned(Value::Int(1)),
            ),
            (
                Instruction::SubInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                i64::MIN,
                1,
                Exit::Panicked(panic_value(PanicKind::IntegerOverflow)),
            ),
            (
                Instruction::MulInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                i64::MIN,
                -1,
                Exit::Panicked(panic_value(PanicKind::IntegerOverflow)),
            ),
            (
                Instruction::DivInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                7,
                0,
                Exit::Panicked(panic_value(PanicKind::DivisionByZero)),
            ),
            (
                Instruction::DivInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                i64::MIN,
                -1,
                Exit::Panicked(panic_value(PanicKind::IntegerOverflow)),
            ),
            (
                Instruction::RemInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                i64::MIN,
                -1,
                Exit::Panicked(panic_value(PanicKind::IntegerOverflow)),
            ),
        ];
        for (instruction, lhs, rhs, expected) in cases {
            assert_eq!(run_binary(instruction, lhs, rhs), expected);
        }
    }

    fn panic_value(kind: PanicKind) -> Panic {
        Panic {
            kind,
            location: SourceLoc { line: 1, column: 3 },
            function: "main".into(),
        }
    }

    #[test]
    fn comparison_and_conditional_jump_cover_both_branches() {
        let branch = |condition| {
            program(
                vec![
                    Instruction::LoadBool {
                        dst: 0,
                        value: condition,
                    },
                    Instruction::JumpIfFalse {
                        condition: 0,
                        target: 3,
                    },
                    Instruction::LoadInt { dst: 1, value: 10 },
                    Instruction::Return { src: 1 },
                ],
                0,
            )
        };
        assert_eq!(
            Vm::new(branch(true), &[]).unwrap().run_to_completion(),
            Exit::Returned(Value::Int(10))
        );
        assert_eq!(
            Vm::new(branch(false), &[]).unwrap().run_to_completion(),
            Exit::Returned(Value::Nil)
        );

        let compare = program(
            vec![
                Instruction::LoadInt { dst: 0, value: 2 },
                Instruction::LoadInt { dst: 1, value: 3 },
                Instruction::LtInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                Instruction::Return { src: 2 },
            ],
            0,
        );
        assert_eq!(
            Vm::new(compare, &[]).unwrap().run_to_completion(),
            Exit::Returned(Value::Bool(true))
        );
        let compare_false = program(
            vec![
                Instruction::LoadInt { dst: 0, value: 3 },
                Instruction::LoadInt { dst: 1, value: 2 },
                Instruction::LtInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                Instruction::Return { src: 2 },
            ],
            0,
        );
        assert_eq!(
            Vm::new(compare_false, &[]).unwrap().run_to_completion(),
            Exit::Returned(Value::Bool(false))
        );
        let equal = program(
            vec![
                Instruction::LoadInt { dst: 0, value: 3 },
                Instruction::LoadInt { dst: 1, value: 3 },
                Instruction::EqInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                Instruction::Return { src: 2 },
            ],
            0,
        );
        assert_eq!(
            Vm::new(equal, &[]).unwrap().run_to_completion(),
            Exit::Returned(Value::Bool(true))
        );
        let not_equal = program(
            vec![
                Instruction::LoadInt { dst: 0, value: 3 },
                Instruction::LoadInt { dst: 1, value: 2 },
                Instruction::EqInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                Instruction::Return { src: 2 },
            ],
            0,
        );
        assert_eq!(
            Vm::new(not_equal, &[]).unwrap().run_to_completion(),
            Exit::Returned(Value::Bool(false))
        );
    }

    #[test]
    fn jump_loop_resumes_without_repeating_work() {
        let p = program(
            vec![
                Instruction::LoadInt { dst: 0, value: 0 },
                Instruction::LoadInt { dst: 1, value: 3 },
                Instruction::LoadInt { dst: 2, value: 1 },
                Instruction::LtInt {
                    dst: 3,
                    lhs: 0,
                    rhs: 1,
                },
                Instruction::JumpIfFalse {
                    condition: 3,
                    target: 7,
                },
                Instruction::AddInt {
                    dst: 0,
                    lhs: 0,
                    rhs: 2,
                },
                Instruction::Jump { target: 3 },
                Instruction::Return { src: 0 },
            ],
            0,
        );
        let mut vm = Vm::new(p, &[]).unwrap();
        while vm.run_slice(2) == SliceResult::Yielded {}
        assert_eq!(vm.run_to_completion(), Exit::Returned(Value::Int(3)));
        assert_eq!(vm.counters().reductions, 18);
    }

    #[test]
    fn arguments_are_installed_in_parameter_registers() {
        let p = program(
            vec![
                Instruction::Move { dst: 1, src: 0 },
                Instruction::Return { src: 1 },
            ],
            1,
        );
        assert_eq!(
            Vm::new(p, &[Value::Int(17)]).unwrap().run_to_completion(),
            Exit::Returned(Value::Int(17))
        );
    }

    #[test]
    fn arithmetic_panic_uses_the_operator_location() {
        let p = program(
            vec![
                Instruction::LoadInt {
                    dst: 0,
                    value: i64::MAX,
                },
                Instruction::LoadInt { dst: 1, value: 1 },
                Instruction::AddInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                Instruction::Return { src: 2 },
            ],
            0,
        );
        let Exit::Panicked(panic) = Vm::new(p, &[]).unwrap().run_to_completion() else {
            panic!("expected panic")
        };
        assert_eq!(
            panic,
            Panic {
                kind: PanicKind::IntegerOverflow,
                location: SourceLoc { line: 1, column: 3 },
                function: "main".into(),
            }
        );
    }

    #[test]
    fn invalid_bool_condition_reports_expected_bool_at_instruction() {
        let p = program(
            vec![
                Instruction::LoadInt { dst: 0, value: 1 },
                Instruction::JumpIfFalse {
                    condition: 0,
                    target: 2,
                },
                Instruction::Return { src: 0 },
            ],
            0,
        );
        let Exit::Panicked(panic) = Vm::new(p, &[]).unwrap().run_to_completion() else {
            panic!("expected panic")
        };
        assert_eq!(panic.kind, PanicKind::ExpectedBool);
        assert_eq!(panic.location, SourceLoc { line: 1, column: 2 });
    }

    #[test]
    fn non_integer_arithmetic_reports_expected_int() {
        let p = program(
            vec![
                Instruction::AddInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                Instruction::Return { src: 2 },
            ],
            0,
        );
        let Exit::Panicked(panic) = Vm::new(p, &[]).unwrap().run_to_completion() else {
            panic!("expected panic")
        };
        assert_eq!(panic.kind, PanicKind::ExpectedInt);
        assert_eq!(panic.location, SourceLoc { line: 1, column: 1 });

        let comparison = program(
            vec![
                Instruction::LtInt {
                    dst: 1,
                    lhs: 0,
                    rhs: 0,
                },
                Instruction::Return { src: 1 },
            ],
            1,
        );
        let Exit::Panicked(panic) = Vm::new(comparison, &[Value::Bool(true)])
            .unwrap()
            .run_to_completion()
        else {
            panic!("expected panic")
        };
        assert_eq!(panic.kind, PanicKind::ExpectedInt);
    }

    #[test]
    fn vm_checks_argument_count_and_exposes_verified_program_errors() {
        let arity_one = program(vec![Instruction::Return { src: 0 }], 1);
        assert!(matches!(
            Vm::new(arity_one, &[]),
            Err(VmError::InvalidArgumentCount {
                expected: 1,
                actual: 0
            })
        ));

        let invalid = program(vec![Instruction::Return { src: 9 }], 0);
        assert!(matches!(
            Vm::new(invalid, &[]),
            Err(VmError::InvalidProgram(_))
        ));
    }

    #[test]
    fn zero_quantum_and_resume_after_done_are_stable() {
        let p = program(
            vec![
                Instruction::LoadInt { dst: 0, value: 8 },
                Instruction::Return { src: 0 },
            ],
            0,
        );
        let mut vm = Vm::new(p, &[]).unwrap();
        assert_eq!(vm.run_slice(0), SliceResult::Yielded);
        assert_eq!(vm.counters().reductions, 0);
        let done = vm.run_to_completion();
        assert_eq!(done, Exit::Returned(Value::Int(8)));
        assert_eq!(vm.run_slice(1), SliceResult::Done(done));
    }
}
