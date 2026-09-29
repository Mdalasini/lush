//! Resumable single-process interpreter for verified Lush bytecode.

use lush_ir::{verify, Instruction, Program, SourceLoc, VerifyError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Nil,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panic {
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
    InvalidEntry,
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
        let entry = program
            .functions
            .get(function)
            .ok_or(VmError::InvalidEntry)?;
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
            let function = &self.program.functions[self.function];
            let instruction = function.code[self.pc].clone();
            let location = function
                .locations
                .get(self.pc)
                .copied()
                .unwrap_or(SourceLoc { line: 0, column: 0 });
            let name = function.name.clone();
            self.pc += 1;
            self.counters.reductions += 1;
            self.counters.instructions += 1;
            spent += 1;
            let panic = |message: String| Panic {
                message,
                location,
                function: name,
            };
            let read_int = |registers: &[Value], r: u8| -> Result<i64, String> {
                match registers.get(r as usize) {
                    Some(Value::Int(n)) => Ok(*n),
                    Some(_) => Err("expected Int".into()),
                    None => Err("invalid register".into()),
                }
            };
            match instruction {
                Instruction::Move { dst, src } => {
                    self.registers[dst as usize] = self.registers[src as usize].clone()
                }
                Instruction::LoadInt { dst, value } => {
                    self.registers[dst as usize] = Value::Int(value)
                }
                Instruction::AddInt { dst, lhs, rhs } => {
                    let result = read_int(&self.registers, lhs).and_then(|a| {
                        read_int(&self.registers, rhs)
                            .and_then(|b| a.checked_add(b).ok_or_else(|| "integer overflow".into()))
                    });
                    match result {
                        Ok(n) => self.registers[dst as usize] = Value::Int(n),
                        Err(m) => return self.finish(Exit::Panicked(panic(m))),
                    }
                }
                Instruction::SubInt { dst, lhs, rhs } => {
                    let result = read_int(&self.registers, lhs).and_then(|a| {
                        read_int(&self.registers, rhs)
                            .and_then(|b| a.checked_sub(b).ok_or_else(|| "integer overflow".into()))
                    });
                    match result {
                        Ok(n) => self.registers[dst as usize] = Value::Int(n),
                        Err(m) => return self.finish(Exit::Panicked(panic(m))),
                    }
                }
                Instruction::MulInt { dst, lhs, rhs } => {
                    let result = read_int(&self.registers, lhs).and_then(|a| {
                        read_int(&self.registers, rhs)
                            .and_then(|b| a.checked_mul(b).ok_or_else(|| "integer overflow".into()))
                    });
                    match result {
                        Ok(n) => self.registers[dst as usize] = Value::Int(n),
                        Err(m) => return self.finish(Exit::Panicked(panic(m))),
                    }
                }
                Instruction::DivInt { dst, lhs, rhs } | Instruction::RemInt { dst, lhs, rhs } => {
                    let is_rem = matches!(instruction, Instruction::RemInt { .. });
                    let result = read_int(&self.registers, lhs).and_then(|a| {
                        read_int(&self.registers, rhs).and_then(|b| {
                            if b == 0 {
                                Err("division by zero".into())
                            } else if a == i64::MIN && b == -1 {
                                Err("integer overflow".into())
                            } else if is_rem {
                                Ok(a % b)
                            } else {
                                Ok(a / b)
                            }
                        })
                    });
                    match result {
                        Ok(n) => self.registers[dst as usize] = Value::Int(n),
                        Err(m) => return self.finish(Exit::Panicked(panic(m))),
                    }
                }
                Instruction::Jump { target } => self.pc = target as usize,
                Instruction::JumpIfFalse { condition, target } => match self.registers
                    [condition as usize]
                {
                    Value::Bool(false) => self.pc = target as usize,
                    Value::Bool(true) => {}
                    _ => {
                        return self.finish(Exit::Panicked(panic("expected Bool condition".into())))
                    }
                },
                Instruction::Return { src } => {
                    return self.finish(Exit::Returned(self.registers[src as usize].clone()))
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

    fn finish(&mut self, exit: Exit) -> SliceResult {
        self.done = Some(exit.clone());
        SliceResult::Done(exit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lush_ir::{Function, Program};

    fn program(code: Vec<Instruction>) -> Program {
        let locations = vec![SourceLoc { line: 1, column: 1 }; code.len()];
        Program {
            functions: vec![Function {
                name: "main".into(),
                arity: 0,
                register_count: 2,
                code,
                locations,
            }],
            entry: 0,
        }
    }

    #[test]
    fn quantum_yields_and_resume_returns_value() {
        let p = program(vec![
            Instruction::LoadInt { dst: 0, value: 40 },
            Instruction::LoadInt { dst: 1, value: 2 },
            Instruction::AddInt {
                dst: 0,
                lhs: 0,
                rhs: 1,
            },
            Instruction::Return { src: 0 },
        ]);
        let mut vm = Vm::new(p, &[]).unwrap();
        assert_eq!(vm.run_slice(1), SliceResult::Yielded);
        assert_eq!(vm.counters().reductions, 1);
        assert_eq!(vm.run_to_completion(), Exit::Returned(Value::Int(42)));
        assert_eq!(vm.counters().reductions, 4);
    }

    #[test]
    fn checked_overflow_panics_at_instruction_location() {
        let p = program(vec![
            Instruction::LoadInt {
                dst: 0,
                value: i64::MAX,
            },
            Instruction::LoadInt { dst: 1, value: 1 },
            Instruction::AddInt {
                dst: 0,
                lhs: 0,
                rhs: 1,
            },
            Instruction::Return { src: 0 },
        ]);
        let mut vm = Vm::new(p, &[]).unwrap();
        let Exit::Panicked(panic) = vm.run_to_completion() else {
            panic!("expected panic")
        };
        assert_eq!(panic.message, "integer overflow");
        assert_eq!(panic.location, SourceLoc { line: 1, column: 1 });
    }
}
