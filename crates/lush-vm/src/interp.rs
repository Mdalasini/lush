//! Single-process bytecode interpreter with proper TailCall and reductions.

use lush_ir::bytecode::{Constant, Op, Program};
use lush_types::numeric;

use crate::arena::Arena;
use crate::builtin;
use crate::panic_report::{self, FrameInfo};
use crate::value::Value;

pub const DEFAULT_QUANTUM: u64 = 4000;
pub const MAX_STACK_FRAMES: usize = 1_000_000;

#[derive(Clone, Debug)]
pub struct VmConfig {
    pub max_stack_frames: usize,
    pub default_quantum: u64,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            max_stack_frames: MAX_STACK_FRAMES,
            default_quantum: DEFAULT_QUANTUM,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunResult {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub reductions: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SliceResult {
    Yielded,
    Done(RunResult),
}

struct Frame {
    func: usize,
    pc: usize,
    regs: Vec<Value>,
    ret_dst: u8,
    is_entry: bool,
}

pub struct Vm {
    program: Program,
    config: VmConfig,
    arena: Arena,
    frames: Vec<Frame>,
    pub reductions: u64,
    pub instructions: u64,
    pub max_frame_depth: usize,
    pub had_tail_call: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    done: Option<i32>,
}

impl Vm {
    pub fn new(program: Program, config: VmConfig) -> Self {
        let entry = program.entry as usize;
        let f = &program.functions[entry];
        let regs = vec![Value::nil(); f.regs.max(1) as usize];
        Self {
            program,
            config,
            arena: Arena::new(),
            frames: vec![Frame {
                func: entry,
                pc: 0,
                regs,
                ret_dst: 0,
                is_entry: true,
            }],
            reductions: 0,
            instructions: 0,
            max_frame_depth: 1,
            had_tail_call: false,
            stdout: Vec::new(),
            stderr: Vec::new(),
            done: None,
        }
    }

    pub fn stdout_bytes(&self) -> &[u8] {
        &self.stdout
    }
    pub fn stderr_bytes(&self) -> &[u8] {
        &self.stderr
    }

    pub fn run_to_completion(&mut self) -> RunResult {
        loop {
            match self.run_slice(u64::MAX) {
                SliceResult::Yielded => continue,
                SliceResult::Done(r) => return r,
            }
        }
    }

    pub fn run_slice(&mut self, quantum: u64) -> SliceResult {
        if let Some(status) = self.done {
            return SliceResult::Done(self.result(status));
        }
        let mut left = quantum;
        while left > 0 {
            if self.frames.is_empty() {
                self.done = Some(0);
                return SliceResult::Done(self.result(0));
            }
            match self.step() {
                Ok(charged) => {
                    left = left.saturating_sub(charged.max(1));
                    if let Some(status) = self.done {
                        return SliceResult::Done(self.result(status));
                    }
                }
                Err(msg) => {
                    self.write_panic(&msg);
                    self.done = Some(1);
                    return SliceResult::Done(self.result(1));
                }
            }
        }
        SliceResult::Yielded
    }

    fn result(&self, status: i32) -> RunResult {
        RunResult {
            status,
            stdout: self.stdout.clone(),
            stderr: self.stderr.clone(),
            reductions: self.reductions,
        }
    }

    fn step(&mut self) -> Result<u64, String> {
        let fi = self.frames.len() - 1;
        let func_idx = self.frames[fi].func;
        let pc = self.frames[fi].pc;
        let fun = &self.program.functions[func_idx];
        if pc >= fun.code.len() {
            return Err("pc out of range".into());
        }
        let op = fun.code[pc].clone();
        self.frames[fi].pc = pc + 1;
        self.instructions += 1;
        let mut charge = 1u64;

        match op {
            Op::Move { dst, src } => {
                let v = self.reg(fi, src);
                self.set_reg(fi, dst, v);
            }
            Op::LoadConst { dst, idx } => {
                let v = self.load_const(idx)?;
                self.set_reg(fi, dst, v);
            }
            Op::LoadInt { dst, value } => {
                let v = self.arena.canonicalize_int(value);
                self.set_reg(fi, dst, v);
            }
            Op::LoadBool { dst, value } => self.set_reg(fi, dst, Value::from_bool(value)),
            Op::LoadNil { dst } => self.set_reg(fi, dst, Value::nil()),
            Op::Add { dst, a, b } => {
                let r = self.binop_num(self.reg(fi, a), self.reg(fi, b), numeric::int_add, numeric::float_add)?;
                self.set_reg(fi, dst, r);
            }
            Op::Sub { dst, a, b } => {
                let r = self.binop_num(self.reg(fi, a), self.reg(fi, b), numeric::int_sub, numeric::float_sub)?;
                self.set_reg(fi, dst, r);
            }
            Op::Mul { dst, a, b } => {
                let r = self.binop_num(self.reg(fi, a), self.reg(fi, b), numeric::int_mul, numeric::float_mul)?;
                self.set_reg(fi, dst, r);
            }
            Op::Div { dst, a, b } => {
                let r = self.binop_num(self.reg(fi, a), self.reg(fi, b), numeric::int_div, numeric::float_div)?;
                self.set_reg(fi, dst, r);
            }
            Op::Rem { dst, a, b } => {
                let ai = self.arena.read_int(self.reg(fi, a)).ok_or("rem expects Int")?;
                let bi = self.arena.read_int(self.reg(fi, b)).ok_or("rem expects Int")?;
                let n = numeric::int_rem(ai, bi).map_err(|e| e.to_string())?;
                self.set_reg(fi, dst, self.arena.canonicalize_int(n));
            }
            Op::Neg { dst, src } => {
                let v = self.reg(fi, src);
                if let Some(n) = self.arena.read_int(v) {
                    let n = numeric::int_neg(n).map_err(|e| e.to_string())?;
                    self.set_reg(fi, dst, self.arena.canonicalize_int(n));
                } else if let Some(f) = self.arena.read_float(v) {
                    self.set_reg(fi, dst, self.arena.alloc_float(-f));
                } else {
                    return Err("negation on non-numeric".into());
                }
            }
            Op::Concat { dst, a, b } => {
                let sa = self.arena.read_string(self.reg(fi, a)).ok_or("concat expects String")?.to_vec();
                let sb = self.arena.read_string(self.reg(fi, b)).ok_or("concat expects String")?.to_vec();
                let mut out = sa;
                out.extend_from_slice(&sb);
                charge += out.len() as u64 / 8 + 1;
                self.set_reg(fi, dst, self.arena.alloc_string(out));
            }
            Op::Eq { dst, a, b } => {
                let r = self.eq_values(self.reg(fi, a), self.reg(fi, b), &mut charge)?;
                self.set_reg(fi, dst, Value::from_bool(r));
            }
            Op::Ne { dst, a, b } => {
                let r = self.eq_values(self.reg(fi, a), self.reg(fi, b), &mut charge)?;
                self.set_reg(fi, dst, Value::from_bool(!r));
            }
            Op::Lt { dst, a, b } => {
                let r = self.cmp_int(self.reg(fi, a), self.reg(fi, b), |x, y| x < y)?;
                self.set_reg(fi, dst, Value::from_bool(r));
            }
            Op::Le { dst, a, b } => {
                let r = self.cmp_int(self.reg(fi, a), self.reg(fi, b), |x, y| x <= y)?;
                self.set_reg(fi, dst, Value::from_bool(r));
            }
            Op::Gt { dst, a, b } => {
                let r = self.cmp_int(self.reg(fi, a), self.reg(fi, b), |x, y| x > y)?;
                self.set_reg(fi, dst, Value::from_bool(r));
            }
            Op::Ge { dst, a, b } => {
                let r = self.cmp_int(self.reg(fi, a), self.reg(fi, b), |x, y| x >= y)?;
                self.set_reg(fi, dst, Value::from_bool(r));
            }
            Op::Jump { target } => {
                self.frames[fi].pc = target as usize;
            }
            Op::JumpIfFalse { cond, target } => {
                if self.reg(fi, cond).is_falsey() {
                    self.frames[fi].pc = target as usize;
                }
            }
            Op::IsInt { dst, src, value } => {
                let ok = self.arena.read_int(self.reg(fi, src)) == Some(value);
                self.set_reg(fi, dst, Value::from_bool(ok));
            }
            Op::Call { dst, func, args } => {
                self.call(fi, func as usize, &args, dst, false)?;
            }
            Op::TailCall { func, args } => {
                self.call(fi, func as usize, &args, 0, true)?;
            }
            Op::CallClosure { dst, clo, args } => {
                let fun_id = self.closure_fun(self.reg(fi, clo))?;
                self.call(fi, fun_id, &args, dst, false)?;
            }
            Op::TailCallClosure { clo, args } => {
                let fun_id = self.closure_fun(self.reg(fi, clo))?;
                self.call(fi, fun_id, &args, 0, true)?;
            }
            Op::Return { src } => {
                let v = self.reg(fi, src);
                self.do_return(fi, v);
            }
            Op::Panic { msg } => {
                let msg = match &self.program.constants[msg as usize] {
                    Constant::String(s) => s.clone(),
                    _ => "panic".into(),
                };
                return Err(msg);
            }
            Op::Builtin { dst, builtin, args } => {
                let vals: Vec<_> = args.iter().map(|a| self.reg(fi, *a)).collect();
                let (v, extra) = builtin::call_builtin(
                    builtin,
                    &vals,
                    &mut self.arena,
                    &mut self.stdout,
                    &mut self.stderr,
                )?;
                charge += extra;
                self.set_reg(fi, dst, v);
            }
            Op::MakeTuple { dst, fields } => {
                let fs: Vec<_> = fields.iter().map(|a| self.reg(fi, *a)).collect();
                self.set_reg(fi, dst, self.arena.alloc_tuple(fs));
            }
            Op::MakeCons { dst, head, tail } => {
                let v = self.arena.alloc_cons(self.reg(fi, head), self.reg(fi, tail));
                self.set_reg(fi, dst, v);
            }
            Op::MakeEmptyList { dst } => self.set_reg(fi, dst, Arena::empty_list()),
            Op::MakeAdt {
                dst,
                type_tag,
                variant,
                fields,
            } => {
                let fs: Vec<_> = fields.iter().map(|a| self.reg(fi, *a)).collect();
                self.set_reg(fi, dst, self.arena.alloc_adt(type_tag, variant, fs));
            }
            Op::GetField { dst, base, index } => {
                let obj = self.arena.get(self.reg(fi, base)).ok_or("get_field on non-heap")?;
                let v = *obj.fields.get(index as usize).ok_or("field index out of range")?;
                self.set_reg(fi, dst, v);
            }
            Op::SwitchTag {
                scrutinee,
                arms,
                default,
            } => {
                let tag = self.tag_of(self.reg(fi, scrutinee));
                let mut target = default;
                for (t, pc) in arms {
                    if t == tag {
                        target = pc;
                        break;
                    }
                }
                self.frames[fi].pc = target as usize;
            }
        }

        self.reductions += charge;
        Ok(charge)
    }

    fn closure_fun(&self, c: Value) -> Result<usize, String> {
        let obj = self.arena.get(c).ok_or("not a closure")?;
        match obj.payload {
            crate::arena::Payload::Fun(id) => Ok(id as usize),
            _ => Err("not a closure".into()),
        }
    }

    fn call(
        &mut self,
        caller_fi: usize,
        func: usize,
        args: &[u8],
        ret_dst: u8,
        tail: bool,
    ) -> Result<(), String> {
        let fun = &self.program.functions[func];
        if args.len() != fun.arity as usize {
            return Err(format!(
                "arity mismatch: expected {}, got {}",
                fun.arity,
                args.len()
            ));
        }
        let mut new_regs = vec![Value::nil(); fun.regs.max(1) as usize];
        for (i, a) in args.iter().enumerate() {
            new_regs[i] = self.reg(caller_fi, *a);
        }
        if tail {
            self.had_tail_call = true;
            let frame = &mut self.frames[caller_fi];
            frame.func = func;
            frame.pc = 0;
            frame.regs = new_regs;
        } else {
            if self.frames.len() >= self.config.max_stack_frames {
                return Err("stack overflow".into());
            }
            self.frames.push(Frame {
                func,
                pc: 0,
                regs: new_regs,
                ret_dst,
                is_entry: false,
            });
            self.max_frame_depth = self.max_frame_depth.max(self.frames.len());
        }
        Ok(())
    }

    fn do_return(&mut self, fi: usize, v: Value) {
        let ret_dst = self.frames[fi].ret_dst;
        let is_entry = self.frames[fi].is_entry;
        self.frames.pop();
        if is_entry {
            self.done = Some(0);
        } else if let Some(last) = self.frames.last_mut() {
            if (ret_dst as usize) < last.regs.len() {
                last.regs[ret_dst as usize] = v;
            }
        }
    }

    fn write_panic(&mut self, message: &str) {
        let (site, frames) = if self.frames.is_empty() {
            (
                FrameInfo {
                    module: "?".into(),
                    function: "?".into(),
                    source_path: "src/?.lush".into(),
                    line: 0,
                    col: 0,
                },
                vec![],
            )
        } else {
            let fr = self.frames.last().unwrap();
            let fun = &self.program.functions[fr.func];
            let pc = fr.pc.saturating_sub(1);
            let (line, col) = fun.lines.get(pc).copied().unwrap_or((1, 1));
            let site = panic_report::frame_from_function(fun, line, col);
            let mut stack = Vec::new();
            for f in self.frames.iter().rev() {
                let fun = &self.program.functions[f.func];
                let pc = f.pc.saturating_sub(1);
                let (line, col) = fun.lines.get(pc).copied().unwrap_or((1, 1));
                stack.push(panic_report::frame_from_function(fun, line, col));
            }
            (site, stack)
        };
        let report = panic_report::format_panic(message, &site, &frames, self.had_tail_call);
        let _ = std::io::Write::write_all(&mut self.stderr, report.as_bytes());
    }

    fn reg(&self, fi: usize, r: u8) -> Value {
        self.frames[fi].regs[r as usize]
    }

    fn set_reg(&mut self, fi: usize, r: u8, v: Value) {
        self.frames[fi].regs[r as usize] = v;
    }

    fn load_const(&mut self, idx: u32) -> Result<Value, String> {
        match &self.program.constants[idx as usize] {
            Constant::Int(n) => Ok(self.arena.canonicalize_int(*n)),
            Constant::Float(f) => Ok(self.arena.alloc_float(*f)),
            Constant::String(s) => Ok(self.arena.alloc_string(s.as_bytes().to_vec())),
            Constant::Nullary { type_tag, variant } => {
                Ok(self.arena.alloc_adt(*type_tag, *variant, vec![]))
            }
        }
    }

    fn binop_num(
        &mut self,
        a: Value,
        b: Value,
        int_op: fn(i64, i64) -> Result<i64, numeric::NumericError>,
        float_op: fn(f64, f64) -> Result<f64, numeric::NumericError>,
    ) -> Result<Value, String> {
        if let (Some(x), Some(y)) = (self.arena.read_int(a), self.arena.read_int(b)) {
            let n = int_op(x, y).map_err(|e| e.to_string())?;
            return Ok(self.arena.canonicalize_int(n));
        }
        if let (Some(x), Some(y)) = (self.arena.read_float(a), self.arena.read_float(b)) {
            let n = float_op(x, y).map_err(|e| e.to_string())?;
            return Ok(self.arena.alloc_float(n));
        }
        Err("numeric operation on incompatible values".into())
    }

    fn cmp_int(&self, a: Value, b: Value, op: fn(i64, i64) -> bool) -> Result<bool, String> {
        let x = self.arena.read_int(a).ok_or("cmp expects Int")?;
        let y = self.arena.read_int(b).ok_or("cmp expects Int")?;
        Ok(op(x, y))
    }

    fn eq_values(&self, a: Value, b: Value, charge: &mut u64) -> Result<bool, String> {
        *charge += 1;
        if a == b {
            return Ok(true);
        }
        if let (Some(x), Some(y)) = (self.arena.read_int(a), self.arena.read_int(b)) {
            return Ok(x == y);
        }
        if let (Some(x), Some(y)) = (self.arena.read_float(a), self.arena.read_float(b)) {
            return Ok(x == y);
        }
        if let (Some(x), Some(y)) = (self.arena.read_string(a), self.arena.read_string(b)) {
            *charge += x.len() as u64 / 8;
            return Ok(x == y);
        }
        if let (Some(oa), Some(ob)) = (self.arena.get(a), self.arena.get(b)) {
            if oa.kind != ob.kind || oa.fields.len() != ob.fields.len() {
                return Ok(false);
            }
            let fields_a = oa.fields.clone();
            let fields_b = ob.fields.clone();
            for (x, y) in fields_a.into_iter().zip(fields_b) {
                if !self.eq_values(x, y, charge)? {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        Ok(false)
    }

    fn tag_of(&self, v: Value) -> u16 {
        if let Some(obj) = self.arena.get(v) {
            if let crate::arena::Payload::Adt { variant, .. } = obj.payload {
                return variant;
            }
        }
        0
    }
}
