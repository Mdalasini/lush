//! Resumable single-process interpreter for verified Lush bytecode.

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panic {
    pub kind: PanicKind,
    pub message: String,
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
            match instruction {
                Instruction::Move { dst, src } => {
                    self.registers[dst as usize] = self.registers[src as usize].clone();
                }
                Instruction::LoadInt { dst, value } => {
                    self.registers[dst as usize] = Value::Int(value);
                }
                Instruction::LoadBool { dst, value } => {
                    self.registers[dst as usize] = Value::Bool(value);
                }
                Instruction::AddInt { dst, lhs, rhs }
                | Instruction::SubInt { dst, lhs, rhs }
                | Instruction::MulInt { dst, lhs, rhs }
                | Instruction::DivInt { dst, lhs, rhs }
                | Instruction::RemInt { dst, lhs, rhs } => {
                    let result = self.read_int(lhs).and_then(|a| {
                        self.read_int(rhs)
                            .and_then(|b| checked_binary(instruction, a, b))
                    });
                    match result {
                        Ok(value) => self.registers[dst as usize] = Value::Int(value),
                        Err(message) => return self.fail(pc, message),
                    }
                }
                Instruction::EqInt { dst, lhs, rhs } | Instruction::LtInt { dst, lhs, rhs } => {
                    let result = self.read_int(lhs).and_then(|a| {
                        self.read_int(rhs).map(|b| match instruction {
                            Instruction::EqInt { .. } => a == b,
                            Instruction::LtInt { .. } => a < b,
                            _ => unreachable!(),
                        })
                    });
                    match result {
                        Ok(value) => self.registers[dst as usize] = Value::Bool(value),
                        Err(message) => return self.fail(pc, message),
                    }
                }
                Instruction::Jump { target } => self.pc = target as usize,
                Instruction::JumpIfFalse { condition, target } => {
                    match self.registers[condition as usize] {
                        Value::Bool(false) => self.pc = target as usize,
                        Value::Bool(true) => {}
                        _ => return self.fail(pc, PanicKind::ExpectedBool),
                    }
                }
                Instruction::Return { src } => {
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
            message: kind.message().into(),
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

fn checked_binary(instruction: Instruction, lhs: i64, rhs: i64) -> Result<i64, PanicKind> {
    match instruction {
        Instruction::AddInt { .. } => lhs.checked_add(rhs).ok_or(PanicKind::IntegerOverflow),
        Instruction::SubInt { .. } => lhs.checked_sub(rhs).ok_or(PanicKind::IntegerOverflow),
        Instruction::MulInt { .. } => lhs.checked_mul(rhs).ok_or(PanicKind::IntegerOverflow),
        Instruction::DivInt { .. } | Instruction::RemInt { .. } => {
            if rhs == 0 {
                Err(PanicKind::DivisionByZero)
            } else if lhs == i64::MIN && rhs == -1 {
                Err(PanicKind::IntegerOverflow)
            } else if matches!(instruction, Instruction::DivInt { .. }) {
                Ok(lhs / rhs)
            } else {
                Ok(lhs % rhs)
            }
        }
        _ => unreachable!("checked_binary is only called for integer arithmetic"),
    }
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
                Exit::Panicked(panic_value("integer overflow")),
            ),
            (
                Instruction::MulInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                i64::MIN,
                -1,
                Exit::Panicked(panic_value("integer overflow")),
            ),
            (
                Instruction::DivInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                7,
                0,
                Exit::Panicked(panic_value("division by zero")),
            ),
            (
                Instruction::DivInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                i64::MIN,
                -1,
                Exit::Panicked(panic_value("integer overflow")),
            ),
            (
                Instruction::RemInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                i64::MIN,
                -1,
                Exit::Panicked(panic_value("integer overflow")),
            ),
        ];
        for (instruction, lhs, rhs, expected) in cases {
            let actual = run_binary(instruction, lhs, rhs);
            match (actual, expected) {
                (Exit::Panicked(actual), Exit::Panicked(expected)) => {
                    assert_eq!(actual.message, expected.message)
                }
                (actual, expected) => assert_eq!(actual, expected),
            }
        }
    }

    fn panic_value(message: &str) -> Panic {
        let kind = match message {
            "division by zero" => PanicKind::DivisionByZero,
            _ => PanicKind::IntegerOverflow,
        };
        Panic {
            kind,
            message: message.into(),
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
        assert_eq!(panic.kind, PanicKind::IntegerOverflow);
        assert_eq!(panic.message, "integer overflow");
        assert_eq!(panic.location, SourceLoc { line: 1, column: 3 });
    }
}
